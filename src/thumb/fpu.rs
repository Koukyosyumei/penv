use std::io::Write;

use crate::bus::Bus;

use super::Thumb;

impl Thumb {
    pub(super) fn execute_fpu(
        &mut self,
        high: u32,
        low: u32,
        pc: u32,
        r: &mut [u32; 32],
        bus: &mut Bus,
        output: &mut impl Write,
    ) -> Result<bool, String> {
        if !matches!(high & 0xff00, 0xec00 | 0xed00 | 0xee00)
            || !matches!(low & 0xf00, 0xa00 | 0xb00)
        {
            return Ok(false);
        }
        if !bus.nvic.as_ref().is_some_and(|nvic| nvic.fpu_access()) {
            return Err("Cortex-M FPU access is disabled by CPACR".into());
        }

        let sd = ((low >> 12) * 2 + ((high >> 6) & 1)) as usize;
        let sn = ((high & 15) * 2 + ((low >> 7) & 1)) as usize;
        let sm = ((low & 15) * 2 + ((low >> 5) & 1)) as usize;
        if high & 0xffe0 == 0xec40 && low & 0xd0 == 0x10 {
            let rt = (low >> 12) as usize;
            let rt2 = (high & 15) as usize;
            let first = if low & 0x100 == 0 {
                sm
            } else {
                ((low & 15) * 2 + (low & 0x20)) as usize
            };
            if matches!(rt, 13 | 15)
                || matches!(rt2, 13 | 15)
                || first + 1 >= self.fp.len()
                || (high & 0x10 != 0 && rt == rt2)
            {
                return Ok(false);
            }
            if high & 0x10 == 0 {
                self.fp[first] = r[rt];
                self.fp[first + 1] = r[rt2];
            } else {
                r[rt] = self.fp[first];
                r[rt2] = self.fp[first + 1];
            }
        } else if high & 0xfe00 == 0xec00 {
            let rn = (high & 15) as usize;
            let base = if rn == 15 { pc & !3 } else { r[rn] };
            let offset = (low & 255) * 4;
            let load = high & 0x10 != 0;
            let up = high & 0x80 != 0;
            let writeback = high & 0x20 != 0;
            let preindex = high & 0x100 != 0;
            let (first, count, mut address) = if preindex && !writeback {
                let address = if up {
                    base.wrapping_add(offset)
                } else {
                    base.wrapping_sub(offset)
                };
                if low & 0x100 == 0 {
                    (sd, 1, address)
                } else {
                    let first = (low >> 12) as usize * 2 + (((high >> 6) & 1) as usize * 32);
                    (first, 2, address)
                }
            } else if rn != 15 && ((!preindex && up) || (preindex && !up && writeback)) {
                let first = if low & 0x100 == 0 {
                    sd
                } else {
                    (low >> 12) as usize * 2 + (((high >> 6) & 1) as usize * 32)
                };
                if low & 0x100 != 0 && offset & 7 != 0 {
                    return Ok(false);
                }
                (
                    first,
                    (low & 255) as usize,
                    if up { base } else { base.wrapping_sub(offset) },
                )
            } else {
                return Ok(false);
            };
            if count == 0 || first + count > self.fp.len() {
                return Ok(false);
            }
            if address & 3 != 0 {
                return Err(format!("unaligned FPU transfer at {address:#010x}"));
            }
            for value in &mut self.fp[first..first + count] {
                if load {
                    *value = bus.thumb_read::<4>(address)?;
                } else {
                    bus.thumb_write(address, 4, *value, output)?;
                }
                address = address.wrapping_add(4);
            }
            if writeback {
                r[rn] = if up {
                    base.wrapping_add(offset)
                } else {
                    base.wrapping_sub(offset)
                };
            }
        } else if low & 0x0f7f == 0x0a10 && matches!(high, 0xeee1 | 0xeef1) {
            let rt = (low >> 12) as usize;
            match (high, rt) {
                (0xeef1, 15) => {
                    self.set_apsr((self.apsr() & 0x0fff_ffff) | (self.fpscr & 0xf000_0000))
                }
                (0xeef1, 0..=14) if rt != 13 => r[rt] = self.fpscr,
                (0xeee1, 0..=14) if rt != 13 => self.fpscr = r[rt],
                _ => return Ok(false),
            }
        } else if low & 0x0f7f == 0x0a10 && matches!(high & 0xffe0, 0xee00) {
            let rt = (low >> 12) as usize;
            if matches!(rt, 13 | 15) {
                return Ok(false);
            }
            if high & 0x10 != 0 {
                r[rt] = self.fp[sn];
            } else {
                self.fp[sn] = r[rt];
            }
        } else if low & 0x110 == 0 {
            let a = f32::from_bits(self.fp[sn]);
            let b = f32::from_bits(self.fp[sm]);
            let acc = f32::from_bits(self.fp[sd]);
            let subtract = low & 0x40 != 0;
            let value = match high & 0xffb0 {
                0xee00 if subtract => (acc - a * b).to_bits(),
                0xee00 => (acc + a * b).to_bits(),
                0xee20 if subtract => (-(a * b)).to_bits(),
                0xee20 => (a * b).to_bits(),
                0xee30 if subtract => (a - b).to_bits(),
                0xee30 => (a + b).to_bits(),
                0xee80 if !subtract => (a / b).to_bits(),
                0xee90 if subtract => a.mul_add(b, -acc).to_bits(),
                0xee90 => (-a).mul_add(b, -acc).to_bits(),
                0xeea0 if subtract => (-a).mul_add(b, acc).to_bits(),
                0xeea0 => a.mul_add(b, acc).to_bits(),
                0xeeb0 if low & 0xe0 == 0 => {
                    let imm = ((high & 15) << 4) | (low & 15);
                    let exponent = if imm & 0x40 == 0 { 128 } else { 124 };
                    ((imm & 0x80) << 24)
                        | ((exponent + ((imm >> 4) & 3)) << 23)
                        | ((imm & 15) << 19)
                }
                0xeeb0 if subtract => match (high & 15, low & 0x80 != 0) {
                    (0, false) => self.fp[sm],
                    (0, true) => self.fp[sm] & 0x7fff_ffff,
                    (1, false) => self.fp[sm] ^ 0x8000_0000,
                    (1, true) => b.sqrt().to_bits(),
                    (4 | 5, _) => {
                        let rhs = if high & 15 == 5 { 0.0 } else { b };
                        let flags = match acc.partial_cmp(&rhs) {
                            Some(std::cmp::Ordering::Less) => 0x8000_0000,
                            Some(std::cmp::Ordering::Equal) => 0x6000_0000,
                            Some(std::cmp::Ordering::Greater) => 0x2000_0000,
                            None => 0x3000_0000,
                        };
                        self.fpscr = (self.fpscr & 0x0fff_ffff) | flags;
                        self.fp[sd]
                    }
                    (8, false) => (self.fp[sm] as f32).to_bits(),
                    (8, true) => (self.fp[sm] as i32 as f32).to_bits(),
                    (12, true) => b as u32,
                    (13, true) => b as i32 as u32,
                    _ => return Ok(false),
                },
                _ => return Ok(false),
            };
            self.fp[sd] = value;
        } else {
            return Ok(false);
        }
        self.fp_active = true;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floating_point_access_and_extended_exception_frame() {
        let mut bus = Bus::default();
        bus.map_ram(0, 0x100).unwrap();
        bus.map_ram(0x2000_0000, 0x208).unwrap();
        bus.map_nvic(0xe0, None, true).unwrap();
        bus.memory(44, 4)
            .unwrap()
            .copy_from_slice(&0x81u32.to_le_bytes());
        let mut thumb = Thumb::new(Some(0));
        thumb.thumb2 = true;
        let mut output = std::io::sink();
        let mut registers = [0; 32];
        registers[13] = 0x2000_0204;
        let multiply = |thumb: &mut Thumb, registers: &mut [u32; 32], bus: &mut Bus| {
            thumb.execute_fpu(0xee20, 0x0a28, 0, registers, bus, &mut std::io::sink())
        };
        assert!(multiply(&mut thumb, &mut registers, &mut bus).is_err());
        bus.thumb_write(0xe000_ed88, 4, 0x00f0_0000, &mut output)
            .unwrap();
        thumb.fp[0] = 2.0f32.to_bits();
        thumb.fp[17] = 3.0f32.to_bits();
        assert!(multiply(&mut thumb, &mut registers, &mut bus).unwrap());
        assert_eq!(thumb.fp[0], 6.0f32.to_bits());
        thumb.fpscr = 0x6000_0000;
        assert_eq!(
            thumb
                .exception_entry(11, 0x61, &mut registers, &mut bus)
                .unwrap(),
            0x80
        );
        assert_eq!(registers[14], 0xffff_ffe9);
        thumb.fp[0] = 0;
        thumb.fpscr = 0;
        assert_eq!(
            thumb.branch(registers[14], &mut registers, &bus).unwrap(),
            0x60
        );
        assert_eq!(registers[13], 0x2000_0204);
        assert_eq!(thumb.fp[0], 6.0f32.to_bits());
        assert_eq!(thumb.fpscr, 0x6000_0000);
        assert!(thumb.fp_active);
    }
}
