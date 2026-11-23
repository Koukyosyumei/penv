#[derive(Debug, PartialEq, Eq)]
pub enum CrcConfigError {
    WidthOutOfRange,
    PolynomialTooWide,
}

pub struct CrcEngine {
    shift: u32,
    polynomial: u32,
    reflect_input: bool,
    reflect_output: bool,
    initial: u32,
    xor_output: u32,
    state: u32,
}

impl CrcEngine {
    pub fn new(
        polynomial: u32,
        width: u32,
        reflect_input: bool,
        reflect_output: bool,
        initial: u32,
        xor_output: u32,
    ) -> Result<Self, CrcConfigError> {
        if !(1..=32).contains(&width) {
            return Err(CrcConfigError::WidthOutOfRange);
        }
        let shift = 32 - width;
        let mask = u32::MAX >> shift;
        if polynomial > mask {
            return Err(CrcConfigError::PolynomialTooWide);
        }
        let mut engine = Self {
            shift,
            polynomial: polynomial << shift,
            reflect_input,
            reflect_output,
            initial: initial << shift,
            xor_output: xor_output & mask,
            state: 0,
        };
        engine.reset();
        Ok(engine)
    }

    pub fn reset(&mut self) {
        self.state = self.initial;
    }

    pub fn update(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            let input = if self.reflect_input {
                byte.reverse_bits()
            } else {
                byte
            };
            self.state ^= u32::from(input) << 24;
            for _ in 0..8 {
                let feedback = self.state >> 31;
                self.state = (self.state << 1) ^ (self.polynomial & feedback.wrapping_neg());
            }
        }
    }

    pub fn value(&self) -> u32 {
        let value = if self.reflect_output {
            self.state.reverse_bits()
        } else {
            self.state >> self.shift
        };
        value ^ self.xor_output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CRC32: u32 = 0x04c1_1db7;
    const CCITT: u32 = 0x1021;
    const CRC16: u32 = 0x8005;
    const CHECK_INPUT: &[u8] = b"123456789";

    fn calculate(engine: &mut CrcEngine, input: &[u8]) -> u32 {
        engine.reset();
        engine.update(input);
        engine.value()
    }

    #[test]
    fn renode_crc_rejects_invalid_configuration() {
        use CrcConfigError::*;
        for (width, expected) in [
            (7, PolynomialTooWide),
            (0, WidthOutOfRange),
            (33, WidthOutOfRange),
        ] {
            assert_eq!(
                CrcEngine::new(0xff, width, true, true, 0, 0).err(),
                Some(expected)
            );
        }
    }

    #[test]
    fn renode_crc_calculates_multiple_updates() {
        let mut crc = CrcEngine::new(CRC32, 32, true, true, 0, 0).unwrap();
        for (input, expected) in [
            (&b""[..], 0),
            (&b"1"[..], 0x51de_003a),
            (&b"2"[..], 0x0e8a_5632),
        ] {
            crc.update(input);
            assert_eq!(crc.value(), expected);
        }
    }

    #[test]
    fn renode_crc_handles_independent_reflections() {
        for (input, output, expected) in [
            (false, true, 0xce1e_8e92),
            (true, false, 0xe6ac_054a),
            (false, false, 0x4971_7873),
        ] {
            let mut crc = CrcEngine::new(CRC32, 32, input, output, 0, 0).unwrap();
            crc.update(b"23");
            assert_eq!(crc.value(), expected);
        }
    }

    #[test]
    fn renode_crc_initializes_named_polynomial_configurations() {
        for (polynomial, width, expected) in [
            (CRC32, 32, 0x0e8a_5632),
            (CCITT, 16, 0xbdeb),
            (CRC16, 16, 0x4594),
        ] {
            let mut crc = CrcEngine::new(polynomial, width, true, true, 0, 0).unwrap();
            assert_eq!(calculate(&mut crc, b"12"), expected);
        }
    }

    macro_rules! check {
        ($test:ident, $polynomial:expr, $width:expr, $reflect:expr, $init:expr, $xor:expr, $expected:expr) => {
            #[test]
            fn $test() {
                let mut crc =
                    CrcEngine::new($polynomial, $width, $reflect, $reflect, $init, $xor).unwrap();
                assert_eq!(calculate(&mut crc, CHECK_INPUT), $expected);
            }
        };
    }

    check!(
        renode_crc_32,
        CRC32,
        32,
        true,
        u32::MAX,
        u32::MAX,
        0xcbf4_3926
    );
    check!(
        renode_crc_32_bzip,
        CRC32,
        32,
        false,
        u32::MAX,
        u32::MAX,
        0xfc89_1918
    );
    check!(renode_crc_32_jam, CRC32, 32, true, u32::MAX, 0, 0x340b_c6d9);
    check!(
        renode_crc_32_mpeg2,
        CRC32,
        32,
        false,
        u32::MAX,
        0,
        0x0376_e6e7
    );
    check!(
        renode_crc_32_posix,
        CRC32,
        32,
        false,
        0,
        u32::MAX,
        0x765e_7680
    );
    check!(
        renode_crc_32_sata,
        CRC32,
        32,
        false,
        0x5232_5032,
        0,
        0xcf72_afe8
    );
    check!(
        renode_crc_31_philips,
        CRC32,
        31,
        false,
        0x7fff_ffff,
        0x7fff_ffff,
        0x0ce9_e46c
    );
    check!(renode_crc_16_ccitt, CCITT, 16, true, 0, 0, 0x2189);
    check!(renode_crc_16, CRC16, 16, true, 0, 0, 0xbb3d);
    check!(renode_crc_8, 0x07, 8, false, 0, 0, 0xf4);
    check!(renode_crc_7, 0x09, 7, false, 0, 0, 0x75);
}
