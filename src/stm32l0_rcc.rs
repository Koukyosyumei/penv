pub(crate) struct Stm32l0Rcc {
    pub base: u32,
    pub systick: bool,
    registers: [u32; 21],
    msi_range: u32,
    pll_multiplier: u32,
    pll_divisor: u32,
    hse_frequency: u32,
}

impl Stm32l0Rcc {
    pub const SIZE: u32 = 0x400;

    pub fn new(base: u32, systick: bool, hse_frequency: u32) -> Self {
        let mut registers = [0; 21];
        registers[0] = 0x100;
        registers[1] = 0xb000;
        registers[0x30 / 4] = 0x100;
        registers[0x40 / 4] = 0x100;
        Self {
            base,
            systick,
            hse_frequency,
            registers,
            msi_range: 5,
            pll_multiplier: 3,
            pll_divisor: 1,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new(self.base, self.systick, self.hse_frequency);
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    pub fn systick_frequency(&self) -> u32 {
        let clock = self.registers[0x0c / 4];
        let hsi = if self.registers[0] & 8 == 0 {
            16_000_000
        } else {
            4_000_000
        };
        let source = match clock & 3 {
            0 => 65_536 << self.msi_range,
            1 => hsi,
            2 => self.hse_frequency,
            _ => {
                let input = if clock & (1 << 16) == 0 {
                    hsi
                } else {
                    self.hse_frequency
                };
                (u64::from(input) * u64::from(self.pll_multiplier) / u64::from(self.pll_divisor))
                    .min(u64::from(u32::MAX)) as u32
            }
        };
        let hpre = (clock >> 4) & 15;
        if hpre < 8 {
            source
        } else {
            source >> ((hpre & 7) + 1)
        }
    }

    pub fn read(&self, address: u32) -> u32 {
        match address - self.base {
            0 => {
                let value = self.registers[0];
                value | ((value & 1) << 2) | ((value & 0x10108) << 1) | ((value & (1 << 24)) << 1)
            }
            8 => self.registers[2] | ((self.registers[2] & 1) << 1),
            0x0c => self.registers[3] | ((self.registers[3] & 3) << 2),
            0x14 => self.registers[0x10 / 4] & 0x7f,
            0x18 => 0,
            0x50 => self.registers[20] | ((self.registers[20] & 0x101) << 1),
            offset if offset % 4 == 0 => self
                .registers
                .get((offset / 4) as usize)
                .copied()
                .unwrap_or(0),
            _ => 0,
        }
    }

    pub fn write(&mut self, address: u32, value: u32) {
        match address - self.base {
            0 => self.registers[0] = value & 0x0135_010b,
            4 => {
                self.registers[1] = value;
                let range = (value >> 13) & 7;
                if range < 7 {
                    self.msi_range = range;
                }
            }
            8 => self.registers[2] = value & 0xff01,
            0x0c => {
                self.registers[3] = value & 0x7ffd_bff3;
                const PLL_MULTIPLIERS: [u32; 9] = [3, 4, 6, 8, 12, 16, 24, 32, 48];
                if let Some(&multiplier) = PLL_MULTIPLIERS.get(((value >> 18) & 15) as usize) {
                    self.pll_multiplier = multiplier;
                }
                let divisor = (value >> 22) & 3;
                if divisor != 0 {
                    self.pll_divisor = divisor + 1;
                }
            }
            0x10 => self.registers[4] = value & 0xff,
            0x18 => self.registers[4] &= !(value & 0x7f),
            0x1c..=0x48 if address & 3 == 0 => {
                self.registers[((address - self.base) / 4) as usize] = value
            }
            0x4c => self.registers[19] = value & 0x000f_3c0f,
            0x50 => self.registers[20] = value & 0x0007_1901,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Stm32l0Rcc;

    #[test]
    fn oscillator_ready_and_clock_status_follow_programmed_source_and_divider() {
        let mut rcc = Stm32l0Rcc::new(0, true, 16_000_000);
        assert_eq!(rcc.read(0), 0x300);
        assert_eq!(rcc.systick_frequency(), 2_097_152);
        rcc.write(0, 0x109);
        assert_eq!(rcc.read(0) & 0x1d, 0x1d);
        rcc.write(0x0c, 1 | (8 << 4));
        assert_eq!(rcc.read(0x0c) & 15, 5);
        assert_eq!(rcc.systick_frequency(), 2_000_000);
    }
}
