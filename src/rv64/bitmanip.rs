use super::{Instruction, Isa, sign};

#[derive(Clone, Copy, Default)]
pub(super) struct Extensions {
    pub(super) zba: bool,
    pub(super) zbb: bool,
    zbc: bool,
    pub(super) zbs: bool,
    zicond: bool,
}

impl Extensions {
    pub(super) const NONE: Self = Self {
        zba: false,
        zbb: false,
        zbc: false,
        zbs: false,
        zicond: false,
    };

    pub(super) fn enable_b(&mut self) {
        self.zba = true;
        self.zbb = true;
        self.zbs = true;
    }

    pub(super) fn enable(&mut self, extension: &str) -> bool {
        let flag = match extension {
            "zba" => &mut self.zba,
            "zbb" => &mut self.zbb,
            "zbc" => &mut self.zbc,
            "zbs" => &mut self.zbs,
            "zicond" => &mut self.zicond,
            _ => return false,
        };
        *flag = true;
        true
    }
}

pub(super) fn immediate(isa: Isa, fields: Instruction, lhs: u64) -> Option<u64> {
    let ext = isa.bitmanip;
    let word = fields.opcode() == 0x1b;
    let shamt = (fields.0 >> 20) & 63;
    let bits = fields.0 >> 20;
    let funct3 = fields.funct3();
    if ext.zba && word && funct3 == 1 && bits >> 6 == 0x02 {
        return Some(u64::from(lhs as u32) << shamt);
    }
    if ext.zbb {
        match (word, funct3, bits) {
            (false, 1, 0x600) => return Some(u64::from(lhs.leading_zeros())),
            (false, 1, 0x601) => return Some(u64::from(lhs.trailing_zeros())),
            (false, 1, 0x602) => return Some(u64::from(lhs.count_ones())),
            (true, 1, 0x600) => return Some(u64::from((lhs as u32).leading_zeros())),
            (true, 1, 0x601) => return Some(u64::from((lhs as u32).trailing_zeros())),
            (true, 1, 0x602) => return Some(u64::from((lhs as u32).count_ones())),
            (false, 1, 0x604) => return Some(sign(lhs, 8)),
            (false, 1, 0x605) => return Some(sign(lhs, 16)),
            (false, 5, 0x287) => {
                return Some(u64::from_le_bytes(
                    lhs.to_le_bytes()
                        .map(|byte| if byte == 0 { 0 } else { u8::MAX }),
                ));
            }
            (false, 5, 0x6b8) => return Some(lhs.swap_bytes()),
            (true, 5, _) if bits >> 5 == 0x30 => {
                return Some(sign(u64::from((lhs as u32).rotate_right(shamt)), 32));
            }
            (false, 5, _) if bits >> 6 == 0x18 => return Some(lhs.rotate_right(shamt)),
            _ => {}
        }
    }
    if ext.zbs && !word {
        return bit_mask(fields.funct3(), (bits >> 6) << 1, lhs, shamt);
    }
    None
}

pub(super) fn register(isa: Isa, fields: Instruction, lhs: u64, rhs: u64) -> Option<u64> {
    let ext = isa.bitmanip;
    let word = fields.opcode() == 0x3b;
    let funct3 = fields.funct3();
    let funct7 = fields.funct7();
    let shamt = rhs as u32;
    if ext.zba {
        let base = if word { u64::from(lhs as u32) } else { lhs };
        match (funct3, funct7) {
            (0, 0x04) if word => return Some(base.wrapping_add(rhs)),
            (2 | 4 | 6, 0x10) => return Some((base << (funct3 / 2)).wrapping_add(rhs)),
            _ => {}
        }
    }
    if ext.zbb {
        let result = match (word, funct3, funct7) {
            (false, 4, 0x20) => Some(lhs ^ !rhs),
            (false, 6, 0x20) => Some(lhs | !rhs),
            (false, 7, 0x20) => Some(lhs & !rhs),
            (false, 4, 0x05) => Some((lhs as i64).min(rhs as i64) as u64),
            (false, 5, 0x05) => Some(lhs.min(rhs)),
            (false, 6, 0x05) => Some((lhs as i64).max(rhs as i64) as u64),
            (false, 7, 0x05) => Some(lhs.max(rhs)),
            (false, 1, 0x30) => Some(lhs.rotate_left(shamt)),
            (false, 5, 0x30) => Some(lhs.rotate_right(shamt)),
            (true, 1, 0x30) => Some(sign(u64::from((lhs as u32).rotate_left(shamt)), 32)),
            (true, 5, 0x30) => Some(sign(u64::from((lhs as u32).rotate_right(shamt)), 32)),
            (true, 4, 0x04) if fields.rs2() == 0 => Some(u64::from(lhs as u16)),
            _ => None,
        };
        if result.is_some() {
            return result;
        }
    }
    if ext.zbc && !word && funct7 == 0x05 && matches!(funct3, 1..=3) {
        let mut multiplier = rhs;
        let mut result = 0u128;
        while multiplier != 0 {
            let bit = multiplier.trailing_zeros();
            multiplier &= multiplier - 1;
            result ^= u128::from(lhs) << bit;
        }
        let shift = [0, 63, 64][funct3 as usize - 1];
        return Some((result >> shift) as u64);
    }
    if ext.zicond && !word && funct7 == 7 && matches!(funct3, 5 | 7) {
        return Some(if (rhs == 0) == (funct3 == 5) { 0 } else { lhs });
    }
    if ext.zbs && !word {
        return bit_mask(funct3, funct7, lhs, shamt);
    }
    None
}

fn bit_mask(funct3: u32, funct7: u32, lhs: u64, shift: u32) -> Option<u64> {
    let bit = 1 << (shift & 63);
    Some(match (funct3, funct7) {
        (1, 0x14) => lhs | bit,
        (1, 0x24) => lhs & !bit,
        (5, 0x24) => u64::from(lhs & bit != 0),
        (1, 0x34) => lhs ^ bit,
        _ => return None,
    })
}
