use super::ExecuteError;
use super::{Bus, Thumb};

impl Thumb {
    // FPv4-SP operations used by the FreeRTOS port and the upstream v7-M
    // stacking firmware.
    pub(super) fn execute_fpu(
        &mut self,
        high: u32,
        low: u32,
        next: u32,
        r: &mut [u32; 32],
        bus: &mut Bus,
        output: &mut impl std::io::Write,
    ) -> Result<bool, ExecuteError> {
        let core = (low >> 12) as usize;
        let move_single = high & 0xffe0 == 0xee00 && low & 0xf7f == 0xa10;
        let copy_single = high & 0xffbf == 0xeeb0 && low & 0xfd0 == 0xa40;
        let move_status = matches!(high, 0xeee1 | 0xeef1) && low & 0xfff == 0xa10;
        let multiple = matches!(high & 0xffb0, 0xed20 | 0xecb0) && low & 0xf00 == 0xa00;
        let scalar = high & 0xff20 == 0xed00 && low & 0xe00 == 0xa00;
        let convert = high & 0xffbf == 0xeebd && low & 0xfd0 == 0xac0;
        let from_signed = high & 0xffbf == 0xeeb8 && low & 0xfd0 == 0xac0;
        let compare = high & 0xffbf == 0xeeb4 && low & 0xf50 == 0xa40;
        if !(move_single
            || copy_single
            || move_status
            || multiple
            || scalar
            || convert
            || compare
            || from_signed)
        {
            return Ok(false);
        }
        let nvic = bus
            .nvic
            .as_ref()
            .filter(|nvic| nvic.fpu_access())
            .ok_or(ExecuteError::NoCoprocessor)?;
        let auto_preserve = nvic.fpu_auto_preserve();
        self.preserve_lazy_fp(bus)?;
        self.fp_active |= auto_preserve;
        if copy_single {
            let destination = core * 2 + ((high >> 6) & 1) as usize;
            let source = ((low & 15) * 2 + ((low >> 5) & 1)) as usize;
            self.fp[destination] = self.fp[source];
            return Ok(true);
        }
        if multiple {
            let base = (high & 0xf) as usize;
            let first = core * 2 + ((high >> 6) & 1) as usize;
            let count = (low & 0xff) as usize;
            if base == 15 || count == 0 || first + count > self.fp.len() {
                return Err("invalid FPv4-SP register list".into());
            }
            let load = high & 0x10 != 0;
            let bytes = (count * 4) as u32;
            let address = if load {
                r[base]
            } else {
                r[base].wrapping_sub(bytes)
            };
            if address & 3 != 0 {
                return Err("unaligned FPv4-SP multiple transfer".into());
            }
            let registers = &mut self.fp[first..first + count];
            if load {
                for (index, reg) in registers.iter_mut().enumerate() {
                    *reg = bus.read_ram::<4>(address.wrapping_add(index as u32 * 4))?;
                }
            } else {
                let memory = bus.guest_memory(address, bytes as usize)?;
                for (word, reg) in memory.as_chunks_mut::<4>().0.iter_mut().zip(registers) {
                    *word = reg.to_le_bytes();
                }
            }
            r[base] = if load {
                address.wrapping_add(bytes)
            } else {
                address
            };
        } else if scalar {
            let base = (high & 0xf) as usize;
            let load = high & 0x10 != 0;
            let address = if base == 15 {
                if !load {
                    return Err("invalid FPv4-SP scalar store base".into());
                }
                next & !3
            } else {
                r[base]
            };
            let offset = (low & 0xff) * 4;
            let address = if high & 0x80 == 0 {
                address.wrapping_sub(offset)
            } else {
                address.wrapping_add(offset)
            };
            if address & 3 != 0 {
                return Err("unaligned FPv4-SP scalar transfer".into());
            }
            let extension = ((high >> 6) & 1) as usize;
            let first = if low & 0x100 == 0 {
                core * 2 + extension
            } else {
                if extension != 0 {
                    return Err("invalid FPv4-SP D register".into());
                }
                core * 2
            };
            let wide = low & 0x100 != 0;
            if load {
                let low_word = bus.read_cortex_data::<4>(address)?;
                if wide {
                    self.fp[first + 1] = bus.read_cortex_data::<4>(address.wrapping_add(4))?;
                }
                self.fp[first] = low_word;
            } else {
                bus.write(address, 4, self.fp[first], output)?;
                if wide {
                    bus.write(address.wrapping_add(4), 4, self.fp[first + 1], output)?;
                }
            }
        } else if from_signed {
            let dest = core * 2 + ((high >> 6) & 1) as usize;
            let source = ((low & 15) * 2 + ((low >> 5) & 1)) as usize;
            let integer = self.fp[source] as i32;
            let exact = f64::from(integer);
            let nearest = integer as f32;
            let rounded = match (self.fpscr >> 22) & 3 {
                1 if f64::from(nearest) < exact => nearest.next_up(),
                2 if f64::from(nearest) > exact => nearest.next_down(),
                3 if nearest.is_sign_positive() && f64::from(nearest) > exact => {
                    nearest.next_down()
                }
                3 if nearest.is_sign_negative() && f64::from(nearest) < exact => nearest.next_up(),
                _ => nearest,
            };
            if f64::from(rounded) != exact {
                self.fpscr |= 1 << 4;
            }
            self.fp[dest] = rounded.to_bits();
        } else if convert || compare {
            let dest = core * 2 + ((high >> 6) & 1) as usize;
            let source = ((low & 15) * 2 + ((low >> 5) & 1)) as usize;
            let right = self.fp_operand(source);
            if convert {
                let truncated = right.trunc();
                if !(-2_147_483_648.0..2_147_483_648.0).contains(&truncated) {
                    self.fpscr |= 1; // IOC
                } else if right != truncated {
                    self.fpscr |= 1 << 4; // IXC
                }
                self.fp[dest] = truncated as i32 as u32;
            } else {
                let left = self.fp_operand(dest);
                let flags = match left.partial_cmp(&right) {
                    Some(std::cmp::Ordering::Less) => 8,
                    Some(std::cmp::Ordering::Equal) => 6,
                    Some(std::cmp::Ordering::Greater) => 2,
                    None => {
                        let signaling_nan = |v: f32| v.is_nan() && v.to_bits() & (1 << 22) == 0;
                        if low & 0x80 != 0 || signaling_nan(left) || signaling_nan(right) {
                            self.fpscr |= 1; // IOC
                        }
                        3
                    }
                };
                self.fpscr = (self.fpscr & 0x0fff_ffff) | (flags << 28);
            }
        } else if move_status {
            if high == 0xeee1 && core < 15 {
                self.fpscr = r[core] & 0xffc0_009f;
            } else if high == 0xeef1 && core == 15 {
                self.set_apsr(self.fpscr);
            } else if high == 0xeef1 {
                r[core] = self.fpscr;
            } else {
                return Err("invalid FPv4-SP status transfer".into());
            }
        } else {
            if core == 15 {
                return Err("invalid FPv4-SP core register".into());
            }
            let single = ((high & 15) * 2 + ((low >> 7) & 1)) as usize;
            if high & 0x10 != 0 {
                r[core] = self.fp[single];
            } else {
                self.fp[single] = r[core];
            }
        }
        Ok(true)
    }

    fn fp_operand(&mut self, index: usize) -> f32 {
        let value = f32::from_bits(self.fp[index]);
        if self.fpscr & (1 << 24) != 0 && value.is_subnormal() {
            self.fpscr |= 1 << 7; // IDC
            return 0.0_f32.copysign(value);
        }
        value
    }
}
