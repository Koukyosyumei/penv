mod bitmanip;
mod floating;
#[cfg(test)]
mod upstream_tests;
pub(crate) mod vector;

use std::io::Write;

use crate::{
    bus::{Bus, BusError, RAM_BASE},
    machine::{Step, Stop},
    pmp::{EXECUTE, READ, WRITE},
    rv64_priv::{Mode, Privilege},
    rv64_vm::TranslationCache,
};

#[derive(Clone)]
pub(crate) struct AtomicTrace {
    pub(crate) instruction: u32,
    pub(crate) width: usize,
    pub(crate) before: [u64; 4],
    pub(crate) after: [u64; 4],
}

#[derive(Clone)]
pub struct Rv64 {
    pub(crate) hart_id: usize,
    pub(crate) mhartid_override: Option<u64>,
    pub(crate) trace_custom_csr: bool,
    pub(crate) trace_amo: bool,
    pub(crate) atomic_trace: Option<AtomicTrace>,
    pub(crate) stack_access_trace_name: Option<String>,
    pub pc: u64,
    reset_vector: u64,
    reset_vector_configured: bool,
    pc_follows_reset_vector: bool,
    registers: [u64; 32],
    floating: [u64; 32],
    isa: Isa,
    pub privilege: Privilege,
    translations: TranslationCache,
    pub allow_unaligned: bool,
    waiting: bool,
    wfi_as_nop: bool,
}

#[derive(Clone, Copy)]
pub struct Isa {
    pub multiply: bool,
    pub atomic: bool,
    pub compressed: bool,
    pub single: bool,
    pub double: bool,
    half: bool,
    vector: bool,
    zcb: bool,
    zicboz: bool,
    xandesperf: bool,
    xandesbfhcvt: bool,
    xandesvbfhcvt: bool,
    xandesvqmac: bool,
    pub cas: bool,
    pub smepmp: bool,
    bitmanip: bitmanip::Extensions,
}

impl Isa {
    pub fn parse(value: &str) -> Result<Self, String> {
        let normalized = value.to_ascii_lowercase();
        let mut extensions = normalized
            .strip_prefix("rv64")
            .ok_or_else(|| format!("invalid RV64 ISA {value:?}"))?
            .split('_');
        let base = extensions.next().unwrap_or("");
        if !base.starts_with(['i', 'g'])
            || (base.contains('d') && !base.contains(['g', 'f']))
            || !base
                .chars()
                .all(|ext| matches!(ext, 'i' | 'g' | 'm' | 'a' | 'f' | 'd' | 'c' | 'b' | 'v'))
        {
            return Err(format!("unsupported RV64 ISA {value:?}"));
        }
        let mut isa = Self {
            multiply: base.contains(['g', 'm']),
            atomic: base.contains(['g', 'a']),
            compressed: base.contains('c'),
            vector: base.contains('v'),
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
                "c" => isa.compressed = true,
                "zcb" => isa.zcb = true,
                "v" => isa.vector = true,
                "zacas" if isa.atomic => isa.cas = true,
                "smepmp" => isa.smepmp = true,
                "zfh" if isa.single => isa.half = true,
                "zicboz" => isa.zicboz = true,
                "xandesperf" => isa.xandesperf = true,
                "xandesbfhcvt" => {
                    isa.xandesbfhcvt = true;
                    isa.single = true;
                }
                "xandesvbfhcvt" => {
                    isa.xandesvbfhcvt = true;
                    isa.vector = true;
                }
                "xandesvqmac" => {
                    isa.xandesvqmac = true;
                    isa.vector = true;
                }
                extension if isa.bitmanip.enable(extension) => {}
                _ => return Err(format!("unsupported RV64 ISA extension {extension:?}")),
            }
        }
        if isa.zcb && !isa.compressed {
            return Err("RV64 Zcb requires the compressed base instructions (C)".into());
        }
        Ok(isa)
    }

    pub(crate) fn llvm_features(self) -> String {
        [
            (self.multiply, "+m"),
            (self.atomic, "+a"),
            (self.compressed, "+c"),
            (self.single, "+f"),
            (self.double, "+d"),
            (self.half, "+zfh"),
            (self.vector, "+v"),
            (self.bitmanip.zba, "+zba"),
            (self.bitmanip.zbb, "+zbb"),
            (self.bitmanip.zbc, "+zbc"),
            (self.bitmanip.zbs, "+zbs"),
            (self.bitmanip.zicond, "+zicond"),
            (self.bitmanip.zbkb, "+zbkb"),
            (self.bitmanip.zbkx, "+zbkx"),
        ]
        .into_iter()
        .filter_map(|(enabled, feature)| enabled.then_some(feature))
        .collect::<Vec<_>>()
        .join(",")
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
        half: false,
        vector: false,
        zcb: false,
        zicboz: false,
        xandesperf: false,
        xandesbfhcvt: false,
        xandesvbfhcvt: false,
        xandesvqmac: false,
        cas: false,
        smepmp: false,
        bitmanip: bitmanip::Extensions::NONE,
    };
}

#[derive(Debug)]
pub(crate) struct Fault {
    cause: u64,
    value: u64,
    output: Option<std::io::Error>,
    illegal_physical: bool,
}

impl Fault {
    pub(crate) fn new(cause: u64, value: u64) -> Self {
        Self {
            cause,
            value,
            output: None,
            illegal_physical: false,
        }
    }

    pub(crate) fn illegal_physical(cause: u64, address: u64) -> Self {
        Self {
            illegal_physical: true,
            ..Self::new(cause, address)
        }
    }

    fn physical_diagnostic(&self) -> String {
        let action = if self.cause == 1 {
            "execute code"
        } else {
            "access memory"
        };
        format!(
            "Trying to {action} from an illegal address outside of the physical memory address space: {:#x}",
            self.value
        )
    }
}

impl Rv64 {
    pub fn new(isa: Isa) -> Self {
        let mut privilege = Privilege::default();
        privilege.zicboz = isa.zicboz;
        privilege.andes_bfloat16 = isa.xandesbfhcvt || isa.xandesvbfhcvt;
        privilege.andes_quad_mac = isa.xandesvqmac;
        privilege.vector = isa.vector.then(Box::<vector::VectorState>::default);
        Self {
            hart_id: 0,
            mhartid_override: None,
            trace_custom_csr: false,
            trace_amo: false,
            atomic_trace: None,
            stack_access_trace_name: None,
            pc: u64::from(RAM_BASE),
            reset_vector: u64::from(RAM_BASE),
            reset_vector_configured: false,
            pc_follows_reset_vector: true,
            registers: [0; 32],
            floating: [0; 32],
            isa,
            privilege,
            translations: TranslationCache::default(),
            allow_unaligned: false,
            waiting: false,
            wfi_as_nop: false,
        }
    }

    pub(crate) fn set_reset_vector(&mut self, address: u64) {
        self.reset_vector_configured = true;
        self.reset_vector = address;
        if self.pc_follows_reset_vector {
            self.pc = address;
        }
    }

    pub(crate) fn monitor_sepc(&mut self, value: Option<u64>) -> Result<Option<u64>, String> {
        let alignment = if self.isa.compressed { !1 } else { !3 };
        self.privilege.monitor_sepc(value, alignment)
    }

    pub(crate) fn initialize_monitor_reset_pc(&mut self) {
        self.privilege.initialize_renode_reset_status(self.isa.single || self.isa.double);
        // Renode's vector configuration and memory helpers check the ISA
        // extensions directly. Its vector CSR handlers still check mstatus.VS.
        self.privilege.renode_vector_memory = true;
        // A platform can override the Renode RISC-V default with resetVector.
        if !self.reset_vector_configured {
            self.reset_vector = 0x1000;
        }
        self.pc_follows_reset_vector = true;
        self.pc = self.reset_vector;
    }

    pub(crate) fn set_program_counter(&mut self, address: u64) {
        self.pc = address;
        self.pc_follows_reset_vector = false;
    }

    pub(crate) fn reset_from(&mut self, initial: &Self) {
        let reset_vector = self.reset_vector;
        *self = initial.clone();
        self.pc_follows_reset_vector = true;
        self.set_reset_vector(reset_vector);
    }

    pub fn set_boot_arguments(&mut self, hart: usize, dtb: u64) {
        self.registers[10] = self.mhartid_override.unwrap_or(hart as u64);
        self.registers[11] = dtb;
    }

    pub(crate) fn monitor_external_pmp(&mut self, path: &str, config: bool, index: u32, value: Option<u64>) -> Option<Option<u64>> {
        let pmp = self.privilege.pmp.as_mut().filter(|pmp| pmp.external_path.as_deref() == Some(path))?;
        let result = pmp.external_csr(config, index, value);
        if value.is_some() {
            self.clear_translation_cache();
        }
        Some(result)
    }

    pub(crate) fn clear_translation_cache(&mut self) {
        self.translations.clear();
    }

    pub fn isa(&self) -> Isa {
        self.isa
    }

    pub(crate) fn monitor_register(&self, name: &str) -> Option<u64> {
        match name {
            "pc" => Some(self.pc),
            "resetvector" => Some(self.reset_vector),
            "fflags" if self.isa.single => Some(u64::from(self.privilege.fcsr & 31)),
            name if self.isa.single && name.starts_with('f') => {
                let index = name[1..].parse::<usize>().ok()?;
                self.floating.get(index).copied()
            }
            name => self.privilege.monitor_register(name).or_else(|| {
                let index = name.strip_prefix('x')?.parse::<usize>().ok()?;
                self.registers.get(index).copied()
            }),
        }
    }

    pub(crate) fn initialize_gpr(&mut self, index: usize, value: u64) {
        assert!((1..32).contains(&index));
        self.registers[index] = value;
    }

    pub(crate) fn initialize_freg(&mut self, index: usize, value: u64) -> Result<(), String> {
        if !self.isa.single {
            return Err("floating register setup requires the RV64 F extension".into());
        }
        *self
            .floating
            .get_mut(index)
            .ok_or("floating register must be f0 through f31")? = value;
        Ok(())
    }

    pub(crate) fn initialize_mstatus(&mut self, value: u64) {
        self.privilege.write(0x300, value, self.misa());
    }

    pub(crate) fn translate_read(&self, bus: &mut Bus, address: u64) -> Result<u64, String> {
        self.translations
            .translate(bus, &self.privilege, address, 1, READ)
            .map_err(|fault| {
                format!(
                    "Failed to translate address {address:#x}: {}",
                    self.describe(&fault)
                )
            })
    }

    pub fn validate(&self, bus: &mut Bus) -> Result<(), String> {
        self.fetch(bus)
            .map(|_| ())
            .map_err(|fault| self.describe(&fault))
    }

    fn describe(&self, fault: &Fault) -> String {
        if fault.illegal_physical {
            return fault.physical_diagnostic();
        }
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
        let low =
            ram_read(bus, Some(self.hart_id), physical, width).ok_or(Fault::new(1, pc))? as u32;
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
        let high =
            ram_read(bus, Some(self.hart_id), physical, 2).ok_or(Fault::new(1, upper))? as u32;
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
        bus.rv64_devices.advance_watchdogs(elapsed);
        let pending = u64::from(pending) | bus.rv64_devices.tick(hart);
        let mut count_retired = self.privilege.tick(pending, time);
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
                crate::compressed::expand_rv64(instruction, self.isa.zcb)
                    .ok_or_else(|| Fault::new(2, u64::from(instruction)))?
            } else {
                instruction
            };
            let fields = Instruction(decoded);
            if fields.opcode() == 0x73 && decoded >> 20 == 0xb02 {
                // An explicit minstret write overrides this instruction's increment.
                let operation = fields.funct3() & 3;
                let writes = operation == 1 || (operation != 0 && fields.rs1() != 0);
                count_retired &= !writes;
            }
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
                if fault.output.is_some() || (fault.illegal_physical && fault.cause == 1) {
                    return Err(self.describe(&fault));
                }
                if let Some(target) = self.privilege.trap(self.pc, fault.cause, fault.value) {
                    if fault.illegal_physical {
                        eprintln!("penv: {}", fault.physical_diagnostic());
                    }
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
        let base = (2 << 62) | (1 << 8) | self.privilege.levels.isa_bits();
        base | (u64::from(self.isa.multiply) << 12)
            | u64::from(self.isa.atomic)
            | (u64::from(self.isa.compressed) << 2)
            | (u64::from(self.isa.double) << 3)
            | (u64::from(self.isa.single) << 5)
            | (u64::from(self.isa.vector) << 21)
            | (u64::from(self.isa.bitmanip.zba && self.isa.bitmanip.zbb && self.isa.bitmanip.zbs)
                << 1)
    }

    fn csr(&mut self, bus: &mut Bus, instruction: Instruction, operand: u64, hart: usize) -> Option<u64> {
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
        let operation = mode & 3;
        if operation == 0 {
            return None;
        }
        let key = (self.hart_id, index);
        if let Some(mut hook) = bus.csr_hooks.remove(&key) {
            let result = (|| {
                let writes = operation == 1 || source != 0;
                if (index >> 8) & 3 > self.privilege.mode as u32
                    || writes && index >> 10 == 3
                {
                    return None;
                }
                let old = if operation == 1 && instruction.rd() == 0 {
                    0
                } else {
                    hook.execute_csr(bus, index, None).ok()?
                };
                let new = match operation {
                    1 => Some(operand),
                    2 if source != 0 => Some(old | operand),
                    3 if source != 0 => Some(old & !operand),
                    _ => None,
                };
                if let Some(new) = new {
                    hook.execute_csr(bus, index, Some(new)).ok()?;
                }
                Some(old)
            })();
            bus.csr_hooks.insert(key, hook);
            return result;
        }
        let trace = self.trace_custom_csr && index == 0x7c0;
        let old = if operation == 1 && instruction.rd() == 0 {
            0
        } else if index == 0xf14 && self.privilege.mode == Mode::Machine {
            self.mhartid_override.unwrap_or(hart as u64)
        } else {
            let value = self.privilege.read(index, misa, alignment_mask)?;
            if trace {
                eprintln!("penv: CSR read! {index:#05x}");
            }
            value
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
            if trace {
                eprintln!("penv: CSR written! {index:#05x}");
            }
            if matches!(index, 0x3a0..=0x3ef | 0x747) {
                self.translations.clear();
            }
        }
        Some(old)
    }

    pub(crate) fn set_wfi_as_nop(&mut self, enabled: bool) {
        self.wfi_as_nop = enabled;
        if enabled {
            self.waiting = false;
        }
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
                self.trace_stack_access(bus, fields, address, width, false);
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
                self.trace_stack_access(bus, fields, address, 1 << kind, true);
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
            opcode @ (0x0b | 0x2b) if self.isa.xandesperf => {
                let function = fields.funct3();
                let (width, signed, add) = match (opcode, function) {
                    (0x0b, 0 | 4) => (1, true, false),
                    (0x0b, 2 | 6) => (1, false, false),
                    (0x0b, 1 | 5) => (1, false, true),
                    (0x2b, 1) => (2, true, false),
                    (0x2b, 5) => (2, false, false),
                    (0x2b, 2) => (4, true, false),
                    _ => return Err(illegal),
                };
                let address = self.registers[3].wrapping_add(andes_gp_offset(instruction, width));
                if add {
                    Some(address)
                } else {
                    let value = self.load(bus, address, width)?;
                    Some(if signed {
                        sign(value, width as u32 * 8)
                    } else {
                        value
                    })
                }
            }
            0x5b if self.isa.xandesvbfhcvt && instruction & 0xfe0f_707f == 0x0000_405b => {
                self.andes_vector_bfloat16(fields, instruction & (1 << 15) != 0)?;
                None
            }
            0x5b if self.isa.xandesbfhcvt && instruction & 0xfe0f_707f == 0x0001_405b => {
                self.andes_scalar_bfloat16(fields, instruction & (1 << 15) != 0)?
            }
            0x5b if self.isa.xandesperf && matches!(fields.funct3(), 2 | 3) => {
                Some(andes_bit_field(instruction, lhs, fields.funct3() == 3))
            }
            0x5b if self.isa.xandesperf && fields.funct3() == 7 => {
                let bit = fields.rs2() | ((instruction as usize >> 7) & 1) << 5;
                let test_set = instruction & (1 << 30) != 0;
                if (lhs & (1u64 << bit) != 0) == test_set {
                    let offset = ((instruction >> 31) << 10)
                        | ((instruction >> 20) & 0x3e0)
                        | ((instruction >> 7) & 0x1e);
                    next = self.align_target(self.pc.wrapping_add(sign(u64::from(offset), 11)))?;
                }
                None
            }
            0x5b if self.isa.xandesperf && fields.funct3() == 0 => Some(match instruction >> 25 {
                op @ 5..=7 => lhs.wrapping_add(rhs << (op - 4)),
                op @ 8..=11 => lhs.wrapping_add(u64::from(rhs as u32) << (op - 8)),
                op @ 16..=19 => andes_find_byte(op, lhs, rhs),
                _ => return Err(illegal),
            }),
            0x0f if instruction & 0xfff0_7fff == 0x0040_200f
                && self.privilege.allows_cbo_zero() =>
            {
                const BLOCK_SIZE: usize = 64;
                let base = lhs & !(BLOCK_SIZE as u64 - 1);
                let address = self
                    .translations
                    .translate(bus, &self.privilege, base, BLOCK_SIZE, WRITE)
                    .map_err(|fault| Fault::new(fault.cause, lhs))?;
                bus.cpu_memory64(self.hart_id, address, BLOCK_SIZE)
                    .ok_or(Fault::new(7, lhs))?
                    .fill(0);
                None
            }
            0x0f if matches!(fields.funct3(), 0 | 1) => None,
            0x07 | 0x27 if self.isa.vector && matches!(fields.funct3(), 0 | 5..=7) => {
                self.vector_memory(fields, lhs, rhs, bus, output)?;
                None
            }
            0x07 | 0x27 | 0x43 | 0x47 | 0x4b | 0x4f | 0x53 => {
                self.floating_instruction(fields, bus, output)?
            }
            0x57 if fields.funct3() == 7 => {
                let avl = if fields.rs1() != 0 {
                    Some(lhs)
                } else if fields.rd() != 0 {
                    Some(u64::MAX)
                } else {
                    None
                };
                let (kind, avl) = match instruction >> 30 {
                    0..=1 => (u64::from(instruction >> 20 & 0x7ff), avl),
                    3 => (
                        u64::from(instruction >> 20 & 0x3ff),
                        Some(fields.rs1() as u64),
                    ),
                    2 if instruction >> 25 == 0x40 => (rhs, avl),
                    _ => return Err(illegal),
                };
                Some(self.privilege.configure_vector(kind, avl).ok_or(illegal)?)
            }
            0x57 if self.isa.xandesvqmac && instruction >> 26 >= 60 => {
                self.andes_quad_mac(fields, lhs)?;
                None
            }
            0x57 => {
                self.privilege
                    .execute_vector(instruction, lhs)
                    .ok_or(illegal)?;
                None
            }
            0x2f if self.isa.cas && instruction >> 27 == 5 => {
                self.compare_swap(fields, bus, lhs, rhs)?
            }
            0x2f if self.isa.atomic => Some(self.atomic(fields, bus, hart, lhs, rhs)?),
            0x73 if fields.funct3() != 0 => Some(self.csr(bus, fields, lhs, hart).ok_or(illegal)?),
            0x73 => {
                let epc_mask = if self.isa.compressed { !1 } else { !3 };
                match instruction {
                    0x0000_0073 => return Err(Fault::new(8 + self.privilege.mode as u64, 0)),
                    0x0010_0073 => return Err(Fault::new(3, self.pc)),
                    0x3020_0073 => next = self.privilege.mret(epc_mask).ok_or(illegal)?,
                    0x1020_0073 => next = self.privilege.sret(epc_mask).ok_or(illegal)?,
                    0x1050_0073 if self.privilege.allows_wfi() => self.waiting = !self.wfi_as_nop,
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

    fn trace_stack_access(&self, bus: &mut Bus, inst: Instruction, address: u64, width: usize, write: bool) {
        if inst.rs1() == 2
            && let Some(mut hook) = bus.stack_access_hooks.remove(&self.hart_id)
        {
            if let Err(error) = hook.execute_stack(bus, self.pc, address, width * 8, write) {
                crate::monitor_log::record(3, &format!("Stack access hook: {error}"));
            }
            bus.stack_access_hooks.insert(self.hart_id, hook);
        }
        if inst.rs1() == 2
            && let Some(name) = &self.stack_access_trace_name
        {
            let direction = if write { "True" } else { "False" };
            let message = format!("{name}: StackAccess: 0x{address:X}, {}, {direction}", width * 8);
            crate::monitor_log::record(1, &message);
            eprintln!("penv: {message}");
        }
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
                let byte = bus
                    .read_rv64_tohost(physical, 1)
                    .or_else(|| ram_read(bus, Some(self.hart_id), physical, 1))
                    .ok_or(Fault::new(5, virtual_address))?;
                value |= byte << (offset * 8);
            }
            return Ok(value);
        }
        let physical = self
            .translations
            .translate(bus, &self.privilege, address, width, READ)?;
        if let Some(value) = bus
            .read_rv64_tohost(physical, width)
            .or_else(|| ram_read(bus, Some(self.hart_id), physical, width))
        {
            return Ok(value);
        }
        if let Ok(low) = u32::try_from(physical) {
            if let Some(value) = bus
                .rv64_devices
                .mpfs_cache_way_enable
                .as_ref()
                .and_then(|device| device.read(low, width))
            {
                return Ok(value);
            }
            if let Some(value) = bus.rv64_devices.read_for_hart(self.hart_id, low, width) {
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
            .or_else(|| self.unmapped_data(bus, physical, width, None))
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
                if bus.cpu_ram64(self.hart_id, *slot, 1).is_none()
                    && bus.read_rv64_tohost(*slot, 1).is_none()
                {
                    return Err(Fault::new(7, virtual_address));
                }
            }
            for (physical, byte) in destinations[..width].iter().zip(value.to_le_bytes()) {
                if !bus.write_rv64_tohost(*physical, 1, u64::from(byte)) {
                    bus.cpu_memory64(self.hart_id, *physical, 1).unwrap()[0] = byte;
                }
            }
            bus.observe_rv64_store(destinations[0], value as u8);
            return Ok(());
        }
        let physical = self
            .translations
            .translate(bus, &self.privilege, address, width, WRITE)?;
        if bus.write_rv64_tohost(physical, width, value) {
            return Ok(());
        }
        if let Some(bytes) = bus.cpu_memory64(self.hart_id, physical, width) {
            bytes.copy_from_slice(&value.to_le_bytes()[..width]);
            bus.observe_rv64_store(physical, value as u8);
            return Ok(());
        }
        if let Ok(low) = u32::try_from(physical) {
            if bus
                .rv64_devices
                .mpfs_cache_way_enable
                .as_mut()
                .and_then(|device| device.write(low, width, value))
                .is_some()
            {
                return Ok(());
            }
            if bus.write_rv64_ethernet(low, width, value as u32)
                || bus.write_rv64_sd(low, width, value as u32)
            {
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
                bus.write_rv64_peripheral(self.hart_id, low, width, value as u32, output)
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
                        illegal_physical: false,
                    });
                }
                Err(_) => return Err(Fault::new(7, address)),
            }
        }
        bus.rv64_devices
            .fallback(physical, width)
            .or_else(|| self.unmapped_data(bus, physical, width, Some(value)))
            .map(|_| ())
            .ok_or(Fault::new(7, address))
    }

    fn unmapped_data(
        &self,
        bus: &Bus,
        physical: u64,
        width: usize,
        store: Option<u64>,
    ) -> Option<u64> {
        if !bus.renode_unmapped_data {
            return None;
        }
        let end = physical.checked_add(width as u64)?;
        if bus
            .mapped_ram_ranges64()
            .any(|range| physical < range.end && range.start < end)
        {
            return None;
        }
        if let Some(value) = store {
            let value = value & (u64::MAX >> (64 - width * 8));
            eprintln!(
                "penv: RV64 pc={:#018x}: ignored unmapped {width}-byte write at {physical:#018x}, value {value:#x}",
                self.pc
            );
        } else {
            eprintln!(
                "penv: RV64 pc={:#018x}: unmapped {width}-byte read at {physical:#018x} returned zero",
                self.pc
            );
        }
        Some(0)
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
        let unaligned = address & (width as u64 - 1) != 0;
        if unaligned && !self.allow_unaligned {
            return Err(Fault::new(6, address));
        }
        let fault = || Fault::new(7, address);
        let mut value = [0; 16];
        let mut locations = [0; 16];
        let physical = if unaligned {
            self.read_unaligned_atomic(bus, address, &mut locations[..width], &mut value[..width])?;
            None
        } else {
            let physical =
                self.translations
                    .translate(bus, &self.privilege, address, width, READ | WRITE)?;
            value[..width].copy_from_slice(
                bus.cpu_ram64(self.hart_id, physical, width)
                    .ok_or_else(fault)?,
            );
            Some(physical)
        };
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
            let replacement = replacement.to_le_bytes();
            if let Some(physical) = physical {
                bus.cpu_memory64(self.hart_id, physical, width)
                    .ok_or_else(fault)?
                    .copy_from_slice(&replacement[..width]);
            } else {
                // All bytes were translated and checked before committing any writes.
                for (&location, byte) in locations[..width].iter().zip(replacement) {
                    bus.cpu_memory64(self.hart_id, location, 1).unwrap()[0] = byte;
                }
            }
        }
        if width == 16 && destination != 0 {
            self.registers[destination + 1] = (old >> 64) as u64;
        }
        Ok(Some(sign(old as u64, width.min(8) as u32 * 8)))
    }

    fn read_unaligned_atomic(
        &self,
        bus: &mut Bus,
        address: u64,
        locations: &mut [u64],
        value: &mut [u8],
    ) -> Result<(), Fault> {
        for (offset, (location, byte)) in locations.iter_mut().zip(value).enumerate() {
            let guest = address.wrapping_add(offset as u64);
            *location =
                self.translations
                    .translate(bus, &self.privilege, guest, 1, READ | WRITE)?;
            *byte = *bus
                .cpu_ram64(self.hart_id, *location, 1)
                .and_then(|data| data.first())
                .ok_or(Fault::new(7, guest))?;
        }
        Ok(())
    }

    fn atomic(
        &mut self,
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
            if !self.allow_unaligned || permission != (READ | WRITE) {
                return Err(Fault::new(if load { 4 } else { 6 }, address));
            }
            let mut locations = [0; 8];
            let mut value = [0; 8];
            self.read_unaligned_atomic(bus, address, &mut locations[..width], &mut value[..width])?;
            let previous = sign(u64::from_le_bytes(value), width as u32 * 8);
            let replacement = atomic_result(operation, previous, sign(rhs, width as u32 * 8));
            for (&location, byte) in locations[..width].iter().zip(replacement.to_le_bytes()) {
                bus.cpu_memory64(self.hart_id, location, 1).unwrap()[0] = byte;
            }
            return Ok(previous);
        }
        self.trace_stack_access(bus, inst, address, width, !load);
        let physical =
            self.translations
                .translate(bus, &self.privilege, address, width, permission)?;
        let fault = || Fault::new(if load { 5 } else { 7 }, address);
        if operation == 1
            && let Ok(low) = u32::try_from(physical)
            && let Some(device) = bus.rv64_devices.mpfs_cache_way_enable.as_mut()
            && let Some(previous) = device.swap(low, width, rhs)
        {
            return Ok(sign(previous, width as u32 * 8));
        }
        let previous = ram_read(bus, Some(self.hart_id), physical, width).ok_or_else(fault)?;
        if operation == 2 {
            bus.reserve64(hart, address, physical, width);
        } else if operation == 3 {
            if !bus.matches_reservation64(hart, reservation, address, physical, width) {
                return Ok(1);
            }
            bus.cpu_memory64(self.hart_id, physical, width)
                .ok_or_else(fault)?
                .copy_from_slice(&rhs.to_le_bytes()[..width]);
            return Ok(0);
        } else {
            let old = sign(previous, width as u32 * 8);
            let rhs = sign(rhs, width as u32 * 8);
            let value = atomic_result(operation, old, rhs);
            bus.cpu_memory64(self.hart_id, physical, width)
                .ok_or_else(fault)?
                .copy_from_slice(&value.to_le_bytes()[..width]);
            if self.trace_amo {
                let destination = inst.rd();
                let after = |reg| {
                    if reg == destination && destination != 0 {
                        old
                    } else {
                        self.registers[reg]
                    }
                };
                let mask = u64::MAX >> (64 - width * 8);
                self.atomic_trace = Some(AtomicTrace {
                    instruction: inst.0,
                    width,
                    before: [
                        self.registers[destination],
                        address,
                        self.registers[inst.rs2()],
                        previous,
                    ],
                    after: [
                        after(destination),
                        after(inst.rs1()),
                        after(inst.rs2()),
                        value & mask,
                    ],
                });
            }
        }
        Ok(sign(previous, width as u32 * 8))
    }
}

fn atomic_result(operation: u32, old: u64, rhs: u64) -> u64 {
    match operation {
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
    }
}

pub(crate) fn ram_read(bus: &Bus, hart: Option<usize>, address: u64, width: usize) -> Option<u64> {
    let mut bytes = [0; 8];
    bytes[..width].copy_from_slice(match hart {
        Some(hart) => bus.cpu_ram64(hart, address, width)?,
        None => bus.ram64(address, width)?,
    });
    Some(u64::from_le_bytes(bytes))
}

fn andes_find_byte(function: u32, lhs: u64, rhs: u64) -> u64 {
    let left = lhs.to_le_bytes();
    let right = rhs.to_le_bytes();
    let mut pairs = left.into_iter().zip(right);
    let position = match function {
        16 => left.iter().position(|&byte| byte == right[0]),
        17 => pairs.position(|(a, b)| a == 0 || a != b),
        18 => pairs.position(|(a, b)| a != b),
        19 => pairs.rposition(|(a, b)| a != b),
        _ => unreachable!("decoded AndeStar byte operation"),
    };
    position.map_or(0, |index| (index as u64).wrapping_sub(8))
}

fn andes_gp_offset(instruction: u32, width: usize) -> u64 {
    let middle =
        ((instruction >> 9) & 0x800) | ((instruction >> 5) & 0x7000) | (instruction & 0x18000);
    let offset = if width == 4 {
        middle
            | ((instruction >> 20) & 0x7fc)
            | ((instruction >> 4) & 0x20000)
            | ((instruction >> 13) & 0x40000)
    } else {
        let byte = if width == 1 {
            (instruction >> 14) & 1
        } else {
            0
        };
        middle | ((instruction >> 20) & 0x7fe) | ((instruction >> 14) & 0x20000) | byte
    };
    sign(u64::from(offset), if width == 4 { 19 } else { 18 })
}

fn andes_bit_field(instruction: u32, source: u64, signed: bool) -> u64 {
    let msb = instruction >> 26;
    let lsb = (instruction >> 20) & 63;
    let (shift_in, shift_out, width) = if msb == 0 {
        (0, lsb, 1)
    } else if msb < lsb {
        (0, msb, lsb - msb + 1)
    } else {
        (lsb, 0, msb - lsb + 1)
    };
    let field = (source >> shift_in) & (u64::MAX >> (64 - width));
    let field = if signed { sign(field, width) } else { field };
    field << shift_out
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
            |operation: u32| Instruction((0x344 << 20) | (1 << 15) | (operation << 12) | (2 << 7));

        cpu.privilege.tick(seip, 0);
        assert_eq!(cpu.csr(&mut Bus::default(), instruction(2), stip, 0), Some(seip));
        assert_eq!(cpu.csr(&mut Bus::default(), instruction(3), stip, 0), Some(seip | stip));
        cpu.privilege.tick(0, 0);
        assert_eq!(cpu.privilege.read(0x344, cpu.misa(), !3), Some(0));

        // A guest can still set or clear the software contribution explicitly.
        cpu.privilege.tick(seip, 0);
        assert_eq!(cpu.csr(&mut Bus::default(), instruction(1), seip, 0), Some(seip));
        cpu.privilege.tick(0, 0);
        assert_eq!(cpu.csr(&mut Bus::default(), instruction(3), seip, 0), Some(seip));
        assert_eq!(cpu.privilege.read(0x344, cpu.misa(), !3), Some(0));
    }
}
