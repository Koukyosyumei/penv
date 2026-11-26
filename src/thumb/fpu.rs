use std::io::Write;

use crate::bus::{Bus, BusError};
use crate::nvic::Nvic;

use super::Thumb;

impl Thumb {
    pub(crate) fn preserve_lazy_fp(&mut self, bus: &mut Bus) -> Result<bool, String> {
        let Some((address, hardfault_ready, full_frame)) =
            bus.nvic.as_ref().and_then(Nvic::pending_lazy_fp)
        else {
            return Ok(false);
        };
        let mut state = [0; 34];
        state[..16].copy_from_slice(&self.fp[..16]);
        state[16] = self.fpscr;
        if full_frame {
            state[18..].copy_from_slice(&self.fp[16..]);
        }
        let words = if full_frame { 34 } else { 18 };
        match bus.write_cortex_m_exception_stack(address, &state[..words]) {
            Ok(()) => {}
            Err(BusError::SlaveFault(_) | BusError::Unmapped(..)) => {
                let nvic = bus.nvic.as_mut().unwrap();
                nvic.record_lazy_fault();
                if !hardfault_ready {
                    nvic.enter_lockup();
                    self.lockup();
                    return Ok(true);
                }
                nvic.force_hard_fault();
                nvic.set_hard_fault_pending(true);
            }
            Err(error) => return Err(error.to_string()),
        }
        bus.nvic.as_mut().unwrap().clear_lazy_fp();
        Ok(false)
    }

    pub(super) fn transfer_secure_fp(
        &mut self,
        instruction: u32,
        r: &[u32; 32],
        bus: &mut Bus,
        output: &mut impl Write,
    ) -> Result<(), String> {
        let rn = (instruction & 15) as usize;
        if rn == 15 {
            return Err("secure FP transfer cannot use PC as its base register".into());
        }
        self.fp_active = true;
        let mut fp = self.fp;
        let load = instruction & 0x10 != 0;
        let count = if bus.nvic.as_ref().is_some_and(Nvic::secure_full_fp_frame) {
            32
        } else {
            16
        };
        for index in 0..count {
            let offset = if index < 16 { index } else { index + 2 };
            let address = r[rn].wrapping_add(4 * offset as u32);
            if load {
                fp[index] = bus.thumb_read::<4>(address)?;
            } else {
                bus.thumb_write(address, 4, fp[index], output)?;
            }
        }
        let address = r[rn].wrapping_add(64);
        if load {
            self.fpscr = bus.thumb_read::<4>(address)?;
            self.fp = fp;
        } else {
            bus.thumb_write(address, 4, self.fpscr, output)?;
        }
        Ok(())
    }

    pub(crate) fn monitor_fp_register(
        &mut self,
        register: &str,
        value: Option<u64>,
    ) -> Option<Result<Option<u64>, String>> {
        if register == "fpscr" {
            return Some(match value {
                Some(value) => u32::try_from(value)
                    .map(|value| {
                        self.fpscr = value;
                        None
                    })
                    .map_err(|_| "FPSCR value exceeds 32 bits".into()),
                None => Ok(Some(u64::from(self.fpscr))),
            });
        }
        let (prefix, index) = register.split_at_checked(1)?;
        let index = index.parse::<usize>().ok()?;
        match prefix {
            "s" => {
                let bits = self.fp.get_mut(index)?;
                Some(match value {
                    Some(value) => u32::try_from(value)
                        .map(|value| {
                            *bits = value;
                            None
                        })
                        .map_err(|_| "single-precision register value exceeds 32 bits".into()),
                    None => Ok(Some(u64::from(*bits))),
                })
            }
            "d" if index < self.fp.len() / 2 => {
                let index = index * 2;
                Some(match value {
                    Some(value) => {
                        self.fp[index] = value as u32;
                        self.fp[index + 1] = (value >> 32) as u32;
                        Ok(None)
                    }
                    None => Ok(Some(
                        u64::from(self.fp[index]) | (u64::from(self.fp[index + 1]) << 32),
                    )),
                })
            }
            _ => None,
        }
    }

    fn expand_fp_immediate(high: u32, low: u32) -> u32 {
        let imm = ((high & 15) << 4) | (low & 15);
        let exponent = if imm & 0x40 == 0 { 128 } else { 124 };
        ((imm & 0x80) << 24) | ((exponent + ((imm >> 4) & 3)) << 23) | ((imm & 15) << 19)
    }

    fn fp_round_kind(high: u32, low: u32) -> Option<(u32, bool)> {
        if !matches!(low & 0xf50, 0xa40 | 0xb40) {
            return None;
        }
        match (high & !0x40, low & 0x80 != 0) {
            (0xfeb8, false) => Some((4, false)),
            (0xfeb9, false) => Some((0, false)),
            (0xfeba, false) => Some((1, false)),
            (0xfebb, false) => Some((2, false)),
            (0xeeb6, true) => Some((3, false)),
            (0xeeb6, false) => Some((5, false)),
            (0xeeb7, false) => Some((5, true)),
            _ => None,
        }
    }

    fn fp_convert_kind(high: u32, low: u32) -> Option<u32> {
        if !matches!(low & 0xf50, 0xa40 | 0xb40) {
            return None;
        }
        match high & !0x40 {
            0xfebc => Some(4),
            0xfebd => Some(0),
            0xfebe => Some(1),
            0xfebf => Some(2),
            _ => None,
        }
    }

    fn round_fp_value(value: f64, mode: u32) -> f64 {
        match mode {
            0 => value.round_ties_even(),
            1 => value.ceil(),
            2 => value.floor(),
            3 => value.trunc(),
            _ => value.round(),
        }
    }

    fn convert_fp_to_integer(&mut self, high: u32, low: u32, kind: u32) -> bool {
        let destination = ((low >> 12) * 2 + ((high >> 6) & 1)) as usize;
        let input = if low & 0x100 == 0 {
            let source = ((low & 15) * 2 + ((low >> 5) & 1)) as usize;
            f64::from(f32::from_bits(self.fp[source]))
        } else {
            let source = (low & 15) as usize * 2 + (((low >> 5) & 1) as usize * 32);
            if source >= self.fp.len() {
                return false;
            }
            f64::from_bits(u64::from(self.fp[source]) | (u64::from(self.fp[source + 1]) << 32))
        };
        let value = Self::round_fp_value(input, kind);
        let signed = low & 0x80 != 0;
        let (min, max) = if signed {
            (f64::from(i32::MIN), f64::from(i32::MAX))
        } else {
            (0.0, f64::from(u32::MAX))
        };
        if value.is_nan() || value < min || value > max {
            self.fpscr |= 1;
        } else if input != value {
            self.fpscr |= 1 << 4;
        }
        self.fp[destination] = if signed {
            value as i32 as u32
        } else {
            value as u32
        };
        true
    }

    fn round_to_integral(&mut self, high: u32, low: u32, kind: u32, exact: bool) -> bool {
        let double = low & 0x100 != 0;
        let (destination, source) = if double {
            (
                (low >> 12) as usize * 2 + (((high >> 6) & 1) as usize * 32),
                (low & 15) as usize * 2 + (((low >> 5) & 1) as usize * 32),
            )
        } else {
            (
                ((low >> 12) * 2 + ((high >> 6) & 1)) as usize,
                ((low & 15) * 2 + ((low >> 5) & 1)) as usize,
            )
        };
        if destination >= self.fp.len() || source >= self.fp.len() {
            return false;
        }
        let input = if double {
            u64::from(self.fp[source]) | (u64::from(self.fp[source + 1]) << 32)
        } else {
            u64::from(self.fp[source])
        };
        let (nan_exponent, quiet) = if double {
            (0x7ff0_0000_0000_0000, 1 << 51)
        } else {
            (0x7f80_0000, 1 << 22)
        };
        let result = if input & nan_exponent == nan_exponent && input & (quiet * 2 - 1) != 0 {
            if input & quiet == 0 {
                self.fpscr |= 1;
            }
            if self.fpscr & (1 << 25) != 0 {
                nan_exponent | quiet
            } else {
                input | quiet
            }
        } else {
            let value = if double {
                f64::from_bits(input)
            } else {
                f64::from(f32::from_bits(input as u32))
            };
            let mode = if kind == 5 {
                (self.fpscr >> 22) & 3
            } else {
                kind
            };
            let rounded = Self::round_fp_value(value, mode);
            if exact && rounded != value {
                self.fpscr |= 1 << 4;
            }
            if double {
                rounded.to_bits()
            } else {
                u64::from((rounded as f32).to_bits())
            }
        };
        self.fp[destination] = result as u32;
        if double {
            self.fp[destination + 1] = (result >> 32) as u32;
        }
        true
    }

    pub(crate) fn is_fpu_instruction(high: u32, low: u32) -> bool {
        Self::fp_round_kind(high, low).is_some()
            || Self::fp_convert_kind(high, low).is_some()
            || matches!(high & 0xff00, 0xec00 | 0xed00 | 0xee00)
                && matches!(low & 0xf00, 0xa00 | 0xb00)
    }

    pub(crate) fn execute_fpu(
        &mut self,
        high: u32,
        low: u32,
        pc: u32,
        r: &mut [u32; 32],
        bus: &mut Bus,
        output: &mut impl Write,
    ) -> Result<bool, String> {
        if !Self::is_fpu_instruction(high, low) {
            return Ok(false);
        }

        let sd = ((low >> 12) * 2 + ((high >> 6) & 1)) as usize;
        let sn = ((high & 15) * 2 + ((low >> 7) & 1)) as usize;
        let sm = ((low & 15) * 2 + ((low >> 5) & 1)) as usize;
        if let Some((kind, exact)) = Self::fp_round_kind(high, low) {
            if !(self.armv81m || self.cortex_m33) || !self.round_to_integral(high, low, kind, exact)
            {
                return Ok(false);
            }
        } else if let Some(kind) = Self::fp_convert_kind(high, low) {
            if !(self.armv81m || self.cortex_m33) || !self.convert_fp_to_integer(high, low, kind) {
                return Ok(false);
            }
        } else if high & 0xffe0 == 0xec40 && low & 0xd0 == 0x10 {
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
            // FP context is active before the data access can take an exception.
            self.fp_active = true;
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
        } else if self.armv81m && high & 0xffb0 == 0xeeb0 && low & 0xfe0 == 0xb00 {
            let destination = (low >> 12) as usize * 2 + (((high >> 6) & 1) as usize * 32);
            if destination >= self.fp.len() {
                return Ok(false);
            }
            let bits = f64::from(f32::from_bits(Self::expand_fp_immediate(high, low))).to_bits();
            self.fp[destination] = bits as u32;
            self.fp[destination + 1] = (bits >> 32) as u32;
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
                0xeeb0 if low & 0xe0 == 0 => Self::expand_fp_immediate(high, low),
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
        self.fp_active |= bus
            .nvic
            .as_ref()
            .is_some_and(|nvic| nvic.fpu_auto_preserve());
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vrint_preserves_negative_zero_and_sets_inexact_only_for_vrintx() {
        let mut cpu = Thumb::default();
        cpu.fp[0] = (-0.5_f32).to_bits();
        let (mode, exact) = Thumb::fp_round_kind(0xfeb9, 0x1a40).unwrap();
        assert!(cpu.round_to_integral(0xfeb9, 0x1a40, mode, exact));
        assert_eq!(cpu.fp[2], (-0.0_f32).to_bits());
        assert_eq!(cpu.fpscr, 0);
        let (mode, exact) = Thumb::fp_round_kind(0xeeb7, 0x1a40).unwrap();
        assert!(cpu.round_to_integral(0xeeb7, 0x1a40, mode, exact));
        assert_eq!(cpu.fp[2], (-0.0_f32).to_bits());
        assert_eq!(cpu.fpscr, 1 << 4);
    }

    #[test]
    fn floating_point_access_and_extended_exception_frame() {
        let mut bus = Bus::default();
        bus.map_ram(0, 0x100).unwrap();
        bus.map_ram(0x2000_0000, 0x208).unwrap();
        bus.map_nrf_nvic(0xe0, None, true).unwrap();
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
        bus.thumb_write(0xe000_ed88, 4, 0x00f0_0000, &mut output)
            .unwrap();
        // Exercise eager preservation explicitly.
        bus.thumb_write(0xe000_ef34, 4, 0x8000_0000, &mut output)
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
            thumb
                .branch(registers[14], &mut registers, &mut bus)
                .unwrap(),
            0x60
        );
        assert_eq!(registers[13], 0x2000_0204);
        assert_eq!(thumb.fp[0], 6.0f32.to_bits());
        assert_eq!(thumb.fpscr, 0x6000_0000);
        assert!(thumb.fp_active);
    }
}
