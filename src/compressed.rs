// Expand RV32 compressed integer encodings into the base decoder's format.
pub fn expand(insn: u32) -> Option<u32> {
    expand_integer(insn, false)
}

pub fn expand_rv64(insn: u32, zcb: bool) -> Option<u32> {
    expand_integer(insn, true).or_else(|| zcb.then(|| expand_zcb(insn)).flatten())
}

fn expand_zcb(insn: u32) -> Option<u32> {
    let base = 8 + ((insn >> 7) & 7);
    let operand = 8 + ((insn >> 2) & 7);
    let byte_offset = ((insn >> 4) & 2) | ((insn >> 6) & 1);
    let half_offset = byte_offset & 2;
    let high = (insn >> 6) & 1;
    Some(match insn & 0xfc03 {
        0x8000 => immediate(0x03, 4, operand, base, byte_offset),
        0x8400 => immediate(
            0x03,
            if high == 0 { 5 } else { 1 },
            operand,
            base,
            half_offset,
        ),
        0x8800 => store(base, operand, byte_offset) & !(7 << 12),
        0x8c00 if high == 0 => (store(base, operand, half_offset) & !(7 << 12)) | (1 << 12),
        0x9c01 => match (insn >> 5) & 3 {
            2 => register(0, 1, base, base, operand),
            3 => match (insn >> 2) & 7 {
                0 => immediate(0x13, 7, base, base, 0xff),
                1 => immediate(0x13, 1, base, base, 0x604),
                2 => register(4, 0x04, base, base, 0) | 8,
                3 => immediate(0x13, 1, base, base, 0x605),
                4 => register(0, 0x04, base, base, 0) | 8,
                5 => immediate(0x13, 4, base, base, 0xfff),
                _ => return None,
            },
            _ => return None,
        },
        _ => return None,
    })
}

fn expand_integer(insn: u32, rv64: bool) -> Option<u32> {
    let bits = |lsb: u32, width: u32| (insn >> lsb) & ((1u32 << width) - 1);
    let rd = bits(7, 5);
    let rs2 = bits(2, 5);
    let rs1_prime = 8 + bits(7, 3);
    let rs2_prime = 8 + bits(2, 3);
    let imm = signed((bits(12, 1) << 5) | bits(2, 5), 6);

    Some(match (insn & 3, insn >> 13) {
        (0, 0) => {
            let offset =
                (bits(7, 4) << 6) | (bits(11, 2) << 4) | (bits(5, 1) << 3) | (bits(6, 1) << 2);
            if offset == 0 {
                return None;
            }
            immediate(0x13, 0, rs2_prime, 2, offset)
        }
        (0, 2 | 6) => {
            let offset = (bits(5, 1) << 6) | (bits(10, 3) << 3) | (bits(6, 1) << 2);
            if insn >> 13 == 2 {
                immediate(0x03, 2, rs2_prime, rs1_prime, offset)
            } else {
                store(rs1_prime, rs2_prime, offset)
            }
        }
        (0, kind @ (1 | 3 | 5 | 7)) if rv64 => {
            let offset = (bits(5, 2) << 6) | (bits(10, 3) << 3);
            match kind {
                1 => immediate(0x07, 3, rs2_prime, rs1_prime, offset),
                3 => immediate(0x03, 3, rs2_prime, rs1_prime, offset),
                5 => store(rs1_prime, rs2_prime, offset) | (1 << 12) | 4,
                _ => store(rs1_prime, rs2_prime, offset) | (1 << 12),
            }
        }
        (1, 1) if rv64 && rd != 0 => immediate(0x1b, 0, rd, rd, imm),
        (1, 1) if rv64 => return None,
        (1, 0) => immediate(0x13, 0, rd, rd, imm),
        (1, kind @ (1 | 5)) => {
            let offset = (bits(12, 1) << 11)
                | (bits(8, 1) << 10)
                | (bits(9, 2) << 8)
                | (bits(6, 1) << 7)
                | (bits(7, 1) << 6)
                | (bits(2, 1) << 5)
                | (bits(11, 1) << 4)
                | (bits(3, 3) << 1);
            let offset = signed(offset, 12);
            ((offset & 0x100000) << 11)
                | ((offset & 0x7fe) << 20)
                | ((offset & 0x800) << 9)
                | (offset & 0xff000)
                | (u32::from(kind == 1) << 7)
                | 0x6f
        }
        (1, 2) => immediate(0x13, 0, rd, 0, imm),
        (1, 3) if rd == 2 => {
            let offset = (bits(12, 1) << 9)
                | (bits(3, 2) << 7)
                | (bits(5, 1) << 6)
                | (bits(2, 1) << 5)
                | (bits(6, 1) << 4);
            if offset == 0 {
                return None;
            }
            immediate(0x13, 0, 2, 2, signed(offset, 10))
        }
        (1, 3) if imm != 0 => (imm << 12) | (rd << 7) | 0x37,
        (1, 4) => match bits(10, 2) {
            shift @ (0 | 1) if rv64 || bits(12, 1) == 0 => immediate(
                0x13,
                5,
                rs1_prime,
                rs1_prime,
                (shift << 10) | (bits(12, 1) << 5) | bits(2, 5),
            ),
            2 => immediate(0x13, 7, rs1_prime, rs1_prime, imm),
            3 if bits(12, 1) == 0 => {
                let (funct3, funct7) = [(0, 0x20), (4, 0), (6, 0), (7, 0)][bits(5, 2) as usize];
                register(funct3, funct7, rs1_prime, rs1_prime, rs2_prime)
            }
            3 if rv64 && bits(5, 2) < 2 => {
                register(0, (1 - bits(5, 2)) * 0x20, rs1_prime, rs1_prime, rs2_prime) | 8
            }
            _ => return None,
        },
        (1, kind @ (6 | 7)) => {
            let offset = (bits(12, 1) << 8)
                | (bits(5, 2) << 6)
                | (bits(2, 1) << 5)
                | (bits(10, 2) << 3)
                | (bits(3, 2) << 1);
            let offset = signed(offset, 9);
            ((offset & 0x1000) << 19)
                | ((offset & 0x7e0) << 20)
                | (rs1_prime << 15)
                | ((kind - 6) << 12)
                | ((offset & 0x1e) << 7)
                | ((offset & 0x800) >> 4)
                | 0x63
        }
        (2, 0) if rv64 || bits(12, 1) == 0 => {
            immediate(0x13, 1, rd, rd, (bits(12, 1) << 5) | bits(2, 5))
        }
        (2, 2) if rd != 0 => {
            let offset = (bits(2, 2) << 6) | (bits(12, 1) << 5) | (bits(4, 3) << 2);
            immediate(0x03, 2, rd, 2, offset)
        }
        (2, kind @ (1 | 3)) if rv64 && (kind == 1 || rd != 0) => {
            let offset = (bits(2, 3) << 6) | (bits(12, 1) << 5) | (bits(5, 2) << 3);
            immediate(if kind == 1 { 0x07 } else { 0x03 }, 3, rd, 2, offset)
        }
        (2, kind @ (5 | 7)) if rv64 => {
            let offset = (bits(7, 3) << 6) | (bits(10, 3) << 3);
            store(2, rs2, offset) | (1 << 12) | (u32::from(kind == 5) << 2)
        }
        (2, 4) => match (bits(12, 1), rd, rs2) {
            (0, 0, 0) => return None,
            (0, _, 0) => immediate(0x67, 0, 0, rd, 0),
            (0, _, _) => register(0, 0, rd, 0, rs2),
            (1, 0, 0) => 0x0010_0073,
            (1, _, 0) => immediate(0x67, 0, 1, rd, 0),
            (1, _, _) => register(0, 0, rd, rd, rs2),
            _ => unreachable!(),
        },
        (2, 6) => {
            let offset = (bits(7, 2) << 6) | (bits(9, 4) << 2);
            store(2, rs2, offset)
        }
        _ => return None,
    })
}

fn signed(value: u32, width: u32) -> u32 {
    (((value << (32 - width)) as i32) >> (32 - width)) as u32
}

fn immediate(opcode: u32, funct3: u32, rd: u32, rs1: u32, imm: u32) -> u32 {
    (imm << 20) | (rs1 << 15) | (funct3 << 12) | (rd << 7) | opcode
}

fn register(funct3: u32, funct7: u32, rd: u32, rs1: u32, rs2: u32) -> u32 {
    (funct7 << 25) | (rs2 << 20) | (rs1 << 15) | (funct3 << 12) | (rd << 7) | 0x33
}

fn store(rs1: u32, rs2: u32, offset: u32) -> u32 {
    ((offset & 0xfe0) << 20) | (rs2 << 20) | (rs1 << 15) | (2 << 12) | ((offset & 31) << 7) | 0x23
}
