// JH7100 reset banks provided by the upstream REPL's Python peripherals.
pub struct ResetGenerator {
    pub base: u32,
    asserted: [u32; 4],
    status_invert: [u32; 4],
    words: usize,
}

impl ResetGenerator {
    pub fn from_repl(name: &str, base: u32, size: u64) -> Option<Self> {
        let (words, status_invert) = match (name, base, size) {
            ("rstgen", 0x1184_0000, 0x20) => (4, [0xffff_f3fb, 0xffff_ffe7, 0xffff_fffe, u32::MAX]),
            ("audiorstgen", 0x1049_0000, 0x8) | ("voutsysrstgen", 0x1225_0000, 0x8) => {
                (1, [u32::MAX, 0, 0, 0])
            }
            _ => return None,
        };
        Some(Self {
            base,
            asserted: [u32::MAX; 4],
            status_invert,
            words,
        })
    }

    pub fn size(&self) -> u64 {
        self.words as u64 * 8
    }

    pub fn contains(&self, address: u32) -> bool {
        u64::from(address.wrapping_sub(self.base)) < self.size()
    }

    pub fn read(&self, address: u32) -> u32 {
        let index = ((address - self.base) / 4) as usize;
        let word = index % self.words;
        let asserted = self.asserted[word];
        if index < self.words {
            asserted
        } else {
            asserted ^ self.status_invert[word]
        }
    }

    pub fn write(&mut self, address: u32, value: u32) {
        let index = ((address - self.base) / 4) as usize;
        self.asserted[index % self.words] = value;
    }
}

/// BeagleV StarLight routes the active-low board reset through JH7100 GPIO10.
pub struct StarLightResetPin {
    output: u32,
    output_enable: u32,
}

impl Default for StarLightResetPin {
    fn default() -> Self {
        Self {
            output: 0,
            output_enable: 1,
        }
    }
}

impl StarLightResetPin {
    pub const DOUT: u32 = 0x1191_00a0;
    pub const DOEN: u32 = 0x1191_00a4;

    pub fn read(&self, address: u32) -> Option<u32> {
        match address {
            Self::DOUT => Some(self.output),
            Self::DOEN => Some(self.output_enable),
            _ => None,
        }
    }

    pub fn write(&mut self, address: u32, value: u32) -> Option<bool> {
        match address {
            Self::DOUT => self.output = value,
            Self::DOEN => self.output_enable = value,
            _ => return None,
        }
        // Constant zero selects a low output or asserts the active-low output enable.
        Some(self.output == 0 && self.output_enable == 0)
    }
}

#[cfg(test)]
mod tests {
    use super::StarLightResetPin;

    #[test]
    fn starlight_reset_requires_a_driven_low_pin() {
        let mut pin = StarLightResetPin::default();
        assert_eq!(pin.read(StarLightResetPin::DOEN), Some(1));
        assert_eq!(pin.write(StarLightResetPin::DOUT, 0), Some(false));
        assert_eq!(pin.write(StarLightResetPin::DOUT, 1), Some(false));
        assert_eq!(pin.write(StarLightResetPin::DOEN, 0), Some(false));
        assert_eq!(pin.write(StarLightResetPin::DOEN + 8, 0), None);
        assert_eq!(pin.write(StarLightResetPin::DOUT, 0), Some(true));
    }
}
