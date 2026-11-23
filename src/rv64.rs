mod bitmanip;
mod floating;

use std::io::Write;

use crate::{
    bus::{Bus, BusError, RAM_BASE},
    machine::{Step, Stop},
    pmp::{EXECUTE, READ, WRITE},
    rv64_priv::{Mode, Privilege},
    rv64_vm::TranslationCache,
};

#[derive(Clone)]
pub struct Rv64 {
    pub pc: u64,
    registers: [u64; 32],
    floating: [u64; 32],
    isa: Isa,
    pub privilege: Privilege,
    translations: TranslationCache,
    pub allow_unaligned: bool,
    waiting: bool,
}

#[derive(Clone, Copy)]
pub struct Isa {
    pub multiply: bool,
    pub atomic: bool,
    pub compressed: bool,
    pub single: bool,
    pub double: bool,
    pub cas: bool,
    bitmanip: bitmanip::Extensions,
}

impl Isa {
    pub fn parse(value: &str) -> Result<Self, String> {
        let mut extensions = value
            .strip_prefix("rv64")
            .ok_or_else(|| format!("invalid RV64 ISA {value:?}"))?
            .split('_');
        let base = extensions.next().unwrap_or("");
        if !base.starts_with(['i', 'g'])
            || (base.contains('d') && !base.contains(['g', 'f']))
            || !base
                .chars()
                .all(|ext| matches!(ext, 'i' | 'g' | 'm' | 'a' | 'f' | 'd' | 'c' | 'b'))
        {
            return Err(format!("unsupported RV64 ISA {value:?}"));
        }
        let mut isa = Self {
            multiply: base.contains(['g', 'm']),
            atomic: base.contains(['g', 'a']),
            compressed: base.contains('c'),
            single: base.contains(['g', 'f']),
            double: base.contains(['g', 'd']),
            ..Self::IM
        };
        if base.contains('b') {
            isa.bitmanip.enable_b();
        }
        for extension in extensions {
            match extension {
                "zicsr" | "zifencei" => {}
                "zacas" if isa.atomic => isa.cas = true,
                extension if isa.bitmanip.enable(extension) => {}
                _ => return Err(format!("unsupported RV64 ISA extension {extension:?}")),
            }
        }
        Ok(isa)
    }

    pub fn elf(compressed: bool) -> Self {
        Self {
            compressed,
            atomic: true,
            single: true,
            double: true,
            ..Self::IM
        }
    }

    pub const IM: Self = Self {
        multiply: true,
        atomic: false,
        compressed: false,
        single: false,
        double: false,
        cas: false,
        bitmanip: bitmanip::Extensions::NONE,
    };
}

#[derive(Debug)]
pub(crate) struct Fault {
    cause: u64,
    value: u64,
    output: Option<std::io::Error>,
}

impl Fault {
    pub(crate) fn new(cause: u64, value: u64) -> Self {
        Self {
            cause,
            value,
            output: None,
        }
    }
}

impl Rv64 {
    pub fn new(isa: Isa) -> Self {
        Self {
            pc: u64::from(RAM_BASE),
            registers: [0; 32],
            floating: [0; 32],
            isa,
            privilege: Privilege::default(),
            translations: TranslationCache::default(),
            allow_unaligned: false,
            waiting: false,
        }
    }

    pub fn set_boot_arguments(&mut self, hart: usize, dtb: u64) {
        self.registers[10] = hart as u64;
        self.registers[11] = dtb;
    }

    pub fn isa(&self) -> Isa {
        self.isa
    }

    pub fn validate(&self, bus: &mut Bus) -> Result<(), String> {
        self.fetch(bus)
            .map(|_| ())
            .map_err(|fault| self.describe(&fault))
    }

    fn describe(&self, fault: &Fault) -> String {
        if let Some(error) = &fault.output {
            return format!("RV64 pc={:#018x}: UART: {error}", self.pc);
        }
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

    fn fetch(&self, bus: &mut Bus) -> Result<(u32, u64), Fault> {
        let pc = self.align_target(self.pc)?;
        let width = if self.isa.compressed { 2 } else { 4 };
        let physical = self
            .translations
            .translate(bus, &self.privilege, pc, width, EXECUTE)?;
        let low = ram_read(bus, physical, width).ok_or(Fault::new(1, pc))? as u32;
        if width == 4 || low & 3 != 3 {
            return Ok((low, width as u64));
        }
        let upper = pc.wrapping_add(2);
        let physical = if pc & 0xfff == 0xffe {
            self.translations
                .translate(bus, &self.privilege, upper, 2, EXECUTE)?
        } else {
            if self
                .privilege
                .pmp
                .as_ref()
                .is_some_and(|pmp| !pmp.permits(physical, 4, EXECUTE, self.privilege.mode))
            {
                return Err(Fault::new(1, pc));
            }
            physical + 2
        };
        let high = ram_read(bus, physical, 2).ok_or(Fault::new(1, upper))? as u32;
        Ok((low | high << 16, 4))
    }

    pub fn step(
        &mut self,
        bus: &mut Bus,
        hart: usize,
        elapsed: u64,
        output: &mut impl Write,
    ) -> Result<Step, String> {
        let (mut pending, time) = bus
            .clint
            .as_mut()
            .map_or((0, elapsed), |clint| (clint.tick(hart), clint.time()));
        if hart == 0 {
            pending |= bus.tick_plic();
        }
        let pending = u64::from(pending) | bus.rv64_devices.tick(hart);
        let count_retired = self.privilege.tick(pending, time);
        if let Some(target) = self.privilege.interrupt(self.pc) {
            self.waiting = false;
            self.pc = target;
            return Ok(Step::Trap);
        }

        if self.waiting {
            if !self.privilege.interrupt_enabled_and_pending() {
                return Ok(Step::Trap);
            }
            self.waiting = false;
        }

        let result = self.fetch(bus).and_then(|(instruction, length)| {
            let decoded = if length == 2 {
                crate::compressed::expand_rv64(instruction)
                    .ok_or_else(|| Fault::new(2, u64::from(instruction)))?
            } else {
                instruction
            };
            self.execute(decoded, length, bus, hart, output)
                .map_err(|mut fault| {
                    if fault.cause == 2 {
                        fault.value = u64::from(instruction);
                    }
                    fault
                })
        });
        match result {
            Ok(Step::Retired) if count_retired => {
                self.privilege.retire();
                Ok(Step::Retired)
            }
            Ok(step) => Ok(step),
            Err(fault) => {
                if fault.output.is_some() {
                    return Err(self.describe(&fault));
                }
                if let Some(target) = self.privilege.trap(self.pc, fault.cause, fault.value) {
                    self.pc = target;
                    return Ok(Step::Trap);
                }
                match fault.cause {
                    3 => Ok(Step::Stop(Stop::Ebreak)),
                    8 | 9 | 11 => Ok(Step::Stop(Stop::Ecall)),
                    _ => Err(self.describe(&fault)),
                }
            }
        }
    }

    fn misa(&self) -> u64 {
        let base = (2 << 62) | (1 << 8) | (1 << 18) | (1 << 20);
        base | (u64::from(self.isa.multiply) << 12)
            | u64::from(self.isa.atomic)
            | (u64::from(self.isa.compressed) << 2)
            | (u64::from(self.isa.double) << 3)
            | (u64::from(self.isa.single) << 5)
            | (u64::from(self.isa.bitmanip.zba && self.isa.bitmanip.zbb && self.isa.bitmanip.zbs)
                << 1)
    }

    fn csr(&mut self, instruction: Instruction, operand: u64, hart: usize) -> Option<u64> {
        let index = instruction.0 >> 20;
        let mode = instruction.funct3();
        let source = instruction.rs1();
        let operand = if mode & 4 != 0 {
            source as u64
        } else {
            operand
        };
        let misa = self.misa();
        let alignment_mask = if self.isa.compressed { !1 } else { !3 };
        let old = if index == 0xf14 && self.privilege.mode == Mode::Machine {
            hart as u64
        } else {
            self.privilege.read(index, misa, alignment_mask)?
        };
        let value = match mode & 3 {
            1 => Some(operand),
            2 if source != 0 => Some(self.privilege.csr_rmw_value(index, old) | operand),
            3 if source != 0 => Some(self.privilege.csr_rmw_value(index, old) & !operand),
            2 | 3 => None,
            _ => return None,
        };
        if let Some(value) = value {
            if !self.privilege.write(index, value, misa) {
                return None;
            }
            if matches!(index, 0x3a0..=0x3ef) {
                self.translations.clear();
            }
        }
        Some(old)
    }

    fn execute(
        &mut self,
        instruction: u32,
        length: u64,
        bus: &mut Bus,
        hart: usize,
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
            0x13 | 0x1b => Some(
                immediate_alu(fields, lhs)
                    .or_else(|| bitmanip::immediate(self.isa, fields, lhs))
                    .ok_or(illegal)?,
            ),
            0x33 | 0x3b => Some(
                register_alu(fields, lhs, rhs, self.isa.multiply)
                    .or_else(|| bitmanip::register(self.isa, fields, lhs, rhs))
                    .ok_or(illegal)?,
            ),
            0x0f if matches!(fields.funct3(), 0 | 1) => None,
            0x07 | 0x27 | 0x43 | 0x47 | 0x4b | 0x4f | 0x53 => {
                self.floating_instruction(fields, bus, output)?
            }
            0x2f if self.isa.cas && instruction >> 27 == 5 => {
                self.compare_swap(fields, bus, lhs, rhs)?
            }
            0x2f if self.isa.atomic => Some(self.atomic(fields, bus, hart, lhs, rhs)?),
            0x73 if fields.funct3() != 0 => Some(self.csr(fields, lhs, hart).ok_or(illegal)?),
            0x73 => {
                let epc_mask = if self.isa.compressed { !1 } else { !3 };
                match instruction {
                    0x0000_0073 => return Err(Fault::new(8 + self.privilege.mode as u64, 0)),
                    0x0010_0073 => return Err(Fault::new(3, self.pc)),
                    0x3020_0073 => next = self.privilege.mret(epc_mask).ok_or(illegal)?,
                    0x1020_0073 => next = self.privilege.sret(epc_mask).ok_or(illegal)?,
                    0x1050_0073 if self.privilege.allows_wfi() => self.waiting = true,
                    _ if instruction & 0xfe00_7fff == 0x1200_0073
                        && self.privilege.allows_sfence() =>
                    {
                        self.translations.clear();
                    }
                    _ => return Err(illegal),
                }
                None
            }
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
            if !self.allow_unaligned {
                return Err(Fault::new(4, address));
            }
            let mut value = 0;
            for offset in 0..width {
                let virtual_address = address.wrapping_add(offset as u64);
                let physical =
                    self.translations
                        .translate(bus, &self.privilege, virtual_address, 1, READ)?;
                let byte = ram_read(bus, physical, 1).ok_or(Fault::new(5, virtual_address))?;
                value |= byte << (offset * 8);
            }
            return Ok(value);
        }
        let physical = self
            .translations
            .translate(bus, &self.privilege, address, width, READ)?;
        if let Some(value) = ram_read(bus, physical, width) {
            return Ok(value);
        }
        if let Ok(low) = u32::try_from(physical) {
            if let Some(value) = bus.rv64_devices.read(low, width) {
                return Ok(u64::from(value));
            }
            let result = match width {
                1 => bus.read::<1>(low).map(u64::from),
                2 => bus.read::<2>(low).map(u64::from),
                4 => bus.read::<4>(low).map(u64::from),
                8 => bus
                    .clint
                    .as_ref()
                    .filter(|clint| clint.contains(low))
                    .map(|clint| {
                        u64::from(clint.read(low)) | (u64::from(clint.read(low + 4)) << 32)
                    })
                    .ok_or(BusError::Unmapped(low, 8)),
                _ => unreachable!(),
            };
            match result {
                Ok(value) => return Ok(value),
                Err(BusError::Unmapped(..)) => {}
                Err(_) => return Err(Fault::new(5, address)),
            }
        }
        bus.rv64_devices
            .fallback(physical, width)
            .ok_or(Fault::new(5, address))
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
            if !self.allow_unaligned {
                return Err(Fault::new(6, address));
            }
            let mut destinations = [0; 8];
            for (offset, slot) in destinations[..width].iter_mut().enumerate() {
                let virtual_address = address.wrapping_add(offset as u64);
                *slot =
                    self.translations
                        .translate(bus, &self.privilege, virtual_address, 1, WRITE)?;
                if bus.ram64(*slot, 1).is_none() {
                    return Err(Fault::new(7, virtual_address));
                }
            }
            for (physical, byte) in destinations[..width].iter().zip(value.to_le_bytes()) {
                bus.memory64(*physical, 1).unwrap()[0] = byte;
            }
            return Ok(());
        }
        let physical = self
            .translations
            .translate(bus, &self.privilege, address, width, WRITE)?;
        if let Some(bytes) = bus.memory64(physical, width) {
            bytes.copy_from_slice(&value.to_le_bytes()[..width]);
            return Ok(());
        }
        if let Ok(low) = u32::try_from(physical) {
            if bus.write_rv64_ethernet(low, width, value as u32) {
                return Ok(());
            }
            let result = if width == 8 {
                if let Some(clint) = bus.clint.as_mut().filter(|clint| clint.contains(low)) {
                    clint.write(low, value as u32);
                    clint.write(low + 4, (value >> 32) as u32);
                    return Ok(());
                }
                Err(BusError::Unmapped(low, width))
            } else {
                bus.rv64_devices
                    .write(low, width, value as u32, output)
                    .map_or_else(
                        || bus.write(low, width, value as u32, output),
                        |result| result.map_err(BusError::Output),
                    )
            };
            match result {
                Ok(()) => return Ok(()),
                Err(BusError::Unmapped(..)) => {}
                Err(BusError::Output(error)) => {
                    return Err(Fault {
                        cause: 7,
                        value: address,
                        output: Some(error),
                    });
                }
                Err(_) => return Err(Fault::new(7, address)),
            }
        }
        bus.rv64_devices
            .fallback(physical, width)
            .map(|_| ())
            .ok_or(Fault::new(7, address))
    }

    fn compare_swap(
        &mut self,
        inst: Instruction,
        bus: &mut Bus,
        address: u64,
        replacement: u64,
    ) -> Result<Option<u64>, Fault> {
        let destination = inst.rd();
        let source = inst.rs2();
        let width = match inst.funct3() {
            2 => 4,
            3 => 8,
            4 if (destination | source) & 1 == 0 => 16,
            _ => return Err(Fault::new(2, u64::from(inst.0))),
        };
        if address & (width as u64 - 1) != 0 {
            return Err(Fault::new(6, address));
        }
        let physical =
            self.translations
                .translate(bus, &self.privilege, address, width, READ | WRITE)?;
        let fault = || Fault::new(7, address);
        let mut value = [0; 16];
        value[..width].copy_from_slice(bus.ram64(physical, width).ok_or_else(fault)?);
        let old = u128::from_le_bytes(value);
        let (expected, replacement) = if width == 16 {
            let pair_high = |index| {
                if index == 0 {
                    0
                } else {
                    u128::from(self.registers[index + 1]) << 64
                }
            };
            (
                u128::from(self.registers[destination]) | pair_high(destination),
                u128::from(replacement) | pair_high(source),
            )
        } else {
            let mask = (1u128 << (width * 8)) - 1;
            (
                u128::from(self.registers[destination]) & mask,
                u128::from(replacement) & mask,
            )
        };
        if old == expected {
            bus.memory64(physical, width)
                .ok_or_else(fault)?
                .copy_from_slice(&replacement.to_le_bytes()[..width]);
        }
        if width == 16 && destination != 0 {
            self.registers[destination + 1] = (old >> 64) as u64;
        }
        Ok(Some(sign(old as u64, width.min(8) as u32 * 8)))
    }

    fn atomic(
        &self,
        inst: Instruction,
        bus: &mut Bus,
        hart: usize,
        address: u64,
        rhs: u64,
    ) -> Result<u64, Fault> {
        let illegal = || Fault::new(2, u64::from(inst.0));
        let width = match inst.funct3() {
            2 => 4,
            3 => 8,
            _ => return Err(illegal()),
        };
        let operation = inst.0 >> 27;
        let permission = match operation {
            2 if inst.rs2() == 0 => READ,
            3 => WRITE,
            0 | 1 | 4 | 8 | 12 | 16 | 20 | 24 | 28 => READ | WRITE,
            _ => return Err(illegal()),
        };
        let load = permission == READ;
        let reservation = if operation == 3 {
            bus.take_reservation64(hart)
        } else {
            None
        };
        if address & (width as u64 - 1) != 0 {
            return Err(Fault::new(if load { 4 } else { 6 }, address));
        }
        let physical =
            self.translations
                .translate(bus, &self.privilege, address, width, permission)?;
        let fault = || Fault::new(if load { 5 } else { 7 }, address);
        let previous = ram_read(bus, physical, width).ok_or_else(fault)?;
        if operation == 2 {
            bus.reserve64(hart, physical, width);
        } else if operation == 3 {
            if reservation != bus.reservation_key64(physical, width) {
                return Ok(1);
            }
            bus.memory64(physical, width)
                .ok_or_else(fault)?
                .copy_from_slice(&rhs.to_le_bytes()[..width]);
            return Ok(0);
        } else {
            let old = sign(previous, width as u32 * 8);
            let rhs = sign(rhs, width as u32 * 8);
            let value = match operation {
                0 => old.wrapping_add(rhs),
                1 => rhs,
                4 => old ^ rhs,
                8 => old | rhs,
                12 => old & rhs,
                16 => (old as i64).min(rhs as i64) as u64,
                20 => (old as i64).max(rhs as i64) as u64,
                24 => old.min(rhs),
                28 => old.max(rhs),
                _ => unreachable!(),
            };
            bus.memory64(physical, width)
                .ok_or_else(fault)?
                .copy_from_slice(&value.to_le_bytes()[..width]);
        }
        Ok(sign(previous, width as u32 * 8))
    }
}

pub(crate) fn ram_read(bus: &Bus, address: u64, width: usize) -> Option<u64> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mip_rmw_does_not_latch_external_seip() {
        let mut cpu = Rv64::new(Isa::IM);
        let seip = 1 << 9;
        let stip = 1 << 5;
        let instruction =
            |operation: u32| Instruction((0x344 << 20) | (1 << 15) | (operation << 12));

        cpu.privilege.tick(seip, 0);
        assert_eq!(cpu.csr(instruction(2), stip, 0), Some(seip));
        assert_eq!(cpu.csr(instruction(3), stip, 0), Some(seip | stip));
        cpu.privilege.tick(0, 0);
        assert_eq!(cpu.privilege.read(0x344, cpu.misa(), !3), Some(0));

        // A guest can still set or clear the software contribution explicitly.
        cpu.privilege.tick(seip, 0);
        assert_eq!(cpu.csr(instruction(1), seip, 0), Some(seip));
        cpu.privilege.tick(0, 0);
        assert_eq!(cpu.csr(instruction(3), seip, 0), Some(seip));
        assert_eq!(cpu.privilege.read(0x344, cpu.misa(), !3), Some(0));
    }
}
