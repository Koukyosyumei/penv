//! RV32 vector state for the initial unit-stride transfer subset.

use crate::vector_bootstrap::VectorBootstrap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransferError<E> {
    Illegal,
    Memory(E),
}

#[derive(Clone)]
pub struct Rv32Vector {
    pub csrs: VectorBootstrap,
    registers: Vec<u8>,
}

impl Rv32Vector {
    pub fn new(vlen: u32) -> Option<Self> {
        if !vlen.is_power_of_two() || !(128..=1024).contains(&vlen) {
            return None;
        }
        Some(Self {
            csrs: VectorBootstrap::with_vlen(vlen),
            registers: vec![0; (32 * vlen / 8) as usize],
        })
    }

    pub(crate) fn set_length(&mut self, bits: u64) -> Result<(), &'static str> {
        if u64::from(self.csrs.read(0xc22).unwrap()) * 8 == bits {
            return Ok(());
        }
        self.csrs.set_length(bits)?;
        self.registers = vec![0; (32 * bits / 8) as usize];
        Ok(())
    }

    /// Execute a plain unit-stride VLE/VSE. The caller checks VS and supplies
    /// normal translated guest memory access, propagating the original fault.
    /// A value of None requests a load and Some(value) requests a store.
    pub fn transfer<E>(
        &mut self,
        insn: u32,
        base: u32,
        mut access: impl FnMut(u32, usize, Option<u64>) -> Result<u64, E>,
    ) -> Result<(), TransferError<E>> {
        let load = match insn & 0x7f {
            0x07 => true,
            0x27 => false,
            _ => return Err(TransferError::Illegal),
        };
        // Reject segmented, strided, indexed, extending and special transfers.
        if insn & 0xfdf0_0000 != 0 {
            return Err(TransferError::Illegal);
        }
        let (width, sew) = match (insn >> 12) & 7 {
            0 => (1, 0),
            5 => (2, 1),
            6 => (4, 2),
            7 => (8, 3),
            _ => return Err(TransferError::Illegal),
        };
        let kind = self.csrs.read(0xc21).unwrap();
        let masked = insn & (1 << 25) == 0;
        let register = ((insn >> 7) & 31) as usize;
        // This subset uses EMUL=LMUL=1 and accepts either policy for inactive
        // elements: preserving their values is permitted for agnostic lanes.
        if kind & !0xc0 != sew << 3 || (masked && load && register == 0) {
            return Err(TransferError::Illegal);
        }
        let register_bytes = self.csrs.read(0xc22).unwrap() as usize;
        let start = self.csrs.read(0x008).unwrap() as usize;
        let length = self.csrs.read(0xc20).unwrap() as usize;
        for index in start..length {
            if masked && self.registers[index / 8] & (1 << (index % 8)) == 0 {
                continue;
            }
            let offset = index * width;
            let first = register * register_bytes + offset;
            let bytes = &mut self.registers[first..first + width];
            let value = if load {
                None
            } else {
                let mut word = [0; 8];
                word[..width].copy_from_slice(bytes);
                Some(u64::from_le_bytes(word))
            };
            match access(base.wrapping_add(offset as u32), width, value) {
                Ok(value) if load => bytes.copy_from_slice(&value.to_le_bytes()[..width]),
                Ok(_) => {}
                Err(error) => {
                    self.csrs.write(0x008, index as u32);
                    return Err(TransferError::Memory(error));
                }
            }
        }
        self.csrs.write(0x008, 0);
        Ok(())
    }

    pub(crate) fn quad_mac(&mut self, insn: u32, scalar: u32) -> Option<()> {
        let form = (insn >> 12) & 7;
        let (signed_first, signed_second) = match (insn >> 26, form) {
            (60, 0 | 4) => (false, false),
            (61, 0 | 4) => (true, true),
            (62, 4) => (false, true),
            (63, 0 | 4) => (true, false),
            _ => return None,
        };
        let kind = self.csrs.read(0xc21)?;
        let sew = (kind >> 3) & 7;
        if kind & !0xff != 0 || sew > 1 {
            return None;
        }
        let lmul = match kind & 7 {
            n @ 0..=3 => n as i32,
            n @ 5..=7 => n as i32 - 8,
            _ => return None,
        };
        let width = 1usize << sew;
        let rd = ((insn >> 7) & 31) as usize;
        let rs1 = ((insn >> 15) & 31) as usize;
        let rs2 = ((insn >> 20) & 31) as usize;
        let masked = insn & (1 << 25) == 0;
        if masked && rd == 0 {
            return None;
        }
        let register_bytes = self.csrs.read(0xc22)? as usize;
        let group = |register, multiplier: i32| {
            if !(-3..=3).contains(&multiplier) {
                return None;
            }
            let count = 1 << multiplier.max(0);
            if register % count != 0 || register + count > 32 {
                return None;
            }
            let offset = register * register_bytes;
            let size = (register_bytes << multiplier.max(0)) >> (-multiplier).max(0);
            Some(offset..offset + size)
        };
        let dst = group(rd, lmul + 2)?;
        let src2 = group(rs2, lmul)?;
        let src1 = if form == 0 { Some(group(rs1, lmul)?) } else { None };
        for src in src1.iter().chain(std::iter::once(&src2)) {
            if src.start < dst.end && dst.start < src.end && !(lmul >= 0 && src.end == dst.end) {
                return None;
            }
        }
        let extend = |value: u64, signed| {
            let shift = 64 - width * 8;
            let low = value << shift;
            if signed { ((low as i64) >> shift) as u64 } else { low >> shift }
        };
        for lane in self.csrs.read(0x008)? as usize..self.csrs.read(0xc20)? as usize {
            if masked && self.registers[lane / 8] & (1 << (lane % 8)) == 0 {
                continue;
            }
            let read = |offset, width| {
                let mut bytes = [0; 8];
                bytes[..width].copy_from_slice(&self.registers[offset..offset + width]);
                u64::from_le_bytes(bytes)
            };
            let first = src1.as_ref().map_or(u64::from(scalar), |src| read(src.start + lane * width, width));
            let second = read(src2.start + lane * width, width);
            let offset = dst.start + lane * width * 4;
            let acc = read(offset, width * 4);
            let result = acc.wrapping_add(extend(first, signed_first).wrapping_mul(extend(second, signed_second)));
            self.registers[offset..offset + width * 4].copy_from_slice(&result.to_le_bytes()[..width * 4]);
        }
        self.csrs.write(0x008, 0);
        Some(())
    }

    pub(crate) fn convert_bfloat16(&mut self, insn: u32, fcsr: u8) -> Option<u8> {
        use crate::rv64_float::{Format, Rounding};

        let kind = self.csrs.read(0xc21)?;
        if kind & !0xff != 0 || (kind >> 3) & 7 != 1 {
            return None;
        }
        let lmul = match kind & 7 {
            scale @ 0..=3 => scale as i32,
            scale @ 5..=7 => scale as i32 - 8,
            _ => return None,
        };
        let narrow = insn & (1 << 15) != 0;
        let (source, destination, rounding) = if narrow {
            (Format::Single, Format::BFloat16, Rounding::decode(7, fcsr)?)
        } else {
            (Format::BFloat16, Format::Single, Rounding::NearestEven)
        };
        let register_bytes = self.csrs.read(0xc22)? as usize;
        let group = |register, multiplier: i32| {
            if !(-3..=3).contains(&multiplier) {
                return None;
            }
            let count = 1 << multiplier.max(0);
            if register % count != 0 || register + count > 32 {
                return None;
            }
            let offset = register * register_bytes;
            let size = (register_bytes << multiplier.max(0)) >> (-multiplier).max(0);
            Some(offset..offset + size)
        };
        let src = group(((insn >> 20) & 31) as usize, lmul + i32::from(narrow))?;
        let dst = group(((insn >> 7) & 31) as usize, lmul + i32::from(!narrow))?;
        if src.start < dst.end && dst.start < src.end
            && !(if narrow { src.start == dst.start } else { lmul >= 0 && src.end == dst.end })
        {
            return None;
        }
        let mut flags = 0;
        for lane in self.csrs.read(0x008)? as usize..self.csrs.read(0xc20)? as usize {
            let offset = src.start + lane * source.width();
            let mut bytes = [0; 8];
            bytes[..source.width()].copy_from_slice(&self.registers[offset..offset + source.width()]);
            let converted = destination.convert(source.decode(source.boxed(u64::from_le_bytes(bytes))), rounding);
            let offset = dst.start + lane * destination.width();
            self.registers[offset..offset + destination.width()]
                .copy_from_slice(&converted.bits.to_le_bytes()[..destination.width()]);
            flags |= converted.flags;
        }
        self.csrs.write(0x008, 0);
        Some(flags)
    }

    /// Execute XandesVsIntLoad VLN8.V or VLNU8.V with an RV32 byte reader.
    /// The caller checks VS and the extension and applies the address policy.
    pub fn andes_int4_load<E>(
        &mut self,
        insn: u32,
        base: u32,
        mut read: impl FnMut(u32) -> Result<u8, E>,
    ) -> Result<(), TransferError<E>> {
        // Mask out vm, rs1 and vd. The two load selectors differ at bit 20.
        let signed = match insn & 0xfdf0_707f {
            0x0820_405b => true,
            0x0830_405b => false,
            _ => return Err(TransferError::Illegal),
        };
        let kind = self.csrs.read(0xc21).unwrap();
        let lmul = match kind & 7 {
            scale @ 0..=3 => scale as i32,
            scale @ 5..=7 => scale as i32 - 8,
            _ => return Err(TransferError::Illegal),
        };
        let emul = lmul - ((kind >> 3) & 7) as i32;
        let masked = insn & (1 << 25) == 0;
        let vd = ((insn >> 7) & 31) as usize;
        if kind & !0xff != 0 || !(-3..=3).contains(&emul) || (masked && vd == 0) {
            return Err(TransferError::Illegal);
        }
        let group = 1 << emul.max(0);
        if vd % group != 0 {
            return Err(TransferError::Illegal);
        }

        let first = vd * self.csrs.read(0xc22).unwrap() as usize;
        let start = self.csrs.read(0x008).unwrap() as usize;
        let length = self.csrs.read(0xc20).unwrap() as usize;
        for index in start..length {
            if masked && self.registers[index / 8] & (1 << (index % 8)) == 0 {
                continue;
            }
            let byte = match read(base.wrapping_add((index / 2) as u32)) {
                Ok(byte) => byte,
                Err(error) => {
                    self.csrs.write(0x008, index as u32);
                    return Err(TransferError::Memory(error));
                }
            };
            let nibble = (byte >> (4 * (index % 2))) & 0xf;
            self.registers[first + index] = if signed {
                (((nibble << 4) as i8) >> 4) as u8
            } else {
                nibble
            };
        }
        self.csrs.write(0x008, 0);
        Ok(())
    }
}
