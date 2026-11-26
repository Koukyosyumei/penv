mod ble;
pub mod external_cpu;

use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, VecDeque},
    fs::File,
    io::{BufWriter, Read, Write},
    path::{Path, PathBuf},
    rc::Rc,
};

use crate::{
    bitmanip::BitManip,
    bus::{Bus, BusError, RAM_BASE},
    collapsed_profile::CollapsedProfile,
    csr::{Csr, EXTERNAL},
    event_trace::{self, ExternalEvent, Recorder},
    litex_panel::ButtonAction,
    litex_video::ReferenceFrame,
    nvic::ActiveExceptions,
    rtc_clock::RtcClock,
    rv64::{AtomicTrace, Rv64},
    sv32::TranslationCache,
    sync_action::SyncAction,
    thumb::{Thumb, ThumbStep},
    time_source::TimeSource,
    zephyr_boot::BusyWait,
};

mod floating;

pub const MAX_HARTS: usize = 8;
const DEFAULT_MONITOR_QUANTUM: u64 = 10_000; // 100 microseconds in 10 ns monitor units.
pub const VIRTUAL_STEPS_PER_SECOND: u32 = 1_000_000;

#[derive(Clone)]
pub(crate) struct PcTrace {
    file: Rc<RefCell<BufWriter<File>>>,
    binary: bool,
    amo: bool,
}

impl PcTrace {
    pub(crate) fn new(file: File, binary: bool, amo: bool) -> std::io::Result<Self> {
        let mut file = BufWriter::new(file);
        if binary {
            if amo {
                // The emitted opcodes are base A instructions.
                file.write_all(b"ReTrace\x05\x08\x01\x01\x0Friscv64 rv64ima")?;
            } else {
                file.write_all(b"ReTrace\x05\x08\x00")?;
            }
        }
        Ok(Self {
            file: Rc::new(RefCell::new(file)),
            binary,
            amo,
        })
    }

    fn write(&self, pc: u64) -> std::io::Result<()> {
        let mut file = self.file.borrow_mut();
        if self.binary {
            let mut record = [0; 9];
            record[..8].copy_from_slice(&pc.to_le_bytes());
            file.write_all(&record)
        } else {
            writeln!(file, "0x{pc:X}")
        }
    }

    fn write_atomic(&self, pc: u64, record: &AtomicTrace) -> Result<(), String> {
        let mut file = self.file.borrow_mut();
        let output = (|| -> std::io::Result<()> {
            if self.binary {
                file.write_all(&pc.to_le_bytes())?;
                file.write_all(&[4])?;
                file.write_all(&record.instruction.to_le_bytes())?;
                for (phase, values) in [(0, record.before), (1, record.after)] {
                    file.write_all(&[
                        3,
                        phase,
                        if record.width == 8 { 3 } else { 2 },
                        (record.instruction >> 27) as u8,
                    ])?;
                    for value in values {
                        file.write_all(&value.to_le_bytes()[..record.width])?;
                    }
                }
                file.write_all(&[0])
            } else {
                let input = record
                    .instruction
                    .to_le_bytes()
                    .map(|byte| format!("{byte:#04x}"))
                    .join(" ");
                let decoded = crate::llvm_mc("+m,+a", "--disassemble", &input)
                    .map_err(std::io::Error::other)?;
                let text = String::from_utf8_lossy(&decoded.stdout);
                let instruction = text
                    .lines()
                    .map(str::trim)
                    .find(|line| !line.is_empty() && *line != ".text")
                    .filter(|_| decoded.status.success() && decoded.stderr.is_empty())
                    .ok_or_else(|| std::io::Error::other("llvm-mc could not decode traced AMO"))?;
                let instruction = instruction.split_whitespace().collect::<Vec<_>>().join(" ");
                writeln!(file, "0x{pc:X}: {:08x} {instruction}", record.instruction)?;
                for (phase, [rd, rs1, rs2, value]) in
                    [("before", record.before), ("after", record.after)]
                {
                    writeln!(
                        file,
                        "AMO operands {phase} - RD: 0x{rd:X}, RS1: 0x{rs1:X} (memory value: 0x{value:X}), RS2: 0x{rs2:X}"
                    )?;
                }
                Ok(())
            }
        })();
        output.map_err(|error| format!("RV64 AMO trace: {error}"))
    }

    pub(crate) fn flush(&self) -> std::io::Result<()> {
        self.file.borrow_mut().flush()
    }
}

pub struct Machine {
    pub(crate) cpu_less: bool,
    pub(crate) empty_monitor: bool,
    monitor_active: bool,
    egis_reset_cpu: Option<Box<Hart>>,
    egis_reset_entry: Option<u32>,
    pub(crate) monitor_single_step_blocking: bool,
    pub bus: Bus,
    pub primary: Hart,
    pub function_symbols: Option<BTreeMap<u32, String>>,
    secondary: Vec<Hart>,
    next_hart: usize,
    pc_trace: Option<(usize, PcTrace)>,
    halt_hooks: Vec<(usize, u64, usize)>,
    python_hooks: Vec<(usize, u64, crate::cpu_python_hook::Hook)>,
    halted: u8,
    pub(crate) stop_when_halted: bool,
    elapsed_steps: u64,
    boot_memory_cleared: bool,
    pub(crate) monitor_firmware: Option<std::path::PathBuf>,
    pub(crate) monitor_quantum: u64,
    cpu_paused: bool,
    cpu_halted: bool,
    cpu_clock_stopped: bool,
    pub reset_log: Option<String>,
    uart_step_prefix: bool,
    uart_line_start: bool,
    reset_steps: VecDeque<u64>,
    button_events: VecDeque<(u64, usize, bool)>,
    adc_events: VecDeque<(u64, usize, f64)>,
    inputs: VecDeque<(u64, Input)>,
    uart_device_events: Vec<(Rc<Cell<bool>>, Input)>,
    queued_frame_waits: VecDeque<(u64, Box<ReferenceFrame>)>,
    frame_wait_trigger: Option<Rc<Cell<bool>>>,
    frame_wait_budget: Option<u64>,
    frame_wait_deadline: Option<u64>,
    intermediate_frame_matches: Vec<u64>,
    time_source: TimeSource<1>,
    managed_clock: crate::clock_source::ClockSource<'static>,
    external_cpus: Vec<external_cpu::Registration>,
    pub rtc_clock: RtcClock,
    recorder: Option<Recorder>,
    replay: VecDeque<(u64, ExternalEvent)>,
    profile: Option<CollapsedProfile>,
    adxl_events: VecDeque<(u64, String, [i16; 3])>,
}

#[derive(Clone, Copy)]
pub enum LiteXUartAction {
    Phy(u16),
    Temperature(i32),
    AssertLed(bool),
    Toggle,
}

enum Input {
    Uart(Vec<u8>),
    Button(String, ButtonAction),
    PhyVendorSpecific1 { name: String, value: u16 },
    SensorTemperature { name: String, value: i32 },
    LedAssertion { name: String, expected: bool },
    FramebufferCapture(PathBuf),
    FrameWait(Box<ReferenceFrame>),
}

#[derive(Clone)]
pub struct Hart {
    pub name: Option<String>,
    slot: usize,
    csr: Csr,
    translations: TranslationCache,
    thumb: Option<Thumb>,
    zfh: bool,
    zfhmin: bool,
    zfbfmin: bool,
    zfa: bool,
    zfa_fli: bool,
    zfa_min_max: bool,
    zfa_quiet_compare: bool,
    zfa_round: bool,
    floating: Option<Box<[u64; 32]>>,
    unaligned_accesses: bool,
    zicond: bool,
    pub rv64: Option<Box<Rv64>>,
    busy_wait: Option<BusyWait>,
    zacas: bool,
    zabha: bool,
    zaamo: bool,
    zalrsc: bool,
    bitmanip: BitManip,
    cpu_wait_asserted: bool,
    wfi_as_nop: bool,
    waiting_for_interrupt: bool,
    pub monitor_paused: bool,
    pub monitor_halted: bool,
    monitor_single_step: bool,
    pub monitor_hooks: Vec<(u32, CpuHook)>,
    reset_held: bool,
    reset_release: Option<SyncAction<()>>,
    reset_pc_written: bool,
    pub pc: u32,
    reset_vector: u32,
    pc_written: bool,
    pub registers: [u32; 32],
}

#[derive(Clone)]
pub enum CpuHook {
    Log(String),
    WriteDoubleWord(u32, u32),
    Pause,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Stop {
    FrameMatched(u64),
    Tohost(u64),
    Ecall,
    Ebreak,
    TestExit(u64),
    AllHalted,
    Reset,
    Semihosting(crate::semihosting::Exit),
    Paused,
    Limit,
    ScriptComplete,
    OutputMatch,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CpuResetKind {
    Cpu,
    Machine,
}

pub(crate) enum Step {
    Retired,
    RetiredAndPaused,
    Trap,
    Stop(Stop),
}

#[derive(Clone, Copy)]
enum Access {
    Fetch,
    Load,
    Store,
}

enum Fault {
    Bus(Access, BusError),
    Illegal(u32),
    InvalidCsr(u32, String),
    MisalignedTarget(u32),
    Breakpoint(u32),
}

impl Fault {
    fn exception(&self) -> Option<(u32, u32)> {
        Some(match self {
            Self::Bus(access, error) => match error {
                BusError::Misaligned(address, _) => (
                    match access {
                        Access::Fetch => 0,
                        Access::Load => 4,
                        Access::Store => 6,
                    },
                    *address,
                ),
                BusError::Unmapped(address, _)
                | BusError::Denied(address)
                | BusError::Command(address, _)
                | BusError::AddressError(address)
                | BusError::SlaveFault(address) => (
                    match access {
                        Access::Fetch => 1,
                        Access::Load => 5,
                        Access::Store => 7,
                    },
                    *address,
                ),
                BusError::Page(address) => (
                    match access {
                        Access::Fetch => 12,
                        Access::Load => 13,
                        Access::Store => 15,
                    },
                    *address,
                ),
                BusError::Output(_)
                | BusError::AudioOutput(_)
                | BusError::CommandError(_)
                | BusError::Python(_) => {
                    return None;
                }
            },
            Self::Illegal(insn) | Self::InvalidCsr(insn, _) => (2, *insn),
            Self::MisalignedTarget(address) => (0, *address),
            Self::Breakpoint(address) => (3, *address),
        })
    }
}

impl std::fmt::Display for Fault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bus(_, error) => error.fmt(f),
            Self::Illegal(insn) => write!(f, "unsupported instruction {insn:#010x}"),
            Self::InvalidCsr(_, error) => f.write_str(error),
            Self::Breakpoint(address) => write!(f, "address breakpoint at {address:#010x}"),
            Self::MisalignedTarget(address) => {
                write!(f, "misaligned instruction target {address:#010x}")
            }
        }
    }
}

fn sign(value: u32, bits: u32) -> u32 {
    (((value << (32 - bits)) as i32) >> (32 - bits)) as u32
}

fn multiply_divide(operation: u32, lhs: u32, rhs: u32) -> u32 {
    let signed_lhs = lhs as i32;
    let signed_rhs = rhs as i32;
    match operation {
        0 => lhs.wrapping_mul(rhs),
        1 => ((i64::from(signed_lhs) * i64::from(signed_rhs)) >> 32) as u32,
        2 => ((i64::from(signed_lhs) * i64::from(rhs)) >> 32) as u32,
        3 => ((u64::from(lhs) * u64::from(rhs)) >> 32) as u32,
        4 if rhs != 0 => signed_lhs.wrapping_div(signed_rhs) as u32,
        5 if rhs != 0 => lhs / rhs,
        4 | 5 => u32::MAX,
        6 if rhs != 0 => signed_lhs.wrapping_rem(signed_rhs) as u32,
        7 if rhs != 0 => lhs % rhs,
        6 | 7 => lhs,
        _ => unreachable!("funct3 is a three-bit field"),
    }
}

impl Machine {
    pub fn new(size: usize) -> Result<Self, String> {
        Ok(Self {
            bus: Bus::new(size)?,
            ..Self::without_default_ram()
        })
    }

    pub fn without_default_ram() -> Self {
        Self {
            bus: Bus::default(),
            function_symbols: None,
            primary: Hart {
                name: None,
                slot: 0,
                csr: Csr::default(),
                translations: TranslationCache::default(),
                thumb: None,
                zfh: false,
                zfhmin: false,
                zfbfmin: false,
                zfa: false,
                zfa_fli: false,
                zfa_min_max: false,
                zfa_quiet_compare: false,
                zfa_round: false,
                floating: None,
                unaligned_accesses: false,
                zicond: false,
                rv64: None,
                busy_wait: None,
                zacas: false,
                zabha: false,
                zaamo: false,
                zalrsc: false,
                bitmanip: BitManip::default(),
                cpu_wait_asserted: false,
                wfi_as_nop: true,
                waiting_for_interrupt: false,
                monitor_paused: false,
                monitor_halted: false,
                monitor_single_step: false,
                monitor_hooks: Vec::new(),
                reset_held: false,
                reset_release: None,
                reset_pc_written: false,
                pc: RAM_BASE,
                reset_vector: RAM_BASE,
                pc_written: false,
                registers: [0; 32],
            },
            secondary: Vec::new(),
            next_hart: 0,
            pc_trace: None,
            halt_hooks: Vec::new(),
            python_hooks: Vec::new(),
            halted: 0,
            stop_when_halted: false,
            elapsed_steps: 0,
            cpu_less: false,
            empty_monitor: false,
            monitor_active: false,
            egis_reset_cpu: None,
            egis_reset_entry: None,
            monitor_single_step_blocking: true,
            boot_memory_cleared: false,
            monitor_firmware: None,
            monitor_quantum: DEFAULT_MONITOR_QUANTUM,
            cpu_paused: false,
            cpu_halted: false,
            cpu_clock_stopped: false,
            reset_log: None,
            uart_step_prefix: false,
            uart_line_start: true,
            reset_steps: VecDeque::new(),
            button_events: VecDeque::new(),
            adc_events: VecDeque::new(),
            inputs: VecDeque::new(),
            uart_device_events: Vec::new(),
            queued_frame_waits: VecDeque::new(),
            frame_wait_trigger: None,
            frame_wait_budget: None,
            frame_wait_deadline: None,
            intermediate_frame_matches: Vec::new(),
            time_source: TimeSource::default(),
            managed_clock: crate::clock_source::ClockSource::default(),
            external_cpus: Vec::new(),
            rtc_clock: RtcClock::default(),
            recorder: None,
            replay: VecDeque::new(),
            profile: None,
            adxl_events: VecDeque::new(),
        }
    }

    pub fn enable_function_logging(&mut self) -> Result<(), String> {
        if self.is_cortex_m() {
            return Err("LogFunctionNames requires an RV32 machine".into());
        }
        self.function_symbols = Some(BTreeMap::new());
        Ok(())
    }

    pub fn configure_harts(&mut self, count: usize) {
        self.secondary = (1..count)
            .map(|id| {
                let mut hart = self.primary.clone();
                hart.slot = id;
                hart.csr.hart_id =
                    (self.primary.csr.hart_id as u32).wrapping_add(id as u32) as usize;
                if let Some(cpu) = &mut hart.rv64 {
                    cpu.hart_id = id;
                }
                hart.name = None;
                hart
            })
            .collect();
        self.next_hart = 0;
    }

    pub fn hart_count(&self) -> usize {
        1 + self.secondary.len()
    }

    pub fn hart_mut(&mut self, id: usize) -> &mut Hart {
        if id == 0 {
            &mut self.primary
        } else {
            &mut self.secondary[id - 1]
        }
    }

    pub(crate) fn rv64_hart_by_name(&self, name: &str) -> Result<usize, String> {
        std::iter::once(&self.primary)
            .chain(&self.secondary)
            .enumerate()
            .find_map(|(id, hart)| {
                (hart.name.as_deref() == Some(name) && hart.rv64.is_some()).then_some(id)
            })
            .ok_or_else(|| format!("unknown named RV64 CPU {name:?}"))
    }

    pub(crate) fn configure_monitor_vlen(&mut self, name: &str, bits: u64) -> Result<(), String> {
        let hart = self.named_hart(name)?;
        if let Some(cpu) = &mut hart.rv64 {
            cpu.privilege.vector.as_mut().ok_or("VectorRegisterLength requires the V extension")?
                .set_length(bits).map_err(str::to_owned)
        } else {
            hart.csr.set_vector_length(bits)
        }
    }

    pub(crate) fn install_csr_hook(&mut self, name: &str, csr: u32, source: &str) -> Result<(), String> {
        if csr > 0xfff {
            return Err("CSR address exceeds 12 bits".into());
        }
        let index = self.rv64_hart_by_name(name)?;
        let hart_id = self.hart_mut(index).rv64.as_ref().expect("validated RV64 CPU").hart_id;
        let key = (hart_id, csr);
        if self.bus.csr_hooks.contains_key(&key) {
            return Err(format!("CSR {csr:#x} already has a registered handler"));
        }
        let hook = crate::cpu_python_hook::Hook::new(source, self.monitor_firmware.clone(), Some(name.to_owned()))?;
        self.bus.csr_hooks.insert(key, hook);
        Ok(())
    }

    pub(crate) fn install_stack_access_hook(&mut self, name: &str, source: &str) -> Result<(), String> {
        let index = self.rv64_hart_by_name(name)?;
        let cpu = self.hart_mut(index).rv64.as_ref().expect("validated RV64 CPU");
        let hart_id = cpu.hart_id;
        let hook = crate::cpu_python_hook::Hook::new(source, self.monitor_firmware.clone(), Some(name.to_owned()))?;
        self.bus.stack_access_hooks.insert(hart_id, hook);
        Ok(())
    }

    pub(crate) fn enable_rv64_stack_access_trace(&mut self, name: &str) -> Result<(), String> {
        let id = self.rv64_hart_by_name(name)?;
        self.hart_mut(id).rv64.as_mut().expect("validated RV64 CPU").stack_access_trace_name =
            Some(name.to_owned());
        Ok(())
    }

    pub(crate) fn rv64_translate_read(&mut self, name: &str, address: u64) -> Result<u64, String> {
        let id = self.rv64_hart_by_name(name)?;
        let hart = if id == 0 {
            &self.primary
        } else {
            &self.secondary[id - 1]
        };
        hart.rv64
            .as_ref()
            .expect("validated RV64 CPU")
            .translate_read(&mut self.bus, address)
    }

    pub(crate) fn configure_egis_reset(&mut self) {
        self.egis_reset_cpu = Some(Box::new(self.primary.clone()));
    }

    pub(crate) fn set_egis_reset_entry(&mut self, entry: Option<(&str, u32)>) {
        if self.egis_reset_cpu.is_some() {
            self.egis_reset_entry = entry
                .filter(|(name, _)| self.primary.name.as_deref() == Some(*name))
                .map(|(_, address)| address);
        }
    }

    pub(crate) fn initialize_monitor_reset_pcs(&mut self) {
        for hart in std::iter::once(&mut self.primary).chain(&mut self.secondary) {
            if let Some(cpu) = &mut hart.rv64 {
                cpu.initialize_monitor_reset_pc();
            } else if hart.thumb.is_none() {
                hart.initialize_reset_pc();
            }
        }
    }

    pub(crate) fn monitor_reset_cpu(&mut self, name: &str, initial: &[Hart]) -> Result<(), String> {
        let initial = initial.iter().find(|hart| hart.name.as_deref() == Some(name))
            .ok_or_else(|| format!("unknown CPU {name:?}"))?;
        let hart = self.named_hart(name)?;
        if let Some(cpu) = &mut hart.rv64 {
            cpu.reset_from(initial.rv64.as_ref().ok_or("missing initial RV64 CPU state")?);
            hart.monitor_single_step = false;
            hart.monitor_halted = false;
            Ok(())
        } else {
            hart.monitor_reset(initial)
        }
    }

    pub(crate) fn install_python_hook(&mut self, name: &str, pc: u64, source: &str, file: std::path::PathBuf) -> Result<(), String> {
        let index = std::iter::once(&self.primary).chain(&self.secondary)
            .position(|hart| hart.name.as_deref() == Some(name))
            .ok_or_else(|| format!("unknown hook CPU {name:?}"))?;
        self.python_hooks.push((index, pc, crate::cpu_python_hook::Hook::new(source, Some(file), None)?));
        Ok(())
    }

    pub(crate) fn set_riscv_wfi_as_nop(&mut self, name: &str, enabled: bool) -> Result<(), String> {
        let hart = self.named_hart(name)?;
        if let Some(cpu) = &mut hart.rv64 {
            cpu.set_wfi_as_nop(enabled);
        } else if hart.thumb.is_none() {
            hart.wfi_as_nop = enabled;
            if enabled {
                hart.waiting_for_interrupt = false;
            }
        } else {
            return Err(format!("WfiAsNop requires a RISC-V CPU, got {name:?}"));
        }
        Ok(())
    }

    pub(crate) fn clear_monitor_translation_cache(&mut self, name: &str) -> Result<(), String> {
        let hart = self.named_hart(name)?;
        if let Some(cpu) = &mut hart.rv64 {
            cpu.clear_translation_cache();
        } else {
            hart.translations.clear();
        }
        Ok(())
    }

    pub(crate) fn require_riscv_cpu(&mut self, name: &str) -> Result<(), String> {
        if self.named_hart(name)?.thumb.is_some() {
            return Err(format!("CSR validation requires a RISC-V CPU, got {name:?}"));
        }
        Ok(())
    }

    pub(crate) fn monitor_single_cpu_name(&self) -> Result<&str, String> {
        if self.cpu_less {
            return Err("This machine has no CPUs.".into());
        }
        if self.hart_count() != 1 {
            return Err(format!("This machine has {} CPUs and no CPU name was specified", self.hart_count()));
        }
        self.primary.name.as_deref().ok_or_else(|| "the platform CPU has no monitor name".into())
    }

    pub(crate) fn monitor_named_pc(&mut self, name: &str) -> Result<u64, String> {
        Ok(self.named_hart(name)?.program_counter())
    }

    pub(crate) fn monitor_named_register(
        &mut self, name: &str, register: &str, value: Option<u64>,
    ) -> Result<Option<u64>, String> {
        const ABI: [&str; 32] = [
            "zero", "ra", "sp", "gp", "tp", "t0", "t1", "t2", "s0", "s1", "a0", "a1", "a2",
            "a3", "a4", "a5", "a6", "a7", "s2", "s3", "s4", "s5", "s6", "s7", "s8", "s9",
            "s10", "s11", "t3", "t4", "t5", "t6",
        ];
        let hart = self.named_hart(name)?;
        let register = register.to_ascii_lowercase();
        if hart.thumb.is_some() && register != "resetvector" {
            let index = match register.as_str() {
                "sp" => Some(13),
                "lr" => Some(14),
                "pc" => Some(15),
                name => crate::parse_address(name.strip_prefix('r').unwrap_or(name)).ok(),
            }.filter(|index| *index < 16)
                .ok_or_else(|| format!("unsupported Cortex-M monitor register {register}"))?;
            let value = value.map(u32::try_from).transpose()
                .map_err(|_| "register value exceeds Cortex-M width")?;
            return hart.monitor_integer_register(index, value).map(|value| value.map(u64::from));
        }
        if register == "pc" {
            return match value {
                Some(value) if let Some(cpu) = &mut hart.rv64 => {
                    cpu.set_program_counter(value);
                    Ok(None)
                }
                Some(value) => {
                    let address = u32::try_from(value).map_err(|_| "PC exceeds RV32 width")?;
                    self.set_named_entry(name, address)?;
                    Ok(None)
                }
                None => Ok(Some(hart.program_counter())),
            };
        }
        if register == "sepc" {
            return match &mut hart.rv64 {
                Some(cpu) => cpu.monitor_sepc(value),
                None => hart.csr.monitor_sepc(value),
            };
        }
        if register == "priv" {
            return match &mut hart.rv64 {
                Some(cpu) => cpu.privilege.monitor_privilege(value),
                None => hart.csr.monitor_privilege(value),
            };
        }
        if register == "resetvector" {
            if let Some(cpu) = &mut hart.rv64 {
                return Ok(match value {
                    Some(value) => { cpu.set_reset_vector(value); None }
                    None => cpu.monitor_register("resetvector"),
                });
            }
            if let Some(value) = value {
                hart.monitor_reset_vector(u32::try_from(value).map_err(|_| "reset vector exceeds RV32 width")?)?;
                return Ok(None);
            }
            if hart.thumb.is_some() {
                return Err("unsupported monitor ResetVector for Cortex-M".into());
            }
            return Ok(Some(u64::from(hart.reset_vector)));
        }
        if let Some(vector) = match register.as_str() {
            "mtvec" => Some(crate::csr::TrapVector::Machine),
            "stvec" => Some(crate::csr::TrapVector::Supervisor),
            _ => None,
        } {
            if let Some(cpu) = &mut hart.rv64 {
                return cpu.privilege.monitor_trap_vector(vector, value);
            }
            let value = value.map(u32::try_from).transpose()
                .map_err(|_| "trap vector exceeds RV32 width")?;
            return hart.monitor_trap_vector(vector, value).map(|value| value.map(u64::from));
        }
        if let Some(value) = value.filter(|_| register == "mstatus") {
            if let Some(cpu) = &mut hart.rv64 {
                cpu.initialize_mstatus(value);
                return Ok(None);
            }
            if hart.thumb.is_none() {
                hart.set_mstatus(u32::try_from(value).map_err(|_| "mstatus exceeds RV32 width")?);
                return Ok(None);
            }
        }
        if let Some(value) = value.filter(|_| register == "mepc") {
            if let Some(cpu) = &mut hart.rv64 {
                cpu.privilege.monitor_mepc(value);
                return Ok(None);
            }
            if hart.thumb.is_none() {
                let value = u32::try_from(value).map_err(|_| "MEPC exceeds RV32 width")?;
                hart.csr.monitor_mepc(value);
                return Ok(None);
            }
        }
        if let Some(value) = value.filter(|_| register == "mtval") {
            if let Some(cpu) = &mut hart.rv64 {
                cpu.privilege.set_monitor_trap_value(value);
                return Ok(None);
            }
            if hart.thumb.is_none() {
                let value = u32::try_from(value).map_err(|_| "MTVAL exceeds RV32 width")?;
                hart.csr.set_monitor_trap_value(value);
                return Ok(None);
            }
        }
        if value.is_none() && matches!(register.as_str(), "mstatus" | "mepc" | "mcause" | "mtval") {
            let result = match &hart.rv64 {
                Some(cpu) => cpu.monitor_register(&register),
                None if hart.thumb.is_none() => hart.csr.monitor_trap_register(&register).map(u64::from),
                None => None,
            };
            return result.map(Some).ok_or_else(|| format!("unsupported monitor register {register}"));
        }
        if matches!(register.as_str(), "fflags" | "frm" | "fcsr") {
            let fcsr = if let Some(cpu) = &mut hart.rv64 {
                cpu.monitor_register("fflags").map(|_| &mut cpu.privilege.fcsr)
            } else if hart.csr.f_extension {
                Some(&mut hart.csr.fcsr)
            } else {
                None
            }.ok_or("floating-point control registers require the F extension")?;
            let (mask, shift) = match register.as_str() {
                "fflags" => (0x1f, 0),
                "frm" => (0xe0, 5),
                _ => (0xff, 0),
            };
            if let Some(value) = value {
                *fcsr = (*fcsr & !mask) | (((value as u8) << shift) & mask);
                return Ok(None);
            }
            return Ok(Some(u64::from((*fcsr & mask) >> shift)));
        }
        if let Some(index) = register.strip_prefix('f').and_then(|index| index.parse::<usize>().ok()) {
            if let Some(cpu) = &mut hart.rv64 {
                return match value {
                    Some(value) => cpu.initialize_freg(index, value).map(|()| None),
                    None => cpu.monitor_register(&register).map(Some)
                        .ok_or_else(|| format!("unsupported monitor register {register}")),
                };
            }
            let target = hart.floating.as_mut().and_then(|registers| registers.get_mut(index))
                .ok_or_else(|| format!("unsupported monitor register {register}"))?;
            return if let Some(value) = value {
                *target = value;
                Ok(None)
            } else {
                Ok(Some(*target))
            };
        }
        if let Some(address) = match register.as_str() {
            "vstart" => Some(0x008),
            "vxsat" => Some(0x009),
            "vxrm" => Some(0x00a),
            "vcsr" => Some(0x00f),
            "vl" => Some(0xc20),
            "vtype" => Some(0xc21),
            "vlenb" => Some(0xc22),
            _ => None,
        } {
            if let Some(cpu) = &mut hart.rv64 {
                let vector = cpu.privilege.vector.as_mut().ok_or("vector register requires the V extension")?;
                if let Some(value) = value {
                    if !vector.write(address, value) {
                        return Err("vector register is read-only".into());
                    }
                    return Ok(None);
                }
                return Ok(vector.read(address));
            }
            return hart.csr.monitor_vector_register(address, value);
        }
        let index = match register.as_str() {
            "fp" => Some(8),
            register => ABI.iter().position(|name| *name == register).or_else(|| {
                let value = register.strip_prefix('x').unwrap_or(register);
                crate::parse_address(value).ok().map(|index| index as usize)
            }),
        }.filter(|index| *index < 32)
            .ok_or_else(|| format!("unsupported monitor register {register}"))?;
        if let Some(cpu) = &mut hart.rv64 {
            return match value {
                Some(_) if index == 0 => Err("The 'X0' register is read-only.".into()),
                Some(value) => { cpu.initialize_gpr(index, value); Ok(None) }
                None => Ok(cpu.monitor_register(&format!("x{index}"))),
            };
        }
        let value = value.map(u32::try_from).transpose().map_err(|_| "register value exceeds RV32 width")?;
        hart.monitor_integer_register(index as u32, value).map(|value| value.map(u64::from))
    }

    pub(crate) fn monitor_assembly_target(
        &mut self, name: &str,
    ) -> Result<(Option<usize>, &'static str, String), String> {
        if let Ok(index) = self.rv64_hart_by_name(name) {
            let cpu = self.hart_mut(index).rv64.as_ref().expect("named RV64 CPU");
            return Ok((Some(index), "riscv64", cpu.isa().llvm_features()));
        }
        let hart = self.named_hart(name)?;
        if hart.thumb.is_some() || hart.csr.embedded_base {
            return Err("unsupported monitor assembler target".into());
        }
        let available = [
            ("+m", hart.csr.m_extension), ("+a", hart.csr.a_extension),
            ("+c", hart.csr.c_extension), ("+f", hart.csr.f_extension),
            ("+d", hart.csr.d_extension), ("+v", hart.bitmanip.has_vector_execution()), ("+zba", hart.bitmanip.zba),
            ("+zbb", hart.bitmanip.zbb), ("+zbc", hart.bitmanip.zbc),
            ("+zbs", hart.bitmanip.zbs), ("+zicond", hart.zicond),
            ("+zacas", hart.zacas),
            ("+zbkb", hart.bitmanip.zbkb), ("+zbkx", hart.bitmanip.zbkx),
            ("+zbkc", hart.bitmanip.zbkc), ("+zknh", hart.bitmanip.zknh),
            ("+zkne", hart.bitmanip.zkne), ("+zknd", hart.bitmanip.zknd),
            ("+zksed", hart.bitmanip.zksed), ("+zksh", hart.bitmanip.zksh),
        ];
        let features = available.into_iter().filter_map(|(name, enabled)| enabled.then_some(name))
            .collect::<Vec<_>>().join(",");
        Ok((None, "riscv32", features))
    }

    pub(crate) fn monitor_execution_mode(
        &mut self, name: &str, single_step: Option<bool>,
    ) -> Result<bool, String> {
        let hart = self.named_hart(name)?;
        if hart.thumb.is_some() {
            return Err("unsupported monitor ExecutionMode for Cortex-M".into());
        }
        if let Some(single_step) = single_step {
            hart.monitor_single_step = single_step;
        }
        Ok(hart.monitor_single_step)
    }

    pub(crate) fn monitor_riscv_halted(
        &mut self, name: &str, halted: Option<bool>,
    ) -> Result<bool, String> {
        let hart = self.named_hart(name)?;
        if let Some(halted) = halted {
            hart.monitor_halted = halted;
        }
        Ok(hart.monitor_halted)
    }

    pub(crate) fn monitor_step_riscv(&mut self, name: &str, steps: u64, output: &mut impl Write)
        -> Result<(Stop, u64), String>
    {
        let index = std::iter::once(&self.primary).chain(&self.secondary)
            .position(|hart| hart.name.as_deref() == Some(name) && hart.thumb.is_none())
            .ok_or_else(|| format!("unknown RISC-V CPU {name:?}"))?;
        let hart = self.hart_mut(index);
        if hart.monitor_halted {
            return Ok((Stop::Limit, 0));
        }
        hart.monitor_single_step = true;
        self.run_selected(steps, Some(index), output, |_, _| false)
    }

    pub(crate) fn set_monitor_symbol_pc(&mut self, cpu: &str, symbol: &str) -> Result<(), String> {
        let path = self.monitor_firmware.as_deref().ok_or("no firmware ELF for symbol lookup")?;
        let address = crate::cpu_python_hook::symbol(path, symbol)?;
        if let Some(rv64) = &mut self.named_hart(cpu)?.rv64 {
            rv64.set_program_counter(address);
            Ok(())
        } else {
            let address = u32::try_from(address).map_err(|_| "symbol address exceeds CPU width")?;
            self.set_named_entry(cpu, address)
        }
    }

    pub fn obtain_managed_thread(
        &mut self,
        action: impl FnMut() + 'static,
        frequency: std::num::NonZeroU64,
    ) -> crate::clock_source::ManagedThread {
        self.managed_clock.managed_thread(frequency, action)
    }

    pub fn run_to_nearest_sync_point(
        &mut self,
        quantum: std::time::Duration,
    ) -> Result<(), crate::clock_source::ClockOverflow> {
        self.managed_clock.advance(quantum)
    }

    pub fn set_peripheral_local_name(
        &mut self, handle: &crate::bus_translation::PeripheralHandle, name: Option<&str>,
    ) -> Result<(), String> {
        let name = name.ok_or("missing peripheral name")?;
        self.bus.name_translated_peripheral(handle, name)
    }

    pub fn peripheral_by_path(
        &self, path: &str,
    ) -> Option<&crate::bus_translation::PeripheralHandle> {
        self.bus.translated_peripheral_by_path(path)
    }

    pub fn set_named_entry(&mut self, name: &str, entry: u32) -> Result<(), String> {
        let hart = self.named_hart(name)?;
        if let Some(cpu) = &mut hart.rv64 {
            cpu.set_program_counter(u64::from(entry));
        }
        hart.pc = entry;
        hart.pc_written = true;
        hart.reset_pc_written |= hart.reset_held;
        Ok(())
    }

    pub fn set_boot_vectors(&mut self, name: &str, address: u32) -> Result<(), String> {
        self.set_named_vector_table(name, address)
    }

    pub fn set_zephyr_wait_hooks(
        &mut self,
        name: &str,
        entries: impl Iterator<Item = u32>,
    ) -> Result<(), String> {
        let hart = self.named_hart(name)?;
        if hart.thumb.is_none() {
            return Err("Zephyr busy-wait hooks currently require Cortex-M".into());
        }
        hart.busy_wait = BusyWait::new(entries);
        Ok(())
    }

    fn named_hart(&mut self, name: &str) -> Result<&mut Hart, String> {
        if self.cpu_less {
            return Err("the platform has no CPU".into());
        }
        std::iter::once(&mut self.primary)
            .chain(&mut self.secondary)
            .find(|hart| hart.name.as_deref() == Some(name))
            .ok_or_else(|| format!("unknown CPU {name:?}"))
    }

    pub fn set_named_vector_table(&mut self, name: &str, address: u32) -> Result<(), String> {
        let thumb = self
            .named_hart(name)?
            .thumb
            .as_mut()
            .ok_or("VectorTableOffset requires a Cortex-M CPU")?;
        if address & 0x7f != 0 {
            return Err("VectorTableOffset requires a 128-byte-aligned address".into());
        }
        thumb.set_vector_base(address);
        Ok(())
    }

    pub fn monitor_external_pmp(
        &mut self,
        path: &str,
        config: bool,
        index: u32,
        value: Option<u64>,
    ) -> Result<Option<u64>, String> {
        for hart in std::iter::once(&mut self.primary).chain(&mut self.secondary) {
            if let Some(cpu) = &mut hart.rv64
                && let Some(result) = cpu.monitor_external_pmp(path, config, index, value)
            {
                return Ok(result);
            }
            if let Some(pmp) = hart.csr.external_pmp(path) {
                let value = value.map(u32::try_from).transpose().map_err(|_| "PMP register value exceeds RV32 width")?;
                let result = pmp.external_csr(config, index, value).map(u64::from);
                if value.is_some() {
                    hart.translations.clear();
                }
                return Ok(result);
            }
        }
        Err(format!("unknown external PMP {path:?}"))
    }

    pub fn monitor_pc(&mut self, name: &str, value: Option<u32>) -> Result<Option<u32>, String> {
        if let Some(value) = value {
            self.set_named_entry(name, value)?;
            return Ok(None);
        }
        let hart = self.named_hart(name)?;
        Ok(Some(hart.pc))
    }

    pub fn set_cortex_m_monitor_pc(
        &mut self,
        name: &str,
        entry: u32,
        output: &mut impl Write,
    ) -> Result<(), String> {
        if self.primary.name.as_deref() != Some(name) {
            return Err(format!("unknown CPU {name:?}"));
        }
        let thumb = self
            .primary
            .thumb
            .as_mut()
            .ok_or("expected a Cortex-M CPU")?;
        self.primary.pc = thumb.patch_monitor_pc(entry, name, output)?;
        Ok(())
    }

    pub fn set_entry(&mut self, entry: u32) {
        for hart in std::iter::once(&mut self.primary).chain(&mut self.secondary) {
            if let Some(cpu) = &mut hart.rv64 {
                cpu.set_program_counter(u64::from(entry));
            }
            hart.pc = entry;
            hart.pc_written = true;
        }
    }

    pub fn set_rv64_entry(&mut self, entry: u64) {
        for hart in std::iter::once(&mut self.primary).chain(&mut self.secondary) {
            hart.rv64
                .as_mut()
                .expect("RV64 machine")
                .set_program_counter(entry);
        }
    }

    pub fn raw_load_address(&self) -> u32 {
        self.primary
            .thumb
            .as_ref()
            .map_or(RAM_BASE, Thumb::vector_base)
    }

    pub fn infer_cortex_m_vectors(&mut self, address: u32) {
        if let Some(thumb) = self.primary.thumb.as_mut() {
            thumb.infer_vector_base(address);
        }
    }

    pub fn is_cortex_m(&self) -> bool {
        self.primary.thumb.is_some()
    }

    pub fn has_cortex_m4(&self) -> bool {
        self.primary
            .thumb
            .as_ref()
            .is_some_and(|thumb| thumb.thumb2)
    }

    pub fn validate_entry(&mut self) -> Result<(), String> {
        for hart in std::iter::once(&mut self.primary).chain(&mut self.secondary) {
            if let Some(cpu) = &hart.rv64 {
                cpu.validate(&mut self.bus)?;
                continue;
            }
            if hart.thumb.is_some() {
                let explicit_pc = hart.pc_written.then_some(hart.pc);
                hart.reset_cortex_m(&mut self.bus)?;
                if let Some(pc) = explicit_pc {
                    hart.pc = pc;
                    hart.reset_pc_written = hart.reset_held;
                }
                continue;
            }
            hart.fetch(&mut self.bus)
                .map_err(|error| format!("hart {}: {error}", hart.csr.hart_id))?;
        }
        Ok(())
    }

    pub fn cortex_m_cpu(&mut self, name: &str) -> Result<&Thumb, String> {
        self.named_hart(name)?
            .thumb
            .as_ref()
            .ok_or_else(|| format!("{name:?} is not a Cortex-M CPU"))
    }

    pub fn cortex_m_monitor_cpu(&mut self, name: &str) -> Result<&mut Hart, String> {
        let hart = self.named_hart(name)?;
        if hart.thumb.is_none() {
            return Err(format!("{name:?} is not a Cortex-M CPU"));
        }
        Ok(hart)
    }

    pub fn set_cortex_m_cpu_wait(&mut self, name: &str, asserted: bool) -> Result<(), String> {
        let boundary = self.elapsed_steps().saturating_add(1);
        let hart = self.named_hart(name)?;
        if hart.thumb.is_none() {
            return Err("CpuWaitSignal requires a Cortex-M CPU".into());
        }
        hart.cpu_wait_asserted = asserted;
        hart.reset_release = (hart.reset_held && !asserted).then(|| SyncAction::new(boundary, ()));
        Ok(())
    }

    pub fn reset_named_cortex_m(&mut self, name: &str) -> Result<(), String> {
        if self.primary.name.as_deref() != Some(name) || !self.is_cortex_m() {
            return Err(format!("unknown Cortex-M CPU {name:?}"));
        }
        self.primary.reset_cortex_m(&mut self.bus)?;
        self.primary.monitor_paused = !self.primary.reset_held;
        Ok(())
    }

    pub fn reset_cortex_m_machine(&mut self) -> Result<(), String> {
        if !self.is_cortex_m() {
            return Err("machine Reset requires a Cortex-M CPU".into());
        }
        self.bus.reset_cortex_m_machine_devices();
        self.primary.monitor_paused = false;
        self.primary.reset_cortex_m(&mut self.bus)?;
        Ok(())
    }

    fn check_memory_snapshot(&self) -> Result<(), String> {
        if !self.is_cortex_m() || !self.secondary.is_empty() || self.elapsed_steps() != 0 {
            return Err(
                "memory snapshots require a single-core Cortex-M machine before execution".into(),
            );
        }
        Ok(())
    }

    fn cpu_less_snapshot_prefix(&self) -> Result<Vec<u8>, String> {
        if self.recorder.is_some() || !self.replay.is_empty() || !self.inputs.is_empty() {
            return Err("CPU-less snapshots require an idle event queue".into());
        }
        if self.empty_monitor {
            if self.bus.unhandled_access.is_some() {
                return Err("empty monitor snapshots require the default unmapped access policy".into());
            }
            return Ok(Vec::new());
        }
        let (base, name) = self.bus.snapshot_trivial_uart()?;
        let mut data = base.to_le_bytes().to_vec();
        data.extend_from_slice(&(name.len() as u64).to_le_bytes());
        data.extend_from_slice(name.as_bytes());
        Ok(data)
    }

    fn write_cpu_less_snapshot(&self, output: &mut impl Write) -> Result<(), String> {
        let mut data = if self.empty_monitor { b"PENVIOST\x03" } else { b"PENVIOST\x02" }.to_vec();
        data.extend_from_slice(&self.cpu_less_snapshot_prefix()?);
        data.extend_from_slice(&self.elapsed_steps().to_le_bytes());
        data.extend_from_slice(&self.monitor_quantum.to_le_bytes());
        data.push(u8::from(self.primary.monitor_paused));
        data.extend_from_slice(&self.bus.default_uart_timeout().to_le_bytes());
        output.write_all(&data).map_err(|error| error.to_string())
    }

    pub(crate) fn read_cpu_less_snapshot_platform(input: &mut impl Read) -> Result<Option<(u32, String)>, String> {
        fn read_bytes<const N: usize>(input: &mut impl Read) -> Result<[u8; N], String> {
            let mut bytes = [0; N];
            input.read_exact(&mut bytes).map_err(|e| e.to_string())?;
            Ok(bytes)
        }
        if Self::cpu_less_snapshot_version(input)? == 3 {
            return Ok(None);
        }
        let address = u32::from_le_bytes(read_bytes(input)?);
        let length = u64::from_le_bytes(read_bytes(input)?);
        // Bound the allocation before reading an untrusted snapshot.
        let length = usize::try_from(length).ok().filter(|length| *length <= 4096)
            .ok_or("snapshot UART name exceeds size limit")?;
        let mut name = vec![0; length];
        input.read_exact(&mut name).map_err(|e| e.to_string())?;
        Ok(Some((address, String::from_utf8(name).map_err(|e| e.to_string())?)))
    }

    fn cpu_less_snapshot_version(input: &mut impl Read) -> Result<u8, String> {
        let mut header = [0; 9];
        input.read_exact(&mut header).map_err(|e| e.to_string())?;
        Self::cpu_less_snapshot_format(&header)
    }

    fn cpu_less_snapshot_format(header: &[u8; 9]) -> Result<u8, String> {
        match header {
            b"PENVIOST\x01" => Ok(1),
            b"PENVIOST\x02" => Ok(2),
            b"PENVIOST\x03" => Ok(3),
            _ => Err("unknown CPU-less snapshot format".into()),
        }
    }

    fn cortex_snapshot_has_registers(header: &[u8; 9]) -> std::io::Result<bool> {
        match header {
            b"PENVBOOT\x01" => Ok(false),
            b"PENVBOOT\x02" => Ok(true),
            _ => Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "unknown snapshot format")),
        }
    }

    pub(crate) fn validate_boot_snapshot_header(input: &mut impl Read) -> Result<(), String> {
        let mut header = [0; 9];
        input.read_exact(&mut header).map_err(|error| error.to_string())?;
        if Self::cpu_less_snapshot_format(&header).is_ok() || Self::cortex_snapshot_has_registers(&header).is_ok() {
            Ok(())
        } else {
            Err("unknown snapshot format".into())
        }
    }

    fn read_cpu_less_snapshot(&mut self, input: &mut impl Read) -> Result<(), String> {
        let prefix = self.cpu_less_snapshot_prefix()?;
        let version = Self::cpu_less_snapshot_version(input)?;
        if (version == 3) != self.empty_monitor {
            return Err("CPU-less snapshot does not match the platform or format".into());
        }
        let has_uart_timeout = version >= 2;
        let mut data = vec![0; prefix.len() + if has_uart_timeout { 25 } else { 17 }];
        input.read_exact(&mut data).map_err(|error| error.to_string())?;
        if !data.starts_with(&prefix) || input.read(&mut [0]).map_err(|error| error.to_string())? != 0 {
            return Err("CPU-less snapshot does not match the platform or format".into());
        }
        let state = &data[prefix.len()..];
        let steps = u64::from_le_bytes(state[..8].try_into().unwrap());
        let quantum = u64::from_le_bytes(state[8..16].try_into().unwrap());
        if steps == u64::MAX || quantum == 0 || state[16] > 1 {
            return Err("invalid CPU-less snapshot clock state".into());
        }
        self.reset_monitor_time(steps, quantum);
        self.primary.monitor_paused = state[16] != 0;
        if has_uart_timeout {
            self.bus.set_default_uart_timeout(u64::from_le_bytes(state[17..25].try_into().unwrap()));
        }
        self.boot_memory_cleared = false;
        Ok(())
    }

    pub(crate) fn set_monitor_active(&mut self, active: bool) {
        self.monitor_active = active;
        self.primary.monitor_paused = !active;
    }

    pub(crate) fn monitor_is_active(&self) -> bool {
        self.monitor_active && !self.primary.monitor_paused
    }

    fn reset_monitor_time(&mut self, steps: u64, quantum: u64) {
        self.monitor_active = false;
        self.time_source = TimeSource::default();
        self.time_source.grant_until(steps);
        self.elapsed_steps = steps;
        self.monitor_quantum = quantum;
        self.bus.monitor_clock_ns = 0;
        self.bus.monitor_clock_step = 0;
        self.bus.uart_clock_step = steps;
    }

    pub(crate) fn write_boot_snapshot(&self, output: &mut impl Write) -> Result<(), String> {
        if self.boot_memory_cleared {
            return Err("Load a boot snapshot after Clear".into());
        }
        if self.cpu_less {
            return self.write_cpu_less_snapshot(output);
        }
        self.check_memory_snapshot()?;
        (|| -> std::io::Result<()> {
            output.write_all(b"PENVBOOT\x02")?;
            output.write_all(&self.raw_load_address().to_le_bytes())?;
            for value in self.primary.registers[..15].iter().copied().chain([self.primary.pc]) {
                output.write_all(&value.to_le_bytes())?;
            }
            self.bus.save_memory(output)
        })().map_err(|e| e.to_string())
    }

    pub fn clear_memory_snapshot(&mut self) -> Result<(), String> {
        if self.cpu_less {
            self.cpu_less_snapshot_prefix()?;
            self.reset_monitor_time(0, DEFAULT_MONITOR_QUANTUM);
            self.primary.monitor_paused = false;
            self.boot_memory_cleared = true;
            return Ok(());
        }
        self.check_memory_snapshot()?;
        self.bus.clear_memory();
        self.bus.reset_cortex_m_machine_devices();
        self.primary.thumb.as_mut().unwrap().reset_state();
        self.primary.monitor_halted = false;
        self.primary.registers.fill(0);
        self.primary.pc = 0;
        self.boot_memory_cleared = true;
        Ok(())
    }

    pub(crate) fn read_boot_snapshot(&mut self, input: &mut impl Read) -> Result<(), String> {
        if self.cpu_less {
            return self.read_cpu_less_snapshot(input);
        }
        self.check_memory_snapshot()?;
        let mut load = || -> std::io::Result<_> {
            fn read_word(input: &mut impl Read) -> std::io::Result<u32> {
                let mut bytes = [0; 4];
                input.read_exact(&mut bytes)?;
                Ok(u32::from_le_bytes(bytes))
            }
            let mut header = [0; 9];
            input.read_exact(&mut header)?;
            let has_registers = Self::cortex_snapshot_has_registers(&header)?;
            let address = read_word(input)?;
            if address & 0x7f != 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "invalid snapshot vector table address",
                ));
            }
            let mut registers = [0; 16];
            if has_registers {
                for value in &mut registers {
                    *value = read_word(input)?;
                }
            }
            self.bus.restore_memory(input)?;
            Ok((address, has_registers.then_some(registers)))
        };
        let (address, registers) = load().map_err(|e| e.to_string())?;
        let thumb = self.primary.thumb.as_mut().unwrap();
        thumb.set_vector_base(address);
        let Some(registers) = registers else {
            self.primary.reset_cortex_m(&mut self.bus)?;
            self.boot_memory_cleared = false;
            return Ok(());
        };
        thumb.reset_state();
        if let Some(nvic) = self.bus.nvic.as_mut() {
            nvic.set_vector_base(address);
            nvic.lockup = false;
            nvic.set_active_exceptions(0);
        }
        self.primary.registers.fill(0);
        self.primary.registers[..15].copy_from_slice(&registers[..15]);
        self.primary.pc = registers[15];
        self.boot_memory_cleared = false;
        Ok(())
    }

    pub fn schedule_reset(&mut self, step: u64) -> Result<(), String> {
        if !self.is_cortex_m() {
            return Err("--reset-at requires a Cortex-M platform".into());
        }
        let index = self
            .reset_steps
            .partition_point(|&scheduled| scheduled < step);
        if self.reset_steps.get(index) != Some(&step) {
            self.reset_steps.insert(index, step);
        }
        Ok(())
    }

    pub fn record_to(&mut self, path: &Path) -> Result<(), String> {
        if !self.replay.is_empty() {
            return Err("cannot start recording while replay is pending".into());
        }
        self.recorder = Some(Recorder::create(path)?);
        Ok(())
    }

    pub fn play_from(&mut self, path: &Path) -> Result<(), String> {
        if self.recorder.is_some() || !self.replay.is_empty() {
            return Err("external event recording or replay is already active".into());
        }
        let trace = event_trace::read(path)?;
        if trace
            .front()
            .is_some_and(|(step, _)| *step < self.elapsed_steps())
        {
            return Err("event trace begins before the current emulation time".into());
        }
        self.replay = trace;
        Ok(())
    }

    pub fn external_event(&mut self, event: ExternalEvent) -> Result<(), String> {
        self.apply_external_event(&event)?;
        let step = self.elapsed_steps();
        if let Some(recorder) = &mut self.recorder {
            recorder.record(step, &event)?;
        }
        Ok(())
    }

    fn apply_external_event(&mut self, event: &ExternalEvent) -> Result<(), String> {
        match event {
            ExternalEvent::PeripheralCall { peripheral, method, arguments } => {
                self.bus.dispatch_external_call(self.elapsed_steps(), peripheral, method, arguments)
            }
            ExternalEvent::Button(name, pressed) => self.bus.set_named_button(name, *pressed),
            ExternalEvent::Adc(name, channel, volts) => {
                self.bus.set_named_adc_input(name, *channel, *volts)
            }
            ExternalEvent::UartLine(name, text, ending) => {
                self.bus.write_stm32_uart_line(name, text, ending)
            }
        }
    }

    pub fn schedule_uart_input(&mut self, step: u64, bytes: Vec<u8>) -> Result<(), String> {
        if !self.bus.has_uart_input() {
            return Err("UART input requires a platform UART".into());
        }
        self.schedule_input(step, Input::Uart(bytes));
        Ok(())
    }

    pub fn schedule_stm32_button(
        &mut self,
        step: u64,
        name: &str,
        pressed: bool,
    ) -> Result<(), String> {
        let button = self
            .bus
            .board_button(name)
            .ok_or_else(|| format!("unknown board button {name:?}"))?;
        let index = self
            .button_events
            .partition_point(|(scheduled, _, _)| *scheduled <= step);
        self.button_events.insert(index, (step, button, pressed));
        Ok(())
    }

    pub fn schedule_adxl_input(
        &mut self,
        step: u64,
        name: String,
        acceleration: [i16; 3],
    ) -> Result<(), String> {
        if self.bus.nrf_adxl_input(&name).is_none() {
            return Err(format!("unknown nRF52840 ADXL372 {name:?}"));
        }
        let index = self
            .adxl_events
            .partition_point(|(scheduled, _, _)| *scheduled <= step);
        self.adxl_events.insert(index, (step, name, acceleration));
        Ok(())
    }

    pub fn schedule_adc_input(
        &mut self,
        step: u64,
        channel: usize,
        volts: f64,
    ) -> Result<(), String> {
        self.bus.validate_stm32f0_adc_input(channel, volts)?;
        let index = self
            .adc_events
            .partition_point(|(scheduled, _, _)| *scheduled <= step);
        self.adc_events.insert(index, (step, channel, volts));
        Ok(())
    }

    pub fn schedule_button(
        &mut self,
        step: u64,
        name: String,
        action: ButtonAction,
    ) -> Result<(), String> {
        if !self.bus.has_litex_button(&name) {
            return Err(format!("unknown LiteX button {name:?}"));
        }
        self.schedule_input(step, Input::Button(name, action));
        Ok(())
    }

    pub fn schedule_phy_vendor_specific1(
        &mut self,
        step: u64,
        name: String,
        value: u16,
    ) -> Result<(), String> {
        self.bus.litex_phy_mut(&name)?;
        self.schedule_input(step, Input::PhyVendorSpecific1 { name, value });
        Ok(())
    }

    pub fn arm_litex_uart_action(
        &mut self,
        fired: Rc<Cell<bool>>,
        name: &str,
        action: LiteXUartAction,
    ) -> Result<(), String> {
        let name = name.to_owned();
        let event = match action {
            LiteXUartAction::Phy(value) => {
                self.bus.litex_phy_mut(&name)?;
                Input::PhyVendorSpecific1 { name, value }
            }
            LiteXUartAction::Temperature(value) => {
                self.bus.litex_sensor_mut(&name)?;
                Input::SensorTemperature { name, value }
            }
            LiteXUartAction::AssertLed(expected) => {
                self.bus.litex_led_state(&name)?;
                Input::LedAssertion { name, expected }
            }
            LiteXUartAction::Toggle if self.bus.has_litex_button(&name) => {
                Input::Button(name, ButtonAction::Toggle)
            }
            LiteXUartAction::Toggle => return Err(format!("unknown LiteX button {name:?}")),
        };
        self.uart_device_events.push((fired, event));
        Ok(())
    }

    pub fn schedule_sensor_temperature(
        &mut self,
        step: u64,
        name: String,
        value: i32,
    ) -> Result<(), String> {
        self.bus.litex_sensor_mut(&name)?;
        self.schedule_input(step, Input::SensorTemperature { name, value });
        Ok(())
    }

    pub fn schedule_led_assertion(
        &mut self,
        step: u64,
        name: String,
        expected: bool,
    ) -> Result<(), String> {
        self.bus.litex_led_state(&name)?;
        self.schedule_input(step, Input::LedAssertion { name, expected });
        Ok(())
    }

    pub fn schedule_framebuffer_capture(&mut self, step: u64, path: PathBuf) -> Result<(), String> {
        if self.bus.litex_video.is_none() {
            return Err("--framebuffer-event requires a LiteX framebuffer".into());
        }
        self.schedule_input(step, Input::FramebufferCapture(path));
        Ok(())
    }

    pub fn arm_frame_waits(&mut self, trigger: Rc<Cell<bool>>, budget: u64) {
        self.frame_wait_trigger = Some(trigger);
        self.frame_wait_budget = Some(budget);
    }

    pub fn schedule_frame_wait(
        &mut self,
        step: u64,
        reference: ReferenceFrame,
    ) -> Result<(), String> {
        if self.bus.litex_video.is_none() {
            return Err("framebuffer wait requires a LiteX framebuffer".into());
        }
        let reference = Box::new(reference);
        if self.frame_wait_trigger.is_some()
            || self
                .inputs
                .iter()
                .any(|(_, input)| matches!(input, Input::FrameWait(_)))
        {
            self.queued_frame_waits.push_back((step, reference));
        } else {
            self.schedule_input(step, Input::FrameWait(reference));
        }
        Ok(())
    }

    pub fn start_profile(
        &mut self,
        cpu: &str,
        path: &Path,
        flush: bool,
        limit: Option<u64>,
    ) -> Result<(), String> {
        let pc = self.cortex_m_monitor_cpu(cpu)?.pc;
        if self.profile.is_some() {
            return Err("CPU already has an active profiler".into());
        }
        self.profile = Some(
            CollapsedProfile::new(path, pc, flush, limit)
                .map_err(|e| format!("{}: {e}", path.display()))?,
        );
        Ok(())
    }

    pub fn finish_profile(&mut self) -> Result<(), String> {
        if let Some(profile) = self.profile.take() {
            profile.finish().map_err(|e| format!("profiler: {e}"))?;
        }
        Ok(())
    }

    pub fn intermediate_frame_matches(&self) -> &[u64] {
        &self.intermediate_frame_matches
    }

    pub fn verify_required_events_completed(&self) -> Result<(), String> {
        if let Some((_, event)) = self.uart_device_events.first() {
            let name = match event {
                Input::Button(name, _)
                | Input::LedAssertion { name, .. }
                | Input::PhyVendorSpecific1 { name, .. }
                | Input::SensorTemperature { name, .. } => name,
                _ => unreachable!(),
            };
            return Err(format!(
                "execution ended before applying the LiteX UART action for {name:?}"
            ));
        }
        if self.frame_wait_trigger.is_some() {
            return Err("execution ended before the framebuffer UART trigger".into());
        }
        for (step, input) in &self.inputs {
            let event = match input {
                Input::LedAssertion { name, .. } => format!("LED assertion for {name:?}"),
                Input::FramebufferCapture(path) => {
                    format!("framebuffer capture to {}", path.display())
                }
                Input::FrameWait(_) => {
                    return Err("execution ended before the reference frame matched".into());
                }
                _ => continue,
            };
            return Err(format!("execution ended before {event} at step {step}"));
        }
        Ok(())
    }

    fn schedule_input(&mut self, step: u64, input: Input) {
        let index = self
            .inputs
            .partition_point(|(scheduled, _)| *scheduled <= step);
        self.inputs.insert(index, (step, input));
    }

    pub(crate) fn trace_pc(&mut self, name: &str, trace: PcTrace) -> Result<(), String> {
        let hart = self.rv64_hart_by_name(name)?;
        self.hart_mut(hart)
            .rv64
            .as_mut()
            .expect("named RV64 CPU")
            .trace_amo = trace.amo;
        self.pc_trace = Some((hart, trace));
        Ok(())
    }

    pub(crate) fn halt_rv64_cpu(&mut self, name: &str) -> Result<(), String> {
        let hart = self.rv64_hart_by_name(name)?;
        self.halted |= 1 << hart;
        Ok(())
    }

    pub fn halt_on_pc(&mut self, source: &str, pc: u64, target: &str) -> Result<(), String> {
        let find = |name: &str| {
            let name = name.strip_prefix("sysbus.").unwrap_or(name);
            std::iter::once(&self.primary)
                .chain(&self.secondary)
                .position(|hart| hart.rv64.is_some() && hart.name.as_deref() == Some(name))
                .ok_or_else(|| format!("unknown RV64 hook hart {name:?}"))
        };
        self.halt_hooks.push((find(source)?, pc, find(target)?));
        Ok(())
    }

    fn run_steps(
        &mut self,
        limit: u64,
        output: &mut impl Write,
        mut frames: Option<&mut Vec<crate::nrf52840::AirFrame>>,
    ) -> Result<(Stop, u64), String> {
        let Some(frames) = frames.as_mut() else {
            return self.run(limit, output);
        };
        let mut retired = 0;
        for _ in 0..limit {
            let (stop, count) = self.run(1, output)?;
            retired += count;
            if let Some(frame) = self.bus.tick_radio_air() {
                frames.push(frame);
            }
            if stop != Stop::Limit {
                return Ok((stop, retired));
            }
        }
        Ok((Stop::Limit, retired))
    }

    pub fn report_uart_steps(&mut self, enabled: bool) {
        self.uart_step_prefix = enabled;
    }

    pub fn pause_cpu(&mut self) {
        self.cpu_paused = true;
        self.bus.pause_registered_peripherals();
    }

    pub fn resume_cpu(&mut self) {
        self.cpu_paused = false;
        self.bus.start_registered_peripherals();
    }

    pub fn set_cpu_halted(&mut self, halted: bool) {
        self.cpu_halted = halted;
    }

    pub fn set_cpu_clocked(&mut self, clocked: bool) {
        self.cpu_clock_stopped = !clocked;
    }

    pub fn reset_cpu_control(
        &mut self,
        kind: CpuResetKind,
        output: &mut impl Write,
    ) -> Result<(), String> {
        if !self.is_cortex_m() {
            return Err("CPU reset control requires a Cortex-M CPU".into());
        }
        if kind == CpuResetKind::Machine {
            self.bus.reset_cortex_m_if_requested(true);
        }
        self.primary.reset_cortex_m(&mut self.bus)?;
        self.cpu_halted = false;
        self.cpu_paused = kind == CpuResetKind::Cpu;
        if kind == CpuResetKind::Machine
            && let Some(message) = self.reset_log.as_deref()
        {
            writeln!(output, "{message}").map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    // Monitor steps count completed instructions. A precise fault redirects the
    // CPU without completing the access, so stepping continues in the handler.
    pub fn monitor_step_cortex_m(
        &mut self,
        count: u64,
        output: &mut impl Write,
    ) -> Result<(), String> {
        if !self.is_cortex_m() || count == 0 {
            return Err("Step requires a Cortex-M CPU and a positive instruction count".into());
        }
        self.resume_cpu();
        let result = (|| {
            let mut remaining = count;
            let mut missed = 0;
            while remaining != 0 {
                match self.run(1, output)? {
                    (Stop::Limit, 1) => {
                        remaining -= 1;
                        missed = 0;
                    }
                    (Stop::Limit, 0) => {
                        missed += 1;
                        if missed == 256 || self.cpu_halted || self.cpu_clock_stopped {
                            return Err("Cortex-M Step could not retire an instruction".into());
                        }
                    }
                    (Stop::Paused | Stop::Ebreak, _) => break,
                    (stop, _) => return Err(format!("Cortex-M Step stopped with {stop:?}")),
                }
            }
            Ok(())
        })();
        self.pause_cpu();
        result
    }

    pub fn run(&mut self, limit: u64, output: &mut impl Write) -> Result<(Stop, u64), String> {
        self.run_until(limit, output, |_, _| false)
    }

    pub fn cortex_m_locked_up(&self) -> bool {
        self.primary.thumb.as_ref().is_some_and(Thumb::locked_up)
    }

    pub fn elapsed_steps(&self) -> u64 {
        self.time_source.elapsed()
    }

    pub(crate) fn monitor_elapsed_ns(&self) -> u128 {
        self.bus.monitor_time_ns(self.elapsed_steps())
    }

    pub(crate) fn set_monitor_cpu_rate(&mut self, mips: u32) -> Result<(), String> {
        if self.cpu_less {
            return Err("the platform has no CPU".into());
        }
        let now = self.monitor_elapsed_ns();
        self.bus.set_monitor_cpu_mips(mips)?;
        self.bus.monitor_clock_ns = now;
        self.bus.monitor_clock_step = self.elapsed_steps();
        Ok(())
    }

    pub fn run_until<W: Write>(
        &mut self,
        limit: u64,
        output: &mut W,
        matched: impl Fn(&Bus, &W) -> bool,
    ) -> Result<(Stop, u64), String> {
        if self.primary.monitor_paused {
            return Ok((Stop::Limit, 0));
        }
        self.run_selected(limit, None, output, matched)
    }

    pub(crate) fn step_rv64_cpu(
        &mut self,
        name: &str,
        output: &mut impl Write,
    ) -> Result<Stop, String> {
        let index = self.rv64_hart_by_name(name)?;
        self.run_selected(1, Some(index), output, |_, _| false)
            .map(|(stop, _)| stop)
    }

    fn apply_egis_reset_request(&mut self) -> Result<bool, String> {
        let Some(cause) = self.bus.andes_reset_requested.take() else { return Ok(false) };
        let Some(initial) = self.egis_reset_cpu.as_deref() else { return Ok(true) };
        let vector = self.bus.reset_egis_devices(cause).ok_or("missing Egis AOSMU")?;
        let paused = self.primary.monitor_paused;
        self.primary.configure_reset_vector(vector);
        self.primary.monitor_reset(initial)?;
        if let Some(address) = self.egis_reset_entry {
            self.primary.pc = address;
            self.primary.pc_written = true;
        }
        self.primary.monitor_paused = paused;
        self.halted &= !1;
        Ok(false)
    }

    fn run_selected<W: Write>(
        &mut self,
        limit: u64,
        selected: Option<usize>,
        output: &mut W,
        matched: impl Fn(&Bus, &W) -> bool,
    ) -> Result<(Stop, u64), String> {
        if self.boot_memory_cleared {
            return Err("Load a boot snapshot after Clear".into());
        }
        if self.cpu_less {
            if selected.is_some() {
                return Err("the platform has no CPU".into());
            }
            let end = self.time_source.elapsed().saturating_add(limit);
            self.time_source.grant_until(end);
            self.elapsed_steps = self.time_source.elapsed();
            self.bus.uart_clock_step = self.elapsed_steps;
            return Ok((Stop::Limit, 0));
        }
        if selected.is_none() && limit != 0 && self.monitor_single_step_blocking
            && std::iter::once(&self.primary).chain(&self.secondary).enumerate().any(|(index, hart)| {
                hart.monitor_single_step && !hart.monitor_halted && self.halted & (1 << index) == 0
            })
        {
            return Ok((if matched(&self.bus, output) { Stop::OutputMatch } else { Stop::Paused }, 0));
        }
        let all_harts = ((1u16 << self.hart_count()) - 1) as u8;
        let stop_when_halted = self.stop_when_halted && selected.is_none();
        if stop_when_halted && self.halted == all_harts {
            return Ok((Stop::AllHalted, 0));
        }
        let mut retired = 0;
        let ethernet_link = self.bus.has_rv64_ethernet_link();
        for _ in 0..limit {
            while self
                .replay
                .front()
                .is_some_and(|(step, _)| *step <= self.elapsed_steps())
            {
                let (_, event) = self.replay.pop_front().unwrap();
                self.apply_external_event(&event)?;
            }
            self.time_source
                .grant_until(self.time_source.elapsed().saturating_add(1));
            self.elapsed_steps = self.elapsed_steps.wrapping_add(1);
            self.bus.uart_clock_step = self.elapsed_steps;
            while let Some(index) = self
                .uart_device_events
                .iter()
                .position(|event| event.0.get())
            {
                let (_, input) = self.uart_device_events.remove(index);
                self.schedule_input(self.elapsed_steps, input);
            }
            if self
                .frame_wait_trigger
                .as_ref()
                .is_some_and(|trigger| trigger.get())
            {
                self.frame_wait_trigger = None;
                let (_, reference) = self
                    .queued_frame_waits
                    .pop_front()
                    .ok_or("framebuffer trigger has no reference frame")?;
                self.frame_wait_deadline = self
                    .frame_wait_budget
                    .map(|budget| self.elapsed_steps.saturating_add(budget));
                self.schedule_input(self.elapsed_steps, Input::FrameWait(reference));
            }
            while self
                .inputs
                .front()
                .is_some_and(|(step, _)| *step <= self.elapsed_steps)
            {
                match self.inputs.pop_front().unwrap().1 {
                    Input::Uart(bytes) => self
                        .bus
                        .queue_uart_input(bytes)
                        .map_err(|error| error.to_string())?,
                    Input::Button(name, action) => self.bus.set_litex_button(&name, action)?,
                    Input::PhyVendorSpecific1 { name, value } => {
                        self.bus.litex_phy_mut(&name)?.set_vendor_specific1(value);
                    }
                    Input::SensorTemperature { name, value } => {
                        self.bus.litex_sensor_mut(&name)?.temperature = value;
                    }
                    Input::FrameWait(reference) => {
                        let video = self
                            .bus
                            .litex_video
                            .as_ref()
                            .ok_or("no LiteX framebuffer")?;
                        if video.matches(&self.bus, &reference)? {
                            let Some((step, reference)) = self.queued_frame_waits.pop_front()
                            else {
                                return Ok((Stop::FrameMatched(self.elapsed_steps), retired));
                            };
                            self.intermediate_frame_matches.push(self.elapsed_steps);
                            self.frame_wait_deadline = self
                                .frame_wait_budget
                                .map(|budget| self.elapsed_steps.saturating_add(budget));
                            self.schedule_input(
                                step.max(self.elapsed_steps),
                                Input::FrameWait(reference),
                            );
                        } else {
                            const FRAME_POLL_STEPS: u64 = 1_000_000;
                            let next = self.elapsed_steps.checked_add(FRAME_POLL_STEPS).ok_or(
                                "step counter exhausted before the reference frame matched",
                            )?;
                            let next = if let Some(deadline) = self.frame_wait_deadline {
                                if self.elapsed_steps >= deadline {
                                    return Err("framebuffer wait timed out before the reference frame matched".into());
                                }
                                next.min(deadline)
                            } else {
                                next
                            };
                            self.schedule_input(next, Input::FrameWait(reference));
                        }
                    }
                    Input::FramebufferCapture(path) => {
                        self.bus.capture_litex_framebuffer(&path).map_err(|error| {
                            format!(
                                "framebuffer capture at step {}: {error}",
                                self.elapsed_steps
                            )
                        })?;
                    }
                    Input::LedAssertion { name, expected } => {
                        let actual = self.bus.litex_led_state(&name)?;
                        if actual != expected {
                            let state = |on| if on { "on" } else { "off" };
                            return Err(format!(
                                "LED {name:?} at step {}: expected {}, got {}",
                                self.elapsed_steps,
                                state(expected),
                                state(actual)
                            ));
                        }
                    }
                }
            }
            if ethernet_link && self.elapsed_steps % 16_384 == 0 {
                self.bus.poll_rv64_ethernet();
            }
            if self.bus.tick_watchdogs() {
                self.bus.reset_cortex_m_devices();
                self.primary.reset_cortex_m(&mut self.bus)?;
            }
            while self
                .adxl_events
                .front()
                .is_some_and(|(step, _, _)| *step <= self.elapsed_steps)
            {
                let (_, name, acceleration) = self.adxl_events.pop_front().unwrap();
                *self
                    .bus
                    .nrf_adxl_input(&name)
                    .ok_or_else(|| format!("unknown ADXL372 {name:?}"))? = acceleration;
            }
            self.bus.tick_andes_clocked_devices();
            if self.apply_egis_reset_request()? {
                return Ok((Stop::Reset, retired));
            }
            let index = selected.unwrap_or_else(|| {
                let index = self.next_hart;
                if index == self.secondary.len() {
                    self.next_hart = 0;
                } else {
                    self.next_hart += 1;
                }
                index
            });
            let hart = if index == 0 {
                &mut self.primary
            } else {
                &mut self.secondary[index - 1]
            };
            self.bus.tick_h7_peripherals();
            let rate = self.bus.step_hz();
            if let Some(dwt) = &mut self.bus.dwt {
                dwt.tick(rate);
            }
            if self
                .reset_steps
                .front()
                .is_some_and(|&step| step <= self.elapsed_steps)
            {
                while self
                    .reset_steps
                    .front()
                    .is_some_and(|&step| step <= self.elapsed_steps)
                {
                    self.reset_steps.pop_front();
                }
                self.bus.reset_cortex_m_if_requested(true);
                hart.reset_cortex_m(&mut self.bus)?;
            }
            while self
                .button_events
                .front()
                .is_some_and(|(step, _, _)| *step <= self.elapsed_steps)
            {
                let (_, button, pressed) = self.button_events.pop_front().unwrap();
                self.bus.set_button(button, pressed);
            }
            while self
                .adc_events
                .front()
                .is_some_and(|(step, _, _)| *step <= self.elapsed_steps)
            {
                let (_, channel, volts) = self.adc_events.pop_front().unwrap();
                self.bus.set_stm32f0_adc_input(channel, volts)?;
            }
            let pc = hart.pc;
            let profiled_instruction = if index == 0 && self.profile.is_some() {
                self.bus.read_ram::<2>(pc).ok().map(|high| {
                    let low = if (0xf000..=0xf7ff).contains(&high) {
                        self.bus.read_ram::<2>(pc.wrapping_add(2)).unwrap_or(0)
                    } else {
                        0
                    };
                    (high as u16, low as u16)
                })
            } else {
                None
            };
            self.bus
                .tick_litex_uart(self.elapsed_steps, output)
                .map_err(|error| format!("UART: {error}"))?;
            if self.halted & (1 << index) != 0 {
                continue;
            }
            for &(source, pc, target) in &self.halt_hooks {
                if index == source && hart.rv64.as_ref().is_some_and(|cpu| cpu.pc == pc) {
                    self.halted |= 1 << target;
                    if stop_when_halted && self.halted == all_harts {
                        return Ok((Stop::AllHalted, retired));
                    }
                }
            }
            if self.halted & (1 << index) != 0 {
                continue;
            }
            if hart.thumb.is_none() && (hart.monitor_halted
                || (selected.is_none() && hart.monitor_single_step))
            {
                if matched(&self.bus, output) {
                    return Ok((Stop::OutputMatch, retired));
                }
                continue;
            }
            for (cpu, address, hook) in &mut self.python_hooks {
                if hart.waiting_for_interrupt {
                    break;
                }
                if *cpu == index && *address == hart.program_counter() {
                    hook.execute(&mut self.bus, *address)?;
                }
            }
            let trace = self.pc_trace.as_ref().filter(|(id, _)| *id == index);
            let trace_pc = trace.map(|_| {
                hart.rv64
                    .as_ref()
                    .expect("trace targets a named RV64 CPU")
                    .pc
            });
            match hart.execute_step(
                &mut self.bus,
                self.elapsed_steps,
                self.cpu_halted,
                output,
                self.function_symbols.as_ref(),
            )? {
                Step::Retired => {
                    retired += 1;
                    if index == 0
                        && let Some(profile) = &mut self.profile
                        && !profile
                            .retired(pc, profiled_instruction, hart.pc)
                            .map_err(|e| format!("profiler: {e}"))?
                    {
                        self.profile = None;
                    }
                    if let (Some((_, trace)), Some(pc)) = (trace, trace_pc) {
                        if trace.amo {
                            if let Some(record) =
                                hart.rv64.as_mut().and_then(|cpu| cpu.atomic_trace.take())
                            {
                                trace.write_atomic(pc, &record)?;
                            }
                        } else {
                            trace
                                .write(pc)
                                .map_err(|error| format!("RV64 PC trace: {error}"))?;
                        }
                    }
                }
                Step::RetiredAndPaused => return Ok((Stop::Paused, retired + 1)),
                Step::Trap => {}
                Step::Stop(stop) => {
                    if stop == Stop::Paused {
                        self.pause_cpu();
                    }
                    return Ok((stop, retired));
                }
            }
            if matched(&self.bus, output) {
                return Ok((Stop::OutputMatch, retired));
            }
            if let Some(code) = self.bus.tohost_exit {
                return Ok((Stop::Tohost(code), retired));
            }
            if let Some(code) = self.bus.rv64_test_exit {
                return Ok((Stop::TestExit(code), retired));
            }
            if self.bus.rv64_devices.reset_requested {
                return Ok((Stop::Reset, retired));
            }
            let paused = hart.monitor_paused;
            if self.apply_egis_reset_request()? {
                return Ok((Stop::Reset, retired));
            }
            if paused {
                break;
            }
        }
        Ok((Stop::Limit, retired))
    }

    fn execute_hart(&mut self, index: usize, output: &mut impl Write) -> Result<Step, String> {
        let hart = if index == 0 {
            &mut self.primary
        } else {
            &mut self.secondary[index - 1]
        };
        self.bus
            .tick_litex_uart(self.elapsed_steps, output)
            .map_err(|error| format!("UART: {error}"))?;
        hart.execute_step(
            &mut self.bus,
            self.elapsed_steps,
            self.cpu_halted,
            output,
            self.function_symbols.as_ref(),
        )
    }
}

struct StepUart<'a, W> {
    output: &'a mut W,
    step: u64,
    line_start: &'a mut bool,
}

impl<W: Write> Write for StepUart<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        for line in bytes.split_inclusive(|&byte| byte == b'\n') {
            if *self.line_start {
                write!(self.output, "[step {}] ", self.step)?;
            }
            self.output.write_all(line)?;
            *self.line_start = line.last() == Some(&b'\n');
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.output.flush()
    }
}

impl Hart {
    pub fn set_hart_id(&mut self, id: usize) {
        self.csr.hart_id = id;
    }

    pub(crate) fn set_cortex_m_clocked(&mut self, clocked: bool) -> Result<(), String> {
        self.thumb
            .as_mut()
            .ok_or("Clocked requires Cortex-M")?
            .set_clocked(clocked);
        Ok(())
    }

    pub(crate) fn cortex_m_executed_instructions(&self) -> Option<u64> {
        self.thumb.as_ref().map(Thumb::executed_instructions)
    }

    pub(crate) fn reset_cortex_m(&mut self, bus: &mut Bus) -> Result<(), String> {
        let thumb = self.thumb.as_mut().ok_or("Reset requires Cortex-M")?;
        self.monitor_halted = false;
        self.reset_held = self.cpu_wait_asserted;
        self.reset_release = None;
        self.reset_pc_written = false;
        let (sp, pc, lr) = if self.reset_held {
            thumb.reset_state();
            (0, 0, 0)
        } else if thumb.thumb2 {
            let (sp, pc) = thumb.reset(bus)?;
            thumb.reset_state();
            (sp, pc, 0)
        } else {
            thumb.reset_m0(bus)
        };
        let vectors = thumb.vector_base();
        if let Some(wait) = &mut self.busy_wait {
            wait.reset();
        }
        if let Some(nvic) = bus.nvic.as_mut() {
            nvic.set_vector_base(vectors);
            nvic.lockup = false;
            nvic.set_active_exceptions(0);
        }
        self.registers.fill(0);
        self.registers[13] = sp;
        self.registers[14] = lr;
        self.pc = pc;
        Ok(())
    }

    pub fn configure_cortex_m(&mut self, vectors: Option<u32>, thumb2: bool) {
        let mut thumb = Thumb::new(vectors);
        thumb.thumb2 = thumb2;
        self.thumb = Some(thumb);
    }

    pub fn set_embedded_base(&mut self, enabled: bool) {
        self.csr.embedded_base = enabled;
    }

    pub fn embedded_base(&self) -> bool {
        self.csr.embedded_base
    }

    pub fn set_integer_extensions(&mut self, multiply: bool, atomic: bool, compressed: bool) {
        self.csr.m_extension = multiply;
        self.csr.a_extension = atomic;
        self.csr.c_extension = compressed;
    }

    pub fn set_compressed_float(&mut self, single: bool, double: bool) {
        self.csr.compressed_single = single;
        self.csr.compressed_double = double;
    }

    pub fn set_atomic_subextensions(&mut self, zaamo: bool, zalrsc: bool) {
        self.zaamo = zaamo;
        self.zalrsc = zalrsc;
    }

    pub fn set_zmmul(&mut self, enabled: bool) {
        self.csr.zmmul = enabled;
    }

    pub fn enable_vector_bootstrap(&mut self) {
        self.csr.enable_vector_bootstrap();
    }

    pub fn enable_floating(&mut self, double: bool, half: bool) {
        self.csr.f_extension = true;
        self.csr.d_extension = double;
        self.zfh = half;
        self.floating = Some(Box::new([0; 32]));
    }

    pub fn set_zfa(&mut self, enabled: bool) {
        self.zfa = enabled;
    }

    pub fn set_zfa_diagnostics(
        &mut self,
        fli: bool,
        min_max: bool,
        quiet_compare: bool,
        round: bool,
    ) {
        self.zfa_fli = fli;
        self.zfa_min_max = min_max;
        self.zfa_quiet_compare = quiet_compare;
        self.zfa_round = round;
    }

    pub fn set_zfhmin(&mut self, enabled: bool) {
        self.zfhmin = enabled;
    }

    pub fn set_zfbfmin(&mut self, enabled: bool) {
        self.zfbfmin = enabled;
    }

    pub fn set_bitmanip(&mut self, extensions: BitManip) {
        self.bitmanip = extensions;
        self.csr.andes = (extensions.xandesperf || extensions.xandesbfhcvt || extensions.xandesvbfhcvt || extensions.xandesvqmac).then(|| {
            let mut csrs = crate::andes_csr::AndesCsrs::default();
            csrs.bfloat16 = extensions.xandesbfhcvt;
            csrs.vector_bfloat16 = extensions.xandesvbfhcvt;
            csrs.quad_mac = extensions.xandesvqmac;
            csrs
        });
    }

    pub fn set_sscofpmf(&mut self, enabled: bool) {
        self.csr.sscofpmf = enabled;
    }

    pub fn set_zabha(&mut self, enabled: bool) {
        self.zabha = enabled;
    }

    pub fn set_zacas(&mut self, enabled: bool) {
        self.zacas = enabled;
    }

    pub fn set_zicond(&mut self, enabled: bool) {
        self.zicond = enabled;
    }

    pub fn set_unaligned_accesses(&mut self, enabled: bool) {
        self.unaligned_accesses = enabled;
    }

    pub fn enable_ibex(&mut self) {
        self.csr.ibex = true;
    }

    pub fn enable_vex_core(&mut self, built_in_irq: bool) {
        self.csr.vex = true;
        self.csr.local_irq_mask = built_in_irq.then_some(0);
    }

    pub fn enable_minerva(&mut self) {
        self.csr.minerva = true;
        self.csr.local_irq_mask = Some(0);
    }

    pub fn set_vex_supervisor_irq_mask(&mut self, value: u32) -> Result<(), String> {
        if !self.csr.vex || self.csr.local_irq_mask.is_none() {
            return Err("SetSupervisorIrqMask requires a VexRiscv platform".into());
        }
        self.csr.vex_supervisor_irq_mask = value;
        Ok(())
    }

    pub fn set_vex_irq_mask(&mut self, value: u32) -> Result<(), String> {
        if !self.csr.vex {
            return Err("--vex-irq-mask requires a VexRiscv platform".into());
        }
        let mask = self
            .csr
            .local_irq_mask
            .as_mut()
            .ok_or("--vex-irq-mask requires a VexRiscv platform")?;
        *mask = value;
        Ok(())
    }

    pub fn enable_user_mode(&mut self) {
        self.csr.enable_user_mode();
    }

    pub fn enable_supervisor_mode(&mut self) {
        self.csr.enable_supervisor_mode();
    }

    pub fn enable_address_triggers(&mut self) {
        self.csr.enable_address_triggers();
    }

    pub fn enable_smepmp(&mut self) {
        self.csr.enable_smepmp();
    }

    pub fn attach_external_pmp(&mut self, path: String, entries: usize) {
        self.csr.attach_external_pmp(path, entries);
    }

    pub fn enable_pmp(&mut self, entries: usize) {
        self.csr.enable_pmp(entries);
    }

    fn translate(
        &mut self,
        bus: &mut Bus,
        address: u32,
        width: usize,
        permissions: u8,
    ) -> Result<u32, BusError> {
        if address as usize & (width - 1) != 0 && permissions != crate::pmp::EXECUTE {
            return Err(BusError::Misaligned(address, width));
        }
        let physical = self
            .translations
            .translate(bus, &self.csr, address, permissions)?;
        self.csr
            .permits(
                physical,
                width,
                permissions,
                permissions == crate::pmp::EXECUTE,
            )
            .then_some(physical)
            .ok_or(BusError::Denied(address))
    }

    fn rv32_vector_transfer(
        &mut self,
        instruction: u32,
        run: impl FnOnce(&mut Self, &mut crate::rv32_vector::Rv32Vector)
            -> Result<(), crate::rv32_vector::TransferError<Fault>>,
    ) -> Result<(), Fault> {
        use crate::rv32_vector::TransferError;

        if !self.csr.vectors_active() {
            return Err(Fault::Illegal(instruction));
        }
        let mut vector = self.csr.vector_bootstrap.take().ok_or(Fault::Illegal(instruction))?;
        let result = run(self, &mut vector);
        self.csr.vector_bootstrap = Some(vector);
        match result {
            Err(TransferError::Illegal) => Err(Fault::Illegal(instruction)),
            Err(TransferError::Memory(error)) => {
                self.csr.vectors_dirty();
                Err(error)
            }
            Ok(()) => {
                self.csr.vectors_dirty();
                Ok(())
            }
        }
    }

    fn vector_doubleword(&mut self, bus: &mut Bus, address: u32, value: Option<u64>) -> Result<u64, Fault> {
        let (permission, access) = if value.is_some() {
            (crate::pmp::WRITE, Access::Store)
        } else {
            (crate::pmp::READ, Access::Load)
        };
        if self.csr.address_trigger_matches(address, permission) {
            return Err(Fault::Breakpoint(address));
        }
        if address & 7 != 0 {
            return Err(Fault::Bus(access, BusError::Misaligned(address, 8)));
        }
        let physical = self.translate(bus, address, 8, permission)
            .map_err(|error| Fault::Bus(access, error))?;
        let bytes = bus.memory64(u64::from(physical), 8)
            .ok_or(Fault::Bus(access, BusError::Unmapped(address, 8)))?;
        if let Some(value) = value {
            bytes.copy_from_slice(&value.to_le_bytes());
            Ok(0)
        } else {
            Ok(u64::from_le_bytes(bytes.try_into().expect("eight-byte RAM slice")))
        }
    }

    fn load<const N: usize>(&mut self, bus: &mut Bus, address: u32) -> Result<u32, Fault> {
        if self.csr.address_trigger_matches(address, crate::pmp::READ) {
            return Err(Fault::Breakpoint(address));
        }
        if N > 1 && address as usize & (N - 1) != 0 && self.unaligned_accesses {
            let mut value = 0;
            for index in 0..N {
                let byte = self.load::<1>(bus, address.wrapping_add(index as u32))?;
                value |= byte << (index * 8);
            }
            return Ok(value);
        }
        self.translate(bus, address, N, crate::pmp::READ)
            .and_then(|physical| {
                bus.read::<N>(physical)
                    .map_err(|error| error.at_virtual(address))
            })
            .map_err(|error| Fault::Bus(Access::Load, error))
    }

    fn store(
        &mut self,
        bus: &mut Bus,
        address: u32,
        width: usize,
        value: u32,
        output: &mut impl Write,
    ) -> Result<(), Fault> {
        if self.csr.address_trigger_matches(address, crate::pmp::WRITE) {
            return Err(Fault::Breakpoint(address));
        }
        if address as usize & (width - 1) != 0 && self.unaligned_accesses {
            for (index, byte) in value.to_le_bytes()[..width].iter().enumerate() {
                self.store(
                    bus,
                    address.wrapping_add(index as u32),
                    1,
                    u32::from(*byte),
                    output,
                )?;
            }
            return Ok(());
        }
        self.translate(bus, address, width, crate::pmp::WRITE)
            .and_then(|physical| {
                bus.write_with_origin(physical, width, value, output, Some(self.slot))
                    .map_err(|error| error.at_virtual(address))
            })
            .map_err(|error| Fault::Bus(Access::Store, error))
    }

    pub fn set_mstatus(&mut self, value: u32) {
        self.csr.set_mstatus(value);
        self.translations.clear();
    }

    pub(crate) fn enable_clic(&mut self) {
        self.csr.enable_clic();
    }

    pub fn configure_interrupt_mode(&mut self, mode: crate::csr::InterruptMode) {
        self.csr.configure_interrupt_mode(mode);
    }

    pub fn configure_reset_vector(&mut self, address: u32) {
        self.reset_vector = address;
    }

    pub fn initialize_reset_pc(&mut self) {
        self.pc = self.reset_vector;
    }

    fn set_reset_vector(&mut self, address: u32) {
        self.reset_vector = address;
        if !self.pc_written {
            self.pc = address;
        }
    }

    pub fn monitor_reset_vector(&mut self, address: u32) -> Result<(), String> {
        if self.thumb.is_some() {
            return Err("ResetVector requires a RISC-V CPU".into());
        }
        self.set_reset_vector(address);
        Ok(())
    }

    pub fn monitor_reset(&mut self, initial: &Self) -> Result<(), String> {
        if self.thumb.is_some() {
            return Err("CPU Reset requires a RISC-V CPU".into());
        }
        let reset_vector = self.reset_vector;
        *self = initial.clone();
        self.pc_written = false;
        self.set_reset_vector(reset_vector);
        Ok(())
    }

    pub fn monitor_integer_register(
        &mut self,
        index: u32,
        value: Option<u32>,
    ) -> Result<Option<u32>, String> {
        if self.thumb.is_some() {
            if index >= 16 {
                return Err(format!("Wrong Cortex-M register index: {index}"));
            }
            let target = if index == 15 { &mut self.pc } else { &mut self.registers[index as usize] };
            return match value {
                Some(value) => {
                    *target = value;
                    if index == 15 {
                        self.pc_written = true;
                        self.reset_pc_written |= self.reset_held;
                    }
                    Ok(None)
                }
                None => Ok(Some(*target)),
            };
        }
        let count = if self.csr.embedded_base { 16 } else { 32 };
        let register = self.registers[..count]
            .get_mut(index as usize)
            .ok_or_else(|| format!("Wrong register index: {index}"))?;
        match value {
            Some(_) if index == 0 => Err("The 'X0' register is read-only.".into()),
            Some(value) => {
                *register = value;
                Ok(None)
            }
            None => Ok(Some(*register)),
        }
    }

    pub fn monitor_trap_vector(
        &mut self,
        register: crate::csr::TrapVector,
        value: Option<u32>,
    ) -> Result<Option<u32>, String> {
        if self.thumb.is_some() {
            return Err("RISC-V trap vector command requires a RISC-V CPU".into());
        }
        self.csr.monitor_trap_vector(register, value)
    }

    pub fn program_counter(&self) -> u64 {
        self.rv64.as_ref().map_or(u64::from(self.pc), |cpu| cpu.pc)
    }

    pub fn set_mtvec(&mut self, value: u32) {
        self.csr.set_vector(value);
    }

    pub fn compressed_instructions(&self) -> bool {
        self.csr.c_extension
    }

    fn fetch(&mut self, bus: &mut Bus) -> Result<(u32, u32), BusError> {
        let pc = self.pc;
        let minimum = if self.csr.c_extension { 2 } else { 4 };
        if pc as usize & (minimum - 1) != 0 {
            return Err(BusError::Misaligned(pc, minimum));
        }
        let physical = self.translate(bus, pc, minimum, crate::pmp::EXECUTE)?;
        if minimum == 4 {
            return bus
                .read_ram::<4>(physical)
                .map(|word| (word, 4))
                .map_err(|error| error.at_virtual(pc));
        }
        let low = bus
            .read_ram::<2>(physical)
            .map_err(|error| error.at_virtual(pc))?;
        if low & 3 != 3 {
            return Ok((low, 2));
        }
        let upper = pc.wrapping_add(2);
        let high_physical = if pc & 0xfff == 0xffe {
            self.translate(bus, upper, 2, crate::pmp::EXECUTE)?
        } else {
            // The second parcel uses the same page; check PMP for the entire access.
            if !self.csr.permits(physical, 4, crate::pmp::EXECUTE, true) {
                return Err(BusError::Denied(pc));
            }
            physical + 2
        };
        let high = bus
            .read_ram::<2>(high_physical)
            .map_err(|error| error.at_virtual(upper))?;
        Ok((low | (high << 16), 4))
    }

    fn execute_step(
        &mut self,
        bus: &mut Bus,
        elapsed_steps: u64,
        halted: bool,
        output: &mut impl Write,
        symbols: Option<&BTreeMap<u32, String>>,
    ) -> Result<Step, String> {
        if let Some(cpu) = &mut self.rv64 {
            return cpu.step(bus, self.csr.hart_id, elapsed_steps, output);
        }
        if self.thumb.is_some() && bus.reset_cortex_m_if_requested(false) {
            self.reset_cortex_m(bus)?;
        }
        if self.thumb.is_some() && self.reset_held {
            bus.sample_nvic(&ActiveExceptions::default(), false);
            bus.tick_systick();
            if SyncAction::take_due(&mut self.reset_release, elapsed_steps).is_some() {
                if self.reset_pc_written {
                    self.reset_held = false;
                    self.reset_pc_written = false;
                } else {
                    self.reset_cortex_m(bus)?;
                }
            }
            return Ok(Step::Trap);
        }
        if let Some(thumb) = self.thumb.as_mut() {
            bus.tick_stm32_uarts();
            if self.monitor_halted || halted {
                thumb.sample_interrupts(bus);
                return Ok(Step::Trap);
            }
            bus.advance_nrf_audio()
                .and_then(|()| bus.advance_nrf_ppi(output))
                .and_then(|()| bus.advance_stm32_dma(output))
                .map_err(|error| format!("pc={:#010x}: {error}", self.pc))?;
            if !thumb.clocked() {
                bus.tick_systick();
                return Ok(Step::Trap);
            }
            if self.busy_wait.as_mut().is_some_and(BusyWait::waiting) {
                bus.tick_systick();
                return Ok(Step::Trap);
            }
            if thumb
                .service_interrupts(&mut self.pc, &mut self.registers, bus)
                .map_err(|error| format!("pc={:#010x}: {error}", self.pc))?
            {
                return Ok(Step::Trap);
            }
            if thumb.sleeping() {
                return Ok(Step::Trap);
            }
            if let Some(wait) = &mut self.busy_wait
                && wait.contains(self.pc)
            {
                let return_address = self.registers[14];
                if return_address & 1 == 0 {
                    return Err(format!(
                        "invalid Zephyr busy-wait return address {return_address:#010x}"
                    ));
                }
                self.pc = return_address & !1;
                let delay = self.registers[0];
                if delay != 0 {
                    wait.skip(delay, bus.step_hz());
                    return Ok(Step::Trap);
                }
            }
            if !thumb.locked_up() {
                for (_, hook) in self
                    .monitor_hooks
                    .iter()
                    .filter(|(address, _)| *address == self.pc)
                {
                    match hook {
                        CpuHook::Log(message) => {
                            crate::monitor_log::record(1, message);
                            if bus.monitor_log_enabled(1) {
                                eprintln!("{message}");
                            }
                        }
                        CpuHook::WriteDoubleWord(address, value) => bus
                            .write_scalar(*address, 4, *value, output, true)
                            .map_err(|error| format!("CPU hook at {:#010x}: {error}", self.pc))?,
                        CpuHook::Pause => {
                            self.monitor_paused = true;
                            eprintln!("Machine paused");
                        }
                    }
                }
            }
            return thumb
                .execute_guest(&mut self.pc, &mut self.registers, bus, output)
                .map(|retired| match retired {
                    ThumbStep::Retired => Step::Retired,
                    ThumbStep::Breakpoint => Step::Stop(Stop::Ebreak),
                    ThumbStep::Exception => Step::Trap,
                    ThumbStep::Semihosting(exit) => Step::Stop(Stop::Semihosting(exit)),
                })
                .map_err(|error| format!("pc={:#010x}: {error}", self.pc));
        }
        let clint = bus.clint.as_mut().map_or(0, |clint| clint.tick(self.slot));
        let machine_timer_irq = bus.riscv_machine_timer.as_ref()
            .is_some_and(|timer| self.slot == timer.hart && timer.interrupt_pending());
        let mut pending = clint | (u32::from(machine_timer_irq) * crate::clint::TIMER);
        if let Some(clic) = bus.clic.as_mut() {
            clic.sample_clint(clint);
            if self.csr.clic_mode() {
                pending = 0;
                if let Some(interrupt) = clic.interrupt()
                    && let Some((target, indirect)) = self.csr.clic_interrupt(self.pc, interrupt)
                {
                    self.waiting_for_interrupt = false;
                    clic.acknowledge();
                    if indirect {
                        clic.clear_edge(interrupt.id);
                    }
                    self.pc = if indirect {
                        bus.read_scalar::<4>(target, false)
                            .map_err(|e| format!("CLIC vector at {target:#x}: {e}"))?
                    } else {
                        target
                    };
                    return Ok(Step::Trap);
                }
            }
        }
        let mut time = bus
            .clint
            .as_ref()
            .map_or(elapsed_steps, |clint| clint.time());
        if let Some(timer) = bus.riscv_machine_timer.as_ref()
            && timer.time_provider && timer.hart == self.slot
        {
            time = timer.time();
        }
        if let Some(timer) = bus.litex_cpu_timer.as_mut() {
            if timer.tick() {
                pending |= crate::clint::TIMER;
            }
            time = timer.time;
        }
        let count_retired = self.csr.tick_counters(time);
        let uart = bus
            .uart
            .as_ref()
            .is_some_and(|uart| uart.interrupt_pending());
        let timer = bus.timer.as_mut().is_some_and(|(_, timer)| timer.tick());
        let plic = bus.tick_plic();
        let mut lines = bus.tick_litex(self.slot, elapsed_steps);
        if self.slot == 0 {
            lines |= u64::from(
                (u32::from(uart) * EXTERNAL)
                    | (u32::from(timer) * crate::clint::TIMER)
                    | bus
                        .ethernet
                        .iter()
                        .fold(0, |lines, device| lines | device.vex_irq()),
            );
            if self.csr.vex && self.csr.local_irq_mask.is_some() {
                lines |= u64::from(plic);
            } else {
                pending |= plic;
            }
        }
        pending |= self.csr.sample_local_irq(lines);
        if (pending != 0
            || self.csr.has_local_irq_to_clear()
            || bus.clint.is_some()
            || bus.litex_cpu_timer.is_some()
            || bus.uart.is_some()
            || bus.timer.is_some()
            || bus.has_litex_irq()
            || bus.has_plic()
            || !bus.ethernet.is_empty()
            || self.csr.has_injected_interrupts())
            && let Some(target) = self.csr.interrupt(self.pc, pending)
        {
            self.pc = target;
            self.waiting_for_interrupt = false;
            return Ok(Step::Trap);
        }
        if self.waiting_for_interrupt {
            if !self.csr.wfi_wakeup_pending(pending) {
                return Ok(Step::Trap);
            }
            self.waiting_for_interrupt = false;
        }
        match self.step(bus, output, symbols) {
            Ok(Step::Retired) => {
                self.csr.retire(count_retired);
                Ok(Step::Retired)
            }
            Ok(step) => Ok(step),
            Err(fault) => {
                if let Some(target) = fault
                    .exception()
                    .and_then(|(cause, value)| self.csr.trap(self.pc, cause, value))
                {
                    self.pc = target;
                    Ok(Step::Trap)
                } else {
                    Err(format!(
                        "pc={:#010x}: {fault} (hart {})",
                        self.pc, self.csr.hart_id
                    ))
                }
            }
        }
    }

    fn andes_gp_operation(
        &mut self,
        insn: u32,
        store_value: u32,
        bus: &mut Bus,
        output: &mut impl Write,
    ) -> Result<Option<u32>, Fault> {
        // (width, store, sign extension, ADDIGP), from the custom opcode and funct bits.
        let (width, store, signed, add) = match (insn & 0x7f, (insn >> 12) & 7) {
            (0x0b, 0 | 4) => (1, false, true, false),
            (0x0b, 1 | 5) => (1, false, false, true),
            (0x0b, 2 | 6) => (1, false, false, false),
            (0x0b, 3 | 7) => (1, true, false, false),
            (0x2b, 0) => (2, true, false, false),
            (0x2b, 1) => (2, false, true, false),
            (0x2b, 2) => (4, false, false, false),
            (0x2b, 4) => (4, true, false, false),
            (0x2b, 5) => (2, false, false, false),
            _ => return Err(Fault::Illegal(insn)),
        };
        let bits = |shift: u32, mask: u32| (insn >> shift) & mask;
        let shared = (bits(17, 7) << 12) | (bits(15, 3) << 15);
        let offset = if store {
            let immediate = shared | (bits(25, 63) << 5) | (bits(7, 1) << 11);
            if width == 4 {
                sign(
                    immediate | (bits(9, 7) << 2) | (bits(8, 1) << 17) | (bits(31, 1) << 18),
                    19,
                )
            } else {
                let low_bit = if width == 1 { bits(14, 1) } else { 0 };
                sign(
                    immediate | (bits(8, 15) << 1) | (bits(31, 1) << 17) | low_bit,
                    18,
                )
            }
        } else {
            let immediate = shared | (bits(20, 1) << 11);
            if width == 4 {
                sign(
                    immediate | (bits(22, 511) << 2) | (bits(21, 1) << 17) | (bits(31, 1) << 18),
                    19,
                )
            } else {
                let low_bit = if width == 1 { bits(14, 1) } else { 0 };
                sign(
                    immediate | (bits(21, 1023) << 1) | (bits(31, 1) << 17) | low_bit,
                    18,
                )
            }
        };
        let address = self.registers[3].wrapping_add(offset);
        if add {
            return Ok(Some(address));
        }
        if store {
            self.store(bus, address, width, store_value, output)?;
            return Ok(None);
        }
        let value = match width {
            1 => self.load::<1>(bus, address)?,
            2 => self.load::<2>(bus, address)?,
            _ => self.load::<4>(bus, address)?,
        };
        Ok(Some(if signed {
            sign(value, width as u32 * 8)
        } else {
            value
        }))
    }

    fn uses_rv32e_registers(insn: u32) -> bool {
        const RD_HIGH: u32 = 1 << 11;
        const RS1_HIGH: u32 = 1 << 19;
        const RS2_HIGH: u32 = 1 << 24;

        let mask = match insn & 0x7f {
            0x17 | 0x37 | 0x6f => RD_HIGH,
            0x03 | 0x13 | 0x67 => RD_HIGH | RS1_HIGH,
            0x23 | 0x63 => RS1_HIGH | RS2_HIGH,
            0x2f | 0x33 => RD_HIGH | RS1_HIGH | RS2_HIGH,
            0x0b => {
                if (insn >> 12) & 3 == 3 {
                    RS2_HIGH
                } else {
                    RD_HIGH
                }
            }
            0x2b => match (insn >> 12) & 7 {
                0 | 4 => RS2_HIGH,
                1 | 2 | 5 => RD_HIGH,
                _ => 0,
            },
            0x5b => match (insn >> 12) & 7 {
                0 => RD_HIGH | RS1_HIGH | RS2_HIGH,
                2 | 3 => RD_HIGH | RS1_HIGH,
                5..=7 => RS1_HIGH,
                _ => 0,
            },
            0x73 => match (insn >> 12) & 7 {
                1..=3 => RD_HIGH | RS1_HIGH,
                5..=7 => RD_HIGH,
                _ => 0,
            },
            _ => 0,
        };
        insn & mask == 0
    }

    fn step(
        &mut self,
        bus: &mut Bus,
        output: &mut impl Write,
        symbols: Option<&BTreeMap<u32, String>>,
    ) -> Result<Step, Fault> {
        let pc = self.pc;
        if self.csr.address_trigger_matches(pc, crate::pmp::EXECUTE) {
            return Err(Fault::Breakpoint(pc));
        }
        let (encoded, width) = self
            .fetch(bus)
            .map_err(|error| Fault::Bus(Access::Fetch, error))?;
        if let Some(name) = symbols.and_then(|symbols| symbols.get(&pc)) {
            let message = format!("Entering function {name} at 0x{pc:X}");
            crate::monitor_log::record(1, &message);
            if bus.monitor_log_enabled(1) {
                eprintln!("{message}");
            }
        }
        let illegal = || Fault::Illegal(encoded);
        let insn = if width == 2 {
            let expanded =
                crate::compressed::expand(encoded, self.bitmanip.zcb).ok_or_else(illegal)?;
            if matches!(expanded & 0x7f, 0x07 | 0x27) {
                let enabled = match (expanded >> 12) & 7 {
                    2 => self.csr.compressed_single,
                    3 => self.csr.compressed_double,
                    _ => unreachable!(),
                };
                if !enabled {
                    return Err(illegal());
                }
            }
            expanded
        } else {
            encoded
        };
        if self.csr.embedded_base && !Self::uses_rv32e_registers(insn) {
            return Err(illegal());
        }
        let rd = ((insn >> 7) & 31) as usize;
        let a = self.registers[((insn >> 15) & 31) as usize];
        let b = self.registers[((insn >> 20) & 31) as usize];
        let funct3 = (insn >> 12) & 7;
        let funct7 = insn >> 25;
        let imm = sign(insn >> 20, 12);
        let fallthrough = pc.wrapping_add(width);
        let mut next = fallthrough;

        // Fixed Renode exposes the compressed extensions even without scalar Zbb.
        let bitmanip = BitManip {
            zbb: self.bitmanip.zbb || (width == 2 && self.bitmanip.zcb),
            ..self.bitmanip
        };
        let result = match insn & 0x7f {
            0x07 | 0x27 if bitmanip.has_vector_execution() && matches!(funct3, 0 | 5 | 6 | 7) => {
                self.rv32_vector_transfer(insn, |hart, vector| {
                    vector.transfer(insn, a, |address, width, value| {
                        if width == 8 {
                            return hart.vector_doubleword(bus, address, value);
                        }
                        if let Some(value) = value {
                            hart.store(bus, address, width, value as u32, output)?;
                            return Ok(0);
                        }
                        match width {
                            1 => hart.load::<1>(bus, address).map(u64::from),
                            2 => hart.load::<2>(bus, address).map(u64::from),
                            4 => hart.load::<4>(bus, address).map(u64::from),
                            _ => unreachable!(),
                        }
                    })
                })?;
                None
            }
            0x5b if bitmanip.xandesvpackfph && funct3 == 4 && matches!(insn >> 26, 2 | 3) => {
                if !self.csr.vectors_active() || !self.csr.floating_enabled() {
                    return Err(illegal());
                }
                let fs1 = ((insn >> 15) & 31) as usize;
                let mut scalar = self.floating.as_ref().ok_or_else(illegal)?[fs1];
                if self.csr.d_extension && scalar >> 32 != u64::from(u32::MAX) {
                    scalar = 0x7fc0_0000;
                }
                let fcsr = self.csr.fcsr;
                let flags = self.csr.vector_bootstrap.as_mut()
                    .and_then(|vector| vector.packed_half_multiply_add(insn, scalar as u32, fcsr))
                    .ok_or_else(illegal)?;
                self.csr.fcsr |= flags;
                self.csr.floating_dirty();
                self.csr.vectors_dirty();
                None
            }
            0x5b if bitmanip.xandesvdot && funct3 == 4 && matches!(insn >> 26, 4 | 5 | 7) => {
                if !self.csr.vectors_active() {
                    return Err(illegal());
                }
                self.csr.vector_bootstrap.as_mut().and_then(|vector| vector.dot_product(insn))
                    .ok_or_else(illegal)?;
                self.csr.vectors_dirty();
                None
            }
            0x5b if bitmanip.xandesvbfhcvt && insn & 0xfe0f_707f == 0x0000_405b => {
                if !self.csr.vectors_active() || !self.csr.floating_enabled() {
                    return Err(illegal());
                }
                let fcsr = self.csr.fcsr;
                let flags = self.csr.vector_bootstrap.as_mut().and_then(|vector| {
                    vector.convert_bfloat16(insn, fcsr)
                }).ok_or_else(illegal)?;
                self.csr.fcsr |= flags;
                self.csr.floating_dirty();
                self.csr.vectors_dirty();
                None
            }
            0x5b if (bitmanip.xandesvsintload && matches!(insn & 0xfdf0_707f, 0x0820_405b | 0x0830_405b))
                || (bitmanip.xandesvsinth && insn & 0xfff0_707f == 0x0a00_405b) =>
            {
                self.rv32_vector_transfer(insn, |hart, vector| {
                    vector.andes_int4_load(insn, a, |address| {
                        hart.load::<1>(bus, address).map(|byte| byte as u8)
                    })
                })?;
                None
            }
            0x07 | 0x27 | 0x43 | 0x47 | 0x4b | 0x4f | 0x53 => self
                .floating_instruction(insn, bus, output)
                .map_err(|fault| {
                    if matches!(fault, Fault::Illegal(_)) {
                        illegal()
                    } else {
                        fault
                    }
                })?,
            0x37 => Some(insn & 0xffff_f000),
            0x17 => Some(pc.wrapping_add(insn & 0xffff_f000)),
            0x6f => {
                let offset = ((insn >> 31) << 20)
                    | (insn & 0xff000)
                    | ((insn >> 9) & 0x800)
                    | ((insn >> 20) & 0x7fe);
                next = pc.wrapping_add(sign(offset, 21));
                Some(fallthrough)
            }
            0x67 if funct3 == 0 => {
                next = a.wrapping_add(imm) & !1;
                Some(fallthrough)
            }
            0x63 => {
                let taken = match funct3 {
                    0 => a == b,
                    1 => a != b,
                    4 => (a as i32) < (b as i32),
                    5 => (a as i32) >= (b as i32),
                    6 => a < b,
                    7 => a >= b,
                    _ => return Err(illegal()),
                };
                if taken {
                    let offset = ((insn >> 31) << 12)
                        | ((insn << 4) & 0x800)
                        | ((insn >> 20) & 0x7e0)
                        | ((insn >> 7) & 0x1e);
                    next = pc.wrapping_add(sign(offset, 13));
                }
                None
            }
            0x03 => {
                let address = a.wrapping_add(imm);
                Some(match funct3 {
                    0 => sign(self.load::<1>(bus, address)?, 8),
                    1 => sign(self.load::<2>(bus, address)?, 16),
                    2 => self.load::<4>(bus, address)?,
                    4 => self.load::<1>(bus, address)?,
                    5 => self.load::<2>(bus, address)?,
                    _ => return Err(illegal()),
                })
            }
            0x23 => {
                let offset = ((insn >> 25) << 5) | ((insn >> 7) & 31);
                let address = a.wrapping_add(sign(offset, 12));
                let width = match funct3 {
                    0 => 1,
                    1 => 2,
                    2 => 4,
                    _ => return Err(illegal()),
                };
                self.store(bus, address, width, b, output)?;
                None
            }
            0x13 => Some(match funct3 {
                0 => a.wrapping_add(imm),
                1 if funct7 == 0 => a << (imm & 31),
                2 => u32::from((a as i32) < (imm as i32)),
                3 => u32::from(a < imm),
                4 => a ^ imm,
                5 if funct7 == 0 => a >> (imm & 31),
                5 if funct7 == 0x20 => ((a as i32) >> (imm & 31)) as u32,
                6 => a | imm,
                7 => a & imm,
                _ => bitmanip
                    .immediate(funct3, funct7, a, imm)
                    .ok_or_else(illegal)?,
            }),
            0x33 => Some(match (funct3, funct7) {
                (_, 1) if self.csr.m_extension || (self.csr.zmmul && funct3 < 4) => {
                    multiply_divide(funct3, a, b)
                }
                (5, 7) if self.zicond => {
                    if b == 0 {
                        0
                    } else {
                        a
                    }
                }
                (7, 7) if self.zicond => {
                    if b != 0 {
                        0
                    } else {
                        a
                    }
                }
                (0, 0) => a.wrapping_add(b),
                (0, 0x20) => a.wrapping_sub(b),
                (1, 0) => a << (b & 31),
                (2, 0) => u32::from((a as i32) < (b as i32)),
                (3, 0) => u32::from(a < b),
                (4, 0) => a ^ b,
                (5, 0) => a >> (b & 31),
                (5, 0x20) => ((a as i32) >> (b & 31)) as u32,
                (6, 0) => a | b,
                (7, 0) => a & b,
                _ => bitmanip
                    .register(funct3, funct7, a, b, (insn >> 20) & 31)
                    .ok_or_else(illegal)?,
            }),
            0x2f if insn >> 27 == 5 && self.zacas => {
                let rs2 = ((insn >> 20) & 31) as usize;
                let width = match funct3 {
                    0 | 1 if self.zabha => 1 << funct3,
                    2 => 4,
                    3 if (rd | rs2) & 1 == 0 => 8,
                    _ => return Err(illegal()),
                };
                let operand = |reg| {
                    if width == 8 && reg != 0 {
                        u64::from(self.registers[reg]) | (u64::from(self.registers[reg + 1]) << 32)
                    } else {
                        u64::from(self.registers[reg])
                    }
                };
                let (expected, replacement) = (operand(rd), operand(rs2));
                let permissions = crate::pmp::READ | crate::pmp::WRITE;
                if self.csr.address_trigger_matches(a, permissions) {
                    return Err(Fault::Breakpoint(a));
                }
                let physical = self
                    .translate(bus, a, width, permissions)
                    .map_err(|error| Fault::Bus(Access::Store, error))?;
                let previous = bus
                    .atomic_compare_swap(physical, width, expected, replacement)
                    .map_err(|error| Fault::Bus(Access::Store, error.at_virtual(a)))?;
                if width == 8 && rd != 0 {
                    self.registers[rd + 1] = (previous >> 32) as u32;
                }
                Some(sign(previous as u32, (width.min(4) * 8) as u32))
            }
            0x2f if funct3 == 2 || (self.zabha && funct3 < 2) => {
                let width = 1 << funct3;
                let operand_bits = width as u32 * 8;
                let b = b & (u32::MAX >> (32 - operand_bits));
                let operation = insn >> 27;
                let rs2 = (insn >> 20) & 31;
                let (access, permissions) = match operation {
                    2 if (self.csr.a_extension || self.zalrsc) && width == 4 && rs2 == 0 => {
                        (Access::Load, crate::pmp::READ)
                    }
                    3 if (self.csr.a_extension || self.zalrsc) && width == 4 => {
                        (Access::Store, crate::pmp::WRITE)
                    }
                    0 | 1 | 4 | 8 | 12 | 16 | 20 | 24 | 28
                        if self.csr.a_extension || self.zaamo =>
                    {
                        (Access::Store, crate::pmp::READ | crate::pmp::WRITE)
                    }
                    _ => return Err(illegal()),
                };
                if self.csr.address_trigger_matches(a, permissions) {
                    return Err(Fault::Breakpoint(a));
                }
                let physical = self
                    .translate(bus, a, width, permissions)
                    .map_err(|error| Fault::Bus(access, error))?;
                let value = match operation {
                    2 if rs2 == 0 => bus
                        .load_reserved(self.slot, physical)
                        .map_err(|error| Fault::Bus(Access::Load, error.at_virtual(a))),
                    3 => bus
                        .store_conditional(self.slot, physical, b)
                        .map_err(|error| Fault::Bus(Access::Store, error.at_virtual(a))),
                    0 | 1 | 4 | 8 | 12 | 16 | 20 | 24 | 28 => bus
                        .atomic_update(physical, width, |previous| match operation {
                            0 => previous.wrapping_add(b),
                            1 => b,
                            4 => previous ^ b,
                            8 => previous | b,
                            12 => previous & b,
                            16 => (sign(previous, operand_bits) as i32)
                                .min(sign(b, operand_bits) as i32)
                                as u32,
                            20 => (sign(previous, operand_bits) as i32)
                                .max(sign(b, operand_bits) as i32)
                                as u32,
                            24 => previous.min(b),
                            28 => previous.max(b),
                            _ => unreachable!(),
                        })
                        .map_err(|error| Fault::Bus(Access::Store, error.at_virtual(a))),
                    _ => return Err(illegal()),
                };
                Some(sign(value?, operand_bits))
            }
            // Every instruction fetch reads current RAM, so FENCE.I needs no invalidation.
            0x0f if funct3 <= 1 => None,
            // Renode VexRiscv accepts the custom whole-data-cache flush.
            0x0f if insn == 0x0000_500f && self.csr.vex => None,
            0x0b | 0x2b if bitmanip.xandesperf => self.andes_gp_operation(insn, b, bus, output)?,
            0x5b if bitmanip.xandesbfhcvt && insn & 0xfe0f_707f == 0x0001_405b => {
                self.andes_bfloat_instruction(insn)?;
                None
            }
            0x5b if funct3 == 0 => Some(bitmanip.andes_register(funct7, a, b).ok_or_else(illegal)?),
            0x5b if bitmanip.xandesperf && matches!(funct3, 2 | 3) && insn & 0x8200_0000 == 0 => {
                Some(BitManip::andes_bit_field(insn, a, funct3 == 3))
            }
            0x5b if bitmanip.xandesperf && matches!(funct3, 5..=7) => {
                let immediate = (insn >> 20) & 31;
                let bit30 = (insn >> 30) & 1;
                let bit7 = (insn >> 7) & 1;
                let taken = if funct3 == 7 {
                    if bit7 != 0 {
                        return Err(illegal());
                    }
                    ((a >> immediate) & 1) == bit30
                } else {
                    let constant = (bit30 << 6) | (bit7 << 5) | immediate;
                    (a == constant) == (funct3 == 5)
                };
                if taken {
                    let offset =
                        ((insn >> 31) << 10) | ((insn >> 20) & 0x3e0) | ((insn >> 7) & 0x1e);
                    next = pc.wrapping_add(sign(offset, 11));
                }
                None
            }
            opcode @ (0x57 | 0x5b) if bitmanip.xandesvsinth
                && ((opcode == 0x57 && funct3 == 2 && insn >> 26 == 0x12 && matches!((insn >> 15) & 31, 2..=7))
                    || (opcode == 0x5b && funct3 == 4 && insn >> 26 == 0 && matches!((insn >> 15) & 31, 4..=7))) =>
            {
                let floating = opcode == 0x5b;
                if !self.csr.vectors_active() || (floating && !self.csr.floating_enabled()) {
                    return Err(illegal());
                }
                self.csr.vector_bootstrap.as_mut().and_then(|vector| vector.widen_small_integer(insn, floating))
                    .ok_or_else(illegal)?;
                self.csr.vectors_dirty();
                if floating {
                    self.csr.floating_dirty();
                }
                None
            }
            0x57 if bitmanip.xandesvqmac && insn >> 26 >= 60 => {
                if !self.csr.vectors_active() {
                    return Err(illegal());
                }
                self.csr.vector_bootstrap.as_mut().and_then(|vector| vector.quad_mac(insn, a))
                    .ok_or_else(illegal)?;
                self.csr.vectors_dirty();
                None
            }
            0x57 if !self.csr.vector_configured() => {
                let message = format!(
                    "PC: 0x{pc:x}, opcode: 0x{encoded:08x}, RISC-V 'V' instruction set is not enabled for this CPU!"
                );
                crate::monitor_log::record(3, &message);
                if bus.monitor_log_enabled(3) {
                    eprintln!("{message}");
                }
                return Err(illegal());
            }
            0x57 if funct3 == 7 => Some(self.csr.configure_vector(insn, a, b).ok_or_else(illegal)?),
            0x73 if insn == 0x0000_0073 => {
                return Ok(self.environment_trap(self.csr.ecall_cause(), Stop::Ecall));
            }
            0x73 if insn == 0x0010_0073 => return Ok(self.environment_trap(3, Stop::Ebreak)),
            0x73 if insn & 0xfe00_7fff == 0x1200_0073 && self.csr.can_sfence() => {
                self.translations.clear();
                None
            }
            0x73 if insn == 0x1050_0073 && !self.csr.in_user_mode() => {
                self.waiting_for_interrupt = !self.wfi_as_nop;
                None
            }
            0x73 if insn == 0x3020_0073 => {
                next = self.csr.return_from_trap().ok_or_else(illegal)?;
                if self.csr.clic_return_uses_vector_entry(false) {
                    next = bus.read_scalar::<4>(next, false)
                        .map_err(|error| Fault::Bus(Access::Load, error.at_virtual(next)))? & !1;
                }
                None
            }
            0x73 if insn == 0x1020_0073 => {
                next = self.csr.return_from_supervisor_trap().ok_or_else(illegal)?;
                if self.csr.clic_return_uses_vector_entry(true) {
                    next = bus.read_scalar::<4>(next, false)
                        .map_err(|error| Fault::Bus(Access::Load, error.at_virtual(next)))? & !1;
                }
                None
            }
            0x73 if matches!(funct3, 1..=3 | 5..=7) => {
                let value = if self.csr.clic_mode() && matches!(insn >> 20, 0x145 | 0x345) {
                    bus.clic.as_mut().and_then(|clic| self.csr.clic_next_interrupt(insn, a, clic))
                        .ok_or_else(illegal)?
                } else if matches!(insn >> 20, 0x351..=0x356) && let Some(clic) = bus.clic.as_mut() {
                    self.csr.clic_indirect_access(insn, a, clic).ok_or_else(illegal)?
                } else {
                    self.csr.access(insn, a).map_err(|error| Fault::InvalidCsr(insn, error))?
                };
                if insn >> 20 == crate::andes_csr::MMISC_CTL
                    && (funct3 & 3 == 1 || (insn >> 15) & 31 != 0)
                    && let Some(andes) = self.csr.andes.as_ref()
                {
                    self.unaligned_accesses = andes.unaligned_accesses();
                }
                if matches!(insn >> 20, 0x180 | 0x3a0..=0x3ef | 0x747) {
                    self.translations.clear();
                }
                Some(value)
            }
            _ => return Err(illegal()),
        };

        if next & if self.csr.c_extension { 1 } else { 3 } != 0 {
            return Err(Fault::MisalignedTarget(next));
        }
        if let Some(value) = result.filter(|_| rd != 0) {
            self.registers[rd] = value;
        }
        self.pc = next;
        Ok(Step::Retired)
    }

    fn environment_trap(&mut self, cause: u32, stop: Stop) -> Step {
        match self.csr.trap(self.pc, cause, 0) {
            Some(target) => {
                self.pc = target;
                Step::Trap
            }
            None => Step::Stop(stop),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn started_cortex_m() -> Machine {
        let mut machine = Machine::new(0x1000).unwrap();
        machine.bus.map_ram(0, 0x1000).unwrap();
        machine.bus.map_nvic(0xe0, None).unwrap();
        machine.bus.map_systick().unwrap();
        machine.primary.configure_cortex_m(Some(0), true);
        let mut output = Vec::new();
        for (address, value) in [(0, 0x800), (4, 0x101)] {
            machine.bus.write(address, 4, value, &mut output).unwrap();
        }
        // nop; nop; nop; jump: nop; b jump.
        for (address, value) in [
            (0x100, 0xbf00),
            (0x102, 0xbf00),
            (0x104, 0xbf00),
            (0x106, 0xbf00),
            (0x108, 0xe7fd),
        ] {
            machine.bus.write(address, 2, value, &mut output).unwrap();
        }
        machine
            .bus
            .cpu_hooks
            .insert(0x104, vec![crate::bus::CpuHook::Log("PC moved".into())]);
        machine.validate_entry().unwrap();
        assert_eq!(machine.run(3, &mut output).unwrap(), (Stop::Limit, 3));
        assert_eq!(output, b"PC moved\n");
        machine
    }

    fn clock_test_setup() -> (Machine, Vec<u8>) {
        let mut machine = started_cortex_m();
        let mut output = Vec::new();
        machine.bus.map_dwt(0xe0001000, 72_000_000).unwrap();
        machine
            .bus
            .systick
            .as_mut()
            .unwrap()
            .set_frequency(100_000_000);
        for (address, value) in [(0xe000e014, 0xff_ffff), (0xe000e010, 1), (0xe0001000, 1)] {
            machine.bus.write(address, 4, value, &mut output).unwrap();
        }
        (machine, output)
    }

    fn clock_test_values(machine: &mut Machine) -> (u32, u32) {
        (
            machine.bus.read::<4>(0xe000e018).unwrap(),
            machine.bus.read::<4>(0xe0001004).unwrap(),
        )
    }

    #[test]
    fn cpu_less_snapshot_rewinds_clock_and_rejects_other_uart_layouts() {
        let device = |base| {
            let mut machine = Machine::without_default_ram();
            machine.cpu_less = true;
            machine.bus.map_trivial_uart(base, "uart0").unwrap();
            machine
        };
        let mut machine = device(0x1000);
        machine.run(5, &mut std::io::sink()).unwrap();
        machine.monitor_quantum = 20;
        machine.bus.set_default_uart_timeout(200);
        let mut snapshot = Vec::new();
        machine.write_boot_snapshot(&mut snapshot).unwrap();
        machine.run(7, &mut std::io::sink()).unwrap();
        machine.bus.set_default_uart_timeout(300);
        assert_eq!(machine.elapsed_steps(), 12);

        let mut other = device(0x2000);
        assert!(other.read_boot_snapshot(&mut snapshot.as_slice()).is_err());
        assert_eq!(other.elapsed_steps(), 0);

        machine.clear_memory_snapshot().unwrap();
        assert!(machine.run(1, &mut std::io::sink()).is_err());
        machine.read_boot_snapshot(&mut snapshot.as_slice()).unwrap();
        assert_eq!(machine.elapsed_steps(), 5);
        assert_eq!(machine.monitor_quantum, 20);
        assert_eq!(machine.bus.default_uart_timeout(), 200);
        machine.run(2, &mut std::io::sink()).unwrap();
        assert_eq!(machine.elapsed_steps(), 7);
    }

    #[test]
    fn boot_snapshot_restores_registers_only_after_validating_memory() {
        let mut machine = Machine::without_default_ram();
        machine.bus.map_ram(0, 4096).unwrap();
        machine.primary.configure_cortex_m(Some(0), false);
        machine.primary.pc = 0x102;
        machine.primary.registers[0] = 42;
        machine.bus.write(0x200, 4, 7, &mut std::io::sink()).unwrap();
        let mut snapshot = Vec::new();
        machine.write_boot_snapshot(&mut snapshot).unwrap();

        machine.primary.pc = 0x104;
        machine.primary.registers[0] = 84;
        machine.bus.write(0x200, 4, 8, &mut std::io::sink()).unwrap();
        let mut truncated = &snapshot[..snapshot.len() - 1];
        assert!(machine.read_boot_snapshot(&mut truncated).is_err());
        assert_eq!(machine.primary.pc, 0x104);
        assert_eq!(machine.primary.registers[0], 84);
        assert_eq!(machine.bus.read_ram::<4>(0x200).unwrap(), 8);

        machine.clear_memory_snapshot().unwrap();
        assert_eq!(machine.bus.read_ram::<4>(0x200).unwrap(), 0);
        assert!(machine.run(1, &mut std::io::sink()).is_err());
        machine.read_boot_snapshot(&mut snapshot.as_slice()).unwrap();
        assert_eq!(machine.primary.pc, 0x102);
        assert_eq!(machine.primary.registers[0], 42);
        assert_eq!(machine.bus.read_ram::<4>(0x200).unwrap(), 7);
    }

    #[test]
    fn upstream_cortex_m_independent_systick_dwt_clocks() {
        let (mut machine, mut output) = clock_test_setup();
        machine.run(100_000, &mut output).unwrap();
        let before = clock_test_values(&mut machine);
        machine.run(100_000, &mut output).unwrap();
        let running = clock_test_values(&mut machine);
        assert_ne!(before.0, running.0);
        assert_eq!(running.1 - before.1, 72_000);

        machine.bus.systick.as_mut().unwrap().set_clocked(false);
        machine.bus.dwt.as_mut().unwrap().set_clocked(false);
        machine.run(100_000, &mut output).unwrap();
        assert_eq!(clock_test_values(&mut machine), running);

        machine.bus.systick.as_mut().unwrap().set_clocked(true);
        machine.bus.dwt.as_mut().unwrap().set_clocked(true);
        machine.run(100_000, &mut output).unwrap();
        let resumed = clock_test_values(&mut machine);
        assert_ne!(resumed.0, running.0);
        assert_eq!(resumed.1 - running.1, 72_000);

        machine.bus.systick.as_mut().unwrap().set_clocked(false);
        machine.run(100_000, &mut output).unwrap();
        let independent = clock_test_values(&mut machine);
        assert_eq!(independent.0, resumed.0);
        assert_eq!(independent.1 - resumed.1, 72_000);
    }

    #[test]
    fn upstream_cortex_m_clock_gate_persists_across_reset() {
        let (mut machine, mut output) = clock_test_setup();
        machine.run(100_000, &mut output).unwrap();
        machine.set_cpu_clocked(false);
        machine.bus.systick.as_mut().unwrap().set_clocked(false);
        machine.bus.dwt.as_mut().unwrap().set_clocked(false);
        machine
            .reset_cpu_control(CpuResetKind::Machine, &mut output)
            .unwrap();
        assert_eq!(clock_test_values(&mut machine), (0xff_ffff, 0));
        assert_eq!(machine.run(100_000, &mut output).unwrap(), (Stop::Limit, 0));
        assert_eq!(machine.primary.pc, 0x100);
        assert_eq!(clock_test_values(&mut machine), (0xff_ffff, 0));

        for (address, value) in [(0xe000e014, 0xff_ffff), (0xe000e010, 1), (0xe0001000, 1)] {
            machine.bus.write(address, 4, value, &mut output).unwrap();
        }
        machine.run(100_000, &mut output).unwrap();
        assert_eq!(clock_test_values(&mut machine), (0xff_ffff, 0));

        machine.set_cpu_clocked(true);
        machine.bus.systick.as_mut().unwrap().set_clocked(true);
        machine.bus.dwt.as_mut().unwrap().set_clocked(true);
        assert_eq!(
            machine.run(100_000, &mut output).unwrap(),
            (Stop::Limit, 100_000)
        );
        let resumed = clock_test_values(&mut machine);
        assert_ne!(resumed.0, 0xff_ffff);
        assert_eq!(resumed.1, 72_000);
    }

    fn check_cpu_flow(machine: &mut Machine, paused: bool, halted: bool) {
        assert_eq!(machine.cpu_paused, paused);
        assert_eq!(machine.cpu_halted, halted);
        let before = machine.elapsed_steps;
        let pc = machine.primary.pc;
        let (stop, retired) = machine.run(4, &mut std::io::sink()).unwrap();
        assert_eq!(machine.elapsed_steps, before + if paused { 0 } else { 4 });
        assert_eq!(stop, if paused { Stop::Paused } else { Stop::Limit });
        assert_eq!(retired, if paused || halted { 0 } else { 4 });
        if paused || halted {
            assert_eq!(machine.primary.pc, pc);
        }
    }

    #[test]
    fn upstream_cpu_control_starts() {
        check_cpu_flow(&mut started_cortex_m(), false, false);
    }

    #[test]
    fn upstream_cpu_control_pauses() {
        let mut machine = started_cortex_m();
        machine.pause_cpu();
        check_cpu_flow(&mut machine, true, false);
        machine.resume_cpu();
        check_cpu_flow(&mut machine, false, false);
    }

    #[test]
    fn upstream_cpu_control_halts() {
        let mut machine = started_cortex_m();
        machine.set_cpu_halted(true);
        check_cpu_flow(&mut machine, false, true);
        machine.resume_cpu();
        check_cpu_flow(&mut machine, false, true);
    }

    fn check_cpu_control_reset(paused: bool, halted: bool, kind: CpuResetKind) {
        let mut machine = started_cortex_m();
        if paused {
            machine.pause_cpu();
        }
        machine.set_cpu_halted(halted);
        machine.reset_log = Some("Machine resetting".into());
        let mut output = Vec::new();
        machine.reset_cpu_control(kind, &mut output).unwrap();
        assert_eq!(machine.primary.pc, 0x100);
        assert!(!machine.cpu_halted);
        match kind {
            CpuResetKind::Machine => {
                assert!(!machine.cpu_paused);
                assert_eq!(machine.run(3, &mut output).unwrap(), (Stop::Limit, 3));
                assert_eq!(output, b"Machine resetting\nPC moved\n");
                check_cpu_flow(&mut machine, false, false);
            }
            CpuResetKind::Cpu => {
                assert!(output.is_empty());
                check_cpu_flow(&mut machine, true, false);
            }
        }
    }

    #[test]
    fn upstream_cpu_reset_running_cpu() {
        check_cpu_control_reset(false, false, CpuResetKind::Cpu);
    }

    #[test]
    fn upstream_cpu_reset_running_machine() {
        check_cpu_control_reset(false, false, CpuResetKind::Machine);
    }

    #[test]
    fn upstream_cpu_reset_halted_cpu() {
        check_cpu_control_reset(false, true, CpuResetKind::Cpu);
    }

    #[test]
    fn upstream_cpu_reset_halted_machine() {
        check_cpu_control_reset(false, true, CpuResetKind::Machine);
    }

    #[test]
    fn upstream_cpu_reset_paused_cpu() {
        check_cpu_control_reset(true, false, CpuResetKind::Cpu);
    }

    #[test]
    fn upstream_cpu_reset_paused_machine() {
        check_cpu_control_reset(true, false, CpuResetKind::Machine);
    }

    #[test]
    fn upstream_cpu_reset_halted_paused_cpu() {
        check_cpu_control_reset(true, true, CpuResetKind::Cpu);
    }

    #[test]
    fn upstream_cpu_reset_halted_paused_machine() {
        check_cpu_control_reset(true, true, CpuResetKind::Machine);
    }

    fn check_pause_halt(pause_first: bool, resume_first: bool) {
        let mut machine = started_cortex_m();
        if pause_first {
            machine.pause_cpu();
            machine.set_cpu_halted(true);
        } else {
            machine.set_cpu_halted(true);
            machine.pause_cpu();
        }
        check_cpu_flow(&mut machine, true, true);
        if resume_first {
            machine.resume_cpu();
            check_cpu_flow(&mut machine, false, true);
            machine.set_cpu_halted(false);
        } else {
            machine.set_cpu_halted(false);
            check_cpu_flow(&mut machine, true, false);
            machine.resume_cpu();
        }
        check_cpu_flow(&mut machine, false, false);
    }

    #[test]
    fn upstream_cpu_control_pause_first_resume_first() {
        check_pause_halt(true, true);
    }

    #[test]
    fn upstream_cpu_control_pause_first_unhalt_first() {
        check_pause_halt(true, false);
    }

    #[test]
    fn upstream_cpu_control_halt_first_resume_first() {
        check_pause_halt(false, true);
    }

    #[test]
    fn upstream_cpu_control_halt_first_unhalt_first() {
        check_pause_halt(false, false);
    }

    #[test]
    fn upstream_invalid_reset_state_locks_on_second_step() {
        let mut machine = Machine::new(0x1000).unwrap();
        machine.bus.map_ram(0, 0x1000).unwrap();
        machine
            .bus
            .map_bus_faulting_peripheral(0x100000, None)
            .unwrap();
        machine.bus.map_nvic(0xe0, None).unwrap();
        machine.bus.map_systick().unwrap();
        machine.primary.configure_cortex_m(Some(0), true);
        machine.validate_entry().unwrap();
        let mut out = std::io::sink();

        assert_eq!(machine.run(1, &mut out).unwrap(), (Stop::Limit, 0));
        assert_eq!(machine.primary.pc, 0);
        assert_eq!(
            machine.primary.thumb.as_ref().unwrap().lockup_state(),
            (false, 0, 3)
        );
        assert!(!machine.bus.nvic.as_ref().unwrap().lockup);
        assert_eq!(machine.bus.read::<4>(0xe000_ed28).unwrap(), 1 << 17);
        assert_eq!(machine.bus.read::<4>(0xe000_ed2c).unwrap(), 1 << 30);

        assert_eq!(machine.run(1, &mut out).unwrap(), (Stop::Limit, 0));
        assert_eq!(machine.primary.pc, 0xefff_fffe);
        assert!(machine.primary.thumb.as_ref().unwrap().lockup_state().0);
        assert!(machine.bus.nvic.as_ref().unwrap().lockup);
        assert_eq!(machine.bus.read::<4>(0xe000_ed28).unwrap(), 1 << 17);
        assert_eq!(machine.bus.read::<4>(0xe000_ed2c).unwrap(), 1 << 30);
        let dhcsr = crate::nvic::DHCSR;
        assert_eq!(machine.bus.thumb_read::<4>(dhcsr).unwrap(), 0);
        assert_ne!(machine.bus.read::<4>(dhcsr).unwrap() & (1 << 19), 0);
        machine
            .bus
            .thumb_write(dhcsr, 4, u32::MAX, &mut out)
            .unwrap();
        assert_eq!(machine.bus.thumb_read::<4>(dhcsr).unwrap(), 0);
    }

    #[test]
    fn upstream_sets_and_clears_page_access_via_io() {
        const PC: u32 = 0x800000;
        const PAGES: [u32; 3] = [0x31000000, 0x32000000, 0x33000000];
        let mut machine = Machine::new(0x1000).unwrap();
        machine.bus.map_ram(PC, 0x40000).unwrap();
        machine.bus.map_nvic(0xe0, None).unwrap();
        machine.bus.map_systick().unwrap();
        machine.primary.configure_cortex_m(Some(PC), true);
        machine
            .bus
            .write(PC, 2, 0x6808, &mut std::io::sink())
            .unwrap();
        for (index, page) in PAGES.into_iter().enumerate() {
            let name = format!("test_mem{}", index + 1);
            machine.bus.map_named_ram(&name, page, 0x1000).unwrap();
            machine.bus.log_ram_access(&name).unwrap();
        }
        machine.primary.name = Some("cpu".into());
        let mut setup = Vec::new();
        machine
            .set_cortex_m_monitor_pc("cpu", PC, &mut setup)
            .unwrap();
        assert_eq!(
            String::from_utf8(setup).unwrap(),
            "cpu: Patching PC 0x800000 for Thumb mode.\n"
        );
        let phases: &[(&[(usize, bool)], u8)] = &[
            (&[], 0),
            (&[(0, true)], 1),
            (&[(1, true)], 3),
            (&[(2, true)], 7),
            (&[(0, false)], 6),
            (&[(1, false)], 4),
            (&[(2, false)], 0),
            (&[(1, true)], 2),
            (&[(0, true), (1, false), (2, true)], 5),
        ];
        for (changes, expected_pages) in phases {
            for &(index, enabled) in *changes {
                machine.bus.set_page_access_via_io(PAGES[index], enabled);
            }
            for (index, page) in PAGES.into_iter().enumerate() {
                machine.primary.registers[1] = page;
                machine
                    .set_cortex_m_monitor_pc("cpu", PC, &mut std::io::sink())
                    .unwrap();
                let mut output = Vec::new();
                assert_eq!(machine.run(1, &mut output).unwrap(), (Stop::Limit, 1));
                assert_eq!(machine.primary.registers[0], 0);
                if expected_pages & (1 << index) == 0 {
                    assert!(output.is_empty());
                } else {
                    assert_eq!(
                        String::from_utf8(output).unwrap(),
                        format!(
                            "test_mem{}: [cpu: 0x800000] ReadUInt32 from 0x0 (unknown), returned 0x0\n",
                            index + 1
                        )
                    );
                }
            }
        }
    }

    #[test]
    fn upstream_vtor_reads_privileged_vectors_without_trustzone() {
        let mut machine = Machine::new(0x1000).unwrap();
        machine.bus.map_privileged_ram(0x10000, 0x1000).unwrap();
        machine.bus.map_ram(0x20000, 0x1000).unwrap();
        machine.bus.map_nvic(0xe0, None).unwrap();
        machine.bus.map_systick().unwrap();
        machine.primary.configure_cortex_m(Some(0x10000), true);
        let mut output = std::io::sink();
        machine.bus.write(0x10000, 4, 0x20100, &mut output).unwrap();
        machine.bus.write(0x10004, 4, 0x20201, &mut output).unwrap();
        machine.bus.write(0x20200, 2, 0xe7fe, &mut output).unwrap();
        machine.validate_entry().unwrap();
        assert_eq!(machine.run(1, &mut output).unwrap(), (Stop::Limit, 1));
        assert_eq!(machine.primary.pc, 0x20200);
        assert_eq!(machine.primary.registers[13], 0x20100);

        machine.bus.thumb_unprivileged = true;
        assert!(
            machine
                .bus
                .thumb_read::<4>(0x10000)
                .unwrap_err()
                .contains("registration denied")
        );
        assert!(machine.bus.thumb_write(0x10004, 4, 0, &mut output).is_err());
        assert!(machine.bus.thumb_fetch(0x10000).is_err());
        assert_eq!(machine.bus.read::<4>(0x10004).unwrap(), 0x20201);
    }

    fn check_lazy_fp_fault(original_hard_fault: bool) {
        let mut machine = exception_vector_fault_machine(None);
        let mut out = std::io::sink();
        machine.bus.nvic.as_mut().unwrap().set_vector_base(0);
        for (address, value) in [(8, 0x381), (12, 0x341), (0xe000_ed88, 0xf00000)] {
            machine.bus.write(address, 4, value, &mut out).unwrap();
        }
        for (address, opcode) in [
            (0x340, 0xeeb0),
            (0x342, 0x0a40),
            (0x344, 0xe7fe),
            (0x380, 0xbf00),
            (0x382, 0xeeb0),
            (0x384, 0x0a40),
            (0x386, 0xe7fe),
        ] {
            machine.bus.write(address, 2, opcode, &mut out).unwrap();
        }
        if original_hard_fault {
            for (address, opcode) in [(0x200, 0x6802), (0x202, 0x2122), (0x204, 0xe7fe)] {
                machine.bus.write(address, 2, opcode, &mut out).unwrap();
            }
            machine.primary.registers[0] = 0x100000;
            machine.primary.registers[1] = 0x11;
            machine.primary.registers[2] = 0xa5a5_a5a5;
            assert_eq!(machine.run(1, &mut out).unwrap(), (Stop::Limit, 0));
            assert_eq!(machine.primary.pc, 0x340);
            machine
                .bus
                .write(0xe000_ed28, 4, u32::MAX, &mut out)
                .unwrap();
            machine
                .bus
                .write(0xe000_ed2c, 4, 1 << 30, &mut out)
                .unwrap();
        } else {
            machine.bus.write(0x200, 2, 0xeeb0, &mut out).unwrap();
            machine.bus.write(0x202, 2, 0x0a40, &mut out).unwrap();
            machine.bus.write(0x204, 2, 0xe7fe, &mut out).unwrap();
        }
        assert_eq!(machine.run(1, &mut out).unwrap(), (Stop::Limit, 1));
        machine.bus.set_mock_fault_offset(0x100000, 0x20);
        machine.primary.registers[13] = 0x100068;
        machine
            .bus
            .write(0xe000_ed04, 4, 1 << 31, &mut out)
            .unwrap();
        assert_eq!(machine.run(2, &mut out).unwrap(), (Stop::Limit, 1));
        assert_eq!(machine.primary.pc, 0x382);
        assert_eq!(machine.bus.read::<4>(0xe000_ef38).unwrap(), 0x100020);
        assert_eq!(
            machine.bus.read::<4>(0xe000_ef34).unwrap() & 0x11,
            if original_hard_fault { 1 } else { 0x11 }
        );
        machine.run(1, &mut out).unwrap();
        assert_eq!(machine.bus.read::<4>(0xe000_ed28).unwrap(), 1 << 13);
        let thumb = machine.primary.thumb.as_ref().unwrap();
        assert_eq!(thumb.lockup_state().0, original_hard_fault);
        assert_eq!(
            machine.bus.read::<4>(0xe000_ef34).unwrap() & 1,
            u32::from(original_hard_fault)
        );
        if original_hard_fault {
            assert_eq!(machine.primary.pc, 0xefff_fffe);
            assert_eq!(machine.primary.registers[13], 0x100000);
            assert_eq!(thumb.lockup_state().2, 2);
            assert!(machine.bus.nvic.as_ref().unwrap().lockup);
            let dhcsr = crate::nvic::DHCSR;
            assert_eq!(machine.bus.thumb_read::<4>(dhcsr).unwrap(), 0);
            assert_ne!(machine.bus.read::<4>(dhcsr).unwrap() & (1 << 19), 0);
            machine
                .bus
                .thumb_write(dhcsr, 4, u32::MAX, &mut out)
                .unwrap();
            assert_eq!(machine.bus.thumb_read::<4>(dhcsr).unwrap(), 0);
            assert_eq!(machine.bus.read::<4>(0xe000_ed2c).unwrap(), 0);
            assert_eq!(
                machine.bus.read::<4>(0xe000_ed24).unwrap(),
                (1 << 2) | (1 << 5)
            );
        } else {
            assert_eq!(machine.bus.read::<4>(0xe000_ed2c).unwrap(), 1 << 30);
            assert_ne!(machine.bus.read::<4>(0xe000_ed24).unwrap() & (1 << 21), 0);
        }
    }

    #[test]
    fn upstream_lazy_fp_fault_pends_hard_fault_when_ready() {
        check_lazy_fp_fault(false);
    }

    #[test]
    fn upstream_lazy_fp_fault_locks_over_hard_fault() {
        check_lazy_fp_fault(true);
    }

    fn unstack_fixture(nmi: &[u32]) -> Machine {
        let mut machine = exception_vector_fault_machine(None);
        let mut out = std::io::sink();
        machine.bus.nvic.as_mut().unwrap().set_vector_base(0);
        for (address, value) in [(8, 0x381), (12, 0x341), (20, 0x301)] {
            machine.bus.write(address, 4, value, &mut out).unwrap();
        }
        machine.bus.write(0x300, 2, 0xe7fe, &mut out).unwrap();
        for (index, &opcode) in nmi.iter().enumerate() {
            machine
                .bus
                .write(0x380 + index as u32 * 2, 2, opcode, &mut out)
                .unwrap();
        }
        machine
    }

    #[test]
    fn upstream_unstack_fault_locks_over_hard_fault() {
        let mut machine = unstack_fixture(&[0xf44f, 0x1080, 0xf380, 0x8808, 0x4770]);
        let mut out = std::io::sink();
        machine.bus.write(0x200, 2, 0x6802, &mut out).unwrap();
        machine.primary.registers[0] = 0x100000;
        machine.primary.registers[1] = 0x11;
        machine.primary.registers[2] = 0xa5a5_a5a5;
        assert_eq!(machine.run(1, &mut out).unwrap(), (Stop::Limit, 0));
        assert_eq!(machine.primary.pc, 0x340);
        machine
            .bus
            .write(0xe000_ed28, 4, u32::MAX, &mut out)
            .unwrap();
        machine
            .bus
            .write(0xe000_ed2c, 4, 1 << 30, &mut out)
            .unwrap();
        machine
            .bus
            .write(0xe000_ed04, 4, 1 << 31, &mut out)
            .unwrap();
        assert_eq!(machine.run(4, &mut out).unwrap(), (Stop::Limit, 2));
        assert_eq!(machine.primary.pc, 0xefff_fffe);
        assert_eq!(machine.primary.registers[13], 0x100020);
        assert_eq!(
            machine.primary.thumb.as_ref().unwrap().lockup_state(),
            (true, 0, 3)
        );
        assert!(machine.bus.nvic.as_ref().unwrap().lockup);
        let dhcsr = crate::nvic::DHCSR;
        assert_eq!(machine.bus.thumb_read::<4>(dhcsr).unwrap(), 0);
        assert_ne!(machine.bus.read::<4>(dhcsr).unwrap() & (1 << 19), 0);
        machine
            .bus
            .thumb_write(dhcsr, 4, u32::MAX, &mut out)
            .unwrap();
        assert_eq!(machine.bus.thumb_read::<4>(dhcsr).unwrap(), 0);
        assert_eq!(machine.bus.read::<4>(0xe000_ed28).unwrap(), 1 << 11);
        assert_eq!(machine.bus.read::<4>(0xe000_ed2c).unwrap(), 0);
        assert_eq!(machine.bus.read::<4>(0xe000_ed24).unwrap(), 4);
    }

    #[test]
    fn upstream_retpsr_integrity_failure_locks_over_hard_fault() {
        let mut machine = unstack_fixture(&[0x2000, 0x9007, 0x4770]);
        let mut out = std::io::sink();
        for (address, opcode) in [(0x200, 0x6802), (0x202, 0x2122), (0x204, 0xe7fe)] {
            machine.bus.write(address, 2, opcode, &mut out).unwrap();
        }
        machine.primary.registers[0] = 0x100000;
        machine.primary.registers[1] = 0x11;
        machine.primary.registers[2] = 0xa5a5_a5a5;
        assert_eq!(machine.run(1, &mut out).unwrap(), (Stop::Limit, 0));
        assert_eq!(machine.primary.pc, 0x340);
        machine
            .bus
            .write(0xe000_ed28, 4, u32::MAX, &mut out)
            .unwrap();
        machine
            .bus
            .write(0xe000_ed2c, 4, 1 << 30, &mut out)
            .unwrap();
        machine
            .bus
            .write(0xe000_ed04, 4, 1 << 31, &mut out)
            .unwrap();
        assert_eq!(machine.run(4, &mut out).unwrap(), (Stop::Limit, 2));
        assert_eq!(machine.primary.pc, 0xefff_fffe);
        assert_eq!(machine.primary.registers[13], 0xfe0);
        assert_eq!(
            machine.primary.thumb.as_ref().unwrap().lockup_state(),
            (true, 0, 3)
        );
        assert!(machine.bus.nvic.as_ref().unwrap().lockup);
        let dhcsr = crate::nvic::DHCSR;
        assert_eq!(machine.bus.thumb_read::<4>(dhcsr).unwrap(), 0);
        assert_ne!(machine.bus.read::<4>(dhcsr).unwrap() & (1 << 19), 0);
        machine
            .bus
            .thumb_write(dhcsr, 4, u32::MAX, &mut out)
            .unwrap();
        assert_eq!(machine.bus.thumb_read::<4>(dhcsr).unwrap(), 0);
        assert_eq!(machine.bus.read::<4>(0xe000_ed28).unwrap(), 1 << 18);
        assert_eq!(machine.bus.read::<4>(0xe000_ed2c).unwrap(), 0);
        assert_eq!(machine.bus.read::<4>(0xe000_ed24).unwrap(), 4);
    }

    fn check_unstack_tail_chain(fp: bool) {
        let mut machine = unstack_fixture(if fp {
            &[0xeeb0, 0x0a40, 0x4770]
        } else {
            &[0xbf00, 0x4770]
        });
        let mut out = std::io::sink();
        machine.bus.set_mock_fault_on_writes(0x100000, false);
        machine
            .bus
            .write(0xe000_ed24, 4, 1 << 17, &mut out)
            .unwrap();
        if fp {
            machine
                .bus
                .write(0xe000_ed88, 4, 0xf00000, &mut out)
                .unwrap();
        }
        machine.primary.registers[13] = 0x100020;
        machine
            .bus
            .write(0xe000_ed04, 4, 1 << 31, &mut out)
            .unwrap();
        assert_eq!(machine.run(2, &mut out).unwrap(), (Stop::Limit, 1));
        assert_eq!(machine.primary.pc, if fp { 0x384 } else { 0x382 });
        assert_eq!(
            machine.primary.thumb.as_ref().unwrap().fp_context_active(),
            fp
        );
        assert_eq!(machine.run(1, &mut out).unwrap(), (Stop::Limit, 0));
        assert_eq!(machine.primary.pc, 0x300);
        assert_eq!(machine.primary.registers[13], 0x100000);
        assert_eq!(machine.primary.registers[14], 0xffff_fff9);
        let thumb = machine.primary.thumb.as_ref().unwrap();
        assert_eq!(thumb.lockup_state(), (false, 0, 5));
        assert!(!thumb.fp_context_active());
        assert_eq!(machine.bus.read::<4>(0xe000_ed28).unwrap(), 1 << 11);
        assert_eq!(machine.bus.read::<4>(0xe000_ed2c).unwrap(), 0);
    }

    #[test]
    fn upstream_unstack_fault_tail_chains_to_bus_fault() {
        check_unstack_tail_chain(false);
    }

    #[test]
    fn upstream_unstack_fault_tail_chain_clears_fpca() {
        check_unstack_tail_chain(true);
    }

    fn check_nmi_entry_lockup(vector_fault: bool) {
        let mut machine = exception_vector_fault_machine(None);
        let mut out = std::io::sink();
        machine.bus.nvic.as_mut().unwrap().set_vector_base(0);
        for (address, value) in [(8, 0x381), (12, 0x341)] {
            machine.bus.write(address, 4, value, &mut out).unwrap();
        }
        for (address, opcode) in [
            (0x200, 0x6802),
            (0x202, 0x2122),
            (0x204, 0xe7fe),
            (0x340, 0x2d00),
            (0x342, 0xbf04),
            (0x344, 0x2101),
            (0x346, 0x2202),
            (0x348, 0xe7fe),
            (0x380, 0xe7fe),
        ] {
            machine.bus.write(address, 2, opcode, &mut out).unwrap();
        }
        machine.primary.registers[0] = 0x100000;
        machine.primary.registers[1] = 0x11;
        machine.primary.registers[2] = 0xa5a5_a5a5;
        assert_eq!(machine.run(1, &mut out).unwrap(), (Stop::Limit, 0));
        assert_eq!(machine.primary.pc, 0x340);
        machine
            .bus
            .write(0xe000_ed28, 4, u32::MAX, &mut out)
            .unwrap();
        machine
            .bus
            .write(0xe000_ed2c, 4, 1 << 30, &mut out)
            .unwrap();
        if vector_fault {
            machine.bus.nvic.as_mut().unwrap().set_vector_base(0x100000);
        } else {
            assert_eq!(machine.run(2, &mut out).unwrap(), (Stop::Limit, 2));
            assert_ne!(machine.primary.thumb.as_ref().unwrap().lockup_state().1, 0);
        }
        machine.primary.registers[13] = 0x100020;
        machine
            .bus
            .write(0xe000_ed04, 4, 1 << 31, &mut out)
            .unwrap();
        assert_eq!(machine.run(1, &mut out).unwrap(), (Stop::Limit, 0));
        assert_eq!(machine.primary.pc, 0xefff_fffe);
        assert_eq!(machine.primary.registers[13], 0x100000);
        assert_eq!(
            machine.primary.thumb.as_ref().unwrap().lockup_state(),
            (true, 0, 2)
        );
        assert!(machine.bus.nvic.as_ref().unwrap().lockup);
        assert_eq!(machine.bus.read::<4>(0xe000_ed28).unwrap(), 1 << 12);
        assert_eq!(
            machine.bus.read::<4>(0xe000_ed2c).unwrap(),
            if vector_fault { 2 } else { 0 }
        );
        assert_eq!(
            machine.bus.read::<4>(0xe000_ed24).unwrap(),
            (1 << 2) | (1 << 5)
        );
        let dhcsr = crate::nvic::DHCSR;
        assert_eq!(machine.bus.thumb_read::<4>(dhcsr).unwrap(), 0);
        assert_ne!(machine.bus.read::<4>(dhcsr).unwrap() & (1 << 19), 0);
        machine
            .bus
            .thumb_write(dhcsr, 4, u32::MAX, &mut out)
            .unwrap();
        assert_eq!(machine.bus.thumb_read::<4>(dhcsr).unwrap(), 0);
    }

    #[test]
    fn upstream_nmi_stacking_fault_locks_over_hard_fault() {
        check_nmi_entry_lockup(false);
    }

    #[test]
    fn upstream_nmi_stacking_lockup_records_vector_fault() {
        check_nmi_entry_lockup(true);
    }

    fn check_upstream_stacking_bus_fault(exception: u8, bus_fault_enabled: bool, handler: u8) {
        let mut machine = Machine::new(0x1000).unwrap();
        machine.bus.map_ram(0, 0x1000).unwrap();
        machine
            .bus
            .map_bus_faulting_peripheral(0x100000, None)
            .unwrap();
        machine.bus.map_nvic(0xe0, None).unwrap();
        machine.bus.map_systick().unwrap();
        machine.primary.configure_cortex_m(Some(0), true);
        let mut out = std::io::sink();
        for (address, value) in [
            (0, 0x100020),
            (4, 0x201),
            (8, 0x381),
            (12, 0x341),
            (20, 0x301),
            (64, 0x3c1),
        ] {
            machine.bus.write(address, 4, value, &mut out).unwrap();
        }
        for address in [0x200, 0x300, 0x340, 0x380, 0x3c0] {
            machine.bus.write(address, 2, 0xe7fe, &mut out).unwrap();
        }
        machine.validate_entry().unwrap();
        if bus_fault_enabled {
            machine
                .bus
                .write(0xe000_ed24, 4, 1 << 17, &mut out)
                .unwrap();
        }
        assert_eq!(machine.run(1, &mut out).unwrap(), (Stop::Limit, 1));
        assert_eq!(machine.primary.pc, 0x200);
        if exception == 2 {
            machine
                .bus
                .write(0xe000_ed04, 4, 1 << 31, &mut out)
                .unwrap();
        } else {
            machine.bus.write(0xe000_e100, 4, 1, &mut out).unwrap();
            machine.bus.write(0xe000_e200, 4, 1, &mut out).unwrap();
        }
        assert_eq!(machine.run(1, &mut out).unwrap(), (Stop::Limit, 0));
        assert_eq!(
            machine.primary.pc,
            match handler {
                2 => 0x380,
                3 => 0x340,
                5 => 0x300,
                _ => unreachable!(),
            }
        );
        assert_eq!(machine.primary.registers[13], 0x100000);
        assert_eq!(
            machine.primary.thumb.as_ref().unwrap().lockup_state(),
            (false, 0, handler)
        );
        assert_eq!(machine.bus.read::<4>(0xe000_ed28).unwrap(), 1 << 12);
        assert_eq!(
            machine.bus.read::<4>(0xe000_ed2c).unwrap(),
            if bus_fault_enabled { 0 } else { 1 << 30 }
        );
        if exception == 2 {
            assert_ne!(
                machine.bus.nvic.as_ref().unwrap().hardfault_pending() as u32 * (1 << 3),
                0
            );
        } else {
            assert_eq!(machine.bus.read::<4>(0xe000_e200).unwrap() & 1, 1);
            assert_eq!(machine.bus.read::<4>(0xe000_e300).unwrap() & 1, 0);
        }
    }

    #[test]
    fn upstream_nmi_stacking_fault_pends_hard_fault() {
        check_upstream_stacking_bus_fault(2, false, 2);
    }

    #[test]
    fn upstream_external_irq_stacking_fault_selects_hard_fault() {
        check_upstream_stacking_bus_fault(16, false, 3);
    }

    #[test]
    fn upstream_equal_priority_stacking_fault_selects_bus_fault() {
        check_upstream_stacking_bus_fault(16, true, 5);
    }

    fn exception_vector_fault_machine(offset: Option<usize>) -> Machine {
        let mut machine = Machine::new(0x1000).unwrap();
        machine.bus.map_ram(0, 0x1000).unwrap();
        machine
            .bus
            .map_bus_faulting_peripheral(0x100000, offset)
            .unwrap();
        machine.bus.map_nvic(0xe0, None).unwrap();
        machine.bus.map_systick().unwrap();
        machine.primary.configure_cortex_m(Some(0), true);
        let mut out = std::io::sink();
        for (address, value) in [(0, 0x1000), (4, 0x201)] {
            machine.bus.write(address, 4, value, &mut out).unwrap();
        }
        for address in [0x200, 0x340] {
            machine.bus.write(address, 2, 0xe7fe, &mut out).unwrap();
        }
        machine.validate_entry().unwrap();
        assert_eq!(machine.run(1, &mut out).unwrap(), (Stop::Limit, 1));
        machine.bus.nvic.as_mut().unwrap().set_vector_base(0x100000);
        machine
    }

    #[test]
    fn upstream_external_irq_vector_bus_fault_selects_hard_fault() {
        let mut machine = exception_vector_fault_machine(Some(0x40));
        let mut out = std::io::sink();
        machine.bus.write(0x10000c, 4, 0x341, &mut out).unwrap();
        machine.bus.write(0xe000_e100, 4, 1, &mut out).unwrap();
        machine.bus.write(0xe000_e200, 4, 1, &mut out).unwrap();
        assert_eq!(machine.run(1, &mut out).unwrap(), (Stop::Limit, 0));
        assert_eq!(machine.primary.pc, 0x340);
        assert_eq!(machine.primary.registers[13], 0xfe0);
        assert_eq!(
            machine.primary.thumb.as_ref().unwrap().lockup_state(),
            (false, 0, 3)
        );
        assert_eq!(machine.bus.read::<4>(0xe000_ed28).unwrap(), 0);
        assert_eq!(machine.bus.read::<4>(0xe000_ed2c).unwrap(), 2);
        assert_eq!(machine.bus.read::<4>(0xe000_e200).unwrap() & 1, 1);
        assert_eq!(machine.bus.read::<4>(0xe000_e300).unwrap() & 1, 0);
    }

    #[test]
    fn upstream_nmi_vector_bus_fault_locks_up() {
        let mut machine = exception_vector_fault_machine(None);
        let mut out = std::io::sink();
        machine
            .bus
            .write(0xe000_ed04, 4, 1 << 31, &mut out)
            .unwrap();
        assert_eq!(machine.run(1, &mut out).unwrap(), (Stop::Limit, 0));
        assert_eq!(machine.primary.pc, 0xefff_fffe);
        assert_eq!(machine.primary.registers[13], 0xfe0);
        assert_eq!(machine.primary.registers[14], 0xffff_fff9); // Armv7-M thread/MSP.
        assert_eq!(
            machine.primary.thumb.as_ref().unwrap().lockup_state(),
            (true, 0, 2)
        );
        assert!(machine.bus.nvic.as_ref().unwrap().lockup);
        assert_eq!(machine.bus.read::<4>(0xe000_ed28).unwrap(), 0);
        assert_eq!(machine.bus.read::<4>(0xe000_ed2c).unwrap(), 2);
        assert_eq!(machine.bus.read::<4>(0xe000_ed24).unwrap(), 1 << 5);
        let dhcsr = crate::nvic::DHCSR;
        assert_eq!(machine.bus.thumb_read::<4>(dhcsr).unwrap(), 0);
        assert_ne!(machine.bus.read::<4>(dhcsr).unwrap() & (1 << 19), 0);
        machine
            .bus
            .thumb_write(dhcsr, 4, u32::MAX, &mut out)
            .unwrap();
        assert_eq!(machine.bus.thumb_read::<4>(dhcsr).unwrap(), 0);
        machine.bus.write(0xe000_ed2c, 4, 2, &mut out).unwrap();
        assert_eq!(machine.bus.read::<4>(0xe000_ed2c).unwrap(), 0);
    }

    #[test]
    fn upstream_hard_fault_vector_bus_fault_locks_up() {
        let mut machine = exception_vector_fault_machine(None);
        let mut out = std::io::sink();
        machine
            .bus
            .nvic
            .as_mut()
            .unwrap()
            .set_hard_fault_pending(true);
        assert_eq!(machine.run(1, &mut out).unwrap(), (Stop::Limit, 0));
        assert_eq!(machine.primary.pc, 0xefff_fffe);
        assert_eq!(machine.primary.registers[13], 0xfe0);
        assert_eq!(machine.primary.registers[14], 0xffff_fff9); // Armv7-M thread/MSP.
        assert_eq!(
            machine.primary.thumb.as_ref().unwrap().lockup_state(),
            (true, 0, 3)
        );
        let nvic = machine.bus.nvic.as_ref().unwrap();
        assert!(nvic.lockup);
        assert_eq!(nvic.hardfault_pending(), false);
        assert_eq!(machine.bus.read::<4>(0xe000_ed28).unwrap(), 0);
        assert_eq!(machine.bus.read::<4>(0xe000_ed2c).unwrap(), 2);
        assert_eq!(machine.bus.read::<4>(0xe000_ed24).unwrap(), 1 << 2);
        let dhcsr = crate::nvic::DHCSR;
        assert_eq!(machine.bus.thumb_read::<4>(dhcsr).unwrap(), 0);
        assert_ne!(machine.bus.read::<4>(dhcsr).unwrap() & (1 << 19), 0);
        machine
            .bus
            .thumb_write(dhcsr, 4, u32::MAX, &mut out)
            .unwrap();
        assert_eq!(machine.bus.thumb_read::<4>(dhcsr).unwrap(), 0);
    }

    fn check_reset_bus_fault(offset: usize) {
        let mut machine = Machine::new(0x1000).unwrap();
        machine
            .bus
            .map_bus_faulting_peripheral(0x100000, Some(offset))
            .unwrap();
        machine.bus.map_nvic(0xe0, None).unwrap();
        machine.bus.map_systick().unwrap();
        machine.primary.configure_cortex_m(Some(0x100000), true);
        machine.validate_entry().unwrap();
        let mut out = std::io::sink();
        assert_eq!(machine.run(100, &mut out).unwrap(), (Stop::Limit, 0));
        assert_eq!(machine.primary.pc, 0xefff_fffe);
        assert_eq!(machine.primary.registers[13], 0);
        assert_eq!(machine.primary.registers[14], u32::MAX);
        assert_eq!(
            machine.primary.thumb.as_ref().unwrap().lockup_state(),
            (true, 0, 0)
        );
        assert!(machine.bus.nvic.as_ref().unwrap().lockup);
        assert_eq!(machine.bus.read::<4>(0xe000_ed28).unwrap(), 0);
        assert_eq!(machine.bus.read::<4>(0xe000_ed2c).unwrap(), 2);
        assert_eq!(machine.bus.read::<4>(0xe000_ed24).unwrap(), 4);
        let dhcsr = crate::nvic::DHCSR;
        assert_eq!(machine.bus.thumb_read::<4>(dhcsr).unwrap(), 0);
        assert_ne!(machine.bus.read::<4>(dhcsr).unwrap() & (1 << 19), 0);
        machine
            .bus
            .thumb_write(dhcsr, 4, u32::MAX, &mut out)
            .unwrap();
        assert_eq!(machine.bus.thumb_read::<4>(dhcsr).unwrap(), 0);
        assert_eq!(machine.bus.read::<4>(dhcsr).unwrap(), 1 << 19);
    }

    #[test]
    fn upstream_reset_vector_bus_fault_locks_up() {
        check_reset_bus_fault(4);
    }

    #[test]
    fn upstream_initial_msp_bus_fault_locks_up() {
        check_reset_bus_fault(0);
    }

    fn check_monitor_step_bus_fault(enable_bus_fault: bool) {
        let mut machine = started_cortex_m();
        let mut out = std::io::sink();
        machine
            .bus
            .map_bus_faulting_peripheral(0x100000, None)
            .unwrap();
        for (address, value) in [
            (0, 0x1000),
            (4, 0x201),
            (12, 0x341),
            (20, 0x301),
            (0xe000_ed24, u32::from(enable_bus_fault) << 17),
        ] {
            machine.bus.write(address, 4, value, &mut out).unwrap();
        }
        for base in [0x200, 0x300] {
            for (offset, word) in [(0, 0x6802), (2, 0x2122), (4, 0xe7fe)] {
                machine.bus.write(base + offset, 2, word, &mut out).unwrap();
            }
        }
        machine.bus.write(0x340, 2, 0xe7fe, &mut out).unwrap();
        machine.validate_entry().unwrap();
        machine.primary.registers[0] = 0x100000;
        machine.primary.registers[1] = 0x11;
        machine.primary.registers[2] = 0xa5a5_a5a5;
        machine.monitor_step_cortex_m(1, &mut out).unwrap();
        assert_eq!(machine.primary.pc, 0x340);
        assert_eq!(machine.primary.registers[1], 0x11);
        assert_eq!(machine.primary.registers[2], 0xa5a5_a5a5);
        let (stacked_pc_address, stacked_pc) = if enable_bus_fault {
            (0xfd8, 0x300)
        } else {
            (0xff8, 0x200)
        };
        assert_eq!(
            machine.bus.read::<4>(stacked_pc_address).unwrap(),
            stacked_pc
        );
        assert_eq!(machine.bus.read::<4>(0xe000_ed28).unwrap(), 0x8200);
        assert_eq!(machine.bus.read::<4>(0xe000_ed38).unwrap(), 0x100000);
        assert_eq!(machine.bus.read::<4>(0xe000_ed2c).unwrap(), 0x4000_0000);
        assert!(machine.cpu_paused);
    }

    #[test]
    fn upstream_monitor_step_escalates_nested_bus_fault() {
        check_monitor_step_bus_fault(true);
    }

    #[test]
    fn upstream_monitor_step_escalates_disabled_bus_fault() {
        check_monitor_step_bus_fault(false);
    }

    fn enter_instruction_time_lockup() -> Machine {
        let mut machine = Machine::new(0x1000).unwrap();
        machine.bus.map_ram(0, 0x1000).unwrap();
        machine
            .bus
            .map_bus_faulting_peripheral(0x100000, None)
            .unwrap();
        machine.bus.map_nvic(0xe0, None).unwrap();
        machine.bus.map_systick().unwrap();
        machine.primary.configure_cortex_m(Some(0), true);
        let mut out = std::io::sink();
        for (address, value) in [
            (0, 0x1000),
            (4, 0x201),
            (8, 0x381),
            (12, 0x341),
            (0x354, 0xe000_ed2c),
        ] {
            machine.bus.write(address, 4, value, &mut out).unwrap();
        }
        // The same READ_ASSEMBLY and LOCKUP_ASSEMBLY as the upstream Robot.
        for (base, words) in [
            (0x200, &[0x6802, 0x2122, 0xe7fe][..]),
            (
                0x340,
                &[
                    0x4b04, 0xf04f, 0x4480, 0x601c, 0x2d00, 0xbf04, 0x6802, 0x2122, 0xe7fe,
                ][..],
            ),
            (0x380, &[0xe7fe][..]),
        ] {
            for (index, &word) in words.iter().enumerate() {
                machine
                    .bus
                    .write(base + index as u32 * 2, 2, word, &mut out)
                    .unwrap();
            }
        }
        machine.primary.reset_cortex_m(&mut machine.bus).unwrap();
        machine.primary.registers[0] = 0x100000;
        machine.primary.registers[1] = 0x11;
        machine.primary.registers[2] = 0xa5a5_a5a5;
        assert_eq!(machine.run(20, &mut out).unwrap(), (Stop::Limit, 5));
        machine
    }

    #[test]
    fn upstream_double_bus_fault_locks_until_reset() {
        let mut machine = enter_instruction_time_lockup();
        let mut out = std::io::sink();
        let thumb = machine.primary.thumb.as_ref().unwrap();
        let state = thumb.lockup_state();
        assert!(state.0);
        assert_ne!(state.1, 0);
        assert!(!thumb.sleeping());
        assert!(machine.bus.nvic.as_ref().unwrap().lockup);
        assert_eq!(machine.primary.pc, 0xefff_fffe);
        assert_eq!(machine.primary.registers[1], 0x11);
        assert_eq!(machine.primary.registers[2], 0xa5a5_a5a5);
        for (address, expected) in [
            (0xe000_ed28, 0x8200),
            (0xe000_ed38, 0x100000),
            (0xe000_ed2c, 0),
            (0xe000_ed24, 4),
        ] {
            assert_eq!(machine.bus.read::<4>(address).unwrap(), expected);
        }
        assert_eq!(machine.run(10, &mut out).unwrap(), (Stop::Limit, 0));
        assert_eq!(machine.primary.pc, 0xefff_fffe);
        assert_eq!(
            machine.primary.thumb.as_ref().unwrap().lockup_state(),
            state
        );
        machine.primary.reset_cortex_m(&mut machine.bus).unwrap();
        assert!(!machine.primary.thumb.as_ref().unwrap().lockup_state().0);
        assert!(!machine.bus.nvic.as_ref().unwrap().lockup);
    }

    #[test]
    fn upstream_nmi_preempts_instruction_time_lockup() {
        let mut machine = enter_instruction_time_lockup();
        let mut out = std::io::sink();
        assert!(machine.bus.nvic.as_ref().unwrap().lockup);
        machine
            .bus
            .write(0xe000_ed04, 4, 1 << 31, &mut out)
            .unwrap();
        assert_eq!(machine.run(1, &mut out).unwrap(), (Stop::Limit, 0));
        assert_eq!(machine.primary.pc, 0x380);
        assert_eq!(machine.bus.read::<4>(0xfd8).unwrap(), 0xefff_fffe);
        assert!(!machine.primary.thumb.as_ref().unwrap().lockup_state().0);
        assert!(!machine.bus.nvic.as_ref().unwrap().lockup);
    }

    #[test]
    fn renode_precise_load_store_fault_without_single_step() {
        for opcode in [0x6803, 0x6003] {
            // ldr/str r3, [r0]
            let mut machine = Machine::without_default_ram();
            let mut output = std::io::sink();
            machine.bus.map_ram(0, 0x1000).unwrap();
            machine.bus.map_ram(0x100000, 0x1000).unwrap();
            machine.bus.faulting_device = Some(0x100000..=0x100fff);
            machine.primary.configure_cortex_m(Some(0), false);
            for (addr, value) in [(0, 0x1000), (4, 0x101), (12, 0x201), (0x114, 0x100000)] {
                machine.bus.write(addr, 4, value, &mut output).unwrap();
            }
            for (addr, opcode) in [
                (0x100, 0x4804), // ldr r0, =0x100000
                (0x102, 0xa101), // adr r1, fault_instr
                (0x104, 0x468a), // mov r10, r1
                (0x106, 0x2355), // movs r3, #0x55
                (0x108, opcode),
                (0x10a, 0x2100), // movs r1, #0
                (0x10c, 0x468a), // mov r10, r1
                (0x10e, 0xbf30), // wfi
                (0x110, 0xe7fd), // b 0x10e
                (0x200, 0x9906), // ldr r1, [sp, #24]
                (0x202, 0x468b), // mov r11, r1
                (0x204, 0xbf30), // wfi
                (0x206, 0xe7fd), // b 0x204
            ] {
                machine.bus.write(addr, 2, opcode, &mut output).unwrap();
            }
            machine.validate_entry().unwrap();
            machine.run(100, &mut output).unwrap();
            assert_eq!(machine.primary.registers[10], 0x108);
            assert_eq!(machine.primary.registers[11], 0x108);
            assert_eq!(machine.primary.registers[13], 0xfe0);
            assert_eq!(machine.primary.pc, 0x206);
        }
    }

    fn cpuwait_fixture(asserted: bool) -> Machine {
        let mut machine = Machine::without_default_ram();
        machine.primary.name = Some("cpu".into());
        machine.primary.configure_cortex_m(Some(0x80), false);
        machine.bus.map_ram(0, 0x1000).unwrap();
        machine.bus.map_nvic(0xc0, None).unwrap();
        let mut output = std::io::sink();
        for (address, width, value) in [
            (0x80, 4, 0x800),
            (0x84, 4, 0x101),
            (0x100, 2, 0x2004), // movs r0, #4
            (0x102, 2, 0x6801), // ldr r1, [r0]
            (0x104, 2, 0x3101), // adds r1, #1
            (0x106, 2, 0x6001), // str r1, [r0]
            (0x108, 2, 0xe7fb), // b 0x102
        ] {
            machine
                .bus
                .write(address, width, value, &mut output)
                .unwrap();
        }
        machine.set_cortex_m_cpu_wait("cpu", asserted).unwrap();
        machine.validate_entry().unwrap();
        machine
    }

    #[test]
    fn renode_cpuwait_latches_only_at_reset_and_releases_at_sync_point() {
        let mut machine = cpuwait_fixture(false);
        let mut output = std::io::sink();
        let interval = crate::bus::CORTEX_M_STEP_HZ / 10;
        machine.run(interval, &mut output).unwrap();
        let before = machine.bus.read::<4>(4).unwrap();
        machine.set_cortex_m_cpu_wait("cpu", true).unwrap();
        machine.run(interval, &mut output).unwrap();
        let after = machine.bus.read::<4>(4).unwrap();
        assert!(after > before);
        machine.reset_named_cortex_m("cpu").unwrap();
        assert!(machine.primary.cpu_wait_asserted);
        assert!(machine.primary.reset_held);
        machine.run(interval, &mut output).unwrap();
        assert_eq!(machine.bus.read::<4>(4).unwrap(), after);
        assert_eq!((machine.primary.pc, machine.primary.registers[13]), (0, 0));

        // Change the vectors while held to prove that release reads them
        // then, without retiring the first handler instruction yet.
        machine.bus.write(0x80, 4, 0x900, &mut output).unwrap();
        machine.bus.write(0x84, 4, 0x103, &mut output).unwrap();
        machine.set_cortex_m_cpu_wait("cpu", false).unwrap();
        assert!(machine.primary.reset_held);
        assert_eq!(machine.run(1, &mut output).unwrap(), (Stop::Limit, 0));
        assert!(!machine.primary.reset_held);
        assert_eq!(
            (machine.primary.pc, machine.primary.registers[13]),
            (0x102, 0x900)
        );
    }

    #[test]
    fn renode_initial_cpuwait_prevents_execution_after_machine_reset() {
        let mut machine = cpuwait_fixture(true);
        machine.reset_cortex_m_machine().unwrap();
        machine.set_named_entry("cpu", 0xdead_beef).unwrap();
        assert_eq!(
            machine.run(100, &mut std::io::sink()).unwrap(),
            (Stop::Limit, 0)
        );
        assert!(!machine.cortex_m_locked_up());
        assert!(machine.primary.reset_held);
        assert_eq!(machine.primary.pc, 0xdead_beef);
    }

    #[test]
    fn renode_cpuwait_releases_with_an_explicit_pc() {
        let mut machine = cpuwait_fixture(false);
        machine.set_cortex_m_cpu_wait("cpu", true).unwrap();
        machine.reset_named_cortex_m("cpu").unwrap();
        machine.set_named_entry("cpu", 0x100).unwrap();
        assert!(machine.primary.reset_held);
        machine.set_cortex_m_cpu_wait("cpu", false).unwrap();
        assert_eq!(
            machine.run(1, &mut std::io::sink()).unwrap(),
            (Stop::Limit, 0)
        );
        assert!(!machine.primary.reset_held);
        assert_eq!(
            (machine.primary.pc, machine.primary.registers[13]),
            (0x100, 0)
        );
    }

    #[test]
    fn stm32_usart_frame_and_idle_irq_progress_while_cortex_m_sleeps() {
        use crate::stm32_uart::{Stm32Irq, Stm32Usart};

        let mut machine = Machine::without_default_ram();
        let mut output = std::io::sink();
        let base = 0x4001_3800;
        let ispr = 0xe000_e200;
        let irq = 27;
        let mut uart = Stm32Usart::new("usart1".into(), base, Some(Stm32Irq::Nvic(irq)), false);
        uart.frequency = Some(16_000_000);
        machine.bus.map_stm32_usart(uart).unwrap();
        machine.bus.map_nvic(0xc0, None).unwrap();
        machine.bus.map_ram(0, 0x1000).unwrap();
        machine.bus.write(0, 2, 0xbf30, &mut output).unwrap(); // WFI
        machine.primary.pc = 0;
        machine.primary.configure_cortex_m(None, false);
        machine.bus.write(base, 4, 0x15, &mut output).unwrap(); // UE | RE | IDLEIE
        machine
            .bus
            .write(base + 0x0c, 4, 0x10, &mut output)
            .unwrap();
        machine
            .bus
            .write_stm32_uart_line("sysbus.usart1", "A", b"")
            .unwrap();

        assert_eq!(machine.run(900, &mut output).unwrap(), (Stop::Limit, 1));
        assert_eq!(machine.bus.read::<4>(base + 0x1c).unwrap() & 0x30, 0);
        assert_eq!(machine.run(100, &mut output).unwrap(), (Stop::Limit, 0));
        assert_eq!(machine.bus.read::<4>(base + 0x1c).unwrap() & 0x30, 0x20);
        assert_eq!(machine.bus.read::<4>(base + 0x24).unwrap(), u32::from(b'A'));
        assert_eq!(machine.run(900, &mut output).unwrap(), (Stop::Limit, 0));
        assert_eq!(machine.bus.read::<4>(ispr).unwrap(), 0);
        machine.run(100, &mut output).unwrap();
        assert_eq!(machine.bus.read::<4>(ispr).unwrap(), 1 << irq);
    }

    #[test]
    fn renode_cortex_m_reset_observation_via_sysresetreq() {
        // cpu-pause-halt.robot: adapt the Cortex-M reset observation to WFI/AIRCR.
        const AIRCR: u32 = 0xe000_ed0c;
        const ISER: u32 = 0xe000_e100;
        const ISPR: u32 = 0xe000_e200;
        const IPR: u32 = 0xe000_e400;
        const CSR: u32 = 0xe000_e010;
        const RVR: u32 = 0xe000_e014;
        let mut machine = Machine::without_default_ram();
        machine.bus.map_ram(0, 0x400).unwrap();
        machine.bus.map_systick().unwrap();
        machine.bus.map_nvic(0xc0, None).unwrap();
        machine.primary.configure_cortex_m(Some(0), false);
        let mut output = std::io::sink();
        for (address, width, value) in [
            (0, 4, 0x400),
            (4, 4, 0x21),
            (0x20, 2, 0xbf30), // wfi
            (0x22, 2, 0xbe00), // bkpt
            (0x80, 4, 0xdead_beef),
        ] {
            machine
                .bus
                .write(address, width, value, &mut output)
                .unwrap();
        }
        machine.validate_entry().unwrap();
        assert_eq!(machine.run(1, &mut output).unwrap(), (Stop::Limit, 1));
        machine.primary.registers[0] = 0x42;
        machine.bus.write(AIRCR, 4, 4, &mut output).unwrap(); // incorrect VECTKEY
        assert_eq!(machine.run(1, &mut output).unwrap(), (Stop::Limit, 0));
        assert_eq!(machine.primary.registers[0], 0x42);

        for (address, value) in [
            (ISER, 1),
            (ISPR, 1),
            (IPR, 0xff),
            (crate::nvic::VTOR, 0x100),
            (RVR, 5),
            (CSR, 3),
            (AIRCR, 0x05fa_0004),
        ] {
            machine.bus.write(address, 4, value, &mut output).unwrap();
        }
        assert_eq!(machine.bus.read::<4>(IPR).unwrap(), 0xc0);
        assert_eq!(machine.run(1, &mut output).unwrap(), (Stop::Limit, 1));
        assert_eq!(
            (machine.primary.pc, machine.primary.registers[13]),
            (0x22, 0x400)
        );
        assert_eq!(machine.primary.registers[0], 0);
        for address in [ISER, ISPR, IPR, crate::nvic::VTOR, RVR] {
            assert_eq!(machine.bus.read::<4>(address).unwrap(), 0, "{address:#x}");
        }
        assert_eq!(machine.bus.read::<4>(CSR).unwrap(), 4);
        assert_eq!(machine.bus.read_ram::<4>(0x80).unwrap(), 0xdead_beef);
    }

    fn upstream_rv32_writes_invalidate_other_harts_reservations() {
        // riscv-lr-sc-strong-atomicity.robot runs only the two selected RV32 harts.
        let directory =
            std::env::temp_dir().join(format!("penv-rv32-lr-sc-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let repl = directory.join("cpu.repl");
        std::fs::write(
            &repl,
            r#"dram: Memory.MappedMemory @ sysbus 0x80000000 { size: 0x80000000 }
mmio: Memory.ArrayMemory @ sysbus 0x100000 { size: 0x10000 }
cpu32_0: CPU.RiscV32 @ sysbus
    cpuType: "rv32gc_zicsr_zifencei"
    hartId: 2
    privilegedArchitecture: PrivilegedArchitecture.Priv1_10
    timeProvider: empty
    allowUnalignedAccesses: true
cpu32_1: CPU.RiscV32 @ sysbus
    cpuType: "rv32gc_zicsr_zifencei"
    hartId: 3
    privilegedArchitecture: PrivilegedArchitecture.Priv1_10
    timeProvider: empty
    allowUnalignedAccesses: true
"#,
        )
        .unwrap();
        let mut machine = crate::platform::load(&repl, None, false).unwrap();
        const CORE0: u32 = 0x8000_0000;
        const CORE1: u32 = 0x8000_0100;
        const NEW: u32 = 0xbeee_eeef;
        let mut output = std::io::sink();
        for address in [0x8100_0000, 0x0010_0000] {
            for (name, writer) in [
                ("lr.w/sc.w", &[0x1005_282fu32, 0x18e5_292f][..]),
                ("amoswap.w", &[0x08e5_292f]),
                ("amoadd.w", &[0x0005_272f]),
                ("sb", &[0x00e5_0023]),
                ("sh", &[0x00e5_1023]),
                ("sw", &[0x00e5_2023]),
            ] {
                machine.monitor_pc("cpu32_0", Some(CORE0)).unwrap();
                machine.monitor_pc("cpu32_1", Some(CORE1)).unwrap();
                machine
                    .bus
                    .memory(address, 8)
                    .unwrap()
                    .copy_from_slice(&5u64.to_le_bytes());
                for slot in 0..2 {
                    let hart = machine.hart_mut(slot);
                    hart.registers[10] = address;
                    hart.registers[12] = NEW;
                    hart.registers[14] = 5;
                }
                for (pc, code) in [(CORE0, &[0x1005_25afu32, 0x18c5_26af][..]), (CORE1, writer)] {
                    for (index, instruction) in code.iter().enumerate() {
                        machine
                            .bus
                            .memory(pc + index as u32 * 4, 4)
                            .unwrap()
                            .copy_from_slice(&instruction.to_le_bytes());
                    }
                }
                let mut step = |slot| {
                    machine.elapsed_steps += 1;
                    assert!(matches!(
                        machine.execute_hart(slot, &mut output).unwrap(),
                        Step::Retired
                    ));
                };
                step(0);
                for _ in writer {
                    step(1);
                }
                step(0);

                let lower = machine.bus.read_ram::<4>(address).unwrap();
                let upper = machine.bus.read_ram::<4>(address + 4).unwrap();
                assert_ne!(
                    u64::from(lower) | (u64::from(upper) << 32),
                    u64::from(NEW),
                    "{name} {address:#x}"
                );
                assert_eq!(machine.primary.registers[13], 1, "{name} {address:#x}");
            }
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn upstream_rv32_amocas_single_page() {
        // Renode riscv-amocas.robot selects only cpu32 for these single-step cases.
        let directory =
            std::env::temp_dir().join(format!("penv-rv32-amocas-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let repl = directory.join("cpu.repl");
        std::fs::write(
            &repl,
            r#"cpu32: CPU.RiscV32 @ sysbus
    cpuType: "rv32gc_zicsr_zifencei_zacas"
    hartId: 2
    privilegedArchitecture: PrivilegedArchitecture.Priv1_10
    timeProvider: empty
    allowUnalignedAccesses: true
dram: Memory.MappedMemory @ sysbus 0x80000000
    size: 0x80000000
mtvec: Memory.MappedMemory @ sysbus 0x1000
    size: 0x40000
"#,
        )
        .unwrap();
        const DATA: u32 = RAM_BASE + 0x1000;
        const OLD: u64 = 0x1afe_dead_beef_d00d;
        const NEW: u64 = 0xbeef_f00d_babe_dead;
        const ADJACENT: u64 = 0x2bad_c000_10ff_b0ba;
        for (width, rd, rs2, old, expected, new, result, illegal) in [
            (
                4,
                10,
                18,
                OLD,
                OLD & 0xffff_ffff,
                0xbeef_babe,
                (OLD & !0xffff_ffff) | 0xbeef_babe,
                false,
            ),
            (4, 10, 18, OLD, 0x1234, 0xbeef_babe, OLD, false),
            (8, 10, 18, OLD, OLD, NEW, NEW, false),
            (8, 10, 0, OLD, OLD, 0, 0, false),
            (8, 0, 18, 0, 0, NEW, NEW, false),
            (8, 10, 18, OLD, 0x1_2345_6789_1011, NEW, OLD, false),
            (8, 11, 19, OLD, OLD, NEW, OLD, true),
        ] {
            let mut machine = crate::platform::load(&repl, None, false).unwrap();
            assert_eq!(machine.primary.csr.access(0xf140_2073, 0).unwrap(), 2);
            machine.set_entry(RAM_BASE);
            let opcode = 0x2800_002fu32
                | (rs2 as u32) << 20
                | 13 << 15
                | (if width == 4 { 2 } else { 3 }) << 12
                | (rd as u32) << 7;
            machine
                .bus
                .memory(RAM_BASE, 4)
                .unwrap()
                .copy_from_slice(&opcode.to_le_bytes());
            machine
                .bus
                .memory(DATA, 8)
                .unwrap()
                .copy_from_slice(&old.to_le_bytes());
            machine
                .bus
                .memory(DATA + 8, 8)
                .unwrap()
                .copy_from_slice(&ADJACENT.to_le_bytes());
            let registers = &mut machine.primary.registers;
            if rd == 0 || rs2 == 0 {
                registers[1] = 0x1234;
            }
            for (register, value) in [(rd, expected), (rs2, new)] {
                if register != 0 {
                    registers[register] = value as u32;
                    if width == 8 {
                        registers[register + 1] = (value >> 32) as u32;
                    }
                }
            }
            registers[13] = DATA;
            let before = *registers;
            machine.run(1, &mut std::io::sink()).unwrap();
            let read = |address| machine.bus.read_ram::<4>(address).unwrap();
            assert_eq!(
                u64::from(read(DATA)) | u64::from(read(DATA + 4)) << 32,
                result
            );
            assert_eq!(
                u64::from(read(DATA + 8)) | u64::from(read(DATA + 12)) << 32,
                ADJACENT
            );
            let mut after = before;
            if illegal {
                let csr = &mut machine.primary.csr;
                assert_eq!(machine.primary.pc, 0x1010);
                assert_eq!(csr.access(0x3420_2073, 0).unwrap(), 2);
                assert_eq!(csr.access(0x3430_2073, 0).unwrap(), opcode);
                assert_eq!(csr.access(0x3410_2073, 0).unwrap(), RAM_BASE);
            } else {
                assert_eq!(machine.primary.pc, RAM_BASE + 4);
                if rd != 0 {
                    after[rd] = old as u32;
                    if width == 8 {
                        after[rd + 1] = (old >> 32) as u32;
                    }
                }
            }
            assert_eq!(machine.primary.registers, after, "{opcode:#010x}");
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rv32_amocas_double_handles_register_pairs_and_faults() {
        let data = RAM_BASE + 16;
        for (rd, rs2, enabled, address, initial, expected, cause) in [
            (4, 6, true, data, 3, 5, None),
            (4, 6, true, data, 7, 7, None),
            (0, 6, true, data, 0, 5, None),
            (4, 0, true, data, 3, 0, None),
            (5, 6, true, data, 3, 3, Some(2)),
            (4, 7, true, data, 3, 3, Some(2)),
            (4, 6, false, data, 3, 3, Some(2)),
            (4, 6, true, data + 4, 3, 3, Some(6)),
        ] {
            let mut machine = Machine::new(32).unwrap();
            machine.primary.set_zacas(enabled);
            machine.primary.set_mtvec(RAM_BASE + 4);
            // aqrl, rs1=x2, funct3=3 (doubleword).
            let instruction = 0x2e01_302fu32 | ((rs2 as u32) << 20) | ((rd as u32) << 7);
            machine
                .bus
                .memory(RAM_BASE, 4)
                .unwrap()
                .copy_from_slice(&instruction.to_le_bytes());
            let pair = |value: u32| u64::from(value) * 0x1_0000_0001;
            machine
                .bus
                .memory(data, 8)
                .unwrap()
                .copy_from_slice(&pair(initial).to_le_bytes());
            machine.primary.registers[1] = 42;
            machine.primary.registers[2] = address;
            machine.primary.registers[4..8].copy_from_slice(&[3, 3, 5, 5]);
            let (_, retired) = machine.run(1, &mut std::io::sink()).unwrap();
            assert_eq!(machine.bus.read_ram::<4>(data).unwrap(), expected);
            assert_eq!(machine.bus.read_ram::<4>(data + 4).unwrap(), expected);
            assert_eq!(machine.primary.registers[1], 42);
            if let Some(cause) = cause {
                assert_eq!(retired, 0);
                assert_eq!(machine.primary.csr.access(0x3420_2073, 0).unwrap(), cause);
                assert_eq!(&machine.primary.registers[4..6], &[3, 3]);
            } else {
                assert_eq!(retired, 1);
                if rd != 0 {
                    assert_eq!(&machine.primary.registers[rd..rd + 2], &[initial, initial]);
                }
            }
        }
    }

    #[test]
    fn zicond_rv32_matches_zero_nonzero_and_disabled_cases() {
        // Renode riscv-zicond-extension.robot: RV32 data and trap cases.
        for (funct3, selector, expected) in
            [(5, 0, 0), (5, 1, u32::MAX), (7, 0, u32::MAX), (7, 1, 0)]
        {
            for enabled in [false, true] {
                let mut machine = Machine::new(8).unwrap();
                machine.primary.set_zicond(enabled);
                machine.primary.set_mtvec(RAM_BASE + 4);
                machine.primary.registers[10] = u32::MAX;
                machine.primary.registers[11] = selector;
                // Use rd=rs1 to also check read-before-write behavior.
                let instruction = 0x0eb5_0533u32 | (funct3 << 12);
                machine
                    .bus
                    .memory(RAM_BASE, 4)
                    .unwrap()
                    .copy_from_slice(&instruction.to_le_bytes());
                let (_, retired) = machine.run(1, &mut std::io::sink()).unwrap();
                if enabled {
                    assert_eq!(retired, 1);
                    assert_eq!(machine.primary.registers[10], expected);
                } else {
                    assert_eq!(retired, 0);
                    assert_eq!(machine.primary.pc, RAM_BASE + 4);
                    assert_eq!(machine.primary.csr.access(0x3420_2073, 0).unwrap(), 2);
                    assert_eq!(
                        machine.primary.csr.access(0x3410_2073, 0).unwrap(),
                        RAM_BASE
                    );
                }
            }
        }
    }

    #[test]
    fn vexriscv_accepts_its_data_cache_operations() {
        let mut machine = Machine::new(16).unwrap();
        let instructions = [
            0x0000_500fu32, // VexRiscv data cache flush
            0xcc0a_d073,    // csrwi 0xcc0, 21
            0xcc00_2573,    // csrr a0, 0xcc0
            0x0010_0073,    // ebreak
        ];
        machine
            .bus
            .memory(RAM_BASE, 16)
            .unwrap()
            .copy_from_slice(instructions.map(u32::to_le_bytes).as_flattened());
        assert!(
            machine
                .run(1, &mut std::io::sink())
                .unwrap_err()
                .contains("unsupported instruction")
        );
        machine.primary.enable_vex_core(true);
        assert_eq!(
            machine.run(5, &mut std::io::sink()).unwrap(),
            (Stop::Ebreak, 3)
        );
        assert_eq!(machine.primary.registers[10], 21);
    }

    #[test]
    fn named_pc_hook_halts_only_the_target_hart() {
        let mut machine = Machine::new(16).unwrap();
        machine.configure_harts(2);
        for (index, name) in ["e51", "u54_4"].into_iter().enumerate() {
            let hart = machine.hart_mut(index);
            hart.name = Some(name.into());
            hart.rv64 = Some(Box::new(Rv64::new(crate::rv64::Isa::IM)));
        }
        machine
            .bus
            .memory(RAM_BASE, 4)
            .unwrap()
            .copy_from_slice(&0x0000_006fu32.to_le_bytes());
        machine
            .halt_on_pc("u54_4", u64::from(RAM_BASE), "e51")
            .unwrap();
        assert_eq!(
            machine.run(6, &mut std::io::sink()).unwrap(),
            (Stop::Limit, 4)
        );
    }

    #[test]
    fn handles_ecall_and_returns_to_guest_code() {
        let mut machine = Machine::new(48).unwrap();
        let program: [u32; 12] = [
            0x0000_0297, // auipc t0, 0
            0x0182_8293, // addi t0, t0, 24
            0x3052_9073, // csrw mtvec, t0
            0x3004_6073, // csrsi mstatus, 8
            0x0000_0073, // ecall
            0x0070_0513, // addi a0, zero, 7
            0x3420_2373, // csrr t1, mcause
            0x3000_23f3, // csrr t2, mstatus
            0x3410_2e73, // csrr t3, mepc
            0x004e_0e13, // addi t3, t3, 4
            0x341e_1073, // csrw mepc, t3
            0x3020_0073, // mret
        ];
        for (destination, instruction) in machine
            .bus
            .memory(RAM_BASE, 48)
            .unwrap()
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(program)
        {
            *destination = instruction.to_le_bytes();
        }
        assert_eq!(
            machine.run(12, &mut std::io::sink()).unwrap(),
            (Stop::Limit, 11)
        );
        assert_eq!(machine.primary.pc, RAM_BASE + 24);
        assert_eq!(machine.primary.registers[6], 11);
        assert_eq!(machine.primary.registers[7], (3 << 11) | (1 << 7));
        assert_eq!(machine.primary.registers[28], RAM_BASE + 20);
        assert_eq!(machine.primary.registers[10], 7);
        // Verify that MRET restored MIE from MPIE.
        let read_status = 0x3000_2373u32;
        machine
            .bus
            .memory(machine.primary.pc, 4)
            .unwrap()
            .copy_from_slice(&read_status.to_le_bytes());
        machine.run(1, &mut std::io::sink()).unwrap();
        assert_eq!(
            machine.primary.registers[6],
            (3 << 11) | (1 << 7) | (1 << 3)
        );
    }

    #[test]
    fn upstream_rv32_andes_bfloat_flen32_raw_boxing() {
        for (instruction, source, expected) in [
            (0x0011_42dbu32, 0xffff_3f80, 0xffff_ffff_3f80_0000),
            (0x0011_42db, 0xfffe_7f81, 0xffff_ffff_7fc0_0000),
            (0x0011_c2db, 0x3f80_0000, 0xffff_ffff_ffff_3f80),
        ] {
            let mut machine = Machine::new(4).unwrap();
            machine.primary.enable_floating(false, false);
            machine.primary.set_bitmanip(BitManip {
                xandesbfhcvt: true,
                ..BitManip::default()
            });
            machine.primary.set_mstatus(0x2000);
            machine.primary.floating.as_mut().unwrap()[1] = source;
            machine
                .bus
                .memory(RAM_BASE, 4)
                .unwrap()
                .copy_from_slice(&instruction.to_le_bytes());
            assert_eq!(
                machine.run(1, &mut std::io::sink()).unwrap(),
                (Stop::Limit, 1)
            );
            assert_eq!(machine.primary.pc, RAM_BASE + 4);
            assert_eq!(machine.primary.floating.as_ref().unwrap()[5], expected);
            assert_eq!(machine.primary.csr.fcsr & 31, 0);
        }
    }

    #[test]
    fn upstream_rv32_andes_self_branches_preserve_a0() {
        for (opcode, a0) in [(0x0005_705bu32, 0), (0x4005_705b, 1)] {
            let mut machine = Machine::new(4).unwrap();
            machine.primary.set_bitmanip(BitManip {
                xandesperf: true,
                ..BitManip::default()
            });
            machine.primary.registers[10] = a0;
            machine
                .bus
                .memory(RAM_BASE, 4)
                .unwrap()
                .copy_from_slice(&opcode.to_le_bytes());
            assert_eq!(
                machine.run(1, &mut std::io::sink()).unwrap(),
                (Stop::Limit, 1)
            );
            assert_eq!(machine.primary.pc, RAM_BASE);
            assert_eq!(machine.primary.registers[10], a0);
        }
    }

    #[test]
    fn upstream_rv32_rori() {
        // Renode riscv-zbb-extension.robot: Should Handle RORI Properly on 32 Bit CPU.
        let directory = std::env::temp_dir().join(format!("penv-rv32-rori-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let repl = directory.join("cpu.repl");
        std::fs::write(
            &repl,
            r#"cpu: CPU.RiscV32 @ sysbus { cpuType: "rv32imac_zicsr_zba_zbb"; timeProvider: empty }
mem: Memory.MappedMemory @ sysbus 0x0 { size: 0x1000 }
"#,
        )
        .unwrap();
        let mut machine = crate::platform::load(&repl, None, false).unwrap();
        machine.monitor_pc("cpu", Some(0)).unwrap();
        for (address, opcode) in [(0, 0x4505u16), (2, 0x0505)] {
            machine
                .bus
                .memory(address, 2)
                .unwrap()
                .copy_from_slice(&opcode.to_le_bytes());
        }
        for address in [4, 8] {
            machine
                .bus
                .memory(address, 4)
                .unwrap()
                .copy_from_slice(&0x6015_5513u32.to_le_bytes());
        }
        let mut output = std::io::sink();
        for (pc, expected) in [(2, 1), (4, 2), (8, 1), (12, 0x8000_0000)] {
            assert_eq!(machine.run(1, &mut output).unwrap(), (Stop::Limit, 1));
            assert_eq!(machine.primary.pc, pc);
            assert_eq!(
                machine.primary.monitor_integer_register(10, None).unwrap(),
                Some(expected)
            );
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn upstream_rv32_zbkb() {
        // Renode riscv-bkb.robot: exact RV32 input, result and non-trap checks.
        let directory = std::env::temp_dir().join(format!("penv-rv32-zbkb-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let repl = directory.join("cpu.repl");
        std::fs::write(
            &repl,
            r#"cpu: CPU.RiscV32 @ sysbus { cpuType: "rv32gc_zbkb"; timeProvider: empty }
dram: Memory.MappedMemory @ sysbus 0x80000000 { size: 0x80000000 }
mtvec: Memory.MappedMemory @ sysbus 0x1000 { size: 0x40000 }
"#,
        )
        .unwrap();
        let mut machine = crate::platform::load(&repl, None, false).unwrap();
        for (opcode, rd, inputs) in [
            (
                0x08b5_4633u32,
                12,
                &[(0xffff_beef, 0xffff_dead, 0xdead_beef)][..],
            ),
            (0x08b5_7633, 12, &[(0xffff_ffcd, 0xffff_ffab, 0xabcd)]),
            (
                0x08f5_1593,
                11,
                &[
                    (u32::MAX, 0, u32::MAX),
                    (0xffff, 0, 0x5555_5555),
                    (0xffff_0000, 0, 0xaaaa_aaaa),
                    (0xdead_beef, 0, 0xe7fc_dcf7),
                ],
            ),
            (
                0x08f5_5593,
                11,
                &[
                    (u32::MAX, 0, u32::MAX),
                    (0x5555_5555, 0, 0xffff),
                    (0xaaaa_aaaa, 0, 0xffff_0000),
                ],
            ),
            (
                0x6875_5593,
                11,
                &[(u32::MAX, 0, u32::MAX), (0xdead_beaf, 0, 0x7bb5_7df5)],
            ),
        ] {
            machine.set_entry(RAM_BASE);
            for &(a, b, expected) in inputs {
                let pc = machine.primary.pc;
                machine.primary.registers[10] = a;
                machine.primary.registers[11] = b;
                machine
                    .bus
                    .memory(pc, 4)
                    .unwrap()
                    .copy_from_slice(&opcode.to_le_bytes());
                assert_eq!(
                    machine.run(1, &mut std::io::sink()).unwrap(),
                    (Stop::Limit, 1)
                );
                assert_ne!(machine.primary.pc, 0x1010);
                assert_eq!(
                    machine.primary.registers[rd], expected,
                    "{opcode:#010x}, a0={a:#010x}"
                );
            }
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn upstream_rv32_monitor_rejects_writes_to_x0() {
        let directory = std::env::temp_dir().join(format!("penv-rv32-x0-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let repl = directory.join("cpu.repl");
        std::fs::write(
            &repl,
            r#"clint: IRQControllers.CoreLevelInterruptor @ sysbus 0x44000000 { frequency: 66000000 }
cpu: CPU.RiscV32 @ sysbus { timeProvider: clint; cpuType: "rv32gc_Zfh" }
mem: Memory.MappedMemory @ sysbus 0x1000 { size: 0x40000 }
"#,
        )
        .unwrap();
        let mut machine = crate::platform::load(&repl, None, false).unwrap();
        machine.set_entry(0x2000);
        let hart = &mut machine.primary;
        let error = hart.monitor_integer_register(0, Some(1)).unwrap_err();
        assert!(error.contains("register is read-only"), "{error}");
        assert_eq!(hart.monitor_integer_register(0, None).unwrap(), Some(0));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn upstream_rv32_unit_data_accesses() {
        // Renode riscv-unit-tests.robot: RV32 LB/LH/LW/SB/SH/SW cases.
        let directory = std::env::temp_dir().join(format!("penv-rv32-data-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let repl = directory.join("cpu.repl");
        std::fs::write(
            &repl,
            r#"clint: IRQControllers.CoreLevelInterruptor @ sysbus 0x44000000 { frequency: 66000000 }
cpu: CPU.RiscV32 @ sysbus { timeProvider: clint; cpuType: "rv32gc_Zfh" }
mem: Memory.MappedMemory @ sysbus 0x1000 { size: 0x40000 }
"#,
        )
        .unwrap();
        for (name, opcode, a1, cause) in [
            ("Should Handle LB", 0x0055_8503u32, 0, None),
            ("Should Handle LH", 0x0055_9503, 1, None),
            ("Should Fail LH", 0x0055_9503, 0, Some(4)),
            ("Should Handle LW", 0x0055_a503, 3, None),
            ("Should Fail LW", 0x0055_a503, 0, Some(4)),
            ("Should Handle SB", 0x00a5_82a3, 0, None),
            ("Should Handle SH", 0x00a5_92a3, 1, None),
            ("Should Fail SH", 0x00a5_92a3, 0, Some(6)),
            ("Should Handle SW", 0x00a5_a2a3, 3, None),
            ("Should Fail SW", 0x00a5_a2a3, 0, Some(6)),
        ] {
            let mut machine = crate::platform::load(&repl, None, false).unwrap();
            machine.set_entry(0x2000);
            machine.primary.registers[11] = a1;
            machine
                .bus
                .memory(0x2000, 4)
                .unwrap()
                .copy_from_slice(&opcode.to_le_bytes());
            machine.run(1, &mut std::io::sink()).unwrap();
            let expected_pc = if cause.is_some() { 0x1010 } else { 0x2004 };
            assert_eq!(machine.primary.pc, expected_pc, "{name}");
            if let Some(cause) = cause {
                assert_eq!(
                    machine.primary.csr.access(0x3420_2073, 0).unwrap(),
                    cause,
                    "{name}"
                );
            }
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn upstream_rv32_unit_traps_and_sret() {
        // Renode riscv-unit-tests.robot: three MEPC cases, EBREAK/ECALL,
        // and SRET executed in machine mode.
        let directory =
            std::env::temp_dir().join(format!("penv-rv32-traps-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let repl = directory.join("cpu.repl");
        std::fs::write(
            &repl,
            r#"clint: IRQControllers.CoreLevelInterruptor @ sysbus 0x44000000 { frequency: 66000000 }
cpu: CPU.RiscV32 @ sysbus { timeProvider: clint; cpuType: "rv32gc_Zfh" }
mem: Memory.MappedMemory @ sysbus 0x1000 { size: 0x40000 }
"#,
        )
        .unwrap();
        let write_word = |machine: &mut Machine, address, word: u32| {
            machine
                .bus
                .memory(address, 4)
                .unwrap()
                .copy_from_slice(&word.to_le_bytes());
        };
        let mut output = std::io::sink();
        for (name, word) in [
            ("Should Set MEPC on Illegal Instruction", 0x0000_8000),
            ("Should Set MEPC on Illegal CSR access", 0xf120_d073),
            ("Should Set MEPC on Non-Existing CSR access", 0xfff0_d073),
            ("Should Allow SRET In Machine Mode", 0x1020_0073),
        ] {
            let mut machine = crate::platform::load(&repl, None, false).unwrap();
            machine.set_entry(0x2000);
            write_word(&mut machine, 0x1010, 0x0000_006f); // j .
            write_word(&mut machine, 0x2000, 0x0000_206f); // j 0x4000
            write_word(&mut machine, 0x4000, 0x0000_0013); // nop
            write_word(&mut machine, 0x4004, word);
            if word == 0x1020_0073 {
                machine.primary.csr.access(0x1410_9073, 0x1234).unwrap();
                machine.run(3, &mut output).unwrap();
                assert_eq!(machine.primary.pc, 0x1234, "{name}");
            } else {
                // penv always validates CSR accesses, as in the upstream Full mode.
                machine.run(3, &mut output).unwrap();
                assert_eq!(machine.primary.pc, 0x1010, "{name}");
                let csr = &mut machine.primary.csr;
                assert_eq!(csr.access(0x3420_2073, 0).unwrap(), 2, "{name}");
                assert_eq!(csr.access(0x3430_2073, 0).unwrap(), word, "{name}");
                assert_eq!(csr.access(0x3410_2073, 0).unwrap(), 0x4004, "{name}");
            }
        }

        let mut machine = crate::platform::load(&repl, None, false).unwrap();
        machine.primary.set_mtvec(0x1010);
        write_word(&mut machine, 0x1010, 0x0000_006f); // j .
        write_word(&mut machine, 0x2000, 0x0010_0073); // ebreak
        write_word(&mut machine, 0x2004, 0x0000_0073); // ecall
        for (address, cause) in [(0x2000, 3), (0x2004, 11)] {
            machine.set_entry(address);
            machine.primary.csr.access(0x3430_9073, 0x1234).unwrap();
            machine.run(1, &mut output).unwrap();
            assert_eq!(machine.primary.pc, 0x1010);
            assert_eq!(machine.primary.csr.access(0x3420_2073, 0).unwrap(), cause);
            assert_eq!(machine.primary.csr.access(0x3430_2073, 0).unwrap(), 0);
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn upstream_rv32_mret_restores_pc_and_sets_mpp_to_lowest_privilege() {
        // Renode riscv-mret.robot: User Level [Does Not] Exist[s] 32Bits.
        const START: u32 = 0x2000;
        const TRAP: u32 = 0x4000;
        const CLINT: u32 = 0x4400_0000;
        let directory = std::env::temp_dir().join(format!("penv-rv32-mret-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let repl = directory.join("cpu.repl");
        for (levels, lowest) in [("MachineUser", 0), ("Machine", 3)] {
            std::fs::write(
                &repl,
                format!(
                    r#"clint: IRQControllers.CoreLevelInterruptor @ sysbus 0x44000000 {{ frequency: 66000000 }}
cpu: CPU.RiscV32 @ sysbus {{ timeProvider: clint; cpuType: "rv32gc"; privilegeLevels: PrivilegeLevels.{levels}; interruptMode: 1 }}
mem: Memory.MappedMemory @ sysbus 0x1000 {{ size: 0x40000 }}
"#
                ),
            )
            .unwrap();
            let mut machine = crate::platform::load(&repl, None, false).unwrap();
            machine.set_entry(START);
            machine.primary.set_mtvec(TRAP);
            for (address, word) in [(START, 0xc000_1073u32), (TRAP, 0x3020_0073)] {
                machine
                    .bus
                    .memory(address, 4)
                    .unwrap()
                    .copy_from_slice(&word.to_le_bytes());
            }

            let mut output = std::io::sink();
            machine.run(1, &mut output).unwrap();
            assert_eq!(machine.primary.pc, TRAP, "{levels}");
            assert_eq!(
                (machine.primary.csr.access(0x3000_2073, 0).unwrap() >> 11) & 3,
                3,
                "{levels}"
            );

            machine.run(1, &mut output).unwrap();
            assert_eq!(machine.primary.pc, START, "{levels}");
            assert_eq!(
                (machine.primary.csr.access(0x3000_2073, 0).unwrap() >> 11) & 3,
                lowest,
                "{levels}"
            );

            // The fixture supplies time and MMIO without wiring interrupt outputs.
            let clint = machine.bus.clint.as_mut().unwrap();
            assert_eq!(clint.time(), 1);
            clint.write(CLINT, 1);
            clint.write(CLINT + 0x4000, 0);
            clint.write(CLINT + 0x4004, 0);
            assert_eq!(clint.tick(0), 0);
            assert_eq!(clint.read(CLINT), 1);
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn privilege_controls_gate_sret_and_address_translation_operations() {
        // Renode riscv-priv.robot: RV32 TSR and U-mode SFENCE.VMA cases,
        // plus the equivalent RV32 instructions for its supervisor TVM cases.
        const HANDLER: u32 = RAM_BASE + 12;
        const SRET_TARGET: u32 = RAM_BASE + 16;
        const SUPERVISOR: u32 = (1 << 11) | (1 << 8); // MPP=S, SPP=S
        const TSR: u32 = 1 << 22;
        const TVM: u32 = 1 << 20;
        for (instruction, status, expected_pc) in [
            (0x1020_0073u32, SUPERVISOR | TSR, HANDLER), // sret
            (0x1020_0073, SUPERVISOR, SRET_TARGET),
            (0x1200_0073, 0, HANDLER), // sfence.vma in U
            (0x1200_0073, SUPERVISOR | TVM, HANDLER),
            (0x1200_0073, SUPERVISOR, RAM_BASE + 8),
            (0x1800_2573, SUPERVISOR | TVM, HANDLER), // csrr a0, satp
            (0x1800_2573, SUPERVISOR, RAM_BASE + 8),
            (0x1800_1073, SUPERVISOR | TVM, HANDLER), // csrw satp, zero
            (0x1800_1073, SUPERVISOR, RAM_BASE + 8),
        ] {
            let mut machine = Machine::new(20).unwrap();
            machine.primary.enable_user_mode();
            machine.primary.enable_supervisor_mode();
            machine.primary.set_mtvec(HANDLER);
            machine.primary.csr.access(0x3000_9073, status).unwrap();
            assert_eq!(
                machine.primary.csr.access(0x3000_2073, 0).unwrap() & (TSR | TVM),
                status & (TSR | TVM)
            );
            machine
                .primary
                .csr
                .access(0x3410_9073, RAM_BASE + 4)
                .unwrap();
            machine
                .primary
                .csr
                .access(0x1410_9073, SRET_TARGET)
                .unwrap();
            machine.bus.memory(RAM_BASE, 8).unwrap().copy_from_slice(
                [0x3020_0073u32, instruction]
                    .map(u32::to_le_bytes)
                    .as_flattened(),
            );
            let mut output = std::io::sink();
            assert_eq!(machine.run(1, &mut output).unwrap(), (Stop::Limit, 1));
            let trapped = status == 0 || status & (TSR | TVM) != 0;
            assert_eq!(
                machine.run(1, &mut output).unwrap(),
                (Stop::Limit, u64::from(!trapped))
            );
            assert_eq!(machine.primary.pc, expected_pc);
            if trapped {
                assert_eq!(machine.primary.csr.access(0x3420_2073, 0).unwrap(), 2);
                assert_eq!(
                    machine.primary.csr.access(0x3410_2073, 0).unwrap(),
                    RAM_BASE + 4
                );
                assert_eq!(
                    machine.primary.csr.access(0x3430_2073, 0).unwrap(),
                    instruction
                );
            }
        }
    }

    #[test]
    fn mret_restores_pc_and_resets_mpp_to_lowest_supported_privilege() {
        // Renode riscv-mret.robot: both 32-bit User Level cases.
        const HANDLER: u32 = RAM_BASE + 4;
        for (user_mode, expected_mpp) in [(false, 3), (true, 0)] {
            let mut machine = Machine::new(8).unwrap();
            if user_mode {
                machine.primary.enable_user_mode();
            }
            machine.primary.set_mtvec(HANDLER);
            for (address, instruction) in [
                (RAM_BASE, 0xc000_1073u32), // csrrw zero, cycle, zero
                (HANDLER, 0x3020_0073),     // mret
            ] {
                machine
                    .bus
                    .memory(address, 4)
                    .unwrap()
                    .copy_from_slice(&instruction.to_le_bytes());
            }
            let mut output = std::io::sink();
            assert_eq!(machine.run(1, &mut output).unwrap(), (Stop::Limit, 0));
            assert_eq!(machine.primary.pc, HANDLER);
            assert_eq!(machine.primary.csr.access(0x3420_2073, 0).unwrap(), 2);
            assert_eq!(
                (machine.primary.csr.access(0x3000_2073, 0).unwrap() >> 11) & 3,
                3
            );
            assert_eq!(machine.run(1, &mut output).unwrap(), (Stop::Limit, 1));
            assert_eq!(machine.primary.pc, RAM_BASE);
            assert_eq!(
                (machine.primary.csr.access(0x3000_2073, 0).unwrap() >> 11) & 3,
                expected_mpp
            );
        }
    }

    #[test]
    fn clint_traps_at_the_exact_reprogrammed_compare_tick() {
        // Renode timer-reconfiguration.robot: Should Tick At Exact Time.
        const CLINT: u32 = 0x0200_0000;
        const HANDLER: u32 = RAM_BASE + 16;
        let mut machine = Machine::new(20).unwrap();
        machine
            .bus
            .map_clint_with_frequency(CLINT, 1, 10_000_000)
            .unwrap();
        machine.primary.set_mtvec(HANDLER);
        machine
            .primary
            .csr
            .access(0x3040_9073, crate::clint::TIMER)
            .unwrap();
        machine.primary.csr.access(0x3000_9073, 1 << 3).unwrap();
        machine.primary.registers[1] = CLINT + 0x4000;
        machine.primary.registers[2] = 6;
        machine.primary.registers[3] = CLINT + 0x4004;
        let program = [0x0000_0013u32, 0x0020_a023, 0x0001_a023, 0x0000_006f];
        machine
            .bus
            .memory(RAM_BASE, 16)
            .unwrap()
            .copy_from_slice(program.map(u32::to_le_bytes).as_flattened());

        let mut output = std::io::sink();
        assert_eq!(machine.run(59, &mut output).unwrap(), (Stop::Limit, 59));
        assert_eq!(machine.primary.pc, RAM_BASE + 12);
        assert_eq!(machine.bus.read::<4>(CLINT + 0xbff8).unwrap(), 5);
        assert_eq!(machine.run(1, &mut output).unwrap(), (Stop::Limit, 0));
        assert_eq!(machine.primary.pc, HANDLER);
        assert_eq!(
            machine.primary.csr.access(0x3420_2073, 0).unwrap(),
            0x8000_0007
        );
        assert_eq!(
            machine.primary.csr.access(0x3410_2073, 0).unwrap(),
            RAM_BASE + 12
        );
    }

    // Fixed words, registers and CLINT frequency from timer-reconfiguration.robot.
    // The final self-loop is diagnostic padding for penv's fixed 100 MHz rate.
    fn upstream_timer_reconfiguration(compare: u32, windows: &[u64]) {
        const START: u32 = 0x1000;
        const CLINT: u32 = 0x0200_0000;
        const NOP: u32 = 0x0000_0013;
        let mut machine = Machine::without_default_ram();
        machine.bus.map_ram(START, 0x1000000).unwrap();
        machine
            .bus
            .map_clint_with_frequency(CLINT, 1, 1_000_000)
            .unwrap();
        machine.set_entry(START);
        machine.primary.registers[1] = CLINT + 0x4000;
        machine.primary.registers[2] = compare;
        machine.primary.registers[3] = CLINT + 0x4004;
        let program = [
            NOP,
            NOP,
            0x0020_a023,
            0x0001_a023,
            NOP,
            NOP,
            NOP,
            NOP,
            NOP,
            NOP,
            NOP,
            NOP,
            0x0000_006f,
        ];
        machine
            .bus
            .memory(START, program.len() * 4)
            .unwrap()
            .copy_from_slice(program.map(u32::to_le_bytes).as_flattened());

        let mut output = std::io::sink();
        let mut steps = 0;
        for &window in windows {
            assert_eq!(
                machine.run(window, &mut output).unwrap(),
                (Stop::Limit, window)
            );
            steps += window;
            assert_eq!(machine.primary.pc, START + 4 * steps as u32);
            assert_eq!(machine.bus.read::<4>(CLINT + 0x4000).unwrap(), compare);
            assert_eq!(machine.bus.read::<4>(CLINT + 0x4004).unwrap(), 0);
            assert_eq!(machine.bus.read::<4>(CLINT + 0xbff8).unwrap(), 0);
            assert_eq!(
                machine.primary.csr.access(0x3440_2073, 0).unwrap() & crate::clint::TIMER,
                0
            );
        }

        let steps_until_irq = u64::from(compare) * 100 - steps;
        assert_eq!(
            machine.run(steps_until_irq - 1, &mut output).unwrap(),
            (Stop::Limit, steps_until_irq - 1)
        );
        assert_eq!(machine.bus.read::<4>(CLINT + 0xbff8).unwrap(), compare - 1);
        assert_eq!(
            machine.primary.csr.access(0x3440_2073, 0).unwrap() & crate::clint::TIMER,
            0
        );
        assert_eq!(machine.run(1, &mut output).unwrap(), (Stop::Limit, 1));
        assert_eq!(machine.bus.read::<4>(CLINT + 0xbff8).unwrap(), compare);
        assert_eq!(
            machine.primary.csr.access(0x3440_2073, 0).unwrap() & crate::clint::TIMER,
            crate::clint::TIMER
        );
    }

    #[test]
    fn upstream_timer_reconfiguration_between_chained_blocks() {
        upstream_timer_reconfiguration(5, &[12]);
    }

    #[test]
    fn upstream_timer_reconfiguration_in_the_same_block() {
        upstream_timer_reconfiguration(6, &[8]);
    }

    #[test]
    fn upstream_timer_reconfiguration_at_exact_time() {
        upstream_timer_reconfiguration(6, &[5, 1]);
    }

    #[test]
    fn virt_ns16550_receive_interrupt_routes_through_plic() {
        // Device addresses and source ID from Renode platforms/cpus/riscv_virt.repl.
        const UART: u32 = 0x1000_0000;
        const PLIC: u32 = 0x0c00_0000;
        const CLAIM: u32 = PLIC + 0x20_0004;
        const HANDLER: u32 = RAM_BASE + 4;
        let mut machine = Machine::without_default_ram();
        machine.bus.map_ram(RAM_BASE, 8).unwrap();
        machine.bus.map_plic(PLIC, 52, &[EXTERNAL], true).unwrap();
        machine.bus.map_ns16550(UART, Some(3), false).unwrap();
        machine.primary.set_mtvec(HANDLER);
        machine.primary.csr.access(0x3040_9073, EXTERNAL).unwrap();
        machine.primary.csr.access(0x3000_9073, 1 << 3).unwrap();
        let mut output = Vec::new();
        for (address, width, value) in [(PLIC + 12, 4, 1), (PLIC + 0x2000, 4, 8), (UART + 1, 1, 1)]
        {
            machine
                .bus
                .write(address, width, value, &mut output)
                .unwrap();
        }
        machine.schedule_uart_input(1, b"Q".to_vec()).unwrap();
        assert_eq!(machine.run(1, &mut output).unwrap(), (Stop::Limit, 0));
        assert_eq!(machine.primary.pc, HANDLER);
        assert_eq!(
            machine.primary.csr.access(0x3420_2073, 0).unwrap(),
            0x8000_000b
        );
        assert_eq!(machine.bus.read::<4>(CLAIM).unwrap(), 3);
        assert_eq!(machine.bus.read::<1>(UART + 2).unwrap(), 4);
        let received = machine.bus.read::<1>(UART).unwrap();
        machine.bus.write(UART, 1, received, &mut output).unwrap();
        assert_eq!(output, b"Q");
        assert_eq!(machine.bus.read::<1>(UART + 2).unwrap(), 1);
        assert_eq!(machine.bus.tick_plic(), 0);
        machine.bus.write(CLAIM, 4, 3, &mut output).unwrap();
        assert_eq!(machine.bus.tick_plic(), 0);
    }

    #[test]
    fn delivers_synchronous_faults_with_precise_pc_and_value() {
        let mut machine = Machine::new(16).unwrap();
        let handler = RAM_BASE + 8;
        machine
            .primary
            .csr
            .access(0x3050_9073, handler | 1)
            .unwrap(); // vectored mtvec
        for (pc, opcode, cause, value) in [
            (RAM_BASE, 0x0020_02efu32, 0, RAM_BASE + 2), // jal t0, +2
            (0x2000, 0, 1, 0x2000),                      // unmapped fetch
            (RAM_BASE, u32::MAX, 2, u32::MAX),           // illegal instruction
            (RAM_BASE, 0x7770_22f3, 2, 0x7770_22f3),     // unknown CSR
            (RAM_BASE, 0x0010_2283, 4, 1),               // lw t0, 1(zero)
            (RAM_BASE, 0x0000_2283, 5, 0),               // lw t0, 0(zero)
            (RAM_BASE, 0x0050_20a3, 6, 1),               // sw t0, 1(zero)
            (RAM_BASE, 0x0050_2023, 7, 0),               // sw t0, 0(zero)
        ] {
            machine
                .bus
                .memory(RAM_BASE, 4)
                .unwrap()
                .copy_from_slice(&opcode.to_le_bytes());
            machine.primary.pc = pc;
            machine.primary.registers[5] = 42;
            assert_eq!(
                machine.run(1, &mut std::io::sink()).unwrap(),
                (Stop::Limit, 0)
            );
            assert_eq!(machine.primary.pc, handler);
            assert_eq!(machine.primary.registers[5], 42);
            assert_eq!(machine.primary.csr.access(0x3410_2073, 0).unwrap(), pc);
            assert_eq!(machine.primary.csr.access(0x3420_2073, 0).unwrap(), cause);
            assert_eq!(machine.primary.csr.access(0x3430_2073, 0).unwrap(), value);
        }
    }

    #[test]
    fn prioritizes_software_interrupt_then_delivers_timer_interrupt() {
        use crate::clint::{SOFTWARE, TIMER};

        const CLINT: u32 = 0x0200_0000;
        let mut machine = Machine::new(128).unwrap();
        machine.bus.map_clint(CLINT, 1).unwrap();
        assert!(machine.bus.map_ram(CLINT, 4).is_err());
        let vector = RAM_BASE + 32;
        machine.primary.csr.access(0x3050_9073, vector | 1).unwrap();
        machine
            .primary
            .csr
            .access(0x3040_9073, SOFTWARE | TIMER)
            .unwrap();
        machine.primary.csr.access(0x3000_9073, 1 << 3).unwrap();
        let mut output = std::io::sink();
        for (offset, value) in [(0, 1), (0x4000, 2), (0x4004, 0), (0xbff8, 1)] {
            machine
                .bus
                .write(CLINT + offset, 4, value, &mut output)
                .unwrap();
        }
        let software_entry = vector + 4 * 3;
        machine.primary.registers[5] = CLINT;
        for (address, opcode) in [
            (software_entry, 0x0002_a023u32),  // sw zero, 0(t0)
            (software_entry + 4, 0x3020_0073), // mret
        ] {
            machine
                .bus
                .memory(address, 4)
                .unwrap()
                .copy_from_slice(&opcode.to_le_bytes());
        }
        assert_eq!(machine.run(1, &mut output).unwrap(), (Stop::Limit, 0));
        assert_eq!(machine.primary.pc, software_entry);
        assert_eq!(machine.bus.read::<4>(CLINT + 0xbff8).unwrap(), 2);
        assert_eq!(
            machine.primary.csr.access(0x3440_2073, 0).unwrap(),
            SOFTWARE | TIMER
        );
        assert_eq!(
            machine.primary.csr.access(0x3420_2073, 0).unwrap(),
            0x8000_0003
        );

        assert_eq!(machine.run(2, &mut output).unwrap(), (Stop::Limit, 2));
        assert_eq!(machine.primary.pc, RAM_BASE);
        assert_eq!(machine.bus.read::<4>(CLINT).unwrap(), 0);
        assert_eq!(machine.run(1, &mut output).unwrap(), (Stop::Limit, 0));
        assert_eq!(machine.primary.pc, vector + 4 * 7);
        assert_eq!(
            machine.primary.csr.access(0x3420_2073, 0).unwrap(),
            0x8000_0007
        );
        assert_eq!(
            machine.primary.csr.access(0x3410_2073, 0).unwrap(),
            RAM_BASE
        );
    }

    #[test]
    fn murax_uart_asserts_receive_interrupt_until_data_is_read() {
        const UART: u32 = 0xf001_0000;
        let mut machine = Machine::new(64).unwrap();
        machine.bus.map_uart(UART, true).unwrap();
        machine.bus.uart.as_mut().unwrap().queue_input(vec![b'Q']);
        machine
            .primary
            .csr
            .access(0x3050_9073, RAM_BASE + 16)
            .unwrap();
        machine.primary.csr.access(0x3040_9073, EXTERNAL).unwrap();
        machine.primary.csr.access(0x3000_9073, 1 << 3).unwrap();
        let mut output = Vec::new();
        assert_eq!(machine.bus.read::<4>(UART + 4).unwrap(), 1 << 9);
        machine.bus.write(UART + 4, 4, 2, &mut output).unwrap();
        assert_eq!(machine.run(1, &mut output).unwrap(), (Stop::Limit, 0));
        assert_eq!(machine.primary.pc, RAM_BASE + 16);
        assert_eq!(
            machine.primary.csr.access(0x3420_2073, 0).unwrap(),
            0x8000_000b
        );
        assert_eq!(machine.bus.read::<4>(UART).unwrap(), u32::from(b'Q'));
        assert_eq!(machine.bus.read::<4>(UART + 4).unwrap(), 2);
        assert!(!machine.bus.uart.as_ref().unwrap().interrupt_pending());
    }

    #[test]
    fn executes_word_atomics_and_invalidates_overlapping_reservations() {
        let mut machine = Machine::new(64).unwrap();
        machine.primary.set_integer_extensions(true, true, false);
        machine.primary.registers[1] = RAM_BASE + 32;
        machine.primary.registers[2] = 7;
        let mut output = std::io::sink();
        for (offset, opcode) in [
            (0, 0x1000_a1afu32), // lr.w x3, (x1)
            (4, 0x1820_a22f),    // sc.w x4, x2, (x1)
            (8, 0x0620_a2af),    // amoadd.w.aqrl x5, x2, (x1)
        ] {
            machine
                .bus
                .memory(RAM_BASE + offset, 4)
                .unwrap()
                .copy_from_slice(&opcode.to_le_bytes());
        }
        assert_eq!(machine.run(3, &mut output).unwrap(), (Stop::Limit, 3));
        assert_eq!(&machine.primary.registers[3..6], &[0, 0, 7]);
        assert_eq!(machine.bus.read_ram::<4>(RAM_BASE + 32).unwrap(), 14);
        machine.primary.pc = RAM_BASE;
        machine.run(1, &mut output).unwrap();
        machine.bus.write(RAM_BASE + 33, 1, 1, &mut output).unwrap();
        machine.run(1, &mut output).unwrap();
        assert_eq!(machine.primary.registers[4], 1);
        assert_eq!(machine.bus.read_ram::<4>(RAM_BASE + 32).unwrap(), 0x10e);

        // A guest byte store preserves the reservation held by its own hart.
        machine.primary.pc = RAM_BASE;
        machine.run(1, &mut output).unwrap();
        assert!(
            machine
                .primary
                .store(&mut machine.bus, RAM_BASE + 33, 1, 2, &mut output)
                .is_ok()
        );
        machine.run(1, &mut output).unwrap();
        assert_eq!(machine.primary.registers[4], 0);
        assert_eq!(machine.bus.read_ram::<4>(RAM_BASE + 32).unwrap(), 7);
    }

    #[test]
    fn executes_mixed_width_instructions_and_links_by_two_bytes() {
        let mut machine = Machine::new(14).unwrap();
        machine.primary.set_integer_extensions(true, true, true);
        let bytes = [
            0x11, 0x45, // c.li a0, 4
            0x13, 0x05, 0x15, 0x00, // addi a0, a0, 1 (halfword-aligned)
            0x11, 0x20, // c.jal +4
            0x02, 0x90, // c.ebreak
            0x7d, 0x15, // c.addi a0, -1
            0x82, 0x80, // c.jr ra at the final mapped halfword
        ];
        machine
            .bus
            .memory(RAM_BASE, bytes.len())
            .unwrap()
            .copy_from_slice(&bytes);
        assert_eq!(
            machine.run(8, &mut std::io::sink()).unwrap(),
            (Stop::Ebreak, 5)
        );
        assert_eq!(machine.primary.registers[1], RAM_BASE + 8);
        assert_eq!(machine.primary.registers[10], 4);
        assert_eq!(machine.primary.pc, RAM_BASE + 8);
    }

    #[test]
    fn user_mode_traps_ecall_and_rejects_machine_only_operations() {
        let mut machine = Machine::new(64).unwrap();
        machine.primary.enable_user_mode();
        let user = RAM_BASE + 4;
        let handler = RAM_BASE + 32;
        machine.primary.set_mtvec(handler);
        machine.primary.csr.access(0x3410_9073, user).unwrap();
        for opcode in [0x0000_0073u32, 0x3000_22f3, 0x3020_0073] {
            machine.primary.pc = RAM_BASE;
            machine.primary.csr.access(0x3000_9073, 0).unwrap(); // MPP=U
            machine
                .bus
                .memory(RAM_BASE, 8)
                .unwrap()
                .copy_from_slice(&[0x3020_0073u32.to_le_bytes(), opcode.to_le_bytes()].concat());
            let mut output = std::io::sink();
            assert_eq!(machine.run(1, &mut output).unwrap(), (Stop::Limit, 1));
            assert_eq!(machine.primary.pc, user);
            assert!(machine.primary.csr.in_user_mode());
            assert_eq!(machine.run(1, &mut output).unwrap(), (Stop::Limit, 0));
            assert_eq!(machine.primary.pc, handler);
            assert!(!machine.primary.csr.in_user_mode());
            assert_eq!(machine.primary.csr.access(0x3410_2073, 0).unwrap(), user);
            assert_eq!(
                machine.primary.csr.access(0x3420_2073, 0).unwrap(),
                if opcode == 0x0000_0073 { 8 } else { 2 }
            );
            assert_eq!(
                machine.primary.csr.access(0x3000_2073, 0).unwrap() & (3 << 11),
                0
            );
        }
    }

    #[test]
    fn renode_pmp_napot_and_unmatched_user_fetch() {
        // Renode tests/unit-tests/pmp.robot: fixed PC, MEPC, MTVEC and NAPOT inputs.
        const USER: u32 = 0x8000_1000;
        const DATA: u32 = 0x8000_2000;
        const TRAP: u32 = 0x8008_0000;
        for (pmp, napot) in [(true, true), (true, false), (false, false)] {
            let mut machine = Machine::new(0x81000).unwrap();
            machine.primary.enable_user_mode();
            if pmp {
                machine.primary.enable_pmp(16);
            }
            machine.primary.set_mtvec(TRAP);
            let csr = &mut machine.primary.csr;
            csr.access(0x3410_9073, USER).unwrap();
            if napot {
                csr.access(0x3b00_9073, 0x7fff_ffff).unwrap();
                csr.access(0x3a00_9073, 31).unwrap();
            }
            csr.access(0x3000_9073, 0).unwrap();
            machine
                .bus
                .memory(RAM_BASE, 4)
                .unwrap()
                .copy_from_slice(&0x3020_0073u32.to_le_bytes()); // mret
            machine.bus.memory(USER, 16).unwrap().copy_from_slice(
                &[0x0000_0013u32, 0x8000_2437, 0x0004_2483, 0x0094_2223]
                    .map(u32::to_le_bytes)
                    .concat(),
            ); // nop; lui s0, 0x80002; lw s1, 0(s0); sw s1, 4(s0)
            machine
                .bus
                .memory(DATA, 4)
                .unwrap()
                .copy_from_slice(&0x65657au32.to_le_bytes());
            let (_, retired) = machine
                .run(if napot { 5 } else { 2 }, &mut std::io::sink())
                .unwrap();
            if napot {
                assert_eq!(retired, 5);
                assert_eq!(machine.bus.read_ram::<4>(DATA + 4).unwrap(), 0x65657a);
            } else if pmp {
                assert_eq!(retired, 1);
                assert_eq!(machine.primary.pc, TRAP);
                assert_eq!(machine.primary.csr.access(0x3420_2073, 0).unwrap(), 1);
                assert_eq!(machine.primary.csr.access(0x3410_2073, 0).unwrap(), USER);
            } else {
                assert_eq!(retired, 2);
                assert_eq!(machine.primary.pc, USER + 4);
            }
        }
    }

    #[test]
    fn pmp_blocks_user_load_from_machine_memory() {
        let mut machine = Machine::new(64).unwrap();
        machine.primary.enable_user_mode();
        machine.primary.enable_pmp(16);
        let user = RAM_BASE + 4;
        machine.primary.set_mtvec(RAM_BASE + 32);
        machine.primary.csr.access(0x3410_9073, user).unwrap();
        machine.primary.csr.access(0x3000_9073, 0).unwrap();
        machine.primary.csr.access(0x3b00_9073, user >> 2).unwrap();
        machine.primary.csr.access(0x3a00_9073, 0x14).unwrap(); // NA4, execute only
        machine.primary.registers[1] = RAM_BASE + 16;
        let code = [0x3020_0073u32, 0x0000_a283]; // mret; lw t0, 0(ra)
        machine
            .bus
            .memory(RAM_BASE, 8)
            .unwrap()
            .copy_from_slice(&code.map(u32::to_le_bytes).concat());
        assert_eq!(
            machine.run(2, &mut std::io::sink()).unwrap(),
            (Stop::Limit, 1)
        );
        assert_eq!(machine.primary.pc, RAM_BASE + 32);
        assert_eq!(machine.primary.csr.access(0x3420_2073, 0).unwrap(), 5);
        assert_eq!(
            machine.primary.csr.access(0x3430_2073, 0).unwrap(),
            RAM_BASE + 16
        );
        assert_eq!(machine.primary.registers[5], 0);
    }

    #[test]
    fn delegates_user_ecall_to_supervisor_and_returns_with_sret() {
        let mut machine = Machine::new(64).unwrap();
        machine.primary.enable_user_mode();
        machine.primary.enable_supervisor_mode();
        machine.primary.set_mtvec(RAM_BASE + 48);
        for (csr, value) in [
            (0x105, RAM_BASE + 16),
            (0x302, 1 << 8),
            (0x341, RAM_BASE + 4),
            (0x300, 0),
        ] {
            machine
                .primary
                .csr
                .access((csr << 20) | 0x9073, value)
                .unwrap();
        }
        for (offset, opcode) in [
            (0, 0x3020_0073u32), // mret to U
            (4, 0x0000_0073),    // ecall from U
            (8, 0x0070_0513),    // addi a0, zero, 7
            (16, 0x1410_2373),   // csrr t1, sepc
            (20, 0x0043_0313),   // addi t1, t1, 4
            (24, 0x1413_1073),   // csrw sepc, t1
            (28, 0x1020_0073),   // sret
        ] {
            machine
                .bus
                .memory(RAM_BASE + offset, 4)
                .unwrap()
                .copy_from_slice(&opcode.to_le_bytes());
        }
        let mut output = std::io::sink();
        assert_eq!(machine.run(2, &mut output).unwrap(), (Stop::Limit, 1));
        assert_eq!(machine.primary.pc, RAM_BASE + 16);
        assert_eq!(machine.primary.csr.access(0x1420_2073, 0).unwrap(), 8);
        assert_eq!(
            machine.primary.csr.access(0x1410_2073, 0).unwrap(),
            RAM_BASE + 4
        );
        assert!(machine.primary.csr.access(0x3420_2073, 0).is_err());
        assert_eq!(machine.run(5, &mut output).unwrap(), (Stop::Limit, 5));
        assert!(machine.primary.csr.in_user_mode());
        assert_eq!(machine.primary.registers[10], 7);
    }

    #[test]
    fn sv32_executes_translated_code_and_reports_faulting_virtual_address() {
        const VA: u32 = 0x4000_0000;
        let mut machine = Machine::new(0x4000).unwrap();
        machine.primary.enable_user_mode();
        machine.primary.enable_supervisor_mode();
        machine.primary.enable_pmp(16);
        machine.primary.set_mtvec(RAM_BASE + 0x40);
        let root = RAM_BASE + 0x1000;
        let leaf = RAM_BASE + 0x2000;
        let frame = RAM_BASE + 0x3000;
        let entry = ((frame >> 12) << 10) | 0xf;
        for (address, value) in [
            (RAM_BASE, 0x3020_0073u32), // mret
            (root + ((VA >> 22) * 4), ((leaf >> 12) << 10) | 1),
            (leaf, entry),
            (frame, 0x0020_a023),     // sw x2, 0(x1)
            (frame + 4, 0x0001_a203), // lw x4, 0(x3)
        ] {
            machine
                .bus
                .memory(address, 4)
                .unwrap()
                .copy_from_slice(&value.to_le_bytes());
        }
        for (csr, value) in [
            (0x3b0, u32::MAX),
            (0x3a0, 0x1f), // PMP permits the entire bus
            (0x180, 0x8000_0000 | (root >> 12)),
            (0x341, VA),
            (0x300, 1 << 11),
        ] {
            machine
                .primary
                .csr
                .access((csr << 20) | 0x9073, value)
                .unwrap();
        }
        machine.primary.registers[1] = VA + 0x80;
        machine.primary.registers[2] = 42;
        machine.primary.registers[3] = VA + 0x1000;
        assert_eq!(
            machine.run(3, &mut std::io::sink()).unwrap(),
            (Stop::Limit, 2)
        );
        assert_eq!(machine.bus.read_ram::<4>(frame + 0x80).unwrap(), 42);
        assert_eq!(machine.bus.read_ram::<4>(leaf).unwrap(), entry | 0xc0);
        assert_eq!(machine.primary.pc, RAM_BASE + 0x40);
        assert_eq!(machine.primary.csr.access(0x3420_2073, 0).unwrap(), 13);
        assert_eq!(
            machine.primary.csr.access(0x3430_2073, 0).unwrap(),
            VA + 0x1000
        );
        assert_eq!(machine.primary.csr.access(0x3410_2073, 0).unwrap(), VA + 4);
    }

    #[test]
    fn delivers_vectored_supervisor_software_interrupt_after_mret() {
        let mut machine = Machine::new(64).unwrap();
        machine.primary.enable_user_mode();
        machine.primary.enable_supervisor_mode();
        let vector = RAM_BASE + 16;
        for (csr, value) in [
            (0x303, 2),
            (0x104, 2),
            (0x344, 2),
            (0x105, vector | 1),
            (0x341, RAM_BASE + 4),
            (0x300, (1 << 11) | 2),
        ] {
            machine
                .primary
                .csr
                .access((csr << 20) | 0x9073, value)
                .unwrap();
        }
        for (address, opcode) in [
            (RAM_BASE, 0x3020_0073u32),  // mret
            (RAM_BASE + 4, 0x0070_0513), // addi a0, zero, 7
            (vector + 4, 0x1441_7073),   // csrci sip, 2
            (vector + 8, 0x1020_0073),   // sret
        ] {
            machine
                .bus
                .memory(address, 4)
                .unwrap()
                .copy_from_slice(&opcode.to_le_bytes());
        }
        let mut output = std::io::sink();
        assert_eq!(machine.run(2, &mut output).unwrap(), (Stop::Limit, 1));
        assert_eq!(machine.primary.pc, vector + 4);
        assert_eq!(
            machine.primary.csr.access(0x1420_2073, 0).unwrap(),
            0x8000_0001
        );
        assert_eq!(
            machine.primary.csr.access(0x1410_2073, 0).unwrap(),
            RAM_BASE + 4
        );
        assert_eq!(machine.primary.csr.access(0x1440_2073, 0).unwrap(), 2);
        assert_eq!(machine.run(3, &mut output).unwrap(), (Stop::Limit, 3));
        assert_eq!(machine.primary.csr.access(0x1440_2073, 0).unwrap(), 0);
        assert_eq!(machine.primary.registers[10], 7);
    }

    #[test]
    fn counters_track_retirement_and_clint_time_independently() {
        let mut machine = Machine::new(32).unwrap();
        let clint = 0x0200_0000;
        machine.bus.map_clint(clint, 1).unwrap();
        let mut output = std::io::sink();
        machine
            .bus
            .write(clint + 0xbffc, 4, 5, &mut output)
            .unwrap();
        for (index, opcode) in [
            0xb020_9073u32, // csrw minstret, x1
            0xc020_2173,    // csrr x2, instret
            0xc810_21f3,    // csrr x3, timeh
            0x3202_d073,    // csrwi mcountinhibit, 5
            0x0000_0013,    // nop
            0x0010_0073,    // ebreak
        ]
        .into_iter()
        .enumerate()
        {
            machine
                .bus
                .memory(RAM_BASE + index as u32 * 4, 4)
                .unwrap()
                .copy_from_slice(&opcode.to_le_bytes());
        }
        machine.primary.registers[1] = 40;
        assert_eq!(machine.run(6, &mut output).unwrap(), (Stop::Ebreak, 5));
        assert_eq!(machine.primary.registers[2], 40);
        assert_eq!(machine.primary.registers[3], 5);
        assert_eq!(machine.primary.csr.access(0xc020_2073, 0).unwrap(), 43);
        assert_eq!(machine.primary.csr.access(0xc000_2073, 0).unwrap(), 4);
        assert_eq!(machine.primary.csr.access(0xc010_2073, 0).unwrap(), 6);
    }

    #[test]
    fn blocking_single_step_holds_continuous_harts_until_released() {
        let mut machine = Machine::new(64).unwrap();
        machine.bus.memory(RAM_BASE, 8).unwrap()
            .copy_from_slice(&0x0000_0013u32.to_le_bytes().repeat(2));
        machine.configure_harts(2);
        machine.secondary[0].monitor_single_step = true;
        let mut output = std::io::sink();
        assert_eq!(machine.run(2, &mut output).unwrap(), (Stop::Paused, 0));
        assert_eq!(machine.elapsed_steps(), 0);
        assert_eq!(machine.run_selected(1, Some(1), &mut output, |_, _| false).unwrap(), (Stop::Limit, 1));
        assert_eq!(machine.primary.pc, RAM_BASE);
        assert_eq!(machine.secondary[0].pc, RAM_BASE + 4);
        machine.monitor_single_step_blocking = false;
        assert_eq!(machine.run(2, &mut output).unwrap(), (Stop::Limit, 1));
        assert_eq!(machine.primary.pc, RAM_BASE + 4);
        assert_eq!(machine.secondary[0].pc, RAM_BASE + 4);
    }

    #[test]
    fn schedules_harts_independently_against_shared_ram() {
        let mut machine = Machine::new(64).unwrap();
        let code = [
            0xf140_22f3u32, // csrr t0, mhartid
            0x0050_a023,    // sw t0, 0(ra)
        ];
        machine
            .bus
            .memory(RAM_BASE, 8)
            .unwrap()
            .copy_from_slice(&code.map(u32::to_le_bytes).concat());
        machine.primary.registers[1] = RAM_BASE + 32;
        machine.configure_harts(2);
        let mut output = std::io::sink();
        assert_eq!(machine.run(1, &mut output).unwrap(), (Stop::Limit, 1));
        assert_eq!(machine.primary.pc, RAM_BASE + 4);
        assert_eq!(machine.secondary[0].pc, RAM_BASE);
        assert_eq!(machine.run(3, &mut output).unwrap(), (Stop::Limit, 3));
        assert_eq!(machine.primary.registers[5], 0);
        assert_eq!(machine.secondary[0].registers[5], 1);
        assert_eq!(machine.bus.read_ram::<4>(RAM_BASE + 32).unwrap(), 1);

        let address = RAM_BASE + 32;
        assert_eq!(machine.bus.load_reserved(0, address).unwrap(), 1);
        assert_eq!(machine.bus.load_reserved(1, address).unwrap(), 1);
        assert_eq!(machine.bus.store_conditional(0, address, 2).unwrap(), 0);
        assert_eq!(machine.bus.store_conditional(1, address, 3).unwrap(), 1);
        assert_eq!(machine.bus.read_ram::<4>(address).unwrap(), 2);
    }

    #[test]
    fn clint_routes_software_and_timer_interrupts_to_the_target_hart() {
        use crate::clint::{SOFTWARE, TIMER};

        const CLINT: u32 = 0x0200_0000;
        let mut machine = Machine::new(64).unwrap();
        machine.bus.map_clint(CLINT, 2).unwrap();
        machine.primary.set_mtvec(RAM_BASE + 32);
        machine
            .primary
            .csr
            .access(0x3040_9073, SOFTWARE | TIMER)
            .unwrap();
        machine.primary.csr.access(0x3000_9073, 1 << 3).unwrap();
        machine.configure_harts(2);
        for offset in [0, 4] {
            machine
                .bus
                .memory(RAM_BASE + offset, 4)
                .unwrap()
                .copy_from_slice(&0x0000_0013u32.to_le_bytes());
        }
        let mut output = std::io::sink();
        machine.bus.write(CLINT + 4, 4, 1, &mut output).unwrap();
        assert_eq!(machine.run(2, &mut output).unwrap(), (Stop::Limit, 1));
        assert_eq!(machine.primary.pc, RAM_BASE + 4);
        let secondary = &mut machine.secondary[0];
        assert_eq!(secondary.pc, RAM_BASE + 32);
        assert_eq!(secondary.csr.access(0x3420_2073, 0).unwrap(), 0x8000_0003);

        secondary.csr.access(0x3000_9073, 1 << 3).unwrap();
        for (offset, value) in [(4, 0), (0x4008, 4), (0x400c, 0)] {
            machine
                .bus
                .write(CLINT + offset, 4, value, &mut output)
                .unwrap();
        }
        assert_eq!(machine.run(2, &mut output).unwrap(), (Stop::Limit, 1));
        assert_eq!(machine.primary.pc, RAM_BASE + 8);
        assert_eq!(
            machine.secondary[0].csr.access(0x3420_2073, 0).unwrap(),
            0x8000_0007
        );
        assert_eq!(machine.bus.read::<4>(CLINT + 0xbff8).unwrap(), 4);
    }

    #[test]
    fn uart_events_arrive_in_step_order_and_append_to_existing_input() {
        const UART: u32 = 0xf000_1800;
        let mut machine = Machine::new(8).unwrap();
        machine.bus.map_litex_uart(vec![UART], None).unwrap();
        machine
            .bus
            .memory(RAM_BASE, 4)
            .unwrap()
            .copy_from_slice(&0x0000_006fu32.to_le_bytes());
        machine.schedule_uart_input(3, vec![b'B']).unwrap();
        machine.schedule_uart_input(1, vec![b'A']).unwrap();
        machine.schedule_uart_input(3, vec![b'C']).unwrap();
        let mut output = std::io::sink();
        assert_eq!(machine.run(2, &mut output).unwrap(), (Stop::Limit, 2));
        assert_eq!(machine.run(1, &mut output).unwrap(), (Stop::Limit, 1));
        for byte in b"ABC" {
            assert_eq!(machine.bus.read::<1>(UART).unwrap(), u32::from(*byte));
        }
        assert_eq!(machine.bus.read::<4>(UART + 8).unwrap(), 1);
    }

    #[test]
    fn led_assertions_require_the_named_state_at_the_scheduled_step() {
        use crate::litex_panel::Panel;

        const PANEL: u32 = 0xf000_6800;
        const SHADOW: u32 = 0x6000_6800;
        let mut machine = Machine::new(8).unwrap();
        machine
            .bus
            .map_litex_panel(Panel::new(
                vec![PANEL, SHADOW],
                None,
                Vec::new(),
                vec![("cas.led0".into(), 2)],
            ))
            .unwrap();
        machine
            .bus
            .memory(RAM_BASE, 4)
            .unwrap()
            .copy_from_slice(&0x0000_006fu32.to_le_bytes());
        assert!(
            machine
                .schedule_led_assertion(1, "other.led0".into(), false)
                .is_err()
        );
        machine
            .schedule_led_assertion(1, "cas.led0".into(), false)
            .unwrap();
        machine
            .schedule_led_assertion(2, "sysbus.cas.led0".into(), true)
            .unwrap();
        machine
            .schedule_led_assertion(3, "cas.led0".into(), false)
            .unwrap();

        let mut output = std::io::sink();
        machine.run(1, &mut output).unwrap();
        let missing = machine.verify_required_events_completed().unwrap_err();
        assert!(missing.contains("at step 2"));
        machine.bus.write(SHADOW, 4, 1 << 2, &mut output).unwrap();
        machine.run(1, &mut output).unwrap();
        let mismatch = machine.run(1, &mut output).unwrap_err();
        assert!(mismatch.contains("at step 3: expected off, got on"));
    }

    #[test]
    fn button_events_apply_at_the_scheduled_step_in_submission_order() {
        use crate::litex_panel::{Button, Panel};

        const PANEL: u32 = 0xf000_6800;
        let mut machine = Machine::new(8).unwrap();
        machine
            .bus
            .map_litex_panel(Panel::new(
                vec![PANEL],
                None,
                vec![Button::new("button0".into(), 64, false)],
                Vec::new(),
            ))
            .unwrap();
        machine
            .bus
            .memory(RAM_BASE, 4)
            .unwrap()
            .copy_from_slice(&0x0000_006fu32.to_le_bytes());

        assert!(
            machine
                .schedule_button(9, "missing".into(), ButtonAction::Press)
                .is_err()
        );
        for action in [ButtonAction::Press, ButtonAction::Release] {
            machine
                .schedule_button(3, "cas.button0".into(), action)
                .unwrap();
        }

        let mut output = std::io::sink();
        machine.run(2, &mut output).unwrap();
        assert_eq!(machine.bus.read::<4>(PANEL + 0xc).unwrap(), 0);
        machine.run(1, &mut output).unwrap();
        assert_eq!(machine.bus.read::<4>(PANEL + 8).unwrap(), 0);
        assert_eq!(machine.bus.read::<4>(PANEL + 0xc).unwrap(), 1);
    }

    #[test]
    fn sfence_vma_rewalks_cached_mapping_after_pte_change() {
        const VA: u32 = 0x4000_0000;
        let mut machine = Machine::new(0x5000).unwrap();
        machine.primary.enable_user_mode();
        machine.primary.enable_supervisor_mode();
        let root = RAM_BASE + 0x1000;
        let leaf = RAM_BASE + 0x2000;
        let old = RAM_BASE + 0x3000;
        let new = RAM_BASE + 0x4000;
        let pte = |address: u32| ((address >> 12) << 10) | 0x4b;
        for (address, word) in [
            (root + (VA >> 22) * 4, ((leaf >> 12) << 10) | 1),
            (leaf, pte(old)),
            (old + 4, 17),
            (new, 0x1200_0073), // sfence.vma
            (new + 4, 29),
        ] {
            machine
                .bus
                .memory(address, 4)
                .unwrap()
                .copy_from_slice(&word.to_le_bytes());
        }
        for (csr, value) in [
            (0x180, 0x8000_0000 | (root >> 12)),
            (0x300, 1 << 11),
            (0x341, VA),
        ] {
            machine
                .primary
                .csr
                .access((csr << 20) | 0x9073, value)
                .unwrap();
        }
        machine.primary.pc = machine.primary.csr.return_from_trap().unwrap();
        let load = |machine: &mut Machine| {
            machine
                .primary
                .load::<4>(&mut machine.bus, VA + 4)
                .unwrap_or_else(|fault| panic!("{fault}"))
        };
        assert_eq!(load(&mut machine), 17);
        machine
            .bus
            .memory(leaf, 4)
            .unwrap()
            .copy_from_slice(&pte(new).to_le_bytes());
        assert_eq!(load(&mut machine), 17);
        assert_eq!(
            machine.run(1, &mut std::io::sink()).unwrap(),
            (Stop::Limit, 1)
        );
        assert_eq!(load(&mut machine), 29);
    }

    #[test]
    fn executes_m_extension_edge_cases() {
        let mut machine = Machine::new(8).unwrap();
        for (operation, lhs, rhs, expected) in [
            (0, u32::MAX, 2, 0xffff_fffe),
            (1, u32::MAX, 2, u32::MAX),
            (2, 0x8000_0000, u32::MAX, 0x8000_0000),
            (3, u32::MAX, u32::MAX, 0xffff_fffe),
            (4, 0x8000_0000, u32::MAX, 0x8000_0000),
            (4, (-7i32) as u32, 3, (-2i32) as u32),
            (4, 42, 0, u32::MAX),
            (5, u32::MAX, 2, 0x7fff_ffff),
            (5, 42, 0, u32::MAX),
            (6, 0x8000_0000, u32::MAX, 0),
            (6, (-7i32) as u32, 3, u32::MAX),
            (6, 42, 0, 42),
            (7, u32::MAX, 2, 1),
            (7, 42, 0, 42),
        ] {
            let instruction = 0x0220_81b3u32 | (operation << 12);
            machine
                .bus
                .memory(RAM_BASE, 4)
                .unwrap()
                .copy_from_slice(&instruction.to_le_bytes());
            machine.primary.pc = RAM_BASE;
            machine.primary.registers[1] = lhs;
            machine.primary.registers[2] = rhs;
            assert_eq!(
                machine.run(1, &mut std::io::sink()).unwrap(),
                (Stop::Limit, 1)
            );
            assert_eq!(machine.primary.registers[3], expected, "funct3={operation}");
        }
    }
    #[test]
    fn cortex_m_clock_gate_and_counter_survive_individual_reset_as_expected() {
        let mut machine = Machine::without_default_ram();
        machine.bus.map_ram(0, 0x1000).unwrap();
        let mut out = std::io::sink();
        machine.bus.write(0, 4, 0x800, &mut out).unwrap();
        machine.bus.write(4, 4, 0x101, &mut out).unwrap();
        machine.bus.write(0x100, 2, 0xe7fe, &mut out).unwrap(); // b .
        machine.primary.configure_cortex_m(Some(0), true);
        machine.validate_entry().unwrap();

        machine.run(3, &mut out).unwrap();
        assert_eq!(machine.primary.cortex_m_executed_instructions(), Some(3));
        machine.primary.set_cortex_m_clocked(false).unwrap();
        machine.primary.reset_cortex_m(&mut machine.bus).unwrap();
        machine.run(5, &mut out).unwrap();
        assert_eq!(machine.primary.cortex_m_executed_instructions(), Some(0));
        assert_eq!(machine.primary.pc, 0x100);
        assert_eq!(machine.primary.registers[13], 0x800);
        machine.primary.set_cortex_m_clocked(true).unwrap();
        machine.run(2, &mut out).unwrap();
        assert_eq!(machine.primary.cortex_m_executed_instructions(), Some(2));
    }
}
