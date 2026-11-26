use std::io::Write;

use crate::bus::Bus;

use super::Thumb;

#[derive(Clone, Copy)]
pub(super) enum VectorTransfer {
    Interleaved {
        count: usize,
        beat: usize,
        width: usize,
    },
    Contiguous {
        width: usize,
        offset: u32,
        add: bool,
    },
    GatherScatter {
        width: usize,
        offsets: usize,
        scaled: bool,
    },
    Fpscr {
        offset: u32,
        add: bool,
    },
}

impl VectorTransfer {
    pub(super) fn decode(high: u32, low: u32) -> Option<Self> {
        if high & 0xffc0 == 0xfc80 && low & 0x1e1e == 0x1e00 {
            let count = if low & 1 == 0 { 2 } else { 4 };
            let beat = ((low >> 5) & 3) as usize;
            let size = ((low >> 7) & 3) as usize;
            return (size < 3 && beat < count).then_some(Self::Interleaved {
                count,
                beat,
                width: 1 << size,
            });
        }
        if high & 0xff60 == 0xed00 && low & 0xff80 == 0x2f80 {
            return Some(Self::Fpscr {
                offset: (low & 0x7f) * 4,
                add: high & 0x80 != 0,
            });
        }
        if high & 0xee00 != 0xec00 || low & 0x0f00 != 0x0f00 {
            return None;
        }
        if high & 0x100 == 0 && high & 0x80 != 0 && high & 0x20 == 0 && low & 0x1020 == 0 {
            let width = match low & 0xf0 {
                0x00 => 1,
                0x10 => 2,
                0x40 => 4,
                0xd0 => 8,
                _ => return None,
            };
            return Some(Self::GatherScatter {
                width,
                offsets: ((low >> 1) & 7) as usize,
                scaled: low & 1 != 0,
            });
        }
        if high & 0x100 != 0 || high & 0x20 != 0 {
            let width = if low & 0x1000 != 0 {
                4
            } else if high & 8 != 0 {
                2
            } else {
                1
            };
            return Some(Self::Contiguous {
                width,
                offset: (low & 0x7f) * width as u32,
                add: high & 0x80 != 0,
            });
        }
        None
    }
}

fn offset_address(base: u32, offset: u32, add: bool) -> u32 {
    if add {
        base.wrapping_add(offset)
    } else {
        base.wrapping_sub(offset)
    }
}

fn read_element(bus: &mut Bus, address: u32, width: usize, signed: bool) -> Result<u32, String> {
    match width {
        1 => bus
            .thumb_read::<1>(address)
            .map(|v| if signed { v as i8 as i32 as u32 } else { v }),
        2 => bus
            .thumb_read::<2>(address)
            .map(|v| if signed { v as i16 as i32 as u32 } else { v }),
        4 => bus.thumb_read::<4>(address),
        _ => unreachable!("vector bus beats are at most four bytes"),
    }
}

impl Thumb {
    pub(super) fn execute_vector_transfer(
        &mut self,
        transfer: VectorTransfer,
        high: u32,
        low: u32,
        r: &mut [u32; 32],
        bus: &mut Bus,
        output: &mut impl Write,
    ) -> Result<(), String> {
        let load = high & 0x10 != 0;
        let writeback = high & 0x20 != 0;
        let q = (low >> 13) as usize;
        let mut rn = (high & 15) as usize;
        if matches!(transfer, VectorTransfer::Contiguous { width: 1 | 2, .. }) {
            rn &= 7;
        }
        if rn == 15 {
            return Err("MVE memory transfer cannot use PC as its base register".into());
        }
        self.fp_active = true;
        let base = r[rn];
        let mut fp = self.fp;
        let mut updated_base = base;
        match transfer {
            VectorTransfer::Interleaved { count, beat, width } => {
                if q + count > 8 {
                    return Err("MVE structure transfer register list exceeds Q7".into());
                }
                let lanes = 16 / (count * width);
                for lane in 0..lanes {
                    for member in 0..count {
                        let address =
                            base.wrapping_add((beat * 16 + (lane * count + member) * width) as u32);
                        let byte = (beat * lanes + lane) * width;
                        let word = (q + member) * 4 + byte / 4;
                        let shift = (byte % 4) * 8;
                        if load {
                            let mask = u32::MAX >> (32 - width * 8) << shift;
                            let value = read_element(bus, address, width, false)?;
                            fp[word] = (fp[word] & !mask) | (value << shift);
                        } else {
                            bus.thumb_write(address, width, fp[word] >> shift, output)?;
                        }
                    }
                }
                updated_base = base.wrapping_add((16 * count) as u32);
            }
            VectorTransfer::Contiguous { width, offset, add } => {
                updated_base = offset_address(base, offset, add);
                let address = if high & 0x100 != 0 {
                    updated_base
                } else {
                    base
                };
                for lane in 0..4 {
                    let address = address.wrapping_add((lane * width) as u32);
                    if load {
                        fp[q * 4 + lane] = read_element(bus, address, width, high & 0x1000 == 0)?;
                    } else {
                        bus.thumb_write(address, width, fp[q * 4 + lane], output)?;
                    }
                }
            }
            VectorTransfer::GatherScatter {
                width,
                offsets,
                scaled,
            } => {
                let words = if width == 8 { 2 } else { 1 };
                for lane in (0..4).step_by(words) {
                    let offset = fp[offsets * 4 + lane];
                    let offset = if scaled {
                        offset.wrapping_mul(width as u32)
                    } else {
                        offset
                    };
                    for word in 0..words {
                        let address = base.wrapping_add(offset).wrapping_add(4 * word as u32);
                        let index = q * 4 + lane + word;
                        if load {
                            fp[index] =
                                read_element(bus, address, width.min(4), high & 0x1000 == 0)?;
                        } else {
                            bus.thumb_write(address, width.min(4), fp[index], output)?;
                        }
                    }
                }
            }
            VectorTransfer::Fpscr { offset, add } => {
                let address = offset_address(base, offset, add);
                if load {
                    self.fpscr = bus.thumb_read::<4>(address)?;
                } else {
                    bus.thumb_write(address, 4, self.fpscr, output)?;
                }
            }
        }
        self.fp = fp;
        if writeback {
            r[rn] = updated_base;
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum BinaryOperation {
    HalfAdd,
    RoundHalfAdd,
    HalfSubtract,
    Maximum,
    Minimum,
    SaturatingAdd,
    SaturatingSubtract,
    And,
    BitClear,
    Or,
    OrNot,
    ExclusiveOr,
    Not,
    SaturatingAbsolute,
    SaturatingNegate,
    RoundFloat {
        mode: u32,
        exact: bool,
    },
    FloatToInteger {
        mode: u32,
    },
    NarrowShift {
        shift: u32,
        rounding: bool,
        top: bool,
    },
    Sum {
        accumulator: usize,
        accumulate: bool,
    },
    AbsoluteDifferenceAccumulate {
        accumulator: usize,
    },
    ShiftLeftCarry {
        accumulator: usize,
        shift: u32,
    },
    ComplexAdd {
        rotate_270: bool,
        halve: bool,
    },
}

pub(super) struct VectorArithmetic {
    operation: BinaryOperation,
    destination: usize,
    left: usize,
    right: usize,
    scalar: bool,
    unsigned: bool,
    bits: u32,
}

impl VectorArithmetic {
    pub(super) fn decode(high: u32, low: u32) -> Option<Self> {
        use BinaryOperation::*;
        let instruction = high << 16 | low;
        if instruction & 0xffa0_1ff0 == 0xeea0_0fc0 {
            let accumulator = (low & 15) as usize;
            if high & 0x40 != 0 || matches!(accumulator, 13 | 15) {
                return None;
            }
            let destination = (low >> 13) as usize;
            return Some(Self {
                operation: ShiftLeftCarry {
                    accumulator,
                    shift: ((high & 31) + 31) % 32 + 1,
                },
                destination,
                left: destination,
                right: destination,
                scalar: false,
                unsigned: true,
                bits: 32,
            });
        }
        if instruction & 0xeff3_1fd1 == 0xeef1_0f00 {
            let size = (high >> 2) & 3;
            if size == 3 {
                return None;
            }
            return Some(Self {
                operation: Sum {
                    accumulator: ((low >> 12) & 14) as usize,
                    accumulate: low & 0x20 != 0,
                },
                destination: 0,
                left: ((low >> 1) & 7) as usize,
                right: 0,
                scalar: false,
                unsigned: high & 0x1000 != 0,
                bits: 8 << size,
            });
        }
        if instruction & 0xefc1_0f51 == 0xee80_0f01 {
            let accumulator = (low >> 12) as usize;
            let size = (high >> 4) & 3;
            if size == 3 || low & 0xa0 != 0 || matches!(accumulator, 13 | 15) {
                return None;
            }
            return Some(Self {
                operation: AbsoluteDifferenceAccumulate { accumulator },
                destination: 0,
                left: ((high >> 1) & 7) as usize,
                right: ((low >> 1) & 7) as usize,
                scalar: false,
                unsigned: high & 0x1000 != 0,
                bits: 8 << size,
            });
        }
        if instruction & 0xefa0_0fd1 == 0xee80_0fc1 {
            let immediate = high & 31;
            let width = match immediate {
                8..=15 => 8,
                16..=31 => 16,
                _ => return None,
            };
            if high & 0x40 != 0 || low & 0x20 != 0 {
                return None;
            }
            return Some(Self {
                operation: NarrowShift {
                    shift: 2 * width - immediate,
                    rounding: high & 0x1000 != 0,
                    top: low & 0x1000 != 0,
                },
                destination: (low >> 13) as usize,
                left: ((low >> 1) & 7) as usize,
                right: 0,
                scalar: false,
                unsigned: true,
                bits: 2 * width,
            });
        }
        let float_op = match instruction & 0xffb3_1c51 {
            0xffb2_0440 => {
                let op = (low >> 7) & 7;
                let mode = match op {
                    0 | 1 => 0,
                    2 => 4,
                    3 => 3,
                    5 => 2,
                    7 => 1,
                    _ => return None,
                };
                Some((
                    RoundFloat {
                        mode,
                        exact: op == 1,
                    },
                    false,
                ))
            }
            0xffb3_0040 => {
                let mode = [4, 0, 1, 2][((low >> 8) & 3) as usize];
                Some((FloatToInteger { mode }, low & 0x80 != 0))
            }
            _ => None,
        };
        if let Some((operation, unsigned)) = float_op {
            if (high >> 2) & 3 != 2 || high & 0x40 != 0 || low & 0x20 != 0 {
                return None;
            }
            return Some(Self {
                operation,
                destination: (low >> 13) as usize,
                left: ((low >> 1) & 7) as usize,
                right: 0,
                scalar: false,
                unsigned,
                bits: 32,
            });
        }
        let unary = match instruction & 0xffb3_1fd1 {
            0xffb0_0740 => Some(SaturatingAbsolute),
            0xffb0_07c0 => Some(SaturatingNegate),
            _ => None,
        };
        if let Some(operation) = unary {
            let size = (high >> 2) & 3;
            if size == 3 || high & 0x40 != 0 || low & 0x20 != 0 {
                return None;
            }
            let source = ((low >> 1) & 7) as usize;
            return Some(Self {
                operation,
                destination: (low >> 13) as usize,
                left: source,
                right: source,
                scalar: false,
                unsigned: false,
                bits: 8 << size,
            });
        }
        let logic = match instruction & 0xffb1_1f51 {
            0xef00_0150 => Some(And),
            0xef10_0150 => Some(BitClear),
            0xef20_0150 => Some(Or),
            0xef30_0150 => Some(OrNot),
            0xff00_0150 => Some(ExclusiveOr),
            _ if instruction & 0xffbf_1fd1 == 0xffb0_05c0 => Some(Not),
            _ => None,
        };
        if let Some(operation) = logic {
            if high & 0x40 != 0 || low & 0x20 != 0 {
                return None;
            }
            let source = ((low >> 1) & 7) as usize;
            return Some(Self {
                operation,
                destination: (low >> 13) as usize,
                left: if matches!(operation, Not) {
                    source
                } else {
                    ((high >> 1) & 7) as usize
                },
                right: source,
                scalar: false,
                unsigned: true,
                bits: 8,
            });
        }
        let (operation, scalar, unsigned) = match instruction & 0xef81_1f51 {
            0xef00_0040 => (HalfAdd, false, high & 0x1000 != 0),
            0xef00_0140 => (RoundHalfAdd, false, high & 0x1000 != 0),
            0xef00_0240 => (HalfSubtract, false, high & 0x1000 != 0),
            0xef00_0640 => (Maximum, false, high & 0x1000 != 0),
            0xef00_0650 => (Minimum, false, high & 0x1000 != 0),
            0xef00_0050 => (SaturatingAdd, false, high & 0x1000 != 0),
            0xef00_0250 => (SaturatingSubtract, false, high & 0x1000 != 0),
            0xee00_0f00 | 0xee00_1f00 => (
                ComplexAdd {
                    rotate_270: low & 0x1000 != 0,
                    halve: high & 0x1000 == 0,
                },
                false,
                false,
            ),
            _ => {
                let operation = match instruction & 0xef81_1f70 {
                    0xee00_0f40 => HalfAdd,
                    0xee00_1f40 => HalfSubtract,
                    0xee00_0f60 => SaturatingAdd,
                    0xee00_1f60 => SaturatingSubtract,
                    _ => return None,
                };
                (operation, true, high & 0x1000 != 0)
            }
        };
        let size = (high >> 4) & 3;
        if size == 3 || high & 0x40 != 0 || low & 0x80 != 0 || !scalar && low & 0x20 != 0 {
            return None;
        }
        Some(Self {
            operation,
            destination: (low >> 13) as usize,
            left: ((high >> 1) & 7) as usize,
            right: if scalar {
                (low & 15) as usize
            } else {
                ((low >> 1) & 7) as usize
            },
            scalar,
            unsigned,
            bits: 8 << size,
        })
    }
}

impl Thumb {
    pub(super) fn execute_vector_arithmetic(
        &mut self,
        inst: VectorArithmetic,
        registers: &mut [u32; 32],
    ) {
        use BinaryOperation::*;
        let left = self.vector(inst.left);
        let right = if inst.scalar {
            0
        } else {
            self.vector(inst.right)
        };
        let predicate = self.vector_predicate(registers[14]);
        let limit = 1i64 << inst.bits;
        let mask = (limit - 1) as u128;
        let lane = |value: u128| {
            let value = (value & mask) as i64;
            if !inst.unsigned && value >= limit / 2 {
                value - limit
            } else {
                value
            }
        };
        let (minimum, maximum) = if inst.unsigned {
            (0, limit - 1)
        } else {
            (-limit / 2, limit / 2 - 1)
        };
        if let Sum {
            accumulator,
            accumulate,
        } = inst.operation
        {
            let mut sum = if accumulate {
                registers[accumulator]
            } else {
                0
            };
            for shift in (0..128).step_by(inst.bits as usize) {
                if predicate & (1 << (shift / 8)) != 0 {
                    sum = sum.wrapping_add(lane(left >> shift) as u32);
                }
            }
            registers[accumulator] = sum;
            self.advance_vector_predicate();
            self.fp_active = true;
            return;
        }
        if let AbsoluteDifferenceAccumulate { accumulator } = inst.operation {
            for shift in (0..128).step_by(inst.bits as usize) {
                if predicate & (1 << (shift / 8)) == 0 {
                    continue;
                }
                let difference = lane(left >> shift).abs_diff(lane(right >> shift)) as u32;
                registers[accumulator] = registers[accumulator].wrapping_add(difference);
            }
            self.advance_vector_predicate();
            self.fp_active = true;
            return;
        }
        if let ShiftLeftCarry { accumulator, shift } = inst.operation {
            let mask = (1u64 << shift) - 1;
            let mut carry = u64::from(registers[accumulator]);
            let mut result = 0;
            for word in 0..4 {
                let previous = u64::from(self.fp[inst.destination * 4 + word]);
                let value = ((previous << shift) | (carry & mask)) as u32;
                result |= u128::from(value) << (word * 32);
                if predicate & (1 << (word * 4)) != 0 {
                    carry = previous >> (32 - shift);
                }
            }
            self.write_vector(inst.destination, result, predicate);
            registers[accumulator] = carry as u32;
            self.advance_vector_predicate();
            self.fp_active = true;
            return;
        }
        let mut result = 0u128;
        if let NarrowShift {
            shift,
            rounding,
            top,
        } = inst.operation
        {
            let lane_width = inst.bits / 2;
            let lane_mask = (1u128 << lane_width) - 1;
            let mut write = 0;
            for source in (0..128).step_by(inst.bits as usize) {
                let bias = if rounding { 1 << (shift - 1) } else { 0 };
                let value = ((left >> source) & mask) + bias;
                let destination = source + u32::from(top) * lane_width;
                result |= ((value >> shift) & lane_mask) << destination;
                write |= ((1 << (lane_width / 8)) - 1) << (destination / 8);
            }
            self.write_vector(inst.destination, result, predicate & write);
            self.advance_vector_predicate();
            self.fp_active = true;
            return;
        }
        let mut saturated = false;
        for shift in (0..128).step_by(inst.bits as usize) {
            let lane_active = predicate & (1 << (shift / 8)) != 0;
            let a = lane(left >> shift);
            let b = lane(if inst.scalar {
                registers[inst.right].into()
            } else {
                right >> shift
            });
            let value = match inst.operation {
                HalfAdd => (a + b) >> 1,
                RoundHalfAdd => (a + b + 1) >> 1,
                HalfSubtract => (a - b) >> 1,
                And => a & b,
                BitClear => a & !b,
                Or => a | b,
                OrNot => a | !b,
                ExclusiveOr => a ^ b,
                Not => !a,
                NarrowShift { .. } => unreachable!(),
                RoundFloat { mode, exact } => {
                    let input = f32::from_bits(a as u32);
                    let mode = if exact { (self.fpscr >> 22) & 3 } else { mode };
                    let output = Self::round_fp_value(f64::from(input), mode) as f32;
                    if lane_active && exact && input.is_finite() && input != output {
                        self.fpscr |= 1 << 4;
                    }
                    i64::from(output.to_bits())
                }
                FloatToInteger { mode } => {
                    let input = f64::from(f32::from_bits(a as u32));
                    let rounded = Self::round_fp_value(input, mode);
                    let (lower, upper) = if inst.unsigned {
                        (0.0, f64::from(u32::MAX))
                    } else {
                        (f64::from(i32::MIN), f64::from(i32::MAX))
                    };
                    if lane_active {
                        if rounded.is_nan() || rounded < lower || rounded > upper {
                            self.fpscr |= 1;
                        } else if input != rounded {
                            self.fpscr |= 1 << 4;
                        }
                    }
                    if inst.unsigned {
                        i64::from(rounded as u32)
                    } else {
                        i64::from(rounded as i32)
                    }
                }
                Sum { .. } | AbsoluteDifferenceAccumulate { .. } | ShiftLeftCarry { .. } => {
                    unreachable!()
                }
                SaturatingAbsolute | SaturatingNegate => {
                    let value = if matches!(inst.operation, SaturatingAbsolute) {
                        a.abs()
                    } else {
                        -a
                    };
                    saturated |= lane_active && value > maximum;
                    value.min(maximum)
                }
                ComplexAdd { rotate_270, halve } => {
                    let lane_index = shift / inst.bits;
                    let alternate_shift = (lane_index ^ 1) * inst.bits;
                    let alternate = lane(right >> alternate_shift);
                    let subtract = (lane_index & 1 == 0) != rotate_270;
                    let value = if subtract {
                        a - alternate
                    } else {
                        a + alternate
                    };
                    value >> u32::from(halve)
                }
                Maximum => a.max(b),
                Minimum => a.min(b),
                SaturatingAdd | SaturatingSubtract => {
                    let value = if matches!(inst.operation, SaturatingAdd) {
                        a + b
                    } else {
                        a - b
                    };
                    let clamped = value.clamp(minimum, maximum);
                    saturated |= lane_active && clamped != value;
                    clamped
                }
            };
            result |= (value as u128 & mask) << shift;
        }
        self.write_vector(inst.destination, result, predicate);
        self.advance_vector_predicate();
        if saturated {
            self.fpscr |= 1 << 27;
        }
        self.fp_active = true;
    }
}

pub(super) enum PredicateInstruction {
    Compare {
        left: usize,
        right: usize,
        scalar: bool,
        floating: bool,
        bits: u32,
        comparison: u32,
        block_mask: u32,
    },
    Start(u32),
    Duplicate {
        destination: usize,
        source: usize,
        bits: u32,
    },
}

impl PredicateInstruction {
    pub(super) fn decode(high: u32, low: u32) -> Option<Self> {
        let instruction = high << 16 | low;
        let block_mask = (low >> 13) | ((high & 0x40) >> 3);
        if instruction & 0xffb1_0f5f == 0xfe31_0f4d && block_mask != 0 {
            return Some(Self::Start(block_mask));
        }
        if instruction & 0xffb1_0f50 == 0xeea0_0b10 {
            let size = ((high >> 5) & 2) | ((low >> 5) & 1);
            let source = (low >> 12) as usize;
            if size == 3 || low & 0x80 != 0 || matches!(source, 13 | 15) {
                return None;
            }
            return Some(Self::Duplicate {
                destination: ((high >> 1) & 7) as usize,
                source,
                bits: match size {
                    0 => 32,
                    1 => 16,
                    _ => 8,
                },
            });
        }
        let (floating, scalar) = match instruction & 0xff81_0f50 {
            0xfe01_0f00 if (high >> 4) & 3 != 3 => (false, false),
            0xfe01_0f40 if (high >> 4) & 3 != 3 => (false, true),
            _ => match instruction & 0xefb1_0f50 {
                0xee31_0f00 if low & 0x1001 != 1 => (true, false),
                0xee31_0f40 if low & 15 != 13 => (true, true),
                _ => return None,
            },
        };
        if !scalar && low & 0x20 != 0 {
            return None;
        }
        let end = if scalar { (low >> 5) & 1 } else { low & 1 };
        let b = (low >> 12) & 1;
        let c = (low >> 7) & 1;
        let comparison = if floating {
            b | (end << 1) | (c << 2)
        } else {
            c | (b << 1) | (end << 2)
        };
        Some(Self::Compare {
            left: ((high >> 1) & 7) as usize,
            right: if scalar {
                (low & 15) as usize
            } else {
                ((low >> 1) & 7) as usize
            },
            scalar,
            floating,
            bits: if floating {
                32 >> ((high >> 12) & 1)
            } else {
                8 << ((high >> 4) & 3)
            },
            comparison,
            block_mask,
        })
    }
}

impl Thumb {
    fn vector(&self, index: usize) -> u128 {
        self.fp[index * 4..index * 4 + 4]
            .iter()
            .enumerate()
            .fold(0, |vector, (word, value)| {
                vector | (u128::from(*value) << (word * 32))
            })
    }

    fn write_vector(&mut self, index: usize, value: u128, predicate: u16) {
        let byte_mask = u128::from_le_bytes(std::array::from_fn(|byte| {
            if predicate & (1 << byte) != 0 {
                0xff
            } else {
                0
            }
        }));
        let combined = (self.vector(index) & !byte_mask) | (value & byte_mask);
        for (word, slot) in self.fp[index * 4..index * 4 + 4].iter_mut().enumerate() {
            *slot = (combined >> (word * 32)) as u32;
        }
    }

    fn vector_predicate(&self, link_register: u32) -> u16 {
        let mut predicate = self.vpr as u16;
        if self.vpr & 0x000f_0000 == 0 {
            predicate |= 0x00ff;
        }
        if self.vpr & 0x00f0_0000 == 0 {
            predicate |= 0xff00;
        }
        let tail_size = (self.fpscr >> 16) & 7;
        if tail_size < 4 && link_register < (16 >> tail_size) {
            predicate &= ((1u32 << (link_register << tail_size)) - 1) as u16;
        }
        predicate
    }

    fn advance_vector_predicate(&mut self) {
        let low = (self.vpr >> 16) & 15;
        let high = (self.vpr >> 20) & 15;
        if low > 8 {
            self.vpr ^= 0x00ff;
        }
        if high > 8 {
            self.vpr ^= 0xff00;
        }
        self.vpr =
            (self.vpr & !0x00ff_0000) | ((low << 17) & 0x000f_0000) | ((high << 21) & 0x00f0_0000);
    }

    pub(super) fn execute_vector_predicate(
        &mut self,
        inst: PredicateInstruction,
        registers: &[u32; 32],
    ) {
        let predicate = self.vector_predicate(registers[14]);
        match inst {
            PredicateInstruction::Start(mask) => {
                self.vpr = (self.vpr & !0x00ff_0000) | (mask << 16) | (mask << 20);
            }
            PredicateInstruction::Duplicate {
                destination,
                source,
                bits,
            } => {
                let lane = u128::from(registers[source]) & ((1u128 << bits) - 1);
                let mut value = 0;
                for shift in (0..128).step_by(bits as usize) {
                    value |= lane << shift;
                }
                self.write_vector(destination, value, predicate);
                self.advance_vector_predicate();
            }
            PredicateInstruction::Compare {
                left,
                right,
                scalar,
                floating,
                bits,
                comparison,
                block_mask,
            } => {
                let lhs = self.vector(left);
                let rhs = if scalar {
                    if right == 15 {
                        0
                    } else {
                        u128::from(registers[right])
                    }
                } else {
                    self.vector(right)
                };
                let mask = (1u128 << bits) - 1;
                let mut active = 0;
                for shift in (0..128).step_by(bits as usize) {
                    let a = ((lhs >> shift) & mask) as u32;
                    let b = ((rhs >> if scalar { 0 } else { shift }) & mask) as u32;
                    let selected = if floating {
                        fp_predicate(a, b, bits, comparison)
                    } else {
                        let signed_a = ((a << (32 - bits)) as i32) >> (32 - bits);
                        let signed_b = ((b << (32 - bits)) as i32) >> (32 - bits);
                        match comparison {
                            0 => a == b,
                            1 => a != b,
                            2 => signed_a >= signed_b,
                            3 => signed_a < signed_b,
                            4 => a >= b,
                            5 => a > b,
                            6 => signed_a > signed_b,
                            7 => signed_a <= signed_b,
                            _ => unreachable!(),
                        }
                    };
                    if selected {
                        active |= ((1u16 << (bits / 8)) - 1) << (shift / 8);
                    }
                }
                self.vpr = (self.vpr & !0xffff) | u32::from(active & predicate);
                if block_mask == 0 {
                    self.advance_vector_predicate();
                } else {
                    self.vpr = (self.vpr & !0x00ff_0000) | (block_mask << 16) | (block_mask << 20);
                }
            }
        }
        self.fp_active = true;
    }
}

fn fp_predicate(a: u32, b: u32, bits: u32, comparison: u32) -> bool {
    let decode = |value: u32| {
        if bits == 32 {
            return f32::from_bits(value);
        }
        let sign = (value & 0x8000) << 16;
        let fraction = value & 0x3ff;
        let exponent = (value >> 10) & 31;
        match exponent {
            0 => f32::from_bits(sign | (103 << 23)) * fraction as f32,
            31 => f32::from_bits(sign | (255 << 23) | (fraction << 13)),
            _ => f32::from_bits(sign | ((exponent + 112) << 23) | (fraction << 13)),
        }
    };
    let (a, b) = (decode(a), decode(b));
    match comparison {
        0 => a == b,
        4 => a != b,
        1 => a >= b,
        5 => a < b,
        3 => a > b,
        7 => a <= b,
        _ => false,
    }
}

pub(super) enum LowOverheadBranch {
    Start {
        source: usize,
        skip_offset: Option<u32>,
        tail_size: Option<u32>,
    },
    End {
        offset: u32,
        tail: bool,
        forever: bool,
    },
    ClearTail,
}

impl LowOverheadBranch {
    pub(super) fn decode(high: u32, low: u32) -> Option<Self> {
        let instruction = high << 16 | low;
        let offset = ((low & 0x7fe) << 1) | ((low >> 10) & 2);
        if instruction == 0xf00f_e001 {
            return Some(Self::ClearTail);
        }
        if instruction & 0xffcf_f001 == 0xf00f_c001 {
            return Some(Self::End {
                offset,
                tail: high & 0x10 != 0,
                forever: high & 0x20 != 0,
            });
        }
        let skip_offset = if instruction & 0xff80_f001 == 0xf000_c001 {
            Some(offset)
        } else if instruction & 0xff80_ffff == 0xf000_e001 {
            None
        } else {
            return None;
        };
        let source = (high & 15) as usize;
        if matches!(source, 13 | 15) {
            return None;
        }
        Some(Self::Start {
            source,
            skip_offset,
            tail_size: (high & 0x40 == 0).then_some((high >> 4) & 3),
        })
    }
}

impl Thumb {
    pub(super) fn execute_low_overhead_branch(
        &mut self,
        branch: LowOverheadBranch,
        pc: u32,
        next: u32,
        registers: &mut [u32; 32],
        bus: &mut Bus,
    ) -> Result<u32, String> {
        const TAIL_SIZE_MASK: u32 = 7 << 16;
        let tail_size = (self.fpscr & TAIL_SIZE_MASK) >> 16;
        match branch {
            LowOverheadBranch::Start {
                source,
                skip_offset,
                tail_size,
            } => {
                let count = registers[source];
                if count == 0
                    && let Some(offset) = skip_offset
                {
                    return Ok(next.wrapping_add(offset));
                }
                registers[14] = count;
                if let Some(size) = tail_size {
                    self.create_fp_context(bus);
                    self.fpscr = (self.fpscr & !TAIL_SIZE_MASK) | (size << 16);
                }
            }
            LowOverheadBranch::End {
                offset,
                tail,
                forever,
            } => {
                if !tail && tail_size != 4 && self.fp_active {
                    self.branch_exception = true;
                    return self.invalid_state_fault(pc, registers, bus);
                }
                if forever {
                    return Ok(next.wrapping_sub(offset));
                }
                let lanes = if tail { 16 >> tail_size } else { 1 };
                if registers[14] > lanes {
                    registers[14] -= lanes;
                    return Ok(next.wrapping_sub(offset));
                }
                if tail {
                    self.fpscr = (self.fpscr & !TAIL_SIZE_MASK) | (4 << 16);
                }
            }
            LowOverheadBranch::ClearTail => {
                self.create_fp_context(bus);
                self.fpscr = (self.fpscr & !TAIL_SIZE_MASK) | (4 << 16);
            }
        }
        Ok(next)
    }
}

impl Thumb {
    pub(super) fn execute_long_shift(
        &mut self,
        high: u32,
        low: u32,
        registers: &mut [u32; 32],
    ) -> bool {
        let upper = (((low >> 9) & 7) * 2 + 1) as usize;
        if upper >= 13 {
            return false;
        }
        let lower = (high & 14) as usize;
        let instruction = high << 16 | low;
        let imm = ((low >> 6) & 3) | ((low >> 10) & 28);
        let imm = if imm == 0 { 32 } else { imm as i32 };
        let register = (low >> 12) as usize;
        let immediate = match instruction & 0xfff1_813f {
            0xea50_010f => Some((imm, false, false, None)),
            0xea50_011f => Some((-imm, false, false, None)),
            0xea50_012f => Some((-imm, true, false, None)),
            0xea51_010f => Some((imm, false, false, Some(64))),
            0xea51_011f => Some((-imm, false, true, None)),
            0xea51_012f => Some((-imm, true, true, None)),
            0xea51_013f => Some((imm, true, false, Some(64))),
            _ => None,
        };
        let (shift, signed, rounding, saturate) = if let Some(config) = immediate {
            config
        } else {
            if matches!(register, 13 | 15) || register == lower || register == upper {
                return false;
            }
            let shift = i32::from(registers[register] as i8);
            let saturation = if low & 0x80 != 0 { 48 } else { 64 };
            match instruction & 0xfff1_017f {
                0xea50_010d if low & 0x80 == 0 => (shift, false, false, None),
                0xea50_012d if low & 0x80 == 0 => (-shift, true, false, None),
                0xea51_010d => (shift, false, true, Some(saturation)),
                0xea51_012d => (-shift, true, true, Some(saturation)),
                _ => return false,
            }
        };
        let input = u64::from(registers[lower]) | (u64::from(registers[upper]) << 32);
        let value = if signed {
            i128::from(input as i64)
        } else {
            i128::from(input)
        };
        let result = if shift < 0 {
            let count = shift.unsigned_abs();
            let width = saturate.unwrap_or(64);
            if count > width || count == width && (!rounding || signed) {
                if signed && !rounding && value < 0 {
                    -1
                } else {
                    0
                }
            } else {
                let bias = if rounding { 1i128 << (count - 1) } else { 0 };
                (value + bias) >> count
            }
        } else if shift >= 64 {
            if saturate.is_some() {
                value.signum() * (1i128 << 64)
            } else {
                0
            }
        } else {
            value << shift
        };
        let result = if let Some(bits) = saturate {
            let (min, max) = if signed {
                (-(1i128 << (bits - 1)), (1i128 << (bits - 1)) - 1)
            } else {
                (0, (1i128 << bits) - 1)
            };
            let clamped = result.clamp(min, max);
            self.saturation |= clamped != result;
            clamped as u64
        } else {
            result as u64
        };
        registers[lower] = result as u32;
        registers[upper] = (result >> 32) as u32;
        true
    }
}
