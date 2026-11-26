use std::io::Write;

use super::{Fault, Instruction, Rv64, sign};
use crate::{
    bus::Bus,
    rv64_float::{Format, Integer, ResultBits, Rounding},
    rv64_priv::FS,
};

impl Rv64 {
    fn format(&self, encoding: u32) -> Option<Format> {
        match encoding {
            0 if self.isa.single => Some(Format::Single),
            1 if self.isa.double => Some(Format::Double),
            2 if self.isa.half => Some(Format::Half),
            _ => None,
        }
    }

    fn fp_write(&mut self, rd: usize, format: Format, bits: u64) {
        self.floating[rd] = format.boxed(bits);
        self.privilege.status |= FS;
    }

    pub(super) fn andes_scalar_bfloat16(
        &mut self,
        inst: Instruction,
        narrow: bool,
    ) -> Result<Option<u64>, Fault> {
        let illegal = || Fault::new(2, u64::from(inst.0));
        if self.privilege.status & FS == 0 {
            return Err(illegal());
        }
        let mut bits = self.floating[inst.rs2()];
        if !self.isa.double {
            bits = Format::Single.boxed(bits);
        }
        let (source, destination, rounding) = if narrow {
            (
                Format::Single,
                Format::BFloat16,
                Rounding::decode(7, self.privilege.fcsr).ok_or_else(illegal)?,
            )
        } else {
            (Format::BFloat16, Format::Single, Rounding::NearestEven)
        };
        let result = destination.convert(source.decode(bits), rounding);
        self.fp_write(inst.rd(), destination, result.bits);
        self.privilege.fcsr |= result.flags;
        Ok(None)
    }

    pub(super) fn floating_instruction(
        &mut self,
        inst: Instruction,
        bus: &mut Bus,
        output: &mut impl Write,
    ) -> Result<Option<u64>, Fault> {
        let illegal = || Fault::new(2, u64::from(inst.0));
        if self.privilege.status & FS == 0 {
            return Err(illegal());
        }
        let opcode = inst.opcode();
        let format = match opcode {
            0x07 | 0x27 => match inst.funct3() {
                1 => self.format(2),
                2 => self.format(0),
                3 => self.format(1),
                _ => None,
            },
            _ => self.format((inst.0 >> 25) & 3),
        }
        .ok_or_else(illegal)?;
        let rd = inst.rd();
        let rs1 = inst.rs1();
        let rs2 = inst.rs2();
        if opcode == 0x07 {
            let address = self.registers[rs1].wrapping_add(inst.immediate());
            self.trace_stack_access(bus, inst, address, format.width(), false);
            let bits = self.load(bus, address, format.width())?;
            self.fp_write(rd, format, bits);
            return Ok(None);
        }
        if opcode == 0x27 {
            let immediate = ((inst.0 >> 25) << 5) | ((inst.0 >> 7) & 31);
            let address = self.registers[rs1].wrapping_add(sign(u64::from(immediate), 12));
            self.trace_stack_access(bus, inst, address, format.width(), true);
            self.store(bus, address, format.width(), self.floating[rs2], output)?;
            return Ok(None);
        }

        let lhs = self.floating[rs1];
        let rhs = self.floating[rs2];
        let decode_lhs = || format.decode(lhs);
        let decode_rhs = || format.decode(rhs);
        let rounding = || Rounding::decode(inst.funct3(), self.privilege.fcsr).ok_or_else(illegal);
        let mut integer_result = false;
        let result = if opcode == 0x53 {
            match inst.0 >> 27 {
                0x00 => format.add(decode_lhs(), decode_rhs(), rounding()?),
                0x01 => format.add(decode_lhs(), decode_rhs().negate(), rounding()?),
                0x02 => format.multiply(decode_lhs(), decode_rhs(), rounding()?),
                0x03 => format.divide(decode_lhs(), decode_rhs(), rounding()?),
                0x04 => format
                    .sign_inject(lhs, rhs, inst.funct3())
                    .ok_or_else(illegal)?,
                0x05 if inst.funct3() <= 1 => format.min_max(lhs, rhs, inst.funct3() == 1),
                0x08 if rs2 as u32 != (inst.0 >> 25) & 3 => {
                    let source = self.format(rs2 as u32).ok_or_else(illegal)?;
                    format.convert(source.decode(lhs), rounding()?)
                }
                0x0b if rs2 == 0 => format.sqrt(decode_lhs(), rounding()?),
                0x14 => {
                    integer_result = true;
                    format
                        .compare(lhs, rhs, inst.funct3())
                        .ok_or_else(illegal)?
                }
                0x18 => {
                    integer_result = true;
                    decode_lhs().to_integer(Integer::decode(rs2).ok_or_else(illegal)?, rounding()?)
                }
                0x1a => format.convert_integer(
                    self.registers[rs1],
                    Integer::decode(rs2).ok_or_else(illegal)?,
                    rounding()?,
                ),
                0x1c if rs2 == 0 => {
                    integer_result = true;
                    let bits = match inst.funct3() {
                        0 => sign(lhs, format.width() as u32 * 8),
                        1 => format.class(lhs),
                        _ => return Err(illegal()),
                    };
                    ResultBits { bits, flags: 0 }
                }
                0x1e if rs2 == 0 && inst.funct3() == 0 => ResultBits {
                    bits: self.registers[rs1],
                    flags: 0,
                },
                _ => return Err(illegal()),
            }
        } else {
            let rs3 = (inst.0 >> 27) as usize;
            let mut lhs = decode_lhs();
            let mut addend = format.decode(self.floating[rs3]);
            if matches!(opcode, 0x4b | 0x4f) {
                lhs = lhs.negate();
            }
            if matches!(opcode, 0x47 | 0x4f) {
                addend = addend.negate();
            }
            format.fused(lhs, decode_rhs(), addend, rounding()?)
        };
        if result.flags != 0 {
            self.privilege.fcsr |= result.flags;
            self.privilege.status |= FS;
        }
        if integer_result {
            Ok(Some(result.bits))
        } else {
            self.fp_write(rd, format, result.bits);
            Ok(None)
        }
    }
}
