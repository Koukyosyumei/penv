use super::{Bus, Thumb, Write};

impl Thumb {
    pub(super) fn modified_immediate(&mut self, high: u32, low: u32, r: &mut [u32; 32]) -> bool {
        if high & 0xfa00 != 0xf000 || low & 0x8000 != 0 {
            return false;
        }
        let op = (high >> 5) & 15;
        let rn = (high & 15) as usize;
        let rd = ((low >> 8) & 15) as usize;
        let update_flags = high & 0x10 != 0;
        if rd == 15 || (rn == 15 && !matches!(op, 2 | 3)) {
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
        let lhs = if rn == 15 { 0 } else { r[rn] };
        let result = match op {
            0 => lhs & immediate,
            1 => lhs & !immediate,
            2 => lhs | immediate,
            3 => lhs | !immediate,
            4 => lhs ^ immediate,
            8 | 13 => {
                r[rd] = if update_flags {
                    self.arithmetic(lhs, immediate, op == 13)
                } else if op == 13 {
                    lhs.wrapping_sub(immediate)
                } else {
                    lhs.wrapping_add(immediate)
                };
                return true;
            }
            _ => return false,
        };
        r[rd] = result;
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
        r: &mut [u32; 32],
        bus: &mut Bus,
        output: &mut impl Write,
    ) -> Result<(), String> {
        let unsupported = || format!("unsupported Thumb instruction {high:#06x} {low:#06x}");
        let rn = (high & 15) as usize;
        let rt = (low >> 12) as usize;
        let rm = (low & 15) as usize;
        let rd = ((low >> 8) & 15) as usize;
        match high {
            0xe9c0..=0xe9de if rn < 15 && rt < 13 && rd < 13 && (high & 0x10 == 0 || rt != rd) => {
                let address = r[rn].wrapping_add((low & 0xff) * 4);
                let second = address.wrapping_add(4);
                if high & 0x10 != 0 {
                    let first = bus.read::<4>(address).map_err(|error| error.to_string())?;
                    let second = bus.read::<4>(second).map_err(|error| error.to_string())?;
                    r[rt] = first;
                    r[rd] = second;
                } else {
                    bus.write(address, 4, r[rt], output)
                        .map_err(|error| error.to_string())?;
                    bus.write(second, 4, r[rd], output)
                        .map_err(|error| error.to_string())?;
                }
            }
            0xea40..=0xea4e | 0xea80..=0xea8e
                if rt < 8 && rd < 15 && rm < 15 && low & 0x30 == 0 =>
            {
                let shift = ((low >> 10) & 0x1c) | ((low >> 6) & 3);
                let operand = r[rm] << shift;
                r[rd] = if high & 0x80 == 0 {
                    r[rn] | operand
                } else {
                    r[rn] ^ operand
                };
            }
            0xfb00..=0xfb0c if low & 0xf0 == 0 && rm < 13 && rd < 13 && (rt < 13 || rt == 15) => {
                let addend = if rt == 15 { 0 } else { r[rt] };
                r[rd] = r[rn].wrapping_mul(r[rm]).wrapping_add(addend);
            }
            0xf880..=0xf8df if rn < 15 && rt < 15 => {
                let address = r[rn].wrapping_add(low & 0xfff);
                let width = 1 << ((high >> 5) & 3);
                if high & 0x10 != 0 {
                    r[rt] = match width {
                        1 => bus.read::<1>(address),
                        2 => bus.read::<2>(address),
                        _ => bus.read::<4>(address),
                    }
                    .map_err(|error| error.to_string())?;
                } else {
                    bus.write(address, width, r[rt], output)
                        .map_err(|error| error.to_string())?;
                }
            }
            0xf800..=0xf85e
                if matches!(high & 0xffb0, 0xf800 | 0xf810)
                    && rt < 15
                    && (low & 0x0fc0 == 0 || (low & 0x800 != 0 && low & 0x500 != 0)) =>
            {
                let width = if high & 0x40 == 0 { 1 } else { 4 };
                let writeback = low & 0x100 != 0;
                if writeback && rn == rt {
                    return Err(unsupported());
                }
                let register_offset = low & 0x800 == 0;
                if register_offset && rm == 15 {
                    return Err(unsupported());
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
                    bus.write(address, width, r[rt], output)
                        .map_err(|error| error.to_string())?;
                } else {
                    r[rt] = if width == 1 {
                        bus.read::<1>(address)
                    } else {
                        bus.read::<4>(address)
                    }
                    .map_err(|error| error.to_string())?;
                }
                if writeback {
                    r[rn] = indexed;
                }
            }
            _ => return Err(unsupported()),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_branch_preserves_lr_and_runs_only_on_cortex_m4() {
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
}
