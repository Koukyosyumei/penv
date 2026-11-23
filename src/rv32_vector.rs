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

    /// Execute a plain unit-stride VLE/VSE. The caller checks VS and supplies
    /// normal translated guest memory access, propagating the original fault.
    /// A value of None requests a load and Some(value) requests a store.
    pub fn transfer<E>(
        &mut self,
        insn: u32,
        base: u32,
        mut access: impl FnMut(u32, usize, Option<u32>) -> Result<u32, E>,
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
                let mut word = [0; 4];
                word[..width].copy_from_slice(bytes);
                Some(u32::from_le_bytes(word))
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
