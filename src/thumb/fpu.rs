use super::{Bus, Thumb};

impl Thumb {
    // FPv4-SP register transfers used by the Cortex-M4F FreeRTOS port.
    pub(super) fn execute_fpu(
        &mut self,
        high: u32,
        low: u32,
        r: &mut [u32; 32],
        bus: &mut Bus,
    ) -> Result<bool, String> {
        let core = (low >> 12) as usize;
        let move_single = high & 0xffe0 == 0xee00 && low & 0xf7f == 0xa10;
        let move_status = matches!(high, 0xeee1 | 0xeef1) && low & 0xfff == 0xa10;
        let multiple = matches!(high & 0xffb0, 0xed20 | 0xecb0) && low & 0xf00 == 0xa00;
        if !(move_single || move_status || multiple) {
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
}
