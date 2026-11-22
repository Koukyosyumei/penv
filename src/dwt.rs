use crate::machine::VIRTUAL_STEPS_PER_SECOND;

const DEMCR: u32 = 0xe000_edfc;

pub struct Dwt {
    pub base: u32,
    cycles_per_step: u32,
    fractional_cycles: u32,
    fraction: u32,
    words: [u32; 23],
    demcr: u32,
}

impl Dwt {
    pub const SIZE: u32 = 0x1000;

    pub fn new(base: u32, frequency: u32) -> Self {
        Self {
            base,
            cycles_per_step: frequency / VIRTUAL_STEPS_PER_SECOND,
            fractional_cycles: frequency % VIRTUAL_STEPS_PER_SECOND,
            fraction: 0,
            words: [0; 23],
            demcr: 0,
        }
    }

    pub fn ranges(&self) -> [(u64, u64); 2] {
        let base = u64::from(self.base);
        [
            (base, base + u64::from(Self::SIZE)),
            (u64::from(DEMCR), u64::from(DEMCR) + 4),
        ]
    }

    pub fn tick(&mut self) {
        if self.words[0] & 1 == 0 {
            return;
        }
        self.fraction += self.fractional_cycles;
        let carry = u32::from(self.fraction >= VIRTUAL_STEPS_PER_SECOND);
        self.fraction -= carry * VIRTUAL_STEPS_PER_SECOND;
        self.words[1] = self.words[1].wrapping_add(self.cycles_per_step + carry);
    }

    pub fn read(&self, address: u32) -> Option<u32> {
        if address == DEMCR {
            return Some(self.demcr);
        }
        let offset = address.checked_sub(self.base)?;
        if offset >= Self::SIZE || offset & 3 != 0 {
            return None;
        }
        let ids = [
            0x04, 0x00, 0x00, 0x00, 0x02, 0xb0, 0x1b, 0x00, 0x0d, 0xe0, 0x05, 0xb1,
        ];
        if offset >= 0xfd0 {
            return Some(ids[((offset - 0xfd0) / 4) as usize]);
        }
        Some(self.words.get((offset / 4) as usize).copied().unwrap_or(0))
    }

    pub fn write(&mut self, address: u32, value: u32) -> bool {
        if address == DEMCR {
            self.demcr = value;
            return true;
        }
        let offset = address.wrapping_sub(self.base);
        if offset >= Self::SIZE || offset & 3 != 0 {
            return false;
        }
        let mask = match offset {
            0 => 0xf07f_1fff,
            8..=0x18 => 0xff,
            0x24 | 0x34 | 0x44 | 0x54 => 0xf,
            _ => u32::MAX,
        };
        if let Some(word) = self.words.get_mut((offset / 4) as usize) {
            *word = value & mask;
        }
        true
    }
}
