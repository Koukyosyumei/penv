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
    ) -> Result<bool, String> {
        let core = (low >> 12) as usize;
        let move_single = high & 0xffe0 == 0xee00 && low & 0xf7f == 0xa10;
        let move_status = matches!(high, 0xeee1 | 0xeef1) && low & 0xfff == 0xa10;
        let multiple = matches!(high & 0xffb0, 0xed20 | 0xecb0) && low & 0xf00 == 0xa00;
        let scalar = high & 0xff20 == 0xed00 && low & 0xf00 == 0xa00;
        let convert = high & 0xffbf == 0xeebd && low & 0xfd0 == 0xac0;
        let compare = high & 0xffbf == 0xeeb4 && low & 0xf50 == 0xa40;
        if !(move_single || move_status || multiple || scalar || convert || compare) {
            return Ok(false);
        }
        let nvic = bus
            .nvic
            .as_ref()
            .filter(|nvic| nvic.fpu_access())
            .ok_or("FPv4-SP instruction with coprocessor access disabled")?;
        let auto_preserve = nvic.fpu_auto_preserve();
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
                    *reg = bus
                        .read_ram::<4>(address.wrapping_add(index as u32 * 4))
                        .map_err(|e| e.to_string())?;
                }
            } else {
                let memory = bus
                    .guest_memory(address, bytes as usize)
                    .map_err(|e| e.to_string())?;
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
            let single = core * 2 + ((high >> 6) & 1) as usize;
            if load {
                self.fp[single] = bus.read::<4>(address).map_err(|e| e.to_string())?;
            } else {
                bus.write(address, 4, self.fp[single], output)
                    .map_err(|e| e.to_string())?;
            }
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
        self.fp_active |= auto_preserve;
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
