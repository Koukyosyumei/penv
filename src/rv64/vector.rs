//! RVV control state and vector length configuration.

const DEFAULT_VLEN: u64 = 512;
const MAX_VLEN: u64 = 1 << 16;
const ELEN: u64 = 64;
const VILL: u64 = 1 << 63;

#[derive(Clone)]
pub(crate) struct VectorState {
    start: u64,
    control: u64,
    length: u64,
    kind: u64,
    bits: u64,
    registers: Vec<u8>,
}

impl Default for VectorState {
    fn default() -> Self {
        Self::new(DEFAULT_VLEN).expect("default vector length")
    }
}

impl VectorState {
    pub(crate) fn new(bits: u64) -> Result<Self, &'static str> {
        if !(ELEN..=MAX_VLEN).contains(&bits) || !bits.is_power_of_two() {
            return Err(
                "RV64 vector register length must be a power of two between 64 and 65536 bits",
            );
        }
        Ok(Self {
            start: 0,
            control: 0,
            length: 0,
            kind: VILL,
            bits,
            registers: vec![0; 32 * (bits / 8) as usize],
        })
    }

    pub(crate) fn read(&self, csr: u32) -> Option<u64> {
        Some(match csr {
            0x008 => self.start,
            0x009 => self.control & 1,
            0x00a => self.control >> 1,
            0x00f => self.control,
            0xc20 => self.length,
            0xc21 => self.kind,
            0xc22 => self.bits / 8,
            _ => return None,
        })
    }

    pub(crate) fn write(&mut self, csr: u32, value: u64) -> bool {
        match csr {
            0x008 => self.start = value & (self.bits - 1),
            0x009 => self.control = (self.control & 6) | (value & 1),
            0x00a => self.control = (self.control & 1) | ((value & 3) << 1),
            0x00f => self.control = value & 7,
            _ => return false,
        }
        true
    }

    fn max_length(&self, kind: u64) -> Option<u64> {
        let sew = 8 << ((kind >> 3) & 7);
        let (numerator, denominator) = match kind & 7 {
            lmul @ 0..=3 => (1 << lmul, 1),
            lmul @ 5..=7 => (1, 1 << (8 - lmul)),
            _ => return None,
        };
        if kind >> 8 != 0 || sew > ELEN / denominator {
            return None;
        }
        Some(self.bits * numerator / (sew * denominator))
    }

    pub(crate) fn configure(&mut self, kind: u64, avl: Option<u64>) -> u64 {
        let max = self.max_length(kind);
        self.kind = if max.is_some() { kind } else { VILL };
        self.length = avl.unwrap_or(self.length).min(max.unwrap_or(0));
        self.start = 0;
        self.length
    }

    fn group_is_valid(&self, register: usize) -> bool {
        let group = 1 << (self.kind as usize & 7).min(4);
        self.kind & 7 >= 5 || register % group == 0
    }

    fn element(&self, register: usize, lane: usize, width: usize) -> i128 {
        let start = register * (self.bits / 8) as usize + lane * width;
        let mut bytes = [0; 8];
        bytes[..width].copy_from_slice(&self.registers[start..start + width]);
        let shift = 64 - width * 8;
        i128::from((u64::from_le_bytes(bytes) << shift) as i64 >> shift)
    }

    fn set_element(&mut self, register: usize, lane: usize, width: usize, value: u64) {
        let start = register * (self.bits / 8) as usize + lane * width;
        self.registers[start..start + width].copy_from_slice(&value.to_le_bytes()[..width]);
    }

    pub(crate) fn execute(&mut self, instruction: u32, scalar: u64) -> Option<()> {
        if self.kind == VILL {
            return None;
        }
        let rd = ((instruction >> 7) & 31) as usize;
        let rs1 = ((instruction >> 15) & 31) as usize;
        let rs2 = ((instruction >> 20) & 31) as usize;
        let unmasked = instruction & (1 << 25) != 0;
        let width = 1 << ((self.kind >> 3) & 7);
        if !self.group_is_valid(rd) {
            return None;
        }
        match (instruction >> 26, (instruction >> 12) & 7) {
            (0x17, 4) if unmasked && rs2 == 0 => {
                for lane in self.start..self.length {
                    self.set_element(rd, lane as usize, width, scalar);
                }
            }
            (0x21, 0)
                if (unmasked || rd != 0)
                    && self.group_is_valid(rs1)
                    && self.group_is_valid(rs2) =>
            {
                let bound = 1i128 << (width * 8 - 1);
                for lane in self.start as usize..self.length as usize {
                    if !unmasked && self.registers[lane / 8] & (1 << (lane % 8)) == 0 {
                        continue;
                    }
                    let sum = self.element(rs1, lane, width) + self.element(rs2, lane, width);
                    let result = sum.clamp(-bound, bound - 1);
                    self.control |= u64::from(result != sum);
                    self.set_element(rd, lane, width, result as u64);
                }
            }
            _ => return None,
        }
        self.start = 0;
        Some(())
    }
}

use super::{Fault, Instruction, Rv64};
use crate::{
    bus::Bus,
    rv64_float::{Format, Rounding},
    rv64_priv::{FS, VS},
};
use std::{io::Write, ops::Range};

impl VectorState {
    fn sew_log_bytes(&self) -> u32 {
        ((self.kind >> 3) & 7) as u32
    }

    fn log_multiplier(&self) -> i32 {
        let encoding = (self.kind & 7) as i32;
        if encoding >= 4 {
            encoding - 8
        } else {
            encoding
        }
    }

    fn group(&self, register: usize, multiplier: i32) -> Option<Range<usize>> {
        if !(-3..=3).contains(&multiplier) {
            return None;
        }
        let registers = 1 << multiplier.max(0);
        if register % registers != 0 || register + registers > 32 {
            return None;
        }
        let bytes = (self.bits / 8) as usize;
        let start = register * bytes;
        let length = (bytes << multiplier.max(0)) >> (-multiplier).max(0);
        Some(start..start + length)
    }
}

impl Rv64 {
    pub(super) fn andes_vector_bfloat16(
        &mut self,
        inst: Instruction,
        narrow: bool,
    ) -> Result<(), Fault> {
        let illegal = || Fault::new(2, u64::from(inst.0));
        if !self.isa.single || self.privilege.status & VS == 0 || self.privilege.status & FS == 0 {
            return Err(illegal());
        }
        let vector = self.privilege.vector.as_mut().ok_or_else(illegal)?;
        if vector.kind == VILL || vector.sew_log_bytes() != 1 {
            return Err(illegal());
        }
        let lmul = vector.log_multiplier();
        let (source, destination, rounding) = if narrow {
            (
                Format::Single,
                Format::BFloat16,
                Rounding::decode(7, self.privilege.fcsr).ok_or_else(illegal)?,
            )
        } else {
            (Format::BFloat16, Format::Single, Rounding::NearestEven)
        };
        let src = vector
            .group(inst.rs2(), lmul + i32::from(narrow))
            .ok_or_else(illegal)?;
        let dst = vector
            .group(inst.rd(), lmul + i32::from(!narrow))
            .ok_or_else(illegal)?;
        if src.start < dst.end
            && dst.start < src.end
            && !(if narrow {
                src.start == dst.start
            } else {
                lmul >= 0 && src.end == dst.end
            })
        {
            return Err(illegal());
        }
        let mut flags = 0;
        for lane in vector.start as usize..vector.length as usize {
            let bits = vector.element(inst.rs2(), lane, source.width()) as u64;
            let converted = destination.convert(source.decode(source.boxed(bits)), rounding);
            flags |= converted.flags;
            vector.set_element(inst.rd(), lane, destination.width(), converted.bits);
        }
        vector.start = 0;
        self.privilege.fcsr |= flags;
        self.privilege.status |= VS | FS;
        Ok(())
    }

    pub(super) fn vector_unit_stride(
        &mut self,
        inst: Instruction,
        base: u64,
        bus: &mut Bus,
        output: &mut impl Write,
    ) -> Result<(), Fault> {
        let illegal = || Fault::new(2, u64::from(inst.0));
        if self.privilege.status & VS == 0 || inst.0 >> 25 != 1 || inst.rs2() != 0 {
            return Err(illegal());
        }
        let vector = self.privilege.vector.as_ref().ok_or_else(illegal)?;
        if vector.kind == VILL {
            return Err(illegal());
        }
        let log_width: u32 = match inst.funct3() {
            0 => 0,
            kind => kind - 4,
        };
        let width = 1usize << log_width;
        let register = inst.rd();
        let multiplier = vector.log_multiplier() + log_width as i32 - vector.sew_log_bytes() as i32;
        vector.group(register, multiplier).ok_or_else(illegal)?;
        for lane in vector.start as usize..vector.length as usize {
            let address = base.wrapping_add((lane * width) as u64);
            self.privilege.vector.as_mut().expect("vector state").start = lane as u64;
            self.privilege.status |= VS;
            if inst.opcode() == 0x07 {
                let value = self.load(bus, address, width)?;
                self.privilege
                    .vector
                    .as_mut()
                    .expect("vector state")
                    .set_element(register, lane, width, value);
            } else {
                let value = self
                    .privilege
                    .vector
                    .as_ref()
                    .expect("vector state")
                    .element(register, lane, width);
                self.store(bus, address, width, value as u64, output)?;
            }
        }
        self.privilege.vector.as_mut().expect("vector state").start = 0;
        self.privilege.status |= VS;
        Ok(())
    }
}
