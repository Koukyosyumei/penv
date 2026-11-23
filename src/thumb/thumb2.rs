use super::ExecuteError;
use super::{Bus, Thumb, Write};

impl Thumb {
    fn shift_immediate(&self, value: u32, kind: u32, shift: u32) -> (u32, bool) {
        match (kind, shift) {
            (0, 0) => (value, self.carry),
            (0, n) => (value << n, value & (1 << (32 - n)) != 0),
            (1, 0) => (0, value >> 31 != 0),
            (1, n) => (value >> n, value & (1 << (n - 1)) != 0),
            (2, 0) => (((value as i32) >> 31) as u32, value >> 31 != 0),
            (2, n) => (((value as i32) >> n) as u32, value & (1 << (n - 1)) != 0),
            (3, 0) => ((value >> 1) | (u32::from(self.carry) << 31), value & 1 != 0),
            (_, n) => {
                let result = value.rotate_right(n);
                (result, result >> 31 != 0)
            }
        }
    }

    pub(super) fn modified_immediate(&mut self, high: u32, low: u32, r: &mut [u32; 32]) -> bool {
        if high & 0xfa00 != 0xf000 || low & 0x8000 != 0 {
            return false;
        }
        let encoded = ((high & 0x400) << 1) | ((low & 0x7000) >> 4) | (low & 0xff);
        let byte = encoded & 0xff;
        let (immediate, carry) = if encoded < 0x400 {
            let pattern = match encoded >> 8 {
                0 => byte,
                1 => byte * 0x0001_0001,
                2 => byte * 0x0100_0100,
                _ => byte * 0x0101_0101,
            };
            (pattern, self.carry)
        } else {
            let rotated = (0x80 | (encoded & 0x7f)).rotate_right(encoded >> 7);
            (rotated, rotated >> 31 != 0)
        };
        self.wide_alu(high, low, immediate, carry, r)
    }

    fn wide_alu(
        &mut self,
        high: u32,
        low: u32,
        operand: u32,
        carry: bool,
        r: &mut [u32; 32],
    ) -> bool {
        let op = (high >> 5) & 15;
        let rn = (high & 15) as usize;
        let rd = ((low >> 8) & 15) as usize;
        let update_flags = high & 0x10 != 0;
        if (rd == 15 && !(update_flags && matches!(op, 0 | 4 | 8 | 13)))
            || (rn == 15 && !matches!(op, 2 | 3))
        {
            return false;
        }
        let lhs = if rn == 15 { 0 } else { r[rn] };
        let result = match op {
            0 => lhs & operand,
            1 => lhs & !operand,
            2 => lhs | operand,
            3 => lhs | !operand,
            4 => lhs ^ operand,
            10 | 11 => {
                let saved = self.apsr();
                r[rd] = self.data_processing(op - 5, lhs, operand).unwrap();
                if !update_flags {
                    self.set_apsr(saved);
                }
                return true;
            }
            8 | 13 | 14 => {
                let (lhs, rhs) = if op == 14 {
                    (operand, lhs)
                } else {
                    (lhs, operand)
                };
                let value = if update_flags {
                    self.arithmetic(lhs, rhs, op != 8)
                } else if op == 8 {
                    lhs.wrapping_add(rhs)
                } else {
                    lhs.wrapping_sub(rhs)
                };
                if rd != 15 {
                    r[rd] = value;
                }
                return true;
            }
            _ => return false,
        };
        if rd != 15 {
            r[rd] = result;
        }
        if update_flags {
            self.nz(result);
            self.carry = carry;
        }
        true
    }

    pub(super) fn execute_thumb2(
        &mut self,
        high: u32,
        low: u32,
        next: &mut u32,
        r: &mut [u32; 32],
        bus: &mut Bus,
        output: &mut impl Write,
    ) -> Result<(), ExecuteError> {
        let unsupported = || format!("unsupported Thumb instruction {high:#06x} {low:#06x}");
        let rn = (high & 15) as usize;
        let rt = (low >> 12) as usize;
        let rm = (low & 15) as usize;
        let rd = ((low >> 8) & 15) as usize;
        if matches!(high, 0xec00..=0xeeff) && self.execute_fpu(high, low, *next, r, bus, output)? {
            return Ok(());
        }
        match high {
            0xe8d0..=0xe8df if low & 0xffe0 == 0xf000 && rm < 15 => {
                let base = if rn == 15 { *next } else { r[rn] };
                let offset = if low & 0x10 == 0 {
                    bus.read_cortex_data::<1>(base.wrapping_add(r[rm]))
                } else {
                    self.read_data::<2>(bus, base.wrapping_add(r[rm].wrapping_mul(2)))
                }?;
                *next = next.wrapping_add(offset * 2);
            }
            _ if matches!(high & 0xffc0, 0xe880 | 0xe900)
                && rn < 15
                && low.count_ones() >= 2
                && low & 0xa000 == 0
                && (high & 0x20 == 0 || low & (1 << rn) == 0) =>
            {
                let load = high & 0x10 != 0;
                let bytes = low.count_ones() * 4;
                let base = r[rn];
                let decrement = high & 0x100 != 0;
                let mut address = if decrement {
                    base.wrapping_sub(bytes)
                } else {
                    base
                };
                for register in 0..15 {
                    if low & (1 << register) == 0 {
                        continue;
                    }
                    if load {
                        r[register] = bus.read_cortex_data::<4>(address)?;
                    } else {
                        bus.write(address, 4, r[register], output)?;
                    }
                    address = address.wrapping_add(4);
                }
                if high & 0x20 != 0 {
                    r[rn] = if decrement {
                        base.wrapping_sub(bytes)
                    } else {
                        address
                    };
                }
            }
            0xe850..=0xe85e if low & 0x0f00 == 0x0f00 && rt < 13 => {
                let address = r[rn].wrapping_add((low & 0xff) * 4);
                r[rt] = bus.read_exclusive(address)?;
            }
            0xe840..=0xe84e if rt < 13 && rd < 13 && rd != rt && rd != rn => {
                let address = r[rn].wrapping_add((low & 0xff) * 4);
                r[rd] = bus.write_exclusive(address, r[rt], output)?;
            }
            _ if high & 0xfe40 == 0xe840
                && high & 0x120 != 0
                && rn < 15
                && !matches!(rt, 13 | 15)
                && !matches!(rd, 13 | 15)
                && (high & 0x10 == 0 || rt != rd)
                && (high & 0x20 == 0 || (rn != rt && rn != rd)) =>
            {
                let offset = (low & 0xff) * 4;
                let indexed = if high & 0x80 != 0 {
                    r[rn].wrapping_add(offset)
                } else {
                    r[rn].wrapping_sub(offset)
                };
                let address = if high & 0x100 != 0 { indexed } else { r[rn] };
                let second = address.wrapping_add(4);
                if high & 0x10 != 0 {
                    let first = bus.read_cortex_data::<4>(address)?;
                    let second = bus.read_cortex_data::<4>(second)?;
                    r[rt] = first;
                    r[rd] = second;
                } else {
                    bus.write(address, 4, r[rt], output)?;
                    bus.write(second, 4, r[rd], output)?;
                }
                if high & 0x20 != 0 {
                    r[rn] = indexed;
                }
            }
            _ if matches!(high & 0xfbf0, 0xf200 | 0xf2a0) && rn < 15 && rd < 15 => {
                let immediate = ((high & 0x400) << 1) | ((low & 0x7000) >> 4) | (low & 0xff);
                r[rd] = if high & 0x80 == 0 {
                    r[rn].wrapping_add(immediate)
                } else {
                    r[rn].wrapping_sub(immediate)
                };
            }
            _ if matches!(high & 0xfbf0, 0xf240 | 0xf2c0)
                && !matches!(rd, 13 | 15)
                && low & 0x8000 == 0 =>
            {
                let value = ((high & 15) << 12)
                    | ((high & 0x400) << 1)
                    | ((low & 0x7000) >> 4)
                    | (low & 0xff);
                r[rd] = if high & 0x80 != 0 {
                    (r[rd] & 0xffff) | (value << 16)
                } else {
                    value
                };
            }
            _ if matches!(high & 0xfff0, 0xf340 | 0xf3c0)
                && low & 0x8020 == 0
                && !matches!(rn, 13 | 15)
                && !matches!(rd, 13 | 15) =>
            {
                let lsb = ((low & 0x7000) >> 10) | ((low & 0xc0) >> 6);
                let width = (low & 31) + 1;
                if lsb + width > 32 {
                    return Err(unsupported().into());
                }
                let field = (r[rn] >> lsb) << (32 - width);
                r[rd] = if high & 0x80 != 0 {
                    field >> (32 - width)
                } else {
                    ((field as i32) >> (32 - width)) as u32
                };
            }
            _ if high & 0xfff0 == 0xf360 && rd < 15 => {
                let lsb = ((low >> 10) & 0x1c) | ((low >> 6) & 3);
                let msb = low & 31;
                if msb < lsb {
                    return Err(unsupported().into());
                }
                let mask = (u32::MAX >> (31 - msb + lsb)) << lsb;
                let value = if rn == 15 { 0 } else { r[rn] };
                r[rd] = (r[rd] & !mask) | ((value << lsb) & mask);
            }
            0xea00..=0xebff if low & 0x8000 == 0 && rm < 15 => {
                let shift = ((low >> 10) & 0x1c) | ((low >> 6) & 3);
                let kind = (low >> 4) & 3;
                let (shifted, carry) = self.shift_immediate(r[rm], kind, shift);
                if !self.wide_alu(high, low, shifted, carry, r) {
                    return Err(unsupported().into());
                }
            }
            0xfa00..=0xfa1f | 0xfa40..=0xfa5f if low & 0xf0c0 == 0xf080 && rd < 15 && rm < 15 => {
                let value = r[rm].rotate_right(((low >> 4) & 3) * 8);
                let extended = match high & 0xfff0 {
                    0xfa00 => value as i16 as i32 as u32,
                    0xfa10 => value & 0xffff,
                    0xfa40 => value as i8 as i32 as u32,
                    _ => value & 0xff,
                };
                let addend = if rn == 15 { 0 } else { r[rn] };
                r[rd] = extended.wrapping_add(addend);
            }
            0xfa00..=0xfa7f if low & 0xf0f0 == 0xf000 && rn < 15 && rd < 15 && rm < 15 => {
                let saved = self.apsr();
                let kind = ((high >> 5) & 3) as usize;
                r[rd] = self
                    .data_processing([2, 3, 4, 7][kind], r[rn], r[rm])
                    .unwrap();
                if high & 0x10 == 0 {
                    self.set_apsr(saved);
                }
            }
            0xfa90..=0xfa9e if low & 0xf0c0 == 0xf080 && rm == rn && rd < 15 => {
                let value = r[rn];
                r[rd] = match (low >> 4) & 3 {
                    0 => value.swap_bytes(),
                    1 => ((value & 0xff00_ff00) >> 8) | ((value & 0x00ff_00ff) << 8),
                    2 => value.reverse_bits(),
                    _ => (value as u16).swap_bytes() as i16 as i32 as u32,
                };
            }
            0xfab0..=0xfabe if low & 0xf0f0 == 0xf080 && rm == rn && rd < 15 => {
                r[rd] = r[rn].leading_zeros();
            }
            0xfb90..=0xfb9e | 0xfbb0..=0xfbbe if low & 0xf0f0 == 0xf0f0 && rd < 15 && rm < 15 => {
                let numerator = r[rn];
                let denominator = r[rm];
                if denominator == 0 {
                    if bus
                        .nvic
                        .as_ref()
                        .is_some_and(|nvic| nvic.divide_by_zero_traps())
                    {
                        return Err("Cortex-M division by zero".into());
                    }
                    r[rd] = 0;
                } else if high & 0x20 != 0 {
                    r[rd] = numerator / denominator;
                } else {
                    r[rd] = (numerator as i32).wrapping_div(denominator as i32) as u32;
                }
            }
            0xfb80..=0xfb8c | 0xfba0..=0xfbac
                if low & 0xf0 == 0 && rt < 13 && rd < 13 && rt != rd && rm < 13 =>
            {
                let product = if high & 0x20 != 0 {
                    u64::from(r[rn]) * u64::from(r[rm])
                } else {
                    (i64::from(r[rn] as i32) * i64::from(r[rm] as i32)) as u64
                };
                r[rt] = product as u32;
                r[rd] = (product >> 32) as u32;
            }
            0xfb10..=0xfb1e if low & 0xf0c0 == 0xf000 && rd < 15 && rm < 15 => {
                let lhs = (r[rn] >> ((low & 0x20) >> 1)) as i16 as i32;
                let rhs = (r[rm] >> (low & 0x10)) as i16 as i32;
                r[rd] = (lhs * rhs) as u32;
            }
            0xfb00..=0xfb0e
                if low & 0xe0 == 0
                    && rn != 13
                    && !matches!(rd, 13 | 15)
                    && !matches!(rm, 13 | 15)
                    && rt != 13
                    && (low & 0x10 == 0 || rt != 15) =>
            {
                let product = r[rn].wrapping_mul(r[rm]);
                r[rd] = if rt == 15 {
                    product
                } else if low & 0x10 == 0 {
                    r[rt].wrapping_add(product)
                } else {
                    r[rt].wrapping_sub(product)
                };
            }
            0xf990..=0xf99e | 0xf9b0..=0xf9be if rt < 15 => {
                let address = r[rn].wrapping_add(low & 0xfff);
                r[rt] = if high & 0x20 == 0 {
                    self.read_data::<1>(bus, address)
                        .map(|value| value as i8 as i32 as u32)
                } else {
                    self.read_data::<2>(bus, address)
                        .map(|value| value as i16 as i32 as u32)
                }?;
            }
            0xf85f | 0xf8df => {
                let base = *next & !3;
                let offset = low & 0xfff;
                let address = if high & 0x80 == 0 {
                    base.wrapping_sub(offset)
                } else {
                    base.wrapping_add(offset)
                };
                let value = self.read_data::<4>(bus, address)?;
                if rt == 15 {
                    *next = self.branch(value, r, bus)?;
                } else {
                    r[rt] = value;
                }
            }
            0xf880..=0xf8df if rn < 15 && rt < 15 => {
                let address = r[rn].wrapping_add(low & 0xfff);
                let width = 1 << ((high >> 5) & 3);
                if high & 0x10 != 0 {
                    r[rt] = match width {
                        1 => self.read_data::<1>(bus, address),
                        2 => self.read_data::<2>(bus, address),
                        _ => self.read_data::<4>(bus, address),
                    }?;
                } else {
                    self.write_data(bus, address, width, r[rt], output)?;
                }
            }
            0xf800..=0xf85e | 0xf910..=0xf93e
                if matches!(
                    high & 0xffb0,
                    0xf800 | 0xf810 | 0xf820 | 0xf830 | 0xf910 | 0xf930
                ) && rn < 15
                    && (rt < 15 || high & 0xfff0 == 0xf850)
                    && (low & 0x0fc0 == 0 || (low & 0x800 != 0 && low & 0x500 != 0)) =>
            {
                let width = 1 << ((high >> 5) & 3);
                let writeback = low & 0x100 != 0;
                if writeback && rn == rt {
                    return Err(unsupported().into());
                }
                let register_offset = low & 0x800 == 0;
                if register_offset && rm == 15 {
                    return Err(unsupported().into());
                }
                let offset = if register_offset {
                    r[rm] << ((low >> 4) & 3)
                } else {
                    low & 0xff
                };
                let indexed = if !register_offset && low & 0x200 == 0 {
                    r[rn].wrapping_sub(offset)
                } else {
                    r[rn].wrapping_add(offset)
                };
                let address = if register_offset || low & 0x400 != 0 {
                    indexed
                } else {
                    r[rn]
                };
                if high & 0x10 == 0 {
                    self.write_data(bus, address, width, r[rt], output)?;
                } else {
                    let signed = high & 0x100 != 0;
                    let value = match (width, signed) {
                        (1, true) => self
                            .read_data::<1>(bus, address)
                            .map(|value| value as i8 as i32 as u32),
                        (2, true) => self
                            .read_data::<2>(bus, address)
                            .map(|value| value as i16 as i32 as u32),
                        (1, false) => self.read_data::<1>(bus, address),
                        (2, false) => self.read_data::<2>(bus, address),
                        _ => self.read_data::<4>(bus, address),
                    }?;
                    if writeback {
                        r[rn] = indexed;
                    }
                    if rt == 15 {
                        *next = self.branch(value, r, bus)?;
                    } else {
                        r[rt] = value;
                    }
                }
                if writeback && high & 0x10 == 0 {
                    r[rn] = indexed;
                }
            }
            _ => return Err(unsupported().into()),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_branch_requires_thumb2_and_preserves_lr() {
        let mut bus = Bus::default();
        bus.map_ram(0, 16).unwrap();
        // b.w +4; nop; nop; mov.w r1, #0xaa
        bus.memory(0, 12).unwrap().copy_from_slice(&[
            0x00, 0xf0, 0x02, 0xb8, 0x00, 0xbf, 0x00, 0xbf, 0x4f, 0xf0, 0xaa, 0x01,
        ]);
        let mut pc = 0;
        let mut registers = [0; 32];
        registers[14] = 0x101;
        let mut output = Vec::new();
        let mut thumb = Thumb::new(Some(0));
        assert!(
            thumb
                .step(&mut pc, &mut registers, &mut bus, &mut output)
                .is_err()
        );
        thumb.thumb2 = true;
        thumb
            .step(&mut pc, &mut registers, &mut bus, &mut output)
            .unwrap();
        assert_eq!((pc, registers[14]), (8, 0x101));
        thumb
            .step(&mut pc, &mut registers, &mut bus, &mut output)
            .unwrap();
        assert_eq!((pc, registers[1]), (12, 0xaa));
    }

    #[test]
    fn strd_post_index_stores_two_words_and_updates_the_base() {
        let mut bus = Bus::default();
        bus.map_ram(0, 32).unwrap();
        // strd r0, r1, [r3], #8
        bus.memory(0, 4)
            .unwrap()
            .copy_from_slice(&[0xe3, 0xe8, 0x02, 0x01]);
        let mut thumb = Thumb::new(Some(0));
        let mut pc = 0;
        let mut registers = [0; 32];
        registers[0] = 0x7654_3210;
        registers[1] = 0xfedc_ba98;
        registers[3] = 16;
        assert!(
            thumb
                .step(&mut pc, &mut registers, &mut bus, &mut std::io::sink())
                .is_err()
        );
        thumb.thumb2 = true;
        thumb
            .step(&mut pc, &mut registers, &mut bus, &mut std::io::sink())
            .unwrap();
        assert_eq!((pc, registers[3]), (4, 24));
        assert_eq!(bus.read_ram::<4>(16).unwrap(), registers[0]);
        assert_eq!(bus.read_ram::<4>(20).unwrap(), registers[1]);
    }
}
