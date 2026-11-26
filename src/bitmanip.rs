#[derive(Clone, Copy, Default)]
pub struct BitManip {
    pub zba: bool,
    pub zbb: bool,
    pub zbc: bool,
    pub zbkc: bool,
    pub zknh: bool,
    pub zkne: bool,
    pub zknd: bool,
    pub zksh: bool,
    pub zksed: bool,
    pub zbs: bool,
    pub zbkb: bool,
    pub zbkx: bool,
    pub zcb: bool,
    pub xandesperf: bool,
    pub xandesbfhcvt: bool,
    pub xandesvsintload: bool,
    pub xandesvbfhcvt: bool,
    pub xandesvqmac: bool,
    pub xandesvdot: bool,
    pub xandesvpackfph: bool,
    pub xandesvsinth: bool,
    pub vector: bool,
}

impl BitManip {
    pub(crate) fn has_vector_execution(self) -> bool {
        self.vector || self.xandesvsintload || self.xandesvbfhcvt || self.xandesvqmac
            || self.xandesvdot || self.xandesvpackfph || self.xandesvsinth
    }

    pub fn immediate(self, funct3: u32, funct7: u32, value: u32, imm: u32) -> Option<u32> {
        let index = imm & 31;
        if self.zksh && funct3 == 1 && funct7 == 0x08 {
            match index {
                8 => return Some(value ^ value.rotate_left(9) ^ value.rotate_left(17)),
                9 => return Some(value ^ value.rotate_left(15) ^ value.rotate_left(23)),
                _ => {}
            }
        }
        if self.zknh && funct3 == 1 && funct7 == 0x08 {
            return Some(match index {
                0 => value.rotate_right(2) ^ value.rotate_right(13) ^ value.rotate_right(22),
                1 => value.rotate_right(6) ^ value.rotate_right(11) ^ value.rotate_right(25),
                2 => value.rotate_right(7) ^ value.rotate_right(18) ^ (value >> 3),
                3 => value.rotate_right(17) ^ value.rotate_right(19) ^ (value >> 10),
                _ => return None,
            });
        }
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
                (5, 0x34, 7) => return Some(crate::bit_field::reverse_by_byte(value)),
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
            (0, 0x28..=0x2b | 0x2e | 0x2f) if self.zknh => sha512_half(funct7, a, b),
            (0, _) if self.zkne && matches!(funct7 & 0x1f, 0x11 | 0x13) => {
                a ^ aes32_transform(funct7, b, false)
            }
            (0, _) if self.zknd && matches!(funct7 & 0x1f, 0x15 | 0x17) => {
                a ^ aes32_transform(funct7, b, true)
            }
            (0, _) if self.zksed && matches!(funct7 & 0x1f, 0x18 | 0x1a) => {
                a ^ sm4_transform(funct7, b)
            }
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
            (1..=3, 0x05) if self.zbc || (self.zbkc && funct3 != 2) => {
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

// SHA-512 split instructions return one 32-bit half, with source order reversed for the high form.
fn sha512_half(funct7: u32, a: u32, b: u32) -> u32 {
    let high = matches!(funct7, 0x2e | 0x2f);
    let (lo, hi) = if high { (b, a) } else { (a, b) };
    let value = (u64::from(hi) << 32) | u64::from(lo);
    let transformed = match funct7 {
        0x28 => value.rotate_right(28) ^ value.rotate_right(34) ^ value.rotate_right(39),
        0x29 => value.rotate_right(14) ^ value.rotate_right(18) ^ value.rotate_right(41),
        0x2a | 0x2e => value.rotate_right(1) ^ value.rotate_right(8) ^ (value >> 7),
        0x2b | 0x2f => value.rotate_right(19) ^ value.rotate_right(61) ^ (value >> 6),
        _ => unreachable!(),
    };
    if high {
        (transformed >> 32) as u32
    } else {
        transformed as u32
    }
}

const SM4_SBOX: [u8; 256] = [
    0xd6, 0x90, 0xe9, 0xfe, 0xcc, 0xe1, 0x3d, 0xb7, 0x16, 0xb6, 0x14, 0xc2, 0x28, 0xfb, 0x2c, 0x05,
    0x2b, 0x67, 0x9a, 0x76, 0x2a, 0xbe, 0x04, 0xc3, 0xaa, 0x44, 0x13, 0x26, 0x49, 0x86, 0x06, 0x99,
    0x9c, 0x42, 0x50, 0xf4, 0x91, 0xef, 0x98, 0x7a, 0x33, 0x54, 0x0b, 0x43, 0xed, 0xcf, 0xac, 0x62,
    0xe4, 0xb3, 0x1c, 0xa9, 0xc9, 0x08, 0xe8, 0x95, 0x80, 0xdf, 0x94, 0xfa, 0x75, 0x8f, 0x3f, 0xa6,
    0x47, 0x07, 0xa7, 0xfc, 0xf3, 0x73, 0x17, 0xba, 0x83, 0x59, 0x3c, 0x19, 0xe6, 0x85, 0x4f, 0xa8,
    0x68, 0x6b, 0x81, 0xb2, 0x71, 0x64, 0xda, 0x8b, 0xf8, 0xeb, 0x0f, 0x4b, 0x70, 0x56, 0x9d, 0x35,
    0x1e, 0x24, 0x0e, 0x5e, 0x63, 0x58, 0xd1, 0xa2, 0x25, 0x22, 0x7c, 0x3b, 0x01, 0x21, 0x78, 0x87,
    0xd4, 0x00, 0x46, 0x57, 0x9f, 0xd3, 0x27, 0x52, 0x4c, 0x36, 0x02, 0xe7, 0xa0, 0xc4, 0xc8, 0x9e,
    0xea, 0xbf, 0x8a, 0xd2, 0x40, 0xc7, 0x38, 0xb5, 0xa3, 0xf7, 0xf2, 0xce, 0xf9, 0x61, 0x15, 0xa1,
    0xe0, 0xae, 0x5d, 0xa4, 0x9b, 0x34, 0x1a, 0x55, 0xad, 0x93, 0x32, 0x30, 0xf5, 0x8c, 0xb1, 0xe3,
    0x1d, 0xf6, 0xe2, 0x2e, 0x82, 0x66, 0xca, 0x60, 0xc0, 0x29, 0x23, 0xab, 0x0d, 0x53, 0x4e, 0x6f,
    0xd5, 0xdb, 0x37, 0x45, 0xde, 0xfd, 0x8e, 0x2f, 0x03, 0xff, 0x6a, 0x72, 0x6d, 0x6c, 0x5b, 0x51,
    0x8d, 0x1b, 0xaf, 0x92, 0xbb, 0xdd, 0xbc, 0x7f, 0x11, 0xd9, 0x5c, 0x41, 0x1f, 0x10, 0x5a, 0xd8,
    0x0a, 0xc1, 0x31, 0x88, 0xa5, 0xcd, 0x7b, 0xbd, 0x2d, 0x74, 0xd0, 0x12, 0xb8, 0xe5, 0xb4, 0xb0,
    0x89, 0x69, 0x97, 0x4a, 0x0c, 0x96, 0x77, 0x7e, 0x65, 0xb9, 0xf1, 0x09, 0xc5, 0x6e, 0xc6, 0x84,
    0x18, 0xf0, 0x7d, 0xec, 0x3a, 0xdc, 0x4d, 0x20, 0x79, 0xee, 0x5f, 0x3e, 0xd7, 0xcb, 0x39, 0x48,
];

fn sm4_transform(funct7: u32, source: u32) -> u32 {
    let shift = (funct7 >> 5) * 8;
    let byte = ((source >> shift) & 0xff) as usize;
    let x = u32::from(SM4_SBOX[byte]);
    // Apply the round or key-schedule linear transform to the selected byte.
    let transformed = if funct7 & 0x02 == 0 {
        x ^ (x << 8) ^ (x << 2) ^ (x << 18) ^ ((x & 0x3f) << 26) ^ ((x & 0xc0) << 10)
    } else {
        x ^ ((x & 0x07) << 29) ^ ((x & 0xfe) << 7) ^ ((x & 0x01) << 23) ^ ((x & 0xf8) << 13)
    };
    transformed.rotate_left(shift)
}

const AES_SBOXES: ([u8; 256], [u8; 256]) = aes_sboxes();

const fn aes_sboxes() -> ([u8; 256], [u8; 256]) {
    let mut forward = [0; 256];
    let mut inverse = [0; 256];
    let mut index = 0;
    while index < 256 {
        // Compute the field inverse x^254, then apply the AES affine transform.
        let mut power = index as u8;
        let mut reciprocal = 1;
        let mut exponent = 254;
        while exponent != 0 {
            if exponent & 1 != 0 {
                reciprocal = aes_multiply(reciprocal, power);
            }
            power = aes_multiply(power, power);
            exponent >>= 1;
        }
        let substituted = reciprocal
            ^ reciprocal.rotate_left(1)
            ^ reciprocal.rotate_left(2)
            ^ reciprocal.rotate_left(3)
            ^ reciprocal.rotate_left(4)
            ^ 0x63;
        forward[index] = substituted;
        inverse[substituted as usize] = index as u8;
        index += 1;
    }
    (forward, inverse)
}

const fn aes_multiply(mut value: u8, mut factor: u8) -> u8 {
    let mut product = 0;
    let mut bit = 0;
    while bit < 8 {
        product ^= value & 0u8.wrapping_sub(factor & 1);
        value = (value << 1) ^ (0x1b & 0u8.wrapping_sub(value >> 7));
        factor >>= 1;
        bit += 1;
    }
    product
}

fn aes32_transform(funct7: u32, source: u32, decrypt: bool) -> u32 {
    let shift = (funct7 >> 5) * 8;
    let byte = ((source >> shift) & 0xff) as usize;
    let substituted = if decrypt {
        AES_SBOXES.1[byte]
    } else {
        AES_SBOXES.0[byte]
    };
    let transformed = if funct7 & 0x02 == 0 {
        u32::from(substituted)
    } else {
        // This MixColumn contribution occupies the four output bytes in little-endian order.
        let coefficients = if decrypt {
            [14, 9, 13, 11]
        } else {
            [2, 1, 1, 3]
        };
        u32::from_le_bytes(coefficients.map(|factor| aes_multiply(substituted, factor)))
    };
    transformed.rotate_left(shift)
}
