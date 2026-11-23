// RV32 decoding adapted from penv/rv64 0712104489a2342fc63c29eda1a0b41710a7518c.
use std::io::Write;

use super::{Access, Fault, Hart, sign};
use crate::{
    bus::Bus,
    float::{Format, Integer, ResultBits, Rounding},
    pmp,
};

impl Hart {
    fn format(&self, encoding: u32) -> Option<Format> {
        match encoding {
            0 if self.csr.f_extension => Some(Format::Single),
            1 if self.csr.d_extension => Some(Format::Double),
            2 if self.zfh => Some(Format::Half),
            _ => None,
        }
    }

    fn fp_write(&mut self, rd: usize, format: Format, bits: u64) {
        self.floating.as_mut().unwrap()[rd] = format.boxed(bits);
        self.csr.floating_dirty();
    }

    pub(super) fn floating_instruction(
        &mut self,
        insn: u32,
        bus: &mut Bus,
        output: &mut impl Write,
    ) -> Result<Option<u32>, Fault> {
        let illegal = || Fault::Illegal(insn);
        if !self.csr.floating_enabled() {
            return Err(illegal());
        }
        let opcode = insn & 0x7f;
        let funct3 = (insn >> 12) & 7;
        if self.zfbfmin {
            match insn & 0xfff0_007f {
                0x4480_0053 => {
                    self.convert_bfloat(insn, ((insn >> 15) & 31) as usize, true, funct3)?;
                    return Ok(None);
                }
                0x4060_0053 if funct3 == 0 => {
                    self.convert_bfloat(insn, ((insn >> 15) & 31) as usize, false, 0)?;
                    return Ok(None);
                }
                _ => {}
            }
        }
        let format = match opcode {
            0x07 | 0x27 => match funct3 {
                1 if self.zfbfmin => Some(Format::Half),
                1 => self.format(2),
                2 => self.format(0),
                3 => self.format(1),
                _ => None,
            },
            0x53 if self.zfbfmin
                && funct3 == 0
                && matches!(insn & 0xfff0_007f, 0xe400_0053 | 0xf400_0053) =>
            {
                Some(Format::Half)
            }
            _ => self.format((insn >> 25) & 3),
        }
        .ok_or_else(illegal)?;
        let rd = ((insn >> 7) & 31) as usize;
        let rs1 = ((insn >> 15) & 31) as usize;
        let rs2 = ((insn >> 20) & 31) as usize;
        let floating = self.floating.as_ref().unwrap();
        if opcode == 0x07 {
            let address = self.registers[rs1].wrapping_add(sign(insn >> 20, 12));
            let bits = match format {
                Format::Half | Format::BFloat => u64::from(self.load::<2>(bus, address)?),
                Format::Single => u64::from(self.load::<4>(bus, address)?),
                Format::Double => self.fp_load_double(bus, address)?,
            };
            self.fp_write(rd, format, bits);
            return Ok(None);
        }
        if opcode == 0x27 {
            let immediate = ((insn >> 25) << 5) | ((insn >> 7) & 31);
            let address = self.registers[rs1].wrapping_add(sign(immediate, 12));
            let bits = floating[rs2];
            match format {
                Format::Half | Format::BFloat | Format::Single => {
                    self.store(bus, address, format.width(), bits as u32, output)?;
                }
                Format::Double => self.fp_store_double(bus, address, bits, output)?,
            }
            return Ok(None);
        }

        let lhs = floating[rs1];
        let rhs = floating[rs2];
        let decode_lhs = || format.decode(lhs);
        let decode_rhs = || format.decode(rhs);
        let rounding = || Rounding::decode(funct3, self.csr.fcsr).ok_or_else(illegal);
        let mut integer_result = false;
        let result = if opcode == 0x53 {
            match insn >> 27 {
                0x00 => format.add(decode_lhs(), decode_rhs(), rounding()?),
                0x01 => format.add(decode_lhs(), decode_rhs().negate(), rounding()?),
                0x02 => format.multiply(decode_lhs(), decode_rhs(), rounding()?),
                0x03 => format.divide(decode_lhs(), decode_rhs(), rounding()?),
                0x04 => format.sign_inject(lhs, rhs, funct3).ok_or_else(illegal)?,
                0x05 if funct3 <= 1 => format.min_max(lhs, rhs, funct3 == 1),
                0x08 if rs2 as u32 != (insn >> 25) & 3 => {
                    let source = self.format(rs2 as u32).ok_or_else(illegal)?;
                    format.convert(source.decode(lhs), rounding()?)
                }
                0x0b if rs2 == 0 => format.sqrt(decode_lhs(), rounding()?),
                0x14 => {
                    integer_result = true;
                    format.compare(lhs, rhs, funct3).ok_or_else(illegal)?
                }
                0x18 => {
                    integer_result = true;
                    decode_lhs().to_integer(Integer::decode(rs2).ok_or_else(illegal)?, rounding()?)
                }
                0x1a => format.convert_integer(
                    u64::from(self.registers[rs1]),
                    Integer::decode(rs2).ok_or_else(illegal)?,
                    rounding()?,
                ),
                0x1c if rs2 == 0 => {
                    integer_result = true;
                    let bits = match funct3 {
                        0 if matches!(format, Format::Half) => u64::from(sign(lhs as u32, 16)),
                        0 if matches!(format, Format::Single) => lhs,
                        1 => format.class(lhs),
                        _ => return Err(illegal()),
                    };
                    ResultBits { bits, flags: 0 }
                }
                0x1e if rs2 == 0 && funct3 == 0 && !matches!(format, Format::Double) => {
                    ResultBits {
                        bits: u64::from(self.registers[rs1]),
                        flags: 0,
                    }
                }
                _ => return Err(illegal()),
            }
        } else {
            let rs3 = (insn >> 27) as usize;
            let mut lhs = decode_lhs();
            let mut addend = format.decode(floating[rs3]);
            if matches!(opcode, 0x4b | 0x4f) {
                lhs = lhs.negate();
            }
            if matches!(opcode, 0x47 | 0x4f) {
                addend = addend.negate();
            }
            format.fused(lhs, decode_rhs(), addend, rounding()?)
        };
        if result.flags != 0 {
            self.csr.fcsr |= result.flags;
            self.csr.floating_dirty();
        }
        if integer_result {
            Ok(Some(result.bits as u32))
        } else {
            self.fp_write(rd, format, result.bits);
            Ok(None)
        }
    }
    pub(super) fn andes_bfloat_instruction(&mut self, insn: u32) -> Result<(), Fault> {
        let illegal = || Fault::Illegal(insn);
        if !self.csr.floating_enabled() {
            return Err(illegal());
        }
        self.convert_bfloat(insn, ((insn >> 20) & 31) as usize, insn & 0x8000 != 0, 7)
    }

    fn convert_bfloat(
        &mut self,
        insn: u32,
        source: usize,
        narrow: bool,
        rounding: u32,
    ) -> Result<(), Fault> {
        let rd = ((insn >> 7) & 31) as usize;
        let mut bits = self.floating.as_ref().unwrap()[source];
        if !self.csr.d_extension {
            bits = Format::Single.boxed(u64::from(bits as u32));
        }
        let (format, result) = if narrow {
            let rounding = Rounding::decode(rounding, self.csr.fcsr).ok_or(Fault::Illegal(insn))?;
            (
                Format::BFloat,
                Format::BFloat.convert(Format::Single.decode(bits), rounding),
            )
        } else {
            (
                Format::Single,
                Format::Single.convert(Format::BFloat.decode(bits), Rounding::NearestEven),
            )
        };
        self.csr.fcsr |= result.flags;
        self.fp_write(rd, format, result.bits);
        Ok(())
    }

    fn fp_load_double(&mut self, bus: &mut Bus, address: u32) -> Result<u64, Fault> {
        if self.csr.address_trigger_matches(address, pmp::READ) {
            return Err(Fault::Breakpoint(address));
        }
        if address & 7 != 0 && self.unaligned_accesses {
            let mut bytes = [0; 8];
            for (index, byte) in bytes.iter_mut().enumerate() {
                *byte = self.load::<1>(bus, address.wrapping_add(index as u32))? as u8;
            }
            return Ok(u64::from_le_bytes(bytes));
        }
        self.translate(bus, address, 8, pmp::READ)
            .and_then(|physical| bus.read_ram_bytes::<8>(physical).map(u64::from_le_bytes))
            .map_err(|error| Fault::Bus(Access::Load, error.at_virtual(address)))
    }

    fn fp_store_double(
        &mut self,
        bus: &mut Bus,
        address: u32,
        bits: u64,
        output: &mut impl Write,
    ) -> Result<(), Fault> {
        if self.csr.address_trigger_matches(address, pmp::WRITE) {
            return Err(Fault::Breakpoint(address));
        }
        if address & 7 != 0 && self.unaligned_accesses {
            for (index, byte) in bits.to_le_bytes().into_iter().enumerate() {
                self.store(
                    bus,
                    address.wrapping_add(index as u32),
                    1,
                    u32::from(byte),
                    output,
                )?;
            }
            return Ok(());
        }
        let memory = self
            .translate(bus, address, 8, pmp::WRITE)
            .and_then(|physical| bus.memory(physical, 8))
            .map_err(|error| Fault::Bus(Access::Store, error.at_virtual(address)))?;
        memory.copy_from_slice(&bits.to_le_bytes());
        Ok(())
    }
}
