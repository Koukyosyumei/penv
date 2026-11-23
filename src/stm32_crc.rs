use crate::crc::CrcEngine;

pub struct Stm32Crc {
    pub base: u32,
    pub rcc_bound: bool,
    pub legacy: bool,
    configurable_poly: bool,
    polynomial: u32,
    initial: u32,
    control: u32,
    independent: u8,
    engine: CrcEngine,
}

impl Stm32Crc {
    pub const MODEL: &str = "CRC.STM32_CRC";
    pub const SIZE: u32 = 0x400;

    pub fn new(base: u32, configurable_poly: bool) -> Self {
        Self {
            base,
            rcc_bound: false,
            legacy: false,
            configurable_poly,
            polynomial: 0x04c1_1db7,
            initial: u32::MAX,
            control: 0,
            independent: 0,
            engine: CrcEngine::new(0x04c1_1db7, 32, false, false, u32::MAX, 0).unwrap(),
        }
    }

    pub fn reset(&mut self) {
        let (rcc_bound, legacy) = (self.rcc_bound, self.legacy);
        *self = Self::new(self.base, self.configurable_poly);
        self.rcc_bound = rcc_bound;
        self.legacy = legacy;
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    fn bits(&self) -> u32 {
        [32, 16, 8, 7][((self.control >> 3) & 3) as usize]
    }

    pub fn read(&self, address: u32, width: usize) -> u32 {
        let value = match address - self.base {
            0 => self.engine.value(),
            4 => u32::from(self.independent),
            8 => self.control,
            0x10 if !self.legacy => self.initial,
            0x14 if !self.legacy => self.polynomial,
            _ => 0,
        };
        value & (u32::MAX >> (32 - width * 8))
    }

    pub fn write(&mut self, address: u32, width: usize, value: u32) {
        match (address - self.base, width) {
            (0, _) if !self.legacy || width == 4 => self.update(value, width),
            (4, 4) => self.independent = value as u8,
            (8, 4) if self.legacy => {
                if value & 1 != 0 {
                    self.reset_state();
                }
            }
            (8, 4) => {
                self.control = value & 0xf8;
                self.reset_state();
            }
            (0x10, 4) if !self.legacy => self.initial = value,
            (0x14, 4) if !self.legacy => {
                if self.configurable_poly {
                    self.polynomial = value;
                }
                self.reset_state();
            }
            _ => {}
        }
    }

    fn reset_state(&mut self) {
        let bits = self.bits();
        let mask = u32::MAX >> (32 - bits);
        self.engine = CrcEngine::new(
            self.polynomial & mask,
            bits,
            false,
            self.control & 0x80 != 0,
            self.initial,
            0,
        )
        .unwrap();
    }

    fn update(&mut self, value: u32, width: usize) {
        let mode = (self.control >> 5) & 3;
        let mut bytes = value.to_be_bytes();
        let bytes = &mut bytes[4 - width..];
        let reversal_group = match mode {
            0 | 1 => 1,
            2 => width.min(2),
            _ => width,
        };
        if mode != 0 {
            for group in bytes.chunks_exact_mut(reversal_group) {
                group.reverse();
                for byte in group {
                    *byte = byte.reverse_bits();
                }
            }
        }
        self.engine.update(bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supports_crc32_mpeg2_and_crc8_smbus_check_values() {
        let mut crc = Stm32Crc::new(0, true);
        crc.write(0, 4, u32::from_be_bytes(*b"1234"));
        crc.write(0, 2, u16::from_be_bytes(*b"56").into());
        for byte in b"789" {
            crc.write(0, 1, (*byte).into());
        }
        assert_eq!(crc.read(0, 4), 0x0376_e6e7);

        crc.write(0x10, 4, 0);
        crc.write(0x14, 4, 0x07);
        crc.write(8, 4, 2 << 3 | 1);
        for byte in b"123456789" {
            crc.write(0, 1, (*byte).into());
        }
        assert_eq!(crc.read(0, 1), 0xf4);
    }
}
