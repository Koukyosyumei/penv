use std::fmt;

#[derive(Debug, PartialEq, Eq)]
pub enum CrcConfigError {
    WidthOutOfRange(i32),
    PolynomialTooWide { width: i32, polynomial: u32 },
}

impl fmt::Display for CrcConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WidthOutOfRange(width) => {
                write!(f, "CRCConfig: {width} is incorrect width value")
            }
            Self::PolynomialTooWide { width, polynomial } => write!(
                f,
                "CRCConfig: width ({width}) is too small for given polynomial 0x{polynomial:X}."
            ),
        }
    }
}

impl std::error::Error for CrcConfigError {}

fn validate_polynomial(polynomial: u32, width: i32) -> Result<(), CrcConfigError> {
    if !(1..=32).contains(&width) {
        return Err(CrcConfigError::WidthOutOfRange(width));
    }
    if 32 - polynomial.leading_zeros() > width as u32 {
        return Err(CrcConfigError::PolynomialTooWide { width, polynomial });
    }
    Ok(())
}

pub struct Stm32Crc {
    pub base: u32,
    pub rcc_bound: bool,
    pub legacy: bool,
    configurable_poly: bool,
    polynomial: u32,
    initial: u32,
    control: u32,
    independent: u8,
    state: u32,
    config_dirty: bool,
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
            state: u32::MAX,
            config_dirty: true,
        }
    }

    pub fn reset(&mut self) {
        let (rcc_bound, legacy) = (self.rcc_bound, self.legacy);
        *self = Self::new(self.base, self.configurable_poly);
        self.rcc_bound = rcc_bound;
        self.legacy = legacy;
        self.config_dirty = false;
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    fn bits(&self) -> u32 {
        [32, 16, 8, 7][((self.control >> 3) & 3) as usize]
    }

    pub fn read(&mut self, address: u32, width: usize) -> Result<u32, CrcConfigError> {
        let offset = address - self.base;
        if offset == 0 {
            self.apply_config()?;
        }
        let value = match offset {
            0 if self.control & 0x80 != 0 => self.state.reverse_bits() >> (32 - self.bits()),
            0 => self.state,
            4 => u32::from(self.independent),
            8 => self.control,
            0x10 if !self.legacy => self.initial,
            0x14 if !self.legacy => self.polynomial,
            _ => 0,
        };
        Ok(value & (u32::MAX >> (32 - width * 8)))
    }

    pub fn write(&mut self, address: u32, width: usize, value: u32) -> Result<(), CrcConfigError> {
        match (address - self.base, width) {
            (0, _) if !self.legacy || width == 4 => self.update(value, width)?,
            (4, 4) => self.independent = value as u8,
            (8, 4) if self.legacy => {
                if value & 1 != 0 {
                    self.state = self.initial;
                }
            }
            (8, 4) => {
                self.control = value & 0xf8;
                self.config_dirty = true;
            }
            (0x10, 4) if !self.legacy => self.initial = value,
            (0x14, 4) if !self.legacy => {
                if self.configurable_poly {
                    self.polynomial = value;
                }
                self.config_dirty = true;
            }
            _ => {}
        }
        Ok(())
    }

    fn apply_config(&mut self) -> Result<(), CrcConfigError> {
        if self.config_dirty {
            validate_polynomial(self.polynomial, self.bits() as i32)?;
            self.state = self.initial & (u32::MAX >> (32 - self.bits()));
            self.config_dirty = false;
        }
        Ok(())
    }

    fn update(&mut self, value: u32, width: usize) -> Result<(), CrcConfigError> {
        self.apply_config()?;
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
        // Keep the CRC left-aligned so the feedback bit is always bit 31.
        let shift = 32 - self.bits();
        let polynomial = self.polynomial << shift;
        let mut state = self.state << shift;
        for &byte in bytes.iter() {
            state ^= u32::from(byte) << 24;
            for _ in 0..8 {
                let feedback = state >> 31;
                state = (state << 1) ^ (polynomial & feedback.wrapping_neg());
            }
        }
        self.state = state >> shift;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DR: u32 = 0;
    const CR: u32 = 8;
    const INIT: u32 = 0x10;
    const POL: u32 = 0x14;
    const CRC32: u32 = 0x04c1_1db7;

    fn configure(
        width: u8,
        polynomial: u32,
        init: u32,
        reflect_in: bool,
        reflect_out: bool,
    ) -> Stm32F0Crc {
        let size = match width {
            32 => 0,
            16 => 1,
            8 => 2,
            7 => 3,
            _ => unreachable!(),
        };
        let mut crc = Stm32F0Crc::new(0, true);
        crc.write(
            CR,
            4,
            1 | (size << 3) | (u32::from(reflect_in) << 5) | (u32::from(reflect_out) << 7),
        )
        .unwrap();
        crc.write(POL, 4, polynomial).unwrap();
        crc.write(INIT, 4, init).unwrap();
        crc
    }

    fn update(crc: &mut Stm32F0Crc, input: &[u8]) {
        for &byte in input {
            crc.write(DR, 1, byte.into()).unwrap();
        }
    }

    #[test]
    fn renode_crc_rejects_invalid_configuration() {
        use CrcConfigError::*;
        assert_eq!(
            validate_polynomial(0xff, 7),
            Err(PolynomialTooWide {
                width: 7,
                polynomial: 0xff
            })
        );
        assert_eq!(validate_polynomial(0xff, 0), Err(WidthOutOfRange(0)));
        assert_eq!(validate_polynomial(0xff, 33), Err(WidthOutOfRange(33)));

        let mut crc = Stm32F0Crc::new(0, true);
        crc.write(CR, 4, 3 << 3).unwrap();
        crc.write(POL, 4, 0xff).unwrap();
        crc.write(INIT, 4, 0x23).unwrap();
        let invalid = PolynomialTooWide {
            width: 7,
            polynomial: 0xff,
        };
        assert_eq!(crc.read(POL, 4), Ok(0xff));
        assert_eq!(crc.read(DR, 4), Err(invalid));
        assert_eq!(
            crc.write(DR, 1, 0),
            Err(PolynomialTooWide {
                width: 7,
                polynomial: 0xff
            })
        );
        crc.write(POL, 4, 0x09).unwrap();
        assert_eq!(crc.read(DR, 4), Ok(0x23));
    }

    #[test]
    fn renode_crc_multiple_updates() {
        let mut crc = configure(32, CRC32, 0, true, true);
        update(&mut crc, b"");
        assert_eq!(crc.read(DR, 4).unwrap(), 0);
        update(&mut crc, b"1");
        assert_eq!(crc.read(DR, 4).unwrap(), 0x51de_003a);
        update(&mut crc, b"2");
        assert_eq!(crc.read(DR, 4).unwrap(), 0x0e8a_5632);
    }

    #[test]
    fn renode_crc_reflection_combinations() {
        for (reflect_in, reflect_out, expected) in [
            (false, true, 0xce1e_8e92),
            (true, false, 0xe6ac_054a),
            (false, false, 0x4971_7873),
        ] {
            let mut crc = configure(32, CRC32, 0, reflect_in, reflect_out);
            update(&mut crc, b"23");
            assert_eq!(
                crc.read(DR, 4).unwrap(),
                expected,
                "{reflect_in}/{reflect_out}"
            );
        }
    }

    #[test]
    fn renode_crc_default_configurations() {
        for (width, polynomial, expected) in [
            (32, CRC32, 0x0e8a_5632),
            (16, 0x1021, 0xbdeb),
            (16, 0x8005, 0x4594),
        ] {
            let mut crc = configure(width, polynomial, 0, true, true);
            update(&mut crc, b"12");
            assert_eq!(crc.read(DR, 4).unwrap(), expected, "{polynomial:#x}");
        }
    }

    #[test]
    fn renode_crc_polynomial_check_values() {
        for (name, width, polynomial, init, reflect, xor_out, expected) in [
            ("CRC32", 32, CRC32, u32::MAX, true, u32::MAX, 0xcbf4_3926),
            (
                "CRC32BZIP",
                32,
                CRC32,
                u32::MAX,
                false,
                u32::MAX,
                0xfc89_1918,
            ),
            ("CRC32JAMCRC", 32, CRC32, u32::MAX, true, 0, 0x340b_c6d9),
            ("CRC32POSIX", 32, CRC32, 0, false, u32::MAX, 0x765e_7680),
            ("CRC32SATA", 32, CRC32, 0x5232_5032, false, 0, 0xcf72_afe8),
            ("CRC16CCITT", 16, 0x1021, 0, true, 0, 0x2189),
            ("CRC16", 16, 0x8005, 0, true, 0, 0xbb3d),
            ("CRC7", 7, 0x09, 0, false, 0, 0x75),
        ] {
            let mut crc = configure(width, polynomial, init, reflect, reflect);
            update(&mut crc, b"123456789");
            // STM32 exposes the value before the software final XOR.
            assert_eq!(crc.read(DR, 4).unwrap() ^ xor_out, expected, "{name}");
        }
    }

    #[test]
    fn crc_applies_pending_configuration_at_next_data_access() {
        let mut crc = Stm32F0Crc::new(0, true);
        assert_eq!(crc.read(DR, 4).unwrap(), u32::MAX);
        crc.write(INIT, 4, 0x1234_5678).unwrap();
        assert_eq!(crc.read(DR, 4).unwrap(), u32::MAX);
        crc.write(CR, 4, 1).unwrap();
        crc.write(INIT, 4, 0x8765_4321).unwrap();
        assert_eq!(crc.read(POL, 4).unwrap(), CRC32);
        assert_eq!(crc.read(DR, 4).unwrap(), 0x8765_4321);
        crc.write(POL, 4, CRC32).unwrap();
        crc.write(INIT, 4, 0).unwrap();
        crc.write(DR, 1, 0).unwrap(); // first access may be a write
        assert_eq!(crc.read(DR, 4).unwrap(), 0);

        crc.reset();
        crc.write(INIT, 4, 0).unwrap();
        assert_eq!(crc.read(DR, 4).unwrap(), u32::MAX);
    }

    #[test]
    fn supports_crc32_mpeg2_and_crc8_smbus_check_values() {
        let mut crc = Stm32Crc::new(0, true);
        crc.write(0, 4, u32::from_be_bytes(*b"1234")).unwrap();
        crc.write(0, 2, u16::from_be_bytes(*b"56").into()).unwrap();
        for byte in b"789" {
            crc.write(0, 1, (*byte).into()).unwrap();
        }
        assert_eq!(crc.read(0, 4).unwrap(), 0x0376_e6e7);

        crc.write(0x10, 4, 0).unwrap();
        crc.write(0x14, 4, 0x07).unwrap();
        crc.write(8, 4, 2 << 3 | 1).unwrap();
        for byte in b"123456789" {
            crc.write(0, 1, (*byte).into()).unwrap();
        }
        assert_eq!(crc.read(0, 1).unwrap(), 0xf4);
    }
}

pub type Stm32F0Crc = Stm32Crc;
