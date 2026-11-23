#[derive(Clone, Copy, Default)]
pub struct BitManip {
    pub zba: bool,
    pub zbb: bool,
    pub zbc: bool,
    pub zbs: bool,
    pub zbkb: bool,
    pub zbkx: bool,
    pub zcb: bool,
    pub xandesperf: bool,
    pub xandesbfhcvt: bool,
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
                _ => {}
            }
        }
        if self.zbb || self.zbkb {
            match (funct3, funct7, index) {
                (5, 0x34, 24) => return Some(value.swap_bytes()),
                (5, 0x30, _) => return Some(value.rotate_right(index)),
                _ => {}
            }
        }
        if self.zbkb {
            match (funct3, funct7, index) {
                (5, 0x34, 7) => return Some(value.reverse_bits().swap_bytes()),
                (1, 0x04, 15) => {
                    let mut result = 0;
                    for bit in 0..16 {
                        result |= ((value >> bit) & 1) << (2 * bit);
                        result |= ((value >> (bit + 16)) & 1) << (2 * bit + 1);
                    }
                    return Some(result);
                }
                (5, 0x04, 15) => {
                    let mut result = 0;
                    for bit in 0..16 {
                        result |= ((value >> (2 * bit)) & 1) << bit;
                        result |= ((value >> (2 * bit + 1)) & 1) << (bit + 16);
                    }
                    return Some(result);
                }
                _ => {}
            }
        }
        self.single_bit(funct3, funct7, value, index)
    }

    pub fn register(self, funct3: u32, funct7: u32, a: u32, b: u32, rs2: u32) -> Option<u32> {
        Some(match (funct3, funct7) {
            (2 | 4 | 6, 0x10) if self.zba => (a << (funct3 / 2)).wrapping_add(b),
            (4, 0x20) if self.zbb || self.zbkb => !(a ^ b),
            (6, 0x20) if self.zbb || self.zbkb => a | !b,
            (7, 0x20) if self.zbb || self.zbkb => a & !b,
            (1, 0x30) if self.zbb || self.zbkb => a.rotate_left(b & 31),
            (5, 0x30) if self.zbb || self.zbkb => a.rotate_right(b & 31),
            (4, 0x04) if self.zbkb => (a & 0xffff) | (b << 16),
            (7, 0x04) if self.zbkb => (a & 0xff) | ((b & 0xff) << 8),
            (4, 0x04) if self.zbb && rs2 == 0 => a & 0xffff,
            (4, 0x05) if self.zbb => (a as i32).min(b as i32) as u32,
            (5, 0x05) if self.zbb => a.min(b),
            (6, 0x05) if self.zbb => (a as i32).max(b as i32) as u32,
            (7, 0x05) if self.zbb => a.max(b),
            (2, 0x14) if self.zbkx => permute(a, b, 4),
            (4, 0x14) if self.zbkx => permute(a, b, 8),
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

    pub fn andes_register(self, funct7: u32, a: u32, b: u32) -> Option<u32> {
        if !self.xandesperf {
            return None;
        }
        match funct7 {
            5..=7 => Some(a.wrapping_add(b << (funct7 - 4))),
            16..=19 => {
                let lhs = a.to_le_bytes();
                let rhs = b.to_le_bytes();
                let matches = |index: usize| match funct7 {
                    16 => lhs[index] == rhs[0],
                    17 => lhs[index] == 0 || lhs[index] != rhs[index],
                    _ => lhs[index] != rhs[index],
                };
                let index = if funct7 == 19 {
                    (0..4).rfind(|&index| matches(index))
                } else {
                    (0..4).find(|&index| matches(index))
                };
                Some(index.map_or(0, |index| (index as u32).wrapping_sub(4)))
            }
            _ => None,
        }
    }

    pub fn andes_bit_field(insn: u32, source: u32, signed: bool) -> u32 {
        let msb = (insn >> 26) & 31;
        let lsb = (insn >> 20) & 31;
        let (start, width, shift) = if msb == 0 {
            (0, 1, lsb)
        } else if msb < lsb {
            (0, lsb - msb + 1, msb)
        } else {
            (lsb, msb - lsb + 1, 0)
        };
        let pad = 32 - width;
        let aligned = (source >> start) << pad;
        let field = if signed {
            ((aligned as i32) >> pad) as u32
        } else {
            aligned >> pad
        };
        field << shift
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

// Each selector lane chooses an equally sized source lane; out-of-range selectors give zero.
fn permute(source: u32, selectors: u32, width: u32) -> u32 {
    let lanes = u32::BITS / width;
    let mask = (1 << width) - 1;
    let mut result = 0;
    for lane in 0..lanes {
        let shift = lane * width;
        let selector = (selectors >> shift) & mask;
        if selector < lanes {
            result |= ((source >> (selector * width)) & mask) << shift;
        }
    }
    result
}
