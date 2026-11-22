use std::io::Write;

use crate::{
    bus::{Bus, BusError, RAM_BASE},
    machine::{Step, Stop},
};

#[derive(Clone)]
pub struct Rv64 {
    pub pc: u64,
    registers: [u64; 32],
    isa: Isa,
}

#[derive(Clone, Copy)]
pub struct Isa {
    pub multiply: bool,
    pub compressed: bool,
}

impl Isa {
    pub const IM: Self = Self {
        multiply: true,
        compressed: false,
    };
}

#[derive(Debug)]
struct Fault {
    cause: u64,
    value: u64,
}

impl Fault {
    fn new(cause: u64, value: u64) -> Self {
        Self { cause, value }
    }
}

impl Rv64 {
    pub fn new(isa: Isa) -> Self {
        Self {
            pc: u64::from(RAM_BASE),
            registers: [0; 32],
            isa,
        }
    }

    pub fn compressed(&self) -> bool {
        self.isa.compressed
    }

    pub fn validate(&self, bus: &Bus) -> Result<(), String> {
        self.fetch(bus)
            .map(|_| ())
            .map_err(|fault| self.describe(&fault))
    }

    fn describe(&self, fault: &Fault) -> String {
        format!(
            "RV64 pc={:#018x}: exception {}, value={:#018x}",
            self.pc, fault.cause, fault.value
        )
    }

    fn align_target(&self, address: u64) -> Result<u64, Fault> {
        let alignment = if self.isa.compressed { 2 } else { 4 };
        if address & (alignment - 1) != 0 {
            return Err(Fault::new(0, address));
        }
        Ok(address)
    }

    fn fetch(&self, bus: &Bus) -> Result<(u32, u64), Fault> {
        let pc = self.align_target(self.pc)?;
        let width = if self.isa.compressed { 2 } else { 4 };
        let low = ram_read(bus, pc, width).ok_or(Fault::new(1, pc))? as u32;
        if width == 4 {
            return Ok((low, 4));
        }
        if low & 3 != 3 {
            return Ok((low, 2));
        }
        let upper = pc.wrapping_add(2);
        let high = ram_read(bus, upper, 2).ok_or(Fault::new(1, upper))? as u32;
        Ok((low | high << 16, 4))
    }

    pub fn step(&mut self, bus: &mut Bus, output: &mut impl Write) -> Result<Step, String> {
        let (instruction, length) = self.fetch(bus).map_err(|fault| self.describe(&fault))?;
        let decoded = if length == 2 {
            crate::compressed::expand_rv64(instruction)
        } else {
            Some(instruction)
        };
        match decoded {
            Some(instruction) => self.execute(instruction, length, bus, output),
            None => Err(Fault::new(2, u64::from(instruction))),
        }
        .map_err(|fault| self.describe(&fault))
    }

    fn execute(
        &mut self,
        instruction: u32,
        length: u64,
        bus: &mut Bus,
        output: &mut impl Write,
    ) -> Result<Step, Fault> {
        let fields = Instruction(instruction);
        let lhs = self.registers[fields.rs1()];
        let rhs = self.registers[fields.rs2()];
        let sequential = self.pc.wrapping_add(length);
        let mut next = sequential;
        let illegal = Fault::new(2, u64::from(instruction));
        let result = match fields.opcode() {
            0x37 => Some(sign(u64::from(instruction & 0xffff_f000), 32)),
            0x17 => Some(
                self.pc
                    .wrapping_add(sign(u64::from(instruction & 0xffff_f000), 32)),
            ),
            0x6f => {
                let offset = ((instruction >> 31) << 20)
                    | (instruction & 0xff000)
                    | ((instruction >> 9) & 0x800)
                    | ((instruction >> 20) & 0x7fe);
                next = self.align_target(self.pc.wrapping_add(sign(u64::from(offset), 21)))?;
                Some(sequential)
            }
            0x67 if fields.funct3() == 0 => {
                next = self.align_target(lhs.wrapping_add(fields.immediate()) & !1)?;
                Some(sequential)
            }
            0x63 => {
                let taken = match fields.funct3() {
                    0 => lhs == rhs,
                    1 => lhs != rhs,
                    4 => (lhs as i64) < rhs as i64,
                    5 => (lhs as i64) >= rhs as i64,
                    6 => lhs < rhs,
                    7 => lhs >= rhs,
                    _ => return Err(illegal),
                };
                if taken {
                    let offset = ((instruction >> 31) << 12)
                        | ((instruction << 4) & 0x800)
                        | ((instruction >> 20) & 0x7e0)
                        | ((instruction >> 7) & 0x1e);
                    next = self.align_target(self.pc.wrapping_add(sign(u64::from(offset), 13)))?;
                }
                None
            }
            0x03 => {
                let kind = fields.funct3();
                let width = match kind {
                    0..=3 => 1 << kind,
                    4..=6 => 1 << (kind - 4),
                    _ => return Err(illegal),
                };
                let address = lhs.wrapping_add(fields.immediate());
                let loaded = self.load(bus, address, width)?;
                Some(if kind < 4 {
                    sign(loaded, width as u32 * 8)
                } else {
                    loaded
                })
            }
            0x23 => {
                let kind = fields.funct3();
                if kind > 3 {
                    return Err(illegal);
                }
                let immediate = ((instruction >> 25) << 5) | ((instruction >> 7) & 31);
                let address = lhs.wrapping_add(sign(u64::from(immediate), 12));
                self.store(bus, address, 1 << kind, rhs, output)?;
                None
            }
            0x13 | 0x1b => Some(immediate_alu(fields, lhs).ok_or(illegal)?),
            0x33 | 0x3b => Some(register_alu(fields, lhs, rhs, self.isa.multiply).ok_or(illegal)?),
            0x0f if matches!(fields.funct3(), 0 | 1) => None,
            0x73 if instruction == 0x73 => return Ok(Step::Stop(Stop::Ecall)),
            0x73 if instruction == 0x0010_0073 => return Ok(Step::Stop(Stop::Ebreak)),
            _ => return Err(illegal),
        };
        if let Some(value) = result
            && fields.rd() != 0
        {
            self.registers[fields.rd()] = value;
        }
        self.pc = next;
        Ok(Step::Retired)
    }

    fn load(&self, bus: &mut Bus, address: u64, width: usize) -> Result<u64, Fault> {
        if address & (width as u64 - 1) != 0 {
            return Err(Fault::new(4, address));
        }
        if let Some(value) = ram_read(bus, address, width) {
            return Ok(value);
        }
        let physical = u32::try_from(address).map_err(|_| Fault::new(5, address))?;
        let result = match width {
            1 => bus.read::<1>(physical).map(u64::from),
            2 => bus.read::<2>(physical).map(u64::from),
            4 => bus.read::<4>(physical).map(u64::from),
            8 => bus
                .clint
                .as_ref()
                .filter(|clint| clint.contains(physical))
                .map(|clint| {
                    u64::from(clint.read(physical)) | (u64::from(clint.read(physical + 4)) << 32)
                })
                .ok_or(BusError::Unmapped(physical, 8)),
            _ => unreachable!(),
        };
        result.map_err(|_| Fault::new(5, address))
    }

    fn store(
        &self,
        bus: &mut Bus,
        address: u64,
        width: usize,
        value: u64,
        output: &mut impl Write,
    ) -> Result<(), Fault> {
        if address & (width as u64 - 1) != 0 {
            return Err(Fault::new(6, address));
        }
        if let Some(bytes) = bus.memory64(address, width) {
            bytes.copy_from_slice(&value.to_le_bytes()[..width]);
            return Ok(());
        }
        let physical = u32::try_from(address).map_err(|_| Fault::new(7, address))?;
        if width == 8 {
            let clint = bus
                .clint
                .as_mut()
                .filter(|clint| clint.contains(physical))
                .ok_or(Fault::new(7, address))?;
            clint.write(physical, value as u32);
            clint.write(physical + 4, (value >> 32) as u32);
            return Ok(());
        }
        bus.write(physical, width, value as u32, output)
            .map_err(|_| Fault::new(7, address))
    }
}

fn ram_read(bus: &Bus, address: u64, width: usize) -> Option<u64> {
    let mut bytes = [0; 8];
    bytes[..width].copy_from_slice(bus.ram64(address, width)?);
    Some(u64::from_le_bytes(bytes))
}

fn sign(value: u64, width: u32) -> u64 {
    (((value << (64 - width)) as i64) >> (64 - width)) as u64
}

#[derive(Clone, Copy)]
struct Instruction(u32);

impl Instruction {
    fn opcode(self) -> u32 {
        self.0 & 0x7f
    }
    fn funct3(self) -> u32 {
        (self.0 >> 12) & 7
    }
    fn funct7(self) -> u32 {
        self.0 >> 25
    }
    fn rd(self) -> usize {
        ((self.0 >> 7) & 31) as usize
    }
    fn rs1(self) -> usize {
        ((self.0 >> 15) & 31) as usize
    }
    fn rs2(self) -> usize {
        ((self.0 >> 20) & 31) as usize
    }
    fn immediate(self) -> u64 {
        sign(u64::from(self.0 >> 20), 12)
    }
}

fn immediate_alu(fields: Instruction, lhs: u64) -> Option<u64> {
    let immediate = fields.immediate();
    let shamt = (fields.0 >> 20) & 63;
    let shift = fields.0 >> 26;
    Some(if fields.opcode() == 0x1b {
        let lhs = lhs as u32;
        let result = match (fields.funct3(), fields.funct7()) {
            (0, _) => lhs.wrapping_add(immediate as u32),
            (1, 0) => lhs << shamt,
            (5, 0) => lhs >> shamt,
            (5, 0x20) => ((lhs as i32) >> shamt) as u32,
            _ => return None,
        };
        sign(u64::from(result), 32)
    } else {
        match fields.funct3() {
            0 => lhs.wrapping_add(immediate),
            1 if shift == 0 => lhs << shamt,
            2 => u64::from((lhs as i64) < immediate as i64),
            3 => u64::from(lhs < immediate),
            4 => lhs ^ immediate,
            5 if shift == 0 => lhs >> shamt,
            5 if shift == 0x10 => ((lhs as i64) >> shamt) as u64,
            6 => lhs | immediate,
            7 => lhs & immediate,
            _ => return None,
        }
    })
}

fn register_alu(fields: Instruction, lhs: u64, rhs: u64, multiply: bool) -> Option<u64> {
    let funct3 = fields.funct3();
    let funct7 = fields.funct7();
    let word = fields.opcode() == 0x3b;
    if funct7 == 1 && multiply {
        return multiply_divide(funct3, lhs, rhs, word);
    }
    Some(if word {
        let lhs = lhs as u32;
        let rhs = rhs as u32;
        let shamt = rhs & 31;
        let result = match (funct3, funct7) {
            (0, 0) => lhs.wrapping_add(rhs),
            (0, 0x20) => lhs.wrapping_sub(rhs),
            (1, 0) => lhs << shamt,
            (5, 0) => lhs >> shamt,
            (5, 0x20) => ((lhs as i32) >> shamt) as u32,
            _ => return None,
        };
        sign(u64::from(result), 32)
    } else {
        let shamt = rhs & 63;
        match (funct3, funct7) {
            (0, 0) => lhs.wrapping_add(rhs),
            (0, 0x20) => lhs.wrapping_sub(rhs),
            (1, 0) => lhs << shamt,
            (2, 0) => u64::from((lhs as i64) < rhs as i64),
            (3, 0) => u64::from(lhs < rhs),
            (4, 0) => lhs ^ rhs,
            (5, 0) => lhs >> shamt,
            (5, 0x20) => ((lhs as i64) >> shamt) as u64,
            (6, 0) => lhs | rhs,
            (7, 0) => lhs & rhs,
            _ => return None,
        }
    })
}

fn multiply_divide(operation: u32, lhs: u64, rhs: u64, word: bool) -> Option<u64> {
    if word {
        let lhs = lhs as u32;
        let rhs = rhs as u32;
        let result = match operation {
            0 => lhs.wrapping_mul(rhs),
            4 if rhs != 0 => (lhs as i32).wrapping_div(rhs as i32) as u32,
            5 if rhs != 0 => lhs / rhs,
            4 | 5 => u32::MAX,
            6 if rhs != 0 => (lhs as i32).wrapping_rem(rhs as i32) as u32,
            7 if rhs != 0 => lhs % rhs,
            6 | 7 => lhs,
            _ => return None,
        };
        return Some(sign(u64::from(result), 32));
    }
    Some(match operation {
        0 => lhs.wrapping_mul(rhs),
        1 => ((i128::from(lhs as i64) * i128::from(rhs as i64)) >> 64) as u64,
        2 => ((i128::from(lhs as i64) * i128::from(rhs)) >> 64) as u64,
        3 => ((u128::from(lhs) * u128::from(rhs)) >> 64) as u64,
        4 if rhs != 0 => (lhs as i64).wrapping_div(rhs as i64) as u64,
        5 if rhs != 0 => lhs / rhs,
        4 | 5 => u64::MAX,
        6 if rhs != 0 => (lhs as i64).wrapping_rem(rhs as i64) as u64,
        7 if rhs != 0 => lhs % rhs,
        6 | 7 => lhs,
        _ => unreachable!(),
    })
}
