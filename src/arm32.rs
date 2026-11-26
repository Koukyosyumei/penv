//! AArch32 integer execution and interworking for ARMv7-A and ARMv7-R.
use std::io::Write;

use crate::{
    bus::Bus,
    machine::{Step, Stop},
    thumb::Thumb,
};

const N: u32 = 1 << 31;
const Z: u32 = 1 << 30;
const C: u32 = 1 << 29;
const V: u32 = 1 << 28;
const T: u32 = 1 << 5;
const FLAGS: u32 = N | Z | C | V;

#[derive(Clone, Copy)]
pub(crate) enum Profile {
    Application,
    RealTime,
}

impl Profile {
    pub(crate) fn triple(self) -> &'static str {
        match self {
            Self::Application => "armv7a-none-eabi",
            Self::RealTime => "armv7r-none-eabi",
        }
    }
}

#[derive(Clone)]
pub(crate) struct Cpu {
    pub(crate) profile: Profile,
    cpsr: u32,
    thumb: Thumb,
}

impl Cpu {
    pub(crate) fn new(profile: Profile) -> Self {
        let mut thumb = Thumb::new(None);
        thumb.thumb2 = true;
        Self {
            profile,
            cpsr: 0x1d3,
            thumb,
        }
    }

    pub(crate) fn cpsr(&self) -> u32 {
        self.cpsr
    }

    pub(crate) fn set_cpsr(&mut self, value: u32) {
        self.cpsr = value;
    }

    fn reg(registers: &[u32; 32], index: usize, address: u32) -> u32 {
        if index == 15 {
            address.wrapping_add(8)
        } else {
            registers[index]
        }
    }

    fn exchange(&mut self, address: u32) -> u32 {
        self.cpsr = (self.cpsr & !T) | ((address & 1) << 5);
        address & if self.cpsr & T != 0 { !1 } else { !3 }
    }

    fn condition(&self, condition: u32) -> bool {
        let n = self.cpsr & N != 0;
        let z = self.cpsr & Z != 0;
        let c = self.cpsr & C != 0;
        let v = self.cpsr & V != 0;
        let passes = match condition >> 1 {
            0 => z,
            1 => c,
            2 => n,
            3 => v,
            4 => c && !z,
            5 => n == v,
            6 => n == v && !z,
            _ => true,
        };
        passes ^ (condition & 1 != 0)
    }

    fn shift(&self, value: u32, kind: u32, amount: u32, immediate: bool) -> (u32, bool) {
        let carry = self.cpsr & C != 0;
        if amount == 0 && (!immediate || kind == 0) {
            return (value, carry);
        }
        match (kind, amount) {
            (0, 1..=32) => (
                value.checked_shl(amount).unwrap_or(0),
                value & (1 << (32 - amount)) != 0,
            ),
            (0, _) => (0, false),
            (1, 0 | 32) => (0, value & N != 0),
            (1, 1..=31) => (value >> amount, value & (1 << (amount - 1)) != 0),
            (1, _) => (0, false),
            (2, 0 | 32..) => (((value as i32) >> 31) as u32, value & N != 0),
            (2, amount) => (
                ((value as i32) >> amount) as u32,
                value & (1 << (amount - 1)) != 0,
            ),
            (3, 0) => (value >> 1 | (u32::from(carry) << 31), value & 1 != 0),
            (3, amount) => {
                let shifted = value.rotate_right(amount);
                (shifted, shifted & N != 0)
            }
            _ => unreachable!(),
        }
    }

    fn operand(&self, instruction: u32, registers: &[u32; 32], pc: u32) -> (u32, bool) {
        if instruction & (1 << 25) != 0 {
            let rotation = ((instruction >> 8) & 15) * 2;
            let shifted = (instruction & 255).rotate_right(rotation);
            (
                shifted,
                if rotation == 0 {
                    self.cpsr & C != 0
                } else {
                    shifted & N != 0
                },
            )
        } else {
            let value = Self::reg(registers, (instruction & 15) as usize, pc);
            let immediate = instruction & 0x10 == 0;
            let amount = if immediate {
                (instruction >> 7) & 31
            } else {
                Self::reg(registers, ((instruction >> 8) & 15) as usize, pc) & 255
            };
            self.shift(value, (instruction >> 5) & 3, amount, immediate)
        }
    }

    fn arithmetic(left: u32, right: u32, carry: bool) -> (u32, bool, bool) {
        let sum = u64::from(left) + u64::from(right) + u64::from(carry);
        let signed = i64::from(left as i32) + i64::from(right as i32) + i64::from(carry);
        (sum as u32, sum >> 32 != 0, i32::try_from(signed).is_err())
    }

    fn process(&mut self, instruction: u32, registers: &mut [u32; 32], pc: u32) -> Option<u32> {
        let opcode = (instruction >> 21) & 15;
        let ra = Self::reg(registers, ((instruction >> 16) & 15) as usize, pc);
        let rd = ((instruction >> 12) & 15) as usize;
        let (rb, shift_carry) = self.operand(instruction, registers, pc);
        let carry = self.cpsr & C != 0;
        let old_overflow = self.cpsr & V != 0;
        let logic = |value| (value, shift_carry, old_overflow);
        let (value, carry, overflow) = match opcode {
            0 | 8 => logic(ra & rb),
            1 | 9 => logic(ra ^ rb),
            2 | 10 => Self::arithmetic(ra, !rb, true),
            3 => Self::arithmetic(rb, !ra, true),
            4 | 11 => Self::arithmetic(ra, rb, false),
            5 => Self::arithmetic(ra, rb, carry),
            6 => Self::arithmetic(ra, !rb, carry),
            7 => Self::arithmetic(rb, !ra, carry),
            12 => logic(ra | rb),
            13 => logic(rb),
            14 => logic(ra & !rb),
            _ => logic(!rb),
        };
        let test = matches!(opcode, 8..=11);
        if test || instruction & (1 << 20) != 0 {
            self.cpsr = (self.cpsr & !FLAGS)
                | (value & N)
                | (u32::from(value == 0) * Z)
                | (u32::from(carry) * C)
                | (u32::from(overflow) * V);
        }
        if test {
            None
        } else if rd == 15 {
            Some(self.exchange(value))
        } else {
            registers[rd] = value;
            None
        }
    }

    fn load(bus: &mut Bus, slot: usize, address: u32, width: usize) -> Result<u32, String> {
        if let Some(bytes) = bus.cpu_ram64(slot, u64::from(address), width) {
            let mut word = [0; 4];
            word[..width].copy_from_slice(bytes);
            return Ok(u32::from_le_bytes(word));
        }
        match width {
            1 => bus.read::<1>(address),
            2 => bus.read::<2>(address),
            _ => bus.read::<4>(address),
        }
        .map_err(|error| error.to_string())
    }

    fn store(
        bus: &mut Bus,
        slot: usize,
        address: u32,
        width: usize,
        value: u32,
        output: &mut impl Write,
    ) -> Result<(), String> {
        if let Some(bytes) = bus.cpu_memory64(slot, u64::from(address), width) {
            bytes.copy_from_slice(&value.to_le_bytes()[..width]);
            bus.observe_memory_store(u64::from(address), width, u64::from(value));
            return Ok(());
        }
        bus.write(address, width, value, output)
            .map_err(|error| error.to_string())
    }

    pub(crate) fn step(
        &mut self,
        pc: &mut u32,
        registers: &mut [u32; 32],
        bus: &mut Bus,
        slot: usize,
        output: &mut impl Write,
    ) -> Result<Step, String> {
        if self.cpsr & T != 0 {
            let raw = Self::load(bus, slot, *pc, 2)?;
            if raw & 0xff07 == 0x4700 {
                let reg = ((raw >> 3) & 15) as usize;
                let target = if reg == 15 {
                    pc.wrapping_add(4)
                } else {
                    registers[reg]
                };
                if raw & 0x80 != 0 {
                    registers[14] = pc.wrapping_add(2) | 1;
                }
                *pc = self.exchange(target);
                return Ok(Step::Retired);
            }
            self.thumb.set_apsr(self.cpsr);
            let retired = self.thumb.step(pc, registers, bus, output)?;
            self.cpsr = (self.cpsr & !FLAGS) | (self.thumb.apsr() & FLAGS);
            return Ok(if retired {
                Step::Retired
            } else {
                Step::Stop(Stop::Ebreak)
            });
        }
        let instruction = Self::load(bus, slot, *pc, 4)?;
        let address = *pc;
        let mut next = address.wrapping_add(4);
        let condition = instruction >> 28;
        if instruction & 0xfe00_0000 == 0xfa00_0000 {
            let offset = (((instruction << 8) as i32 >> 6) as u32) | ((instruction >> 23) & 2);
            registers[14] = next;
            next = self.exchange(address.wrapping_add(8).wrapping_add(offset) | 1);
        } else if self.condition(condition) {
            let rn = ((instruction >> 16) & 15) as usize;
            let rd = ((instruction >> 12) & 15) as usize;
            match instruction {
                _ if matches!(instruction & 0x0fff_fff0, 0x012f_ff10 | 0x012f_ff30) => {
                    let target = Self::reg(registers, (instruction & 15) as usize, address);
                    if instruction & 0x20 != 0 {
                        registers[14] = next;
                    }
                    next = self.exchange(target);
                }
                _ if instruction & 0x0e00_0000 == 0x0a00_0000 => {
                    if instruction & (1 << 24) != 0 {
                        registers[14] = next;
                    }
                    next = address
                        .wrapping_add(8)
                        .wrapping_add(((instruction << 8) as i32 >> 6) as u32);
                }
                _ if matches!(instruction & 0x0ff0_0000, 0x0300_0000 | 0x0340_0000) && rd != 15 => {
                    let immediate = ((instruction >> 4) & 0xf000) | (instruction & 0xfff);
                    registers[rd] = if instruction & (1 << 22) == 0 {
                        immediate
                    } else {
                        (registers[rd] & 0xffff) | (immediate << 16)
                    };
                }
                _ if instruction & 0x0c00_0000 == 0x0400_0000
                    && (instruction & (1 << 25) == 0 || instruction & 0x10 == 0) =>
                {
                    let base = Self::reg(registers, rn, address);
                    let offset = if instruction & (1 << 25) == 0 {
                        instruction & 0xfff
                    } else {
                        self.shift(
                            Self::reg(registers, (instruction & 15) as usize, address),
                            (instruction >> 5) & 3,
                            (instruction >> 7) & 31,
                            true,
                        )
                        .0
                    };
                    let updated = if instruction & (1 << 23) != 0 {
                        base.wrapping_add(offset)
                    } else {
                        base.wrapping_sub(offset)
                    };
                    let pre = instruction & (1 << 24) != 0;
                    let target = if pre { updated } else { base };
                    let width = if instruction & (1 << 22) != 0 { 1 } else { 4 };
                    if instruction & (1 << 20) != 0 {
                        let value = Self::load(bus, slot, target, width)?;
                        if rd == 15 {
                            next = self.exchange(value);
                        } else {
                            registers[rd] = value;
                        }
                    } else {
                        Self::store(
                            bus,
                            slot,
                            target,
                            width,
                            Self::reg(registers, rd, address),
                            output,
                        )?;
                    }
                    if rn != 15 && (!pre || instruction & (1 << 21) != 0) {
                        registers[rn] = updated;
                    }
                }
                _ if instruction & 0x0c00_0000 == 0
                    && (instruction & (1 << 25) != 0 || instruction & 0x90 != 0x90) =>
                {
                    if let Some(branch) = self.process(instruction, registers, address) {
                        next = branch;
                    }
                }
                _ => {
                    return Err(format!(
                        "unsupported A32 instruction {instruction:#010x} at {address:#010x}"
                    ));
                }
            }
        }
        *pc = next;
        Ok(Step::Retired)
    }
}
