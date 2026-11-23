#[derive(Clone, Copy, Default)]
pub struct BitManip {
    pub zba: bool,
    pub zbb: bool,
    pub zbc: bool,
    pub zbs: bool,
}

impl BitManip {
    pub fn immediate(self, funct3: u32, funct7: u32, value: u32, imm: u32) -> Option<u32> {
        let index = imm & 31;
        if self.zbb {
            match (funct3, funct7, index) {
                (1, 0x30, 0) => return Some(value.leading_zeros()),
                (1, 0x30, 1) => return Some(value.trailing_zeros()),
                (1, 0x30, 2) => return Some(value.count_ones()),
                (1, 0x30, 4) => return Some(value as i8 as i32 as u32),
                (1, 0x30, 5) => return Some(value as i16 as i32 as u32),
                (5, 0x14, 7) => {
                    return Some(u32::from_le_bytes(
                        value
                            .to_le_bytes()
                            .map(|byte| if byte == 0 { 0 } else { 255 }),
                    ));
                }
                (5, 0x34, 24) => return Some(value.swap_bytes()),
                (5, 0x30, _) => return Some(value.rotate_right(index)),
                _ => {}
            }
        }
        self.single_bit(funct3, funct7, value, index)
    }

    pub fn register(self, funct3: u32, funct7: u32, a: u32, b: u32, rs2: u32) -> Option<u32> {
        Some(match (funct3, funct7) {
            (2 | 4 | 6, 0x10) if self.zba => (a << (funct3 / 2)).wrapping_add(b),
            (4, 0x20) if self.zbb => !(a ^ b),
            (6, 0x20) if self.zbb => a | !b,
            (7, 0x20) if self.zbb => a & !b,
            (1, 0x30) if self.zbb => a.rotate_left(b & 31),
            (5, 0x30) if self.zbb => a.rotate_right(b & 31),
            (4, 0x04) if self.zbb && rs2 == 0 => a & 0xffff,
            (4, 0x05) if self.zbb => (a as i32).min(b as i32) as u32,
            (5, 0x05) if self.zbb => a.min(b),
            (6, 0x05) if self.zbb => (a as i32).max(b as i32) as u32,
            (7, 0x05) if self.zbb => a.max(b),
            (1..=3, 0x05) if self.zbc => {
                let mut product = 0;
                for index in 0..32 {
                    if b & (1 << index) != 0 {
                        product ^= u64::from(a) << index;
                    }
                }
                let shift = match funct3 {
                    1 => 0,
                    2 => 31,
                    _ => 32,
                };
                (product >> shift) as u32
            }
            _ => return self.single_bit(funct3, funct7, a, b & 31),
        })
    }

    fn single_bit(self, funct3: u32, funct7: u32, value: u32, index: u32) -> Option<u32> {
        if !self.zbs {
            return None;
        }
        let mask = 1 << index;
        Some(match (funct3, funct7) {
            (1, 0x14) => value | mask,
            (1, 0x24) => value & !mask,
            (1, 0x34) => value ^ mask,
            (5, 0x24) => (value >> index) & 1,
            _ => return None,
        })
    }
}
