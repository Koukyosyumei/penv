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
