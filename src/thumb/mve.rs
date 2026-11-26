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
    ComplexAdd { rotate_270: bool, halve: bool },
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
        let unary = match instruction & 0xffb3_1fd1 {
            0xffb0_0740 => Some(SaturatingAbsolute),
            0xffb0_07c0 => Some(SaturatingNegate),
            _ => None,
        };
        if let Some(operation) = unary {
            let size = (high >> 2) & 3;
            if size == 3 || high & 0x40 != 0 || low & 0xa0 != 0 {
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
            if high & 0x40 != 0 || low & 0xa0 != 0 {
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
        registers: &[u32; 32],
    ) {
        use BinaryOperation::*;
        let read_q = |q: usize| {
            u128::from_le_bytes(std::array::from_fn(|index| {
                self.fp[q * 4 + index / 4].to_le_bytes()[index % 4]
            }))
        };
        let left = read_q(inst.left);
        let right = if inst.scalar { 0 } else { read_q(inst.right) };
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
        let mut result = 0u128;
        let mut saturated = false;
        for shift in (0..128).step_by(inst.bits as usize) {
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
                SaturatingAbsolute | SaturatingNegate => {
                    let value = if matches!(inst.operation, SaturatingAbsolute) {
                        a.abs()
                    } else {
                        -a
                    };
                    saturated |= value > maximum;
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
                    saturated |= clamped != value;
                    clamped
                }
            };
            result |= (value as u128 & mask) << shift;
        }
        for (word, value) in self.fp[inst.destination * 4..inst.destination * 4 + 4]
            .iter_mut()
            .enumerate()
        {
            *value = (result >> (32 * word)) as u32;
        }
        if saturated {
            self.fpscr |= 1 << 27;
        }
        self.fp_active = true;
    }
}
