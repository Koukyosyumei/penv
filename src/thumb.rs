use std::io::Write;

use crate::bus::Bus;

#[derive(Clone, Default)]
pub struct Thumb {
    pub vectors: u32,
    negative: bool,
    zero: bool,
    carry: bool,
    overflow: bool,
}

impl Thumb {
    pub fn new(vectors: u32) -> Self {
        Self {
            vectors,
            ..Self::default()
        }
    }

    pub fn reset(&self, bus: &Bus) -> Result<(u32, u32), String> {
        let sp = bus
            .read_ram::<4>(self.vectors)
            .map_err(|error| error.to_string())?;
        let vector = self
            .vectors
            .checked_add(4)
            .ok_or("reset vector address overflow")?;
        let target = bus
            .read_ram::<4>(vector)
            .map_err(|error| error.to_string())?;
        if target & 1 == 0 {
            return Err(format!(
                "Cortex-M reset vector {target:#010x} must select Thumb state"
            ));
        }
        bus.read_ram::<2>(target & !1)
            .map_err(|error| error.to_string())?;
        Ok((sp & !3, target & !1))
    }

    fn nz(&mut self, value: u32) {
        self.negative = value >> 31 != 0;
        self.zero = value == 0;
    }

    fn arithmetic(&mut self, lhs: u32, rhs: u32, subtract: bool) -> u32 {
        let (result, carry, overflow) = if subtract {
            (
                lhs.wrapping_sub(rhs),
                lhs >= rhs,
                (lhs as i32).overflowing_sub(rhs as i32).1,
            )
        } else {
            let (result, carry) = lhs.overflowing_add(rhs);
            (result, carry, (lhs as i32).overflowing_add(rhs as i32).1)
        };
        self.nz(result);
        self.carry = carry;
        self.overflow = overflow;
        result
    }

    fn condition(&self, condition: u32) -> bool {
        let positive = match condition >> 1 {
            0 => self.zero,
            1 => self.carry,
            2 => self.negative,
            3 => self.overflow,
            4 => self.carry && !self.zero,
            5 => self.negative == self.overflow,
            6 => !self.zero && self.negative == self.overflow,
            _ => unreachable!(),
        };
        positive ^ (condition & 1 != 0)
    }

    pub fn step(
        &mut self,
        pc: &mut u32,
        r: &mut [u32; 32],
        bus: &mut Bus,
        output: &mut impl Write,
    ) -> Result<bool, String> {
        let inst = bus.read_ram::<2>(*pc).map_err(|error| error.to_string())?;
        let unsupported = || format!("unsupported Thumb instruction {inst:#06x}");
        let mut next = pc.wrapping_add(2);
        let architectural_pc = pc.wrapping_add(4);
        let rd = (inst & 7) as usize;
        let rn = ((inst >> 3) & 7) as usize;
        match inst {
            0x1800..=0x1fff => {
                let field = ((inst >> 6) & 7) as usize;
                let operand = if inst & 0x400 != 0 {
                    field as u32
                } else {
                    r[field]
                };
                r[rd] = self.arithmetic(r[rn], operand, inst & 0x200 != 0);
            }
            0x0000..=0x17ff => {
                let shift = (inst >> 6) & 31;
                let value = r[rn];
                let result = match inst >> 11 {
                    0 if shift == 0 => value,
                    0 => {
                        self.carry = (value >> (32 - shift)) & 1 != 0;
                        value << shift
                    }
                    1 => {
                        let shift = if shift == 0 { 32 } else { shift };
                        self.carry = (value >> (shift - 1)) & 1 != 0;
                        value.checked_shr(shift).unwrap_or(0)
                    }
                    _ => {
                        let shift = if shift == 0 { 32 } else { shift };
                        self.carry = (value >> (shift - 1)) & 1 != 0;
                        ((value as i32) >> shift.min(31)) as u32
                    }
                };
                r[rd] = result;
                self.nz(result);
            }
            0x2000..=0x3fff => {
                let rd = ((inst >> 8) & 7) as usize;
                let value = inst & 0xff;
                match (inst >> 11) & 3 {
                    0 => {
                        r[rd] = value;
                        self.nz(value);
                    }
                    1 => {
                        self.arithmetic(r[rd], value, true);
                    }
                    2 => r[rd] = self.arithmetic(r[rd], value, false),
                    _ => r[rd] = self.arithmetic(r[rd], value, true),
                }
            }
            0x4700..=0x47ff if inst & 7 == 0 => {
                let rm = ((inst >> 3) & 15) as usize;
                let target = if rm == 15 { architectural_pc } else { r[rm] };
                if target & 1 == 0 {
                    return Err(format!(
                        "branch target {target:#010x} does not select Thumb state"
                    ));
                }
                if inst & 0x80 != 0 {
                    r[14] = next | 1;
                }
                next = target & !1;
            }
            0x4800..=0x4fff => {
                let address = (architectural_pc & !3).wrapping_add((inst & 0xff) << 2);
                r[((inst >> 8) & 7) as usize] =
                    bus.read::<4>(address).map_err(|error| error.to_string())?;
            }
            0x6000..=0x8fff => {
                let width = match inst >> 12 {
                    6 => 4,
                    7 => 1,
                    _ => 2,
                };
                let address = r[rn].wrapping_add(((inst >> 6) & 31) * width);
                if inst & 0x800 == 0 {
                    bus.write(address, width as usize, r[rd], output)
                        .map_err(|error| error.to_string())?;
                } else {
                    r[rd] = match width {
                        1 => bus.read::<1>(address),
                        2 => bus.read::<2>(address),
                        _ => bus.read::<4>(address),
                    }
                    .map_err(|error| error.to_string())?;
                }
            }
            0x9000..=0x9fff => {
                let register = ((inst >> 8) & 7) as usize;
                let address = r[13].wrapping_add((inst & 0xff) << 2);
                if inst & 0x800 == 0 {
                    bus.write(address, 4, r[register], output)
                        .map_err(|error| error.to_string())?;
                } else {
                    r[register] = bus.read::<4>(address).map_err(|error| error.to_string())?;
                }
            }
            0xa000..=0xafff => {
                let base = if inst & 0x800 == 0 {
                    architectural_pc & !3
                } else {
                    r[13]
                };
                r[((inst >> 8) & 7) as usize] = base.wrapping_add((inst & 0xff) << 2);
            }
            0xb000..=0xb0ff => {
                let bytes = (inst & 0x7f) << 2;
                r[13] = if inst & 0x80 == 0 {
                    r[13].wrapping_add(bytes)
                } else {
                    r[13].wrapping_sub(bytes)
                };
            }
            0xb400..=0xb5ff | 0xbc00..=0xbdff if inst & 0x1ff != 0 => {
                let pop = inst & 0x800 != 0;
                let mut mask = inst & 0xff;
                if inst & 0x100 != 0 {
                    mask |= 1 << if pop { 15 } else { 14 };
                }
                let bytes = mask.count_ones() * 4;
                let sp = r[13];
                if sp & 3 != 0 {
                    return Err(format!("unaligned stack pointer {sp:#010x}"));
                }
                if pop {
                    let mut restored = *r;
                    let mut address = sp;
                    while mask != 0 {
                        let register = mask.trailing_zeros() as usize;
                        mask &= mask - 1;
                        let value = bus
                            .read_ram::<4>(address)
                            .map_err(|error| error.to_string())?;
                        if register == 15 {
                            if value & 1 == 0 {
                                return Err(format!(
                                    "stack return target {value:#010x} does not select Thumb state"
                                ));
                            }
                            next = value & !1;
                        } else {
                            restored[register] = value;
                        }
                        address = address.wrapping_add(4);
                    }
                    restored[13] = sp.wrapping_add(bytes);
                    *r = restored;
                } else {
                    let new_sp = sp.wrapping_sub(bytes);
                    let stack = bus
                        .memory(new_sp, bytes as usize)
                        .map_err(|error| error.to_string())?;
                    for word in stack.as_chunks_mut::<4>().0 {
                        let register = mask.trailing_zeros() as usize;
                        mask &= mask - 1;
                        *word = r[register].to_le_bytes();
                    }
                    r[13] = new_sp;
                }
            }
            0xbe00..=0xbeff => return Ok(false),
            0xbf00 => {}
            0xd000..=0xddff => {
                if self.condition((inst >> 8) & 15) {
                    next = architectural_pc.wrapping_add(((inst as i8 as i32) * 2) as u32);
                }
            }
            0xe000..=0xe7ff => {
                let offset = (((inst << 21) as i32) >> 20) as u32;
                next = architectural_pc.wrapping_add(offset);
            }
            0xf000..=0xf7ff => {
                let low = bus.read_ram::<2>(next).map_err(|error| error.to_string())?;
                if low & 0xd000 != 0xd000 {
                    return Err(unsupported());
                }
                let sign = (inst >> 10) & 1;
                let i1 = 1 ^ (((low >> 13) & 1) ^ sign);
                let i2 = 1 ^ (((low >> 11) & 1) ^ sign);
                let offset = (sign << 24)
                    | (i1 << 23)
                    | (i2 << 22)
                    | ((inst & 0x3ff) << 12)
                    | ((low & 0x7ff) << 1);
                let offset = (((offset << 7) as i32) >> 7) as u32;
                r[14] = architectural_pc | 1;
                next = architectural_pc.wrapping_add(offset);
            }
            _ => return Err(unsupported()),
        }
        *pc = next;
        Ok(true)
    }
}
