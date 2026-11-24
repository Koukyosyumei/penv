use crate::csr::TrapVector;

use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{self, BufRead, Read, Write},
    net::Ipv4Addr,
    path::{Path, PathBuf},
    time::Duration,
};

use crate::{
    bus::{CORTEX_M_STEP_HZ, UartWatch, UnhandledAccess},
    event_trace::ExternalEvent,
    machine::{CpuHook, Machine, Stop},
    rtc_clock::RtcMode,
    time_handle::TimeHandle,
    uart::{BinaryUartPattern, FailingUartPattern, UartHistory, UartWaitMode},
};

#[derive(Default)]
pub struct Boot {
    pub binaries: Vec<(Asset, u32)>,
    pub network: Network,
    pub video_analyzers: Vec<String>,
    pub pcm_inputs: Vec<(String, Asset)>,
    pub pcm_outputs: Vec<(String, PathBuf)>,
    pub run_steps: Option<u64>,
    pub quantum: Option<u64>,
    pub cpu_mips: Option<u32>,
    pub post_run: Vec<PostRun>,
    pub buttons: Vec<(String, crate::litex_panel::ButtonAction)>,
    pub silence_ranges: Vec<(u32, u32)>,
    pub boot_pcs: Vec<(String, u32)>,
    pub mtvec: Option<u32>,
    pub cpu_control: Vec<CpuControl>,
    pub platform: Option<PathBuf>,
    inline_repl: Vec<(PathBuf, String)>,
    pub generated_repl: Option<GeneratedRepl>,
    pub session: Session,
    pub serial_analyzer: Option<String>,
    pub image: Option<Asset>,
    pub hex: bool,
    pub vector_tables: Vec<(String, u32)>,
    pub runtime: Vec<RuntimeCommand>,
    pub use_virtual_elf_addresses: bool,
    pub log_function_names: bool,
    pub monitor_log_level: Option<i8>,
    pub trace_memory: Vec<(String, bool)>,
    pub riscv_wfi_as_nop: Vec<(String, bool)>,
    pub monitor_user_state: Option<String>,
    pub monitor_output: Vec<String>,
    pub cpu_registers: Vec<CpuRegisterCommand>,
    pub mask: Option<u32>,
    pub supervisor_mask: Option<u32>,
    pub mpfs_buttons: Vec<(String, String, bool)>,
    pub halt_hooks: Vec<(String, String, String, String)>,
    pub python_hooks: Vec<(String, String, String, Asset)>,
    symbols: Vec<String>,
    hooks: HashMap<String, String>,
    pub sd_cards: Vec<SdAttachment>,
    started: bool,
}

#[derive(Default)]
pub struct Session {
    variables: HashMap<String, String>,
    machine: Option<String>,
    number_mode: NumberMode,
}

#[derive(Clone, Copy, Default)]
enum NumberMode {
    #[default]
    Hexadecimal,
    Decimal,
    Both,
}

impl NumberMode {
    fn print(self, value: u64, width: usize, output: &mut impl Write) -> Result<(), String> {
        let digits = 2 * width;
        match self {
            Self::Hexadecimal => writeln!(output, "0x{value:0digits$X}"),
            Self::Decimal => writeln!(output, "{value}"),
            Self::Both => writeln!(output, "0x{value:0digits$X} ({value})"),
        }.map_err(|error| error.to_string())
    }
}

const SESSION_SNAPSHOT_V1: &[u8; 9] = b"PENVSESS\x01";
const SESSION_SNAPSHOT_V2: &[u8; 9] = b"PENVSESS\x02";
const SESSION_SNAPSHOT: &[u8; 9] = b"PENVSESS\x03";
const MAX_MONITOR_PAYLOAD: usize = 8 * 1024 * 1024;

impl Session {
    fn save(&self, machine: &Machine, path: &Path) -> Result<(), String> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let mut data = Vec::new();
        let mut entries: Vec<_> = self.variables.iter()
            .filter(|(key, _)| !matches!(key.as_str(), "global.CWD" | "global.ORIGIN"))
            .collect();
        entries.sort_unstable();
        fn encode(data: &mut Vec<u8>, text: &str) -> Result<(), String> {
            if data.len().saturating_add(text.len()).saturating_add(4) > MAX_MONITOR_PAYLOAD {
                return Err("monitor variable snapshot exceeds size limit".into());
            }
            data.extend_from_slice(&(text.len() as u32).to_le_bytes());
            data.extend_from_slice(text.as_bytes());
            Ok(())
        }
        encode(&mut data, self.machine.as_deref().unwrap_or_default())?;
        for (key, value) in entries {
            encode(&mut data, key)?;
            encode(&mut data, value)?;
        }
        let temporary = path.with_file_name(format!(".penv-snapshot-{}-{}", std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)));
        let mut file = fs::OpenOptions::new().write(true).create_new(true).open(&temporary)
            .map_err(|e| format!("{}: {e}", temporary.display()))?;
        let result = (|| {
            file.write_all(SESSION_SNAPSHOT).map_err(|e| e.to_string())?;
            file.write_all(&machine.monitor_quantum.to_le_bytes()).map_err(|e| e.to_string())?;
            let mips = (machine.bus.monitor_step_hz() / 1_000_000) as u32;
            file.write_all(&mips.to_le_bytes()).map_err(|e| e.to_string())?;
            file.write_all(&(data.len() as u32).to_le_bytes()).map_err(|e| e.to_string())?;
            file.write_all(&data).map_err(|e| e.to_string())?;
            machine.write_boot_snapshot(&mut file)?;
            file.sync_all().map_err(|e| e.to_string())?;
            fs::rename(&temporary, path).map_err(|e| e.to_string())
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result.map_err(|e| format!("{}: {e}", path.display()))
    }

    fn load(&mut self, machine: &mut Machine, path: &Path) -> Result<(), String> {
        let mut input = io::BufReader::new(fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?);
        let result = (|| {
            let header = input.fill_buf().map_err(|e| e.to_string())?;
            let version = if header.starts_with(SESSION_SNAPSHOT) {
                3
            } else if header.starts_with(SESSION_SNAPSHOT_V2) {
                2
            } else if header.starts_with(SESSION_SNAPSHOT_V1) {
                1
            } else {
                return machine.read_boot_snapshot(&mut input);
            };
            input.consume(SESSION_SNAPSHOT.len());
            let quantum = if version >= 2 {
                let mut bytes = [0; 8];
                input.read_exact(&mut bytes).map_err(|e| e.to_string())?;
                let quantum = u64::from_le_bytes(bytes);
                if quantum == 0 {
                    return Err("invalid snapshot synchronization quantum".into());
                }
                Some(quantum)
            } else {
                None
            };
            let cpu_mips = if version >= 3 {
                let mut bytes = [0; 4];
                input.read_exact(&mut bytes).map_err(|e| e.to_string())?;
                let mips = u32::from_le_bytes(bytes);
                if !machine.is_cortex_m() || mips == 0 || mips.checked_mul(1_000_000).is_none() {
                    return Err("invalid snapshot CPU speed".into());
                }
                Some(mips)
            } else {
                None
            };
            fn read_size(input: &mut impl Read) -> Result<usize, String> {
                let mut length = [0; 4];
                input.read_exact(&mut length).map_err(|e| e.to_string())?;
                Ok(u32::from_le_bytes(length) as usize)
            }
            fn read_string(data: &mut &[u8]) -> Result<String, String> {
                let length = read_size(data)?;
                let (text, remaining) = data.split_at_checked(length).ok_or("truncated monitor variable snapshot")?;
                *data = remaining;
                String::from_utf8(text.to_owned()).map_err(|e| e.to_string())
            }
            let size = read_size(&mut input)?;
            if size > MAX_MONITOR_PAYLOAD {
                return Err("monitor variable snapshot exceeds size limit".into());
            }
            let mut data = vec![0; size];
            input.read_exact(&mut data).map_err(|e| e.to_string())?;
            let mut data = data.as_slice();
            let selected = read_string(&mut data)?;
            if selected != self.machine.as_deref().unwrap_or_default() {
                return Err("snapshot uses a different monitor machine".into());
            }
            let mut variables = HashMap::new();
            while !data.is_empty() {
                let key = read_string(&mut data)?;
                let value = read_string(&mut data)?;
                if matches!(key.as_str(), "global.CWD" | "global.ORIGIN") || variables.insert(key, value).is_some() {
                    return Err("invalid monitor variable snapshot".into());
                }
            }
            for key in ["global.CWD", "global.ORIGIN"] {
                if let Some(value) = self.variables.get(key) {
                    variables.insert(key.into(), value.clone());
                }
            }
            machine.read_boot_snapshot(&mut input)?;
            if let Some(mips) = cpu_mips {
                machine.set_monitor_cpu_rate(mips)?;
            }
            if let Some(quantum) = quantum {
                machine.monitor_quantum = quantum;
            }
            self.variables = variables;
            Ok(())
        })();
        result.map_err(|e| format!("{}: {e}", path.display()))
    }
}

pub struct GeneratedRepl(PathBuf);

impl Drop for GeneratedRepl {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

impl Boot {
    fn has_platform(&self) -> bool {
        self.platform.is_some() || !self.inline_repl.is_empty()
    }

    fn materialize_platform(&mut self) -> Result<(), String> {
        if self.inline_repl.is_empty() {
            return Ok(());
        }
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "penv-repl-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir(&dir).map_err(|e| e.to_string())?;
        let guard = GeneratedRepl(dir);
        let path = guard.0.join("platform.repl");
        let mut file = fs::File::create(&path).map_err(|e| e.to_string())?;
        if let Some(base) = &self.platform {
            let base = fs::canonicalize(base).map_err(|e| format!("{}: {e}", base.display()))?;
            writeln!(file, "using {:?}", base.to_string_lossy()).map_err(|e| e.to_string())?;
        }
        for (origin, source) in &self.inline_repl {
            for line in source.lines() {
                let relative = line.trim_start().strip_prefix("using ")
                    .and_then(|value| value.trim().strip_prefix('"')?.strip_suffix('"'));
                if let Some(relative) = relative {
                    let include = origin.join(relative);
                    if include.is_file() {
                        writeln!(file, "using {:?}", include.to_string_lossy())
                            .map_err(|e| e.to_string())?;
                        continue;
                    }
                }
                writeln!(file, "{line}").map_err(|e| e.to_string())?;
            }
        }
        self.platform = Some(path);
        self.generated_repl = Some(guard);
        Ok(())
    }
}

pub struct RuntimeCommand {
    line: usize,
    source: PathBuf,
    action: Action,
}

enum Action {
    RunFor(u64),
    RunUntil(u64),
    RunToSyncPoint,
    SetQuantum(u64),
    LogLevel(i8),
    UserState(Option<String>),
    Echo(String),
    Illuminance(String, f64),
    ClicGpio(String, u32, bool),
    LogUart(String),
    GpioWire(String, u8, String, u8),
    GpioOverlay(Vec<(String, u8, String, u8)>),
    IlluminanceFeed(String, PathBuf),
    RunRemaining,
    TimeSourceInfo,
    PhyRegister(String, u16),
    AdxlAxis(String, usize, i16),
    LiteXLedState(String),
    UartIdle(u64),
    StartCollapsedProfile(String, PathBuf, bool, Option<u64>),
    StopProfile(String),
    SaveMemory(PathBuf),
    ClearMemory,
    LoadMemory(PathBuf),
    SelectMachineZero,
    FindBytes(FindBytes),
    BusScalar(BusScalar),
    RecordTo(PathBuf),
    PlayFrom(PathBuf),
    RtcMode(RtcMode),
    RtcDateTime,
    CreateHostPty(String, PathBuf, bool),
    CpuStep(String, u64),
    CpuMips(u32),
    ReadCpuMips(NumberMode),
    TraceStackAccess(String),
    VectorRegisterLength(String, u64),
    CpuCondition(String, u32),
    CpuXpsr(String),
    CpuWait(String, bool),
    CpuPaused(String, Option<bool>),
    CpuHalted(String, Option<bool>),
    ReadCpuPc(String),
    CpuRegister(String, String, Option<u64>),
    ExecutionMode(String, Option<bool>),
    AddCpuHook(String, Option<u32>, CpuHook),
    NvicLog(String),
    NvicWrite(String, u32, u32),
    SynchroniseQuantum,
    CpuReset(String),
    CpuPc(String, u32),
    MachineReset(Vec<ResetHook>),
    ReadScalar(u64, ScalarWidth),
    WriteScalar(u64, ScalarWidth, u32),
    Tag(u32, u32, u64, bool),
    UnhandledAccess(UnhandledAccess),
    ReadBytes(u32, usize),
    WriteBytes(u32, Vec<u8>),
    Button(String, bool),
    ButtonPulse(String),
    UnregisterGpio(String),
    NvicGpio(String, u8, bool),
    NvicPending(String, u32),
    NvicClocked(String, bool),
    Frequency(String, u32),
    TimerProperty(String, bool),
    TraceRcc,
    WriteRcc(u32, u32),
    WaitForLed(String, bool, u64, u64),
    Adc(String, usize, f64),
    WaitForUart(Option<String>, String, UartWaitMode, Option<u64>),
    CreateUartTester(String, bool),
    DefaultUartTester(Option<usize>),
    WaitForTester(Option<usize>, String, Option<regex::Regex>, UartWaitMode, u64, bool, bool),
    WaitForBytes(Option<usize>, BinaryUartPattern, bool, u64),
    SetUartFailure(Option<usize>, FailingUartPattern, bool),
    WriteUartLine(String, String, &'static [u8]),
}

struct BusScalar {
    address: u64,
    width: usize,
    value: Option<u64>,
    number_mode: NumberMode,
}

impl BusScalar {
    fn apply(&self, bus: &mut crate::bus::Bus, output: &mut impl Write) -> Result<(), String> {
        let Self {
            address,
            width,
            value,
            number_mode,
        } = *self;
        let address32 = u32::try_from(address)
            .ok()
            .filter(|&address| address.checked_add(width as u32 - 1).is_some());
        if let Some(value) = value {
            return match address32 {
                Some(address) if width <= 4 => bus
                    .write_scalar(address, width, value as u32, output, true)
                    .map_err(|error| error.to_string()),
                _ => crate::memory_setup::write_ram(bus, address, width, value),
            };
        }
        let result = match (address32, width) {
            (Some(address), 1) => bus.read_scalar::<1>(address, true),
            (Some(address), 2) => bus.read_scalar::<2>(address, true),
            (Some(address), 4) => bus.read_scalar::<4>(address, true),
            _ => {
                return number_mode.print(
                    crate::memory_setup::read_ram(bus, address, width)?,
                    width,
                    output,
                );
            }
        };
        number_mode.print(
            u64::from(result.map_err(|error| error.to_string())?),
            width,
            output,
        )
    }
}

pub(crate) fn is_static_memory_script(commands: &[RuntimeCommand]) -> bool {
    !commands.is_empty()
        && commands
            .iter()
            .all(|command| matches!(command.action, Action::BusScalar(_) | Action::FindBytes(_)))
}

enum ResetHook {
    Sleep(Duration),
    Log(String),
}

#[derive(Clone, Copy)]
enum ScalarWidth {
    Byte = 1,
    Word = 2,
    DoubleWord = 4,
}

impl ScalarWidth {
    fn read(self, machine: &mut Machine, address: u64) -> Result<u32, String> {
        let Ok(address32) = u32::try_from(address) else {
            let width = self as usize;
            let memory = machine.bus.high_memory(address, width)?;
            let mut bytes = [0; 4];
            bytes[..width].copy_from_slice(memory);
            return Ok(u32::from_le_bytes(bytes));
        };
        match self {
            Self::Byte => machine.bus.read_scalar::<1>(address32, true),
            Self::Word => machine.bus.read_scalar::<2>(address32, true),
            Self::DoubleWord => machine.bus.read_scalar::<4>(address32, true),
        }
        .map_err(|e| e.to_string())
    }
}

struct ScriptOutput<'a, W> {
    output: &'a mut W,
    history: Option<UartHistory>,
}

impl<W: Write> Write for ScriptOutput<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let written = self.output.write(bytes)?;
        if let Some(history) = &mut self.history {
            for &byte in &bytes[..written] {
                history.receive(byte);
            }
        }
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.output.flush()
    }
}

pub fn is_configuration_only(commands: &[RuntimeCommand]) -> bool {
    !commands.is_empty()
        && commands.iter().all(|command| {
            matches!(
                command.action,
                Action::CreateHostPty(..) | Action::RtcMode(_) | Action::RtcDateTime
            )
        })
}

pub fn run(
    machine: &mut Machine,
    _script: &Path,
    commands: &[RuntimeCommand],
    budget: u64,
    output: &mut impl Write,
) -> Result<(Stop, u64), String> {
    run_commands(machine, commands, budget, output, None)
}

fn run_commands(
    machine: &mut Machine,
    commands: &[RuntimeCommand],
    mut budget: u64,
    output: &mut impl Write,
    mut session: Option<&mut Session>,
) -> Result<(Stop, u64), String> {
    if commands.is_empty() {
        return machine.run(budget, output);
    }
    let mut output = ScriptOutput {
        output,
        history: None,
    };
    for command in commands {
        match &command.action {
            Action::WaitForUart(Some(target), ..) => machine.bus.record_stm32_uart(target),
            Action::WaitForUart(None, ..) => {
                output.history.get_or_insert_with(UartHistory::default);
            }
            _ => {}
        }
    }
    let mut retired = 0;
    let mut host_terminals = HashMap::new();
    let mut cleared = false;
    for command in commands {
        if cleared && !matches!(command.action, Action::LoadMemory(_)) {
            return Err(format!(
                "{}:{}: Load a boot snapshot after Clear",
                command.source.display(),
                command.line
            ));
        }
        let result = match &command.action {
            Action::StartCollapsedProfile(cpu, path, flush, limit) => machine
                .start_profile(cpu, path, *flush, *limit)
                .map(|()| None),
            Action::StopProfile(cpu) => machine
                .cortex_m_cpu(cpu)
                .map(|_| ())
                .and_then(|()| machine.finish_profile())
                .map(|()| None),
            Action::SaveMemory(path) => match session.as_deref() {
                Some(session) => session.save(machine, path),
                None => machine.save_memory_snapshot(path),
            }.map(|()| None),
            Action::ClearMemory => machine.clear_memory_snapshot().map(|()| {
                cleared = true;
                None
            }),
            Action::LoadMemory(path) => match session.as_deref_mut() {
                Some(session) => session.load(machine, path),
                None => machine.load_memory_snapshot(path),
            }.map(|()| {
                cleared = false;
                None
            }),
            Action::SelectMachineZero => Ok(None),
            Action::BusScalar(access) => access.apply(&mut machine.bus, &mut output).map(|()| None),
            Action::FindBytes(search) => search
                .write_matches(&machine.bus, "", output.output)
                .map(|()| None),
            Action::RunRemaining => machine.run(budget, &mut output).map(|(stop, count)| {
                retired += count;
                Some(stop)
            }),
            Action::RecordTo(path) => machine.record_to(path).map(|()| None),
            Action::PlayFrom(path) => machine.play_from(path).map(|()| None),
            Action::RtcMode(mode) => machine.rtc_clock.set_mode(*mode).map(|()| None),
            Action::RtcDateTime => {
                let value = machine.rtc_clock.date_time(machine.elapsed_steps());
                writeln!(output.output, "{value}")
                    .map_err(|e| e.to_string())
                    .map(|()| None)
            }
            Action::CreateHostPty(name, path, force) => match host_terminals.entry(name) {
                std::collections::hash_map::Entry::Occupied(_) => {
                    Err(format!("host terminal {name:?} already exists"))
                }
                std::collections::hash_map::Entry::Vacant(entry) => {
                    crate::host_pty::HostPty::open(path, *force)
                        .map(|terminal| {
                            entry.insert(terminal);
                            None
                        })
                        .map_err(|error| {
                            format!("host terminal {name:?} at {}: {error}", path.display())
                        })
                }
            },
            Action::CpuStep(cpu, steps) if !machine.is_cortex_m() => {
                let chunk = (*steps).min(budget);
                let start = machine.elapsed_steps();
                machine.monitor_step_riscv(cpu, chunk, &mut output).map(|(stop, count)| {
                    retired += count;
                    budget -= machine.elapsed_steps().wrapping_sub(start);
                    (stop != Stop::Limit || chunk < *steps).then_some(stop)
                })
            }
            Action::PhyRegister(target, value) => machine.bus.litex_phy_mut(target).map(|phy| {
                phy.set_vendor_specific1(*value);
                None
            }),
            Action::AdxlAxis(target, axis, value) => machine.bus.nrf_adxl_input(target)
                .ok_or_else(|| format!("unsupported ADXL372 target {target:?}"))
                .map(|acceleration| {
                    acceleration[*axis] = *value;
                    None
                }),
            Action::LiteXLedState(target) => machine.bus.litex_led_state(target).and_then(|state| {
                writeln!(output.output, "{}", if state { "True" } else { "False" })
                    .map(|()| None).map_err(|error| error.to_string())
            }),
            Action::VectorRegisterLength(cpu, bits) => machine.configure_rv64_vlen(cpu, *bits).map(|()| None),
            Action::TraceStackAccess(cpu) => machine.enable_rv64_stack_access_trace(cpu).map(|()| None),
            Action::CpuMips(mips) => machine.set_monitor_cpu_rate(*mips).map(|()| None),
            Action::ReadCpuMips(mode) => {
                if !machine.is_cortex_m() {
                    return Err("PerformanceInMips requires a Cortex-M CPU".into());
                }
                mode.print(machine.bus.monitor_step_hz() / 1_000_000, 4, output.output).map(|()| None)
            }
            Action::TimeSourceInfo => {
                writeln!(output.output, "Elapsed Virtual Time: {}\nQuantum: {}",
                    monitor_timestamp(machine.monitor_elapsed_ns()),
                    monitor_timestamp(u128::from(machine.monitor_quantum) * 10))
                    .map(|()| None).map_err(|error| error.to_string())
            }
            Action::UartIdle(_) => Err("UART idle check requires the live monitor".into()),
            Action::SetQuantum(duration) => {
                machine.monitor_quantum = *duration;
                Ok(None)
            }
            Action::RunToSyncPoint | Action::RunFor(_) | Action::RunUntil(_) | Action::CpuStep(..) => {
                let valid_cpu = match &command.action {
                    Action::CpuStep(cpu, _) => machine.cortex_m_cpu(cpu).map(|_| ()),
                    _ => Ok(()),
                };
                valid_cpu.and_then(|()| {
                    if machine.primary.monitor_paused {
                        return Ok(None);
                    }
                    let steps = match command.action {
                        Action::CpuStep(_, steps) => steps,
                        Action::RunFor(duration) => duration_cycles(
                            duration, CORTEX_M_STEP_HZ, machine.bus.monitor_step_hz(),
                        )?,
                        Action::RunUntil(timestamp) => {
                            let deadline = u128::from(timestamp) * 10;
                            let elapsed = machine.monitor_elapsed_ns();
                            if deadline < elapsed {
                                crate::monitor_log::record(2, &format!(
                                    "Given timestamp '{}' is from the past. Current elapsed virtual time is {}",
                                    monitor_timestamp(deadline), monitor_timestamp(elapsed),
                                ));
                            }
                            let remaining = deadline.saturating_sub(elapsed);
                            duration_cycles(
                                remaining.try_into().map_err(|_| "virtual timestamp exceeds the supported range")?,
                                1_000_000_000,
                                machine.bus.monitor_step_hz(),
                            )?
                        }
                        Action::RunToSyncPoint => duration_cycles(
                            machine.monitor_quantum, CORTEX_M_STEP_HZ, machine.bus.monitor_step_hz(),
                        )?,
                        _ => unreachable!(),
                    };
                    let chunk = steps.min(budget);
                    let start = machine.elapsed_steps();
                    TimeHandle::run_grant(chunk, |granted| {
                        let result = machine.run(granted, &mut output)?;
                        Ok((result, machine.elapsed_steps().wrapping_sub(start)))
                    })
                    .map(|((stop, count), used)| {
                        retired += count;
                        budget -= used;
                        (stop != Stop::Limit || (chunk < steps && !machine.primary.monitor_paused))
                            .then_some(stop)
                    })
                })
            }
            Action::CpuCondition(cpu, condition) => machine.cortex_m_cpu(cpu).and_then(|thumb| {
                let result = if thumb.condition(*condition) {
                    "True"
                } else {
                    "False"
                };
                writeln!(output.output, "{result}")
                    .map_err(|e| e.to_string())
                    .map(|()| None)
            }),
            Action::CpuXpsr(cpu) => machine.cortex_m_cpu(cpu).and_then(|thumb| {
                writeln!(output.output, "0x{:08x}", thumb.xpsr())
                    .map_err(|e| e.to_string())
                    .map(|()| None)
            }),
            Action::ExecutionMode(cpu, single_step) => {
                machine.monitor_execution_mode(cpu, *single_step).and_then(|state| {
                    if single_step.is_none() {
                        let mode = if state { "SingleStep" } else { "Continuous" };
                        writeln!(output.output, "{mode}").map_err(|error| error.to_string())?;
                    }
                    Ok(None)
                })
            }
            Action::CpuHalted(cpu, requested) if !machine.is_cortex_m() => {
                machine.monitor_riscv_halted(cpu, *requested).and_then(|state| {
                    if requested.is_none() {
                        let state = if state { "True" } else { "False" };
                        writeln!(output.output, "{state}").map_err(|error| error.to_string())?;
                    }
                    Ok(None)
                })
            }
            Action::CpuPaused(cpu, requested) | Action::CpuHalted(cpu, requested) => {
                machine.cortex_m_monitor_cpu(cpu).and_then(|hart| {
                    let state = if matches!(command.action, Action::CpuPaused(..)) {
                        &mut hart.monitor_paused
                    } else {
                        &mut hart.monitor_halted
                    };
                    if let Some(requested) = requested {
                        *state = *requested;
                        Ok(None)
                    } else {
                        let value = if *state { "True" } else { "False" };
                        writeln!(output.output, "{value}")
                            .map_err(|e| e.to_string())
                            .map(|()| None)
                    }
                })
            }
            Action::AddCpuHook(cpu, address, hook) => {
                machine.cortex_m_monitor_cpu(cpu).map(|hart| {
                    hart.monitor_hooks
                        .push((address.unwrap_or(hart.pc), hook.clone()));
                    None
                })
            }
            Action::ClicGpio(name, id, asserted) => machine.bus.clic.as_mut()
                .filter(|clic| clic.name == *name)
                .ok_or_else(|| format!("unsupported CLIC target {name:?}"))
                .and_then(|clic| clic.gpio(*id, *asserted)).map(|()| None),
            Action::GpioOverlay(links) => machine.bus.connect_stm32_gpios(
                links.iter().map(|(from, output, to, input)| (from.as_str(), *output, to.as_str(), *input)),
            ).map(|()| None),
            Action::GpioWire(from, output_pin, to, input_pin) => machine.bus
                .connect_stm32_gpio(from, *output_pin, to, *input_pin).map(|()| None),
            Action::LogUart(target) => machine.bus.log_uart(target).map(|()| None),
            Action::Illuminance(sensor, value) => machine.bus.set_illuminance(sensor, *value).map(|()| None),
            Action::IlluminanceFeed(sensor, path) => machine.bus.feed_illuminance(sensor, path).map(|()| None),
            Action::Echo(message) => writeln!(output.output, "{message}")
                .map(|()| None).map_err(|error| error.to_string()),
            Action::UserState(value) => {
                if let Some(value) = value {
                    machine.bus.monitor_user_state.clone_from(value);
                    Ok(None)
                } else {
                    writeln!(output.output, "{}", machine.bus.monitor_user_state)
                        .map(|()| None)
                        .map_err(|error| error.to_string())
                }
            }
            Action::LogLevel(level) => {
                machine.bus.set_monitor_log_level(*level);
                Ok(None)
            }
            Action::NvicLog(target) => machine.bus.enable_nvic_log(target).map(|()| None),
            Action::NvicWrite(target, offset, value) => machine
                .bus
                .nvic_register_address(target, *offset)
                .and_then(|address| {
                    machine
                        .bus
                        .write_scalar(address, 4, *value, &mut output, true)
                        .map_err(|e| e.to_string())
                })
                .map(|()| None),
            Action::ReadCpuPc(cpu) => machine.monitor_named_pc(cpu).and_then(|pc| {
                writeln!(output.output, "0x{pc:08x}")
                    .map_err(|error| error.to_string()).map(|()| None)
            }),
            Action::CpuRegister(cpu, register, value) => {
                machine.monitor_named_register(cpu, register, *value).and_then(|value| {
                    if let Some(value) = value {
                        writeln!(output.output, "0x{value:x}").map_err(|error| error.to_string())?;
                    }
                    Ok(None)
                })
            }
            // Synchronous script actions run at completed instruction boundaries.
            Action::SynchroniseQuantum => Ok(None),
            Action::CpuWait(cpu, asserted) => {
                machine.set_cortex_m_cpu_wait(cpu, *asserted).map(|()| None)
            }
            Action::CpuPc(cpu, pc) => machine.set_named_entry(cpu, *pc).map(|()| None),
            Action::CpuReset(cpu) => machine.reset_named_cortex_m(cpu).map(|()| None),
            Action::MachineReset(hooks) => machine.reset_cortex_m_machine().and_then(|()| {
                for hook in hooks {
                    match hook {
                        ResetHook::Sleep(duration) => std::thread::sleep(*duration),
                        ResetHook::Log(message) => {
                            writeln!(output.output, "{message}").map_err(|e| e.to_string())?;
                        }
                    }
                }
                Ok(None)
            }),
            Action::ReadScalar(address, width) => width
                .read(machine, *address)
                .and_then(|value| {
                    let digits = *width as usize * 2;
                    writeln!(output.output, "0x{value:0digits$x}").map_err(|e| e.to_string())
                })
                .map(|()| None),
            Action::WriteScalar(address, width, value) => {
                let width = *width as usize;
                if let Ok(address) = u32::try_from(*address) {
                    machine
                        .bus
                        .write_scalar(address, width, *value, &mut output, true)
                        .map_err(|error| error.to_string())
                        .map(|()| None)
                } else {
                    machine.bus.high_memory(*address, width).map(|memory| {
                        memory.copy_from_slice(&value.to_le_bytes()[..width]);
                        None
                    })
                }
            }
            Action::Tag(start, size, default, throws) => machine
                .bus
                .tag(*start, *size, *default, *throws)
                .map(|()| None),
            Action::UnhandledAccess(policy) => {
                machine.bus.unhandled_access = Some(*policy);
                Ok(None)
            }
            Action::ReadBytes(address, count) => {
                read_bus_bytes(machine, *address, *count, output.output).map(|()| None)
            }
            Action::WriteBytes(address, bytes) => machine
                .bus
                .write_bytes(*address, bytes, &mut output)
                .map_err(|error| error.to_string())
                .map(|()| None),
            Action::Button(name, pressed) => machine
                .external_event(ExternalEvent::Button(name.clone(), *pressed))
                .map(|()| None),
            Action::UnregisterGpio(target) => {
                machine.bus.unregister_stm32_gpio(target).map(|()| None)
            }
            Action::ButtonPulse(name) => machine
                .external_event(ExternalEvent::Button(name.clone(), true))
                .and_then(|()| machine.external_event(ExternalEvent::Button(name.clone(), false)))
                .map(|()| None),
            Action::NvicGpio(target, line, high) => machine
                .bus
                .drive_nvic_line(target, *line, *high)
                .map(|()| None),
            Action::NvicPending(target, exception) => machine
                .bus
                .pend_nvic_exception(target, *exception)
                .map(|()| None),
            Action::TimerProperty(target, enabled) => machine.bus.timer_properties(target).and_then(|(frequency, gate)| {
                let value = if *enabled { u32::from(gate) } else { frequency };
                writeln!(output.output, "{value}").map(|()| None).map_err(|error| error.to_string())
            }),
            Action::TraceRcc => machine.bus.trace_rcc().map(|()| None),
            Action::WriteRcc(offset, value) => machine.bus.rcc_base().and_then(|base| {
                let address = base.checked_add(*offset).ok_or("RCC address overflow")?;
                machine.bus.write_scalar(address, 4, *value, output.output, true)
                    .map(|()| None).map_err(|error| error.to_string())
            }),
            Action::Frequency(target, frequency) => {
                machine.bus.set_frequency(target, *frequency).map(|()| None)
            }
            Action::NvicClocked(target, clocked) => machine
                .bus
                .set_nvic_clocked(target, *clocked)
                .map(|()| None),
            Action::Adc(name, channel, volts) => machine
                .external_event(ExternalEvent::Adc(name.clone(), *channel, *volts))
                .map(|()| None),
            Action::CreateUartTester(target, binary) => machine
                .bus
                .create_uart_tester(target, *binary)
                .and_then(|id| writeln!(output.output, "{id}").map_err(|e| e.to_string()))
                .map(|()| None),
            Action::DefaultUartTester(id) => {
                machine.bus.set_default_uart_tester(*id).map(|()| None)
            }
            Action::SetUartFailure(id, pattern, register) => machine
                .bus
                .set_uart_failure(*id, pattern.clone(), *register)
                .map(|()| None),
            Action::WaitForBytes(id, pattern, match_start, timeout) => {
                let start = machine.elapsed_steps();
                machine.bus.watch_uart_bytes(*id, pattern, *match_start).and_then(|watch| {
                    let result = duration_cycles(*timeout, CORTEX_M_STEP_HZ, machine.bus.monitor_step_hz())
                        .and_then(|cycles| drive_uart_watch(machine, watch, cycles.min(budget), &mut output));
                    let matched = machine.bus.take_uart_byte_match(watch);
                    machine.bus.finish_stm32_uart_watch(watch);
                    result.and_then(|(stop, count)| {
                        retired += count;
                        budget -= machine.elapsed_steps().wrapping_sub(start);
                        let matched = matched.ok_or_else(|| format!("Terminal tester failed! Byte pattern was not received ({stop:?})"))?;
                        write_hex_list(matched.content, 2, output.output).map_err(|e| e.to_string())?;
                        for group in matched.groups {
                            match group {
                                Some(bytes) => write_hex_list(bytes, 2, output.output),
                                None => writeln!(output.output, "null"),
                            }.map_err(|e| e.to_string())?;
                        }
                        Ok(None)
                    })
                })
            }
            Action::WaitForTester(id, pattern, regex, mode, timeout, absent, _) => {
                let start = machine.elapsed_steps();
                machine.bus.watch_uart_tester_text(*id, pattern, regex.clone(), *mode).and_then(|watch| {
                    duration_cycles(*timeout, CORTEX_M_STEP_HZ, machine.bus.monitor_step_hz())
                        .and_then(|cycles| run_uart_watch(machine, watch, cycles.min(budget), &mut output))
                }).and_then(|(stop, count)| {
                    retired += count;
                    budget -= machine.elapsed_steps().wrapping_sub(start);
                    if stop == if *absent { Stop::Limit } else { Stop::OutputMatch } {
                        Ok(None)
                    } else {
                        Err(format!("Terminal tester failed! Line containing {pattern:?} was not received ({stop:?})"))
                    }
                })
            }
            Action::WaitForUart(target, pattern, end, timeout) => {
                let start = machine.elapsed_steps();
                let limit = timeout.map(|duration| duration_cycles(
                    duration, CORTEX_M_STEP_HZ, machine.bus.monitor_step_hz(),
                )).transpose()?.unwrap_or(budget).min(budget);
                wait_for_uart(
                    machine,
                    target.as_deref(),
                    pattern,
                    *end,
                    limit,
                    &mut output,
                )
                .and_then(|(stop, count)| {
                    retired += count;
                    budget -= machine.elapsed_steps().wrapping_sub(start);
                    if stop == Stop::OutputMatch {
                        Ok(None)
                    } else {
                        let source = target
                            .as_ref()
                            .map_or_else(String::new, |target| format!(" on {target}"));
                        let kind = if matches!(end, UartWaitMode::Prompt) {
                            "prompt"
                        } else {
                            "line"
                        };
                        Err(format!(
                            "UART {kind} containing {pattern:?} was not received{source} ({stop:?})"
                        ))
                    }
                })
            }
            Action::WaitForLed(target, state, timeout, hold) => {
                let rate = machine.bus.monitor_step_hz();
                let timeout = duration_cycles(*timeout, CORTEX_M_STEP_HZ, rate)?;
                let hold = duration_cycles(*hold, CORTEX_M_STEP_HZ, rate)?;
                wait_for_led(machine, target, *state, timeout, hold, &mut budget, &mut output)
                    .map(|count| {
                        retired += count;
                        None
                    })
            }
            Action::WriteUartLine(target, text, ending) => machine
                .external_event(ExternalEvent::UartLine(
                    target.clone(),
                    text.clone(),
                    ending.to_vec(),
                ))
                .map(|()| None),
        };
        if let Some(stop) =
            result.map_err(|e| format!("{}:{}: {e}", command.source.display(), command.line))?
        {
            return Ok((stop, retired));
        }
    }
    Ok((Stop::ScriptComplete, retired))
}

/// Execute one runtime command at a time on the same machine. Separate command
/// status from UART/monitor bytes so firmware output cannot create a reply frame.
pub fn serve(
    machine: &mut Machine,
    mut input: impl BufRead,
    mut output: impl Write,
    startup_output: &str,
    mut session: Session,
    root: &Path,
) -> Result<(), String> {
    fn frame(output: &mut impl Write, status: &str, bytes: &[u8]) -> io::Result<()> {
        writeln!(output, "{status} {}", bytes.len())?;
        output.write_all(bytes)?;
        output.flush()
    }

    let initial: Vec<_> = (0..machine.hart_count()).map(|id| machine.hart_mut(id).clone()).collect();
    let path = Path::new("<monitor>");
    let mut line = String::new();
    frame(&mut output, "READY", startup_output.as_bytes()).map_err(|error| error.to_string())?;
    loop {
        line.clear();
        if input.read_line(&mut line).map_err(|error| error.to_string())? == 0 {
            return Ok(());
        }
        let framed;
        let text = if let Some(length) = line.strip_prefix("PENV-COMMAND ") {
            framed = read_monitor_text(&mut input, length)?;
            framed.trim()
        } else {
            line.trim()
        };
        if matches!(text, "start" | "pause") {
            machine.primary.monitor_paused = text == "pause";
            frame(&mut output, "OK", &[]).map_err(|error| error.to_string())?;
            continue;
        }
        if let Some(result) = crate::monitor_asm::handle(machine, text) {
            match result {
                Ok(size) => frame(&mut output, "OK", size.to_string().as_bytes()),
                Err(error) => frame(&mut output, "UNSUPPORTED", error.as_bytes()),
            }.map_err(|error| error.to_string())?;
            continue;
        }
        if let Some(result) = crate::monitor_log::handle(machine, text) {
            match result {
                Ok((status, value)) => frame(&mut output, status, value.as_bytes()),
                Err(error) if error.starts_with("unsupported") => {
                    frame(&mut output, "UNSUPPORTED", error.as_bytes())
                }
                Err(error) => frame(&mut output, "ERROR", error.as_bytes()),
            }.map_err(|error| error.to_string())?;
            continue;
        }
        let mut boot = Boot {
            platform: Some(path.into()),
            image: Some(Asset::new("", path)),
            started: true,
            ..Boot::default()
        };
        boot.session.machine.clone_from(&session.machine);
        boot.session.number_mode = session.number_mode;
        let mut existing_machine = true;
        let mut included_variables = None;
        let parsed = command_words(text).and_then(|words| {
            if let ["include" | "i", value] = words.as_slice() {
                let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
                let source = include_path(value, &cwd.join(path), &cwd, root, &session.variables)?;
                if matches!(source.extension().and_then(|ext| ext.to_str()), Some("repl" | "cs" | "py")) {
                    return Err("live include requires a RESC command script".into());
                }
                let mut state = ScriptState {
                    boot: std::mem::take(&mut boot),
                    machine: true,
                    variables: session.variables.clone(),
                    ..ScriptState::default()
                };
                load_file(&source, root, &mut state)?;
                if !state.boot.post_run.is_empty() {
                    return Err("post-run script actions are unavailable in a live include".into());
                }
                if state.boot.runtime.iter().any(|command| matches!(command.action,
                    Action::SaveMemory(_) | Action::LoadMemory(_)))
                {
                    return Err("use live Save and Load as separate monitor commands".into());
                }
                boot = state.boot;
                included_variables = Some(state.variables);
                return Ok(());
            }
            if assign_variable(text, &words, session.machine.as_deref(), &mut session.variables)? {
                return Ok(());
            }
            execute(text, &words, &session.variables, &mut existing_machine, &mut boot, path, root, 1)
        });
        let mut bytes = Vec::new();
        let reply = match (parsed, boot.runtime.as_slice()) {
            (Ok(()), []) => {
                session.number_mode = boot.session.number_mode;
                if let Some(variables) = included_variables.take() {
                    session.variables = variables;
                }
                frame(&mut output, "OK", &[])
            }
            (Ok(()), [RuntimeCommand { action: Action::UartIdle(steps), .. }]) => {
                let result = duration_cycles(*steps, CORTEX_M_STEP_HZ, machine.bus.monitor_step_hz())
                    .and_then(|cycles| machine.bus.uart_activity().and_then(|(id, previous)| {
                        run_until_deadline(machine, cycles, &mut bytes, |bus, _| bus.uart_activity_changed(id, previous))
                    }));

                match result {
                    Ok((Stop::Limit, _)) => frame(&mut output, "MATCH", &[]),
                    Ok((Stop::OutputMatch, _)) => frame(&mut output, "FAIL", b"UART was not idle"),
                    Ok((stop, _)) => frame(&mut output, "STOP", format!("{stop:?}").as_bytes()),
                    Err(error) => frame(&mut output, "ERROR", error.as_bytes()),
                }
            }

            (Ok(()), [RuntimeCommand { action: Action::WaitForTester(id, expected, regex, mode, timeout, absent, capture_groups), .. }]) => {
                let result = machine.bus.watch_uart_tester_text(*id, expected, regex.clone(), *mode).and_then(|watch| {
                    let stop = duration_cycles(*timeout, CORTEX_M_STEP_HZ, machine.bus.monitor_step_hz())
                        .and_then(|cycles| drive_uart_watch(machine, watch, cycles, &mut bytes));
                    let failure = machine.bus.uart_watch_failure(watch).map(str::to_owned);
                    let matched = machine.bus.uart_tester_matched_line(watch);
                    machine.bus.finish_stm32_uart_watch(watch);
                    if let Some(line) = failure {
                        return Ok(("FAIL", format!("Terminal tester failed! Test failing entry has been found on UART: {line}")));
                    }
                    match (stop?, matched) {
                        ((Stop::OutputMatch, _), Some(line)) if *absent => Ok(("FAIL", format!("Unexpected UART entry: {line}"))),
                        ((Stop::Limit, _), _) if *absent => Ok(("MATCH", String::new())),
                        ((Stop::OutputMatch, _), Some(line)) if *capture_groups => {
                            Ok(("MATCHCAPTURES", uart_capture_payload(&line, regex.as_ref())))
                        }
                        ((Stop::OutputMatch, _), Some(line)) => Ok(("MATCH", line)),
                        ((Stop::Limit, _), _) => Ok(("FAIL", format!("Terminal tester failed! UART line containing {expected:?} was not received (Limit)"))),
                        ((stop, _), _) => Ok(("STOP", format!("{stop:?}"))),
                    }
                });
                match result {
                    Ok((status, value)) => frame(&mut output, status, value.as_bytes()),
                    Err(error) => frame(&mut output, "ERROR", error.as_bytes()),
                }
            }
            (Ok(()), [RuntimeCommand { action: Action::CpuReset(cpu), .. }]) if !machine.is_cortex_m() => {
                match machine.monitor_reset_cpu(cpu, &initial) {
                    Ok(()) => frame(&mut output, "OK", &[]),
                    Err(error) => frame(&mut output, "ERROR", error.as_bytes()),
                }
            }
            (Ok(()), commands) if included_variables.is_some() || commands.len() == 1 => {
                match run_commands(machine, commands, u64::MAX, &mut bytes, Some(&mut session)) {
                    Ok((Stop::ScriptComplete, _)) => {
                        session.number_mode = boot.session.number_mode;
                        if let Some(variables) = included_variables.take() {
                            session.variables = variables;
                        }
                        frame(&mut output, "OK", &bytes)
                    }
                    Ok((stop, _)) => {
                        frame(&mut output, "STOP", format!("{stop:?}").as_bytes())
                    }
                    Err(error) if error.starts_with("<monitor>:1: unsupported ADXL372 target ") => {
                        frame(&mut output, "UNSUPPORTED", error.as_bytes())
                    }
                    Err(error) => {
                        frame(&mut output, "ERROR", error.as_bytes())
                    }
                }
            }
            (result, _) => frame(&mut output, "UNSUPPORTED",
                                result.err().as_deref().unwrap_or("not a runtime command").as_bytes()),
        };
        reply.map_err(|error| error.to_string())?;
    }
}

fn monitor_timestamp(ns: u128) -> String {
    let seconds = ns / 1_000_000_000;
    format!("{:02}:{:02}:{:02}.{:09}",
        seconds / 3600, seconds / 60 % 60, seconds % 60, ns % 1_000_000_000)
}

pub(crate) fn duration_cycles(duration: u64, units_per_second: u64, rate: u64) -> Result<u64, String> {
    let cycles = (u128::from(duration) * u128::from(rate))
        .div_ceil(u128::from(units_per_second));
    u64::try_from(cycles).map_err(|_| "duration exceeds virtual time".to_owned())
}

fn read_bus_bytes(
    machine: &mut Machine,
    address: u32,
    count: usize,
    output: &mut impl Write,
) -> Result<(), String> {
    let bytes = machine
        .bus
        .read_bytes(address, count)
        .map_err(|error| error.to_string())?;
    write!(output, "[").map_err(|error| error.to_string())?;
    for (index, byte) in bytes.enumerate() {
        let byte = byte.map_err(|error| error.to_string())?;
        let separator = if index == 0 { "" } else { ", " };
        write!(output, "{separator}0x{byte:02x}").map_err(|error| error.to_string())?;
    }
    writeln!(output, "]").map_err(|error| error.to_string())
}

fn uart_capture_payload(line: &str, regex: Option<&regex::Regex>) -> String {
    use std::fmt::Write;

    // Tab-separated hex preserves UTF-8 and empty captures.
    let mut payload = String::new();
    let captures = regex.and_then(|regex| regex.captures(line));
    let groups = captures.iter().flat_map(|captures| captures.iter().skip(1));
    for (index, field) in std::iter::once(line)
        .chain(groups.map(|group| group.map_or("", |group| group.as_str())))
        .enumerate()
    {
        if index != 0 {
            payload.push('\t');
        }
        for byte in field.bytes() {
            write!(payload, "{byte:02x}").expect("writing to String");
        }
    }
    payload
}

fn write_hex_list<T: std::fmt::LowerHex>(
    values: impl IntoIterator<Item = T>,
    digits: usize,
    output: &mut impl Write,
) -> io::Result<()> {
    write!(output, "[")?;
    for (index, value) in values.into_iter().enumerate() {
        let separator = if index == 0 { "" } else { ", " };
        write!(output, "{separator}0x{value:0digits$x}")?;
    }
    writeln!(output, "]")
}

fn wait_for_led(
    machine: &mut Machine,
    target: &str,
    state: bool,
    timeout: u64,
    hold: u64,
    budget: &mut u64,
    output: &mut impl Write,
) -> Result<u64, String> {
    let index = machine.bus.gpio_led_index(target)?;
    let mut retired = 0;
    if machine.bus.gpio_led(index).state != state {
        let start = machine.elapsed_steps();
        let limit = timeout.min(*budget);
        let (stop, count) =
            run_until_deadline(machine, limit, output, |bus, _| bus.gpio_led(index).state == state)?;
        *budget -= machine.elapsed_steps().wrapping_sub(start);
        if stop != Stop::OutputMatch {
            return Err(format!(
                "LED {target} did not reach state {state} ({stop:?})"
            ));
        }
        retired += count;
    }

    let transitions = machine.bus.gpio_led(index).transitions;
    let limit = hold.min(*budget);
    let (stop, count) = run_until_deadline(machine, limit, output, |bus, _| {
        bus.gpio_led(index).transitions != transitions
    })?;
    if stop == Stop::OutputMatch {
        return Err(format!("LED {target} changed while holding state {state}"));
    }
    if stop != Stop::Limit || limit != hold {
        return Err(format!(
            "LED {target} hold ended before completion ({stop:?})"
        ));
    }
    *budget -= limit;
    Ok(retired + count)
}

pub(crate) fn run_until_deadline<W: Write>(
    machine: &mut Machine,
    cycles: u64,
    output: &mut W,
    matched: impl Fn(&crate::bus::Bus, &W) -> bool,
) -> Result<(Stop, u64), String> {
    let start = machine.elapsed_steps();
    let (stop, retired) = machine.run_until(cycles, output, matched)?;
    if stop == Stop::Limit && machine.elapsed_steps().wrapping_sub(start) < cycles {
        return Ok((Stop::Paused, retired));
    }
    Ok((stop, retired))
}

fn drive_uart_watch(
    machine: &mut Machine,
    watch: UartWatch,
    budget: u64,
    output: &mut impl Write,
) -> Result<(Stop, u64), String> {
    if machine.bus.stm32_uart_matched(watch) {
        Ok((Stop::OutputMatch, 0))
    } else {
        run_until_deadline(machine, budget, output, |bus, _| bus.stm32_uart_matched(watch))
    }
}

fn run_uart_watch(
    machine: &mut Machine,
    watch: UartWatch,
    budget: u64,
    output: &mut impl Write,
) -> Result<(Stop, u64), String> {
    let result = drive_uart_watch(machine, watch, budget, output).and_then(|result| {
        if let Some(line) = machine.bus.uart_watch_failure(watch) {
            return Err(format!(
                "Terminal tester failed! Test failing entry has been found on UART:\n{line}"
            ));
        }
        Ok(result)
    });
    machine.bus.finish_stm32_uart_watch(watch);
    result
}

fn wait_for_uart(
    machine: &mut Machine,
    target: Option<&str>,
    pattern: &str,
    end: UartWaitMode,
    budget: u64,
    output: &mut ScriptOutput<'_, impl Write>,
) -> Result<(Stop, u64), String> {
    if let Some(target) = target {
        let watch = machine.bus.watch_stm32_uart(target, pattern, end)?;
        run_uart_watch(machine, watch, budget, output)
    } else {
        let history = output
            .history
            .as_mut()
            .expect("a script wait enables history before execution");
        history.begin_wait(pattern, end);
        let result = if history.matched() {
            Ok((Stop::OutputMatch, 0))
        } else {
            machine.run_until(budget, output, |_, uart| {
                uart.history.as_ref().is_some_and(UartHistory::matched)
            })
        };
        if let Some(history) = &mut output.history {
            history.finish_wait();
        }
        result
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CpuRegisterCommand {
    TrapVector(TrapVector, Option<u32>),
    Integer(u32, Option<u32>),
    ExternalPmp(String, bool, u32, Option<u32>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CpuControl {
    Pc(String, Option<u32>),
    ResetVector(u32),
    Reset,
}

pub enum PostRun {
    Echo(String),
    DisposePcm(String),
}

pub(crate) struct SdAttachment {
    file: Asset,
    controller: String,
    capacity: u64,
}

impl SdAttachment {
    pub(crate) fn parse<'a>(
        args: &[&'a str],
        resolve: impl Fn(&'a str) -> Result<&'a str, String>,
        script: &Path,
    ) -> Result<Self, String> {
        let (file, controller, capacity) = match args {
            [file, controller, capacity]
            | [
                file,
                controller,
                capacity,
                "False" | "false" | "persistent=false" | "persistent=False",
            ] => (*file, *controller, *capacity),
            _ => return Err("expected SdCardFromFile path controller capacity [false]".into()),
        };
        let value = unquote(resolve(file)?);
        let value = unquote(value.strip_prefix('@').unwrap_or(value));
        let file = Asset::new(value, script);
        let controller = unquote(resolve(controller)?);
        let capacity = crate::parse_address64(resolve(capacity)?)?;
        Ok(Self {
            file,
            controller: controller.to_owned(),
            capacity,
        })
    }

    pub(crate) fn apply(
        &self,
        bus: &mut crate::bus::Bus,
        root: &Path,
        assets: Option<&Path>,
    ) -> Result<(), String> {
        if self.file.value.is_empty() {
            return Err("No image file provided.".into());
        }
        let path = self.file.resolve(root, assets)?;
        bus.attach_sd_file(&self.controller, &path, self.capacity)
    }
}

impl Boot {
    pub(crate) fn is_sd_setup_only(&self) -> bool {
        !self.sd_cards.is_empty() && self.runtime.is_empty() && !self.started
    }
}

#[derive(Debug)]
#[derive(Clone)]
pub struct Asset {
    value: String,
    script: PathBuf,
}

#[derive(Default)]
pub struct Network {
    switch: Option<String>,
    server: Option<(String, Ipv4Addr)>,
    links: Vec<String>,
    port: Option<u16>,
    file: Option<(Asset, String)>,
}

pub struct TftpBoot {
    pub ethernet: String,
    pub ip: Ipv4Addr,
    pub port: u16,
    pub file: Asset,
    pub filename: String,
}

impl Network {
    pub fn into_tftp(self) -> Result<Option<TftpBoot>, String> {
        if self.switch.is_none() && self.server.is_none() {
            return Ok(None);
        }
        let Some((server, ip)) = self.server else {
            return Err("expected a local network server".into());
        };
        if !self.links.contains(&server) {
            return Err("expected the local network server to be connected to the switch".into());
        }
        let port = self
            .port
            .ok_or("expected StartTFTP for the local network server")?;
        let (file, filename) = self.file.ok_or("expected a file served over local TFTP")?;
        let mut peers = self.links.into_iter().filter(|name| *name != server);
        let ethernet = peers
            .next()
            .ok_or("expected an Ethernet connection to the local switch")?;
        if peers.next().is_some() {
            return Err("only one Ethernet connection to the local switch is supported".into());
        }
        Ok(Some(TftpBoot {
            ethernet,
            ip,
            port,
            file,
            filename,
        }))
    }

    fn is_server(&self, name: &str) -> bool {
        self.server
            .as_ref()
            .is_some_and(|(server, _)| server == name)
    }
}

fn unquote(text: &str) -> &str {
    text.strip_prefix('"')
        .and_then(|text| text.strip_suffix('"'))
        .unwrap_or(text)
}

struct Macro {
    script: PathBuf,
    commands: Vec<(usize, String)>,
}

#[derive(Default)]
struct ScriptState {
    variables: HashMap<String, String>,
    macros: HashMap<String, Macro>,
    visiting: HashSet<PathBuf>,
    boot: Boot,
    machine: bool,
}

pub fn load(path: &Path, root: &Path) -> Result<Boot, String> {
    load_script(path, root, false, None)
}

pub fn load_for_session(path: &Path, root: &Path) -> Result<Boot, String> {
    load_script(path, root, true, None)
}

pub fn load_session_stream(mut input: impl BufRead, root: &Path) -> Result<Boot, String> {
    let mut header = String::new();
    input.by_ref().take(64).read_line(&mut header).map_err(|e| e.to_string())?;
    let length = header.strip_prefix("PENV-STARTUP ")
        .ok_or("expected PENV-STARTUP followed by the startup byte count")?;
    let body = read_monitor_text(&mut input, length)?;
    let source = std::env::current_dir().map_err(|e| e.to_string())?.join("<monitor-startup>");
    load_script(&source, root, true, Some(&body))
}

fn read_monitor_text(input: &mut impl Read, length: &str) -> Result<String, String> {
    let length = length.strip_suffix('\n').and_then(|length| length.trim_end().parse::<usize>().ok())
        .filter(|length| *length <= MAX_MONITOR_PAYLOAD)
        .ok_or("invalid monitor frame byte count")?;
    let mut body = vec![0; length];
    input.read_exact(&mut body).map_err(|e| format!("reading monitor frame: {e}"))?;
    String::from_utf8(body).map_err(|e| e.to_string())
}

fn load_script(path: &Path, root: &Path, session: bool, source: Option<&str>) -> Result<Boot, String> {
    let mut state = ScriptState::default();
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    state.variables.insert("global.CWD".into(), format!("@\"{}\"", cwd.display()));
    state.variables.insert("global.ORIGIN".into(), format!("@\"{}\"", cwd.display()));
    if let Some(source) = source {
        load_text(path, path.to_owned(), root, &mut state, source)?;
    } else {
        load_file(path, root, &mut state)?;
    }
    let mut boot = state.boot;
    boot.session.variables = state.variables;
    boot.materialize_platform()?;
    if let [
        RuntimeCommand {
            action: Action::RunFor(steps),
            ..
        },
    ] = boot.runtime.as_slice()
    {
        boot.run_steps = Some(*steps);
        boot.runtime.clear();
    } else if !boot.post_run.is_empty() {
        return Err("post-run operations require a single RunFor".into());
    }
    if (boot.image.is_some() || !boot.binaries.is_empty())
        && !boot.runtime.is_empty()
        && !session
        && boot.runtime.iter().all(|command| {
            matches!(
                command.action,
                Action::BusScalar(BusScalar { value: Some(_), .. })
            )
        })
    {
        boot.runtime.push(RuntimeCommand {
            line: 0,
            source: path.to_owned(),
            action: Action::RunRemaining,
        });
    }
    if !state.machine
        || boot.platform.is_none()
        || (!session && boot.image.is_none()
            && boot.binaries.is_empty()
            && !is_static_memory_script(&boot.runtime)
            && !is_configuration_only(&boot.runtime)
            && !boot.is_sd_setup_only()
            && boot.cpu_control.is_empty()
            && boot.cpu_registers.is_empty())
    {
        return Err(format!(
            "{}: expected machine creation, platform and firmware loading",
            path.display()
        ));
    }
    Ok(boot)
}

fn load_file(path: &Path, root: &Path, state: &mut ScriptState) -> Result<(), String> {
    let canonical = fs::canonicalize(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let content = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    load_text(path, canonical, root, state, &content)
}

fn load_text(path: &Path, canonical: PathBuf, root: &Path, state: &mut ScriptState, content: &str) -> Result<(), String> {
    if state.visiting.len() >= 64 || !state.visiting.insert(canonical.clone()) {
        return Err(format!(
            "{}: recursive or excessively deep script include",
            path.display()
        ));
    }
    let origin = canonical.parent().unwrap_or(Path::new("."));
    let previous_origin = state.variables.insert(
        "global.ORIGIN".into(),
        format!("@{}", origin.display()),
    );
    let mut cursor = content.lines().enumerate();
    while let Some((line, raw)) = cursor.next() {
        let text = raw.trim();
        let error = |message: &str| format!("{}:{}: {message}", path.display(), line + 1);
        if text.is_empty() || text.starts_with('#') || text.starts_with(':') {
            continue;
        }
        if let Some(value) = text.strip_prefix("machine LoadPlatformDescriptionFromString ") {
            if !state.machine {
                return Err(error("inline REPL requires a machine"));
            }
            let value = value.trim();
            let source = if let Some(first) = value.strip_prefix("\"\"\"") {
                let mut body = first.to_owned();
                while !body.ends_with("\"\"\"") {
                    let (_, next) = cursor.next().ok_or_else(|| error("unterminated inline REPL"))?;
                    body.push('\n');
                    body.push_str(next);
                }
                body.truncate(body.len() - 3);
                body
            } else {
                let value = resolve(&state.variables, value).map_err(|e| error(&e))?;
                let inner = value.strip_prefix('"').and_then(|v| v.strip_suffix('"'))
                    .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
                    .ok_or_else(|| error("expected quoted inline REPL"))?;
                inner.replace("\\\"", "\"")
            };
            if state.boot.started || !state.boot.runtime.is_empty() {
                let links = parse_gpio_overlay(&source, root).map_err(|message| error(&message))?;
                state.boot.runtime.push(RuntimeCommand {
                    line: line + 1, source: path.to_owned(), action: Action::GpioOverlay(links),
                });
            } else {
                state.boot.inline_repl.push((origin.to_owned(), source));
            }
            continue;
        }
        if let Some(name) = text
            .strip_prefix("set ")
            .filter(|name| !name.contains(char::is_whitespace))
        {
            if cursor
                .next()
                .is_none_or(|(_, line)| line.trim() != "\"\"\"")
            {
                return Err(error("expected triple-quoted hook body"));
            }
            let mut raw_body = String::new();
            loop {
                let (_, command) = cursor.next().ok_or_else(|| error("unterminated variable"))?;
                if command.trim() == "\"\"\"" {
                    break;
                }
                raw_body.push_str(command);
                raw_body.push('\n');
            }
            let name = name.trim_start_matches('$');
            let body: Vec<_> = raw_body.lines().map(str::trim)
                .filter(|line| !line.is_empty() && !line.starts_with('#')).collect();
            if let [assign, halt, rest @ ..] = body.as_slice()
                && let Some((variable, suffix)) = assign.split_once(" = machine['sysbus.")
                && let Some(hart) = suffix.strip_suffix("']")
                && !variable.is_empty()
                && !hart.is_empty()
                && *halt == format!("{variable}.IsHalted = True")
                && matches!(rest, [] | ["print(\"halted management CPU\")"])
            {
                state.boot.hooks.insert(name.into(), hart.into());
            } else {
                state.boot.hooks.remove(name);
            }
            state.variables.insert(name.into(), format!("\"{raw_body}\""));
            continue;
        }
        if let Some(declaration) = text.strip_prefix("macro ") {
            let declaration = declaration.trim();
            if let Some((name, body)) = declaration.split_once(char::is_whitespace) {
                let body = body
                    .trim()
                    .strip_prefix("\"\"\"")
                    .and_then(|body| body.strip_suffix("\"\"\""))
                    .ok_or_else(|| error("expected triple-quoted macro body"))?;
                state.macros.insert(
                    name.into(),
                    Macro {
                        script: path.into(),
                        commands: vec![(line, body.into())],
                    },
                );
                continue;
            }
            let name = declaration;
            if cursor
                .next()
                .is_none_or(|(_, line)| line.trim() != "\"\"\"")
            {
                return Err(error("expected triple-quoted macro body"));
            }
            let mut body = Vec::new();
            loop {
                let (index, command) = cursor.next().ok_or_else(|| error("unterminated macro"))?;
                if command.trim() == "\"\"\"" {
                    break;
                }
                body.push((index, command.trim().to_string()));
            }
            state.macros.insert(
                name.into(),
                Macro {
                    script: path.into(),
                    commands: body,
                },
            );
            continue;
        }
        let words = command_words(text).map_err(|e| error(&e))?;
        if assign_variable(text, &words, state.boot.session.machine.as_deref(), &mut state.variables).map_err(|e| error(&e))? {
            continue;
        }
        match words.as_slice() {
            ["include" | "i", value] => {
                let included = include_path(value, path, origin, root, &state.variables)
                    .map_err(|e| error(&e))?;
                match included.extension().and_then(|suffix| suffix.to_str()) {
                    Some("repl") if !state.boot.has_platform() => {
                        state.machine = true;
                        state.boot.platform = Some(included);
                    }
                    Some("repl" | "cs" | "py") => {
                        return Err(error("unsupported command: include needs an available loader"));
                    }
                    _ => load_file(&included, root, state).map_err(|e| error(&e))?,
                }
            }
            ["runMacro", name] if name.starts_with('$') => {
                let body = state
                    .macros
                    .get(&name[1..])
                    .ok_or_else(|| error("unknown macro"))?;
                for (index, command) in &body.commands {
                    let words = command_words(command)
                        .map_err(|e| format!("{}:{}: {e}", body.script.display(), index + 1))?;
                    execute(
                        command,
                        &words,
                        &state.variables,
                        &mut state.machine,
                        &mut state.boot,
                        &body.script,
                        root,
                        index + 1,
                    )
                    .map_err(|e| format!("{}:{}: {e}", body.script.display(), index + 1))?;
                }
            }
            _ => execute(
                text,
                &words,
                &state.variables,
                &mut state.machine,
                &mut state.boot,
                path,
                root,
                line + 1,
            )
            .map_err(|e| error(&e))?,
        }
        if let Some(RuntimeCommand {
            line: command_line,
            action: Action::MachineReset(hooks),
            ..
        }) = state.boot.runtime.last_mut()
            && *command_line == line + 1
            && let Some(body) = state.macros.get("reset")
        {
            for (index, command) in &body.commands {
                parse_reset_hooks(command.trim(), hooks)
                    .map_err(|e| format!("{}:{}: {e}", body.script.display(), index + 1))?;
            }
        }
    }
    if let Some(previous_origin) = previous_origin {
        state.variables.insert("global.ORIGIN".into(), previous_origin);
    } else {
        state.variables.remove("global.ORIGIN");
    }
    state.visiting.remove(&canonical);
    Ok(())
}

fn include_path(
    value: &str, path: &Path, origin: &Path, root: &Path, variables: &HashMap<String, String>,
) -> Result<PathBuf, String> {
    if let Some(relative) = value
        .strip_prefix("$ORIGIN/")
        .or_else(|| value.strip_prefix("$global.ORIGIN/"))
    {
        return Ok(origin.join(path_value(relative)));
    }
    if let Some(relative) = value
        .strip_prefix("$CWD/")
        .or_else(|| value.strip_prefix("$global.CWD/"))
    {
        let cwd = resolve(variables, "$global.CWD")?;
        return Ok(Path::new(unquote(cwd.trim_start_matches('@'))).join(path_value(relative)));
    }
    let value = resolve(variables, value)?;
    let file = path_value(unquote(value.trim_start_matches('@')));
    let candidate = root.join(&file);
    Ok(if candidate.is_file() { candidate } else { local_path(&file, path, root) })
}

fn assign_variable(
    text: &str,
    words: &[&str],
    machine: Option<&str>,
    variables: &mut HashMap<String, String>,
) -> Result<bool, String> {
    let (name, value, conditional) = match words {
        ["set", name, value] => (name.trim_start_matches('$'), *value, false),
        _ if text.starts_with('$') => {
            let (name, value) = text[1..].split_once('=').ok_or("expected variable assignment")?;
            let name = name.trim();
            let name_without_suffix = name.trim_end_matches('?').trim();
            (name_without_suffix, value.trim(), name.ends_with('?'))
        }
        _ => return Ok(false),
    };
    if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.') || value.is_empty() {
        return Err("invalid monitor variable assignment".into());
    }
    let scope = machine.map(|name| name.replace(|c: char| !c.is_alphanumeric() && c != '_', "_"));
    let local = scope.as_ref().and_then(|scope| name.strip_prefix(scope)?.strip_prefix('.'));
    if !conditional || variable_binding(variables, name).is_none() {
        if name.starts_with("global.") {
            variables.insert(name.to_owned(), value.to_owned());
        } else if let Some(scope) = scope {
            let local = local.unwrap_or(name).to_owned();
            variables.insert(format!("{scope}.{local}"), value.to_owned());
            variables.insert(local, value.to_owned());
        } else {
            variables.insert(format!("global.{name}"), value.to_owned());
        }
    }
    Ok(true)
}

fn variable_binding<'a>(variables: &'a HashMap<String, String>, name: &str) -> Option<&'a String> {
    variables.get(name).or_else(|| {
        name.strip_prefix("global.")
            .and_then(|short| variables.get(short))
            .or_else(|| variables.get(&format!("global.{name}")))
    })
}

fn resolve<'a>(variables: &'a HashMap<String, String>, mut value: &'a str) -> Result<&'a str, String> {
    let mut remaining = variables.len();
    while let Some(name) = value.strip_prefix('$').or_else(|| value.strip_prefix("@$")) {
        if name.is_empty() {
            return Err("missing variable name".into());
        }
        if remaining == 0 {
            return Err(format!("unknown or recursive variable {name}"));
        }
        value = variable_binding(variables, name).ok_or_else(|| format!("unknown variable {name}"))?;
        remaining -= 1;
    }
    Ok(value)
}

fn parse_reset_hooks(mut commands: &str, hooks: &mut Vec<ResetHook>) -> Result<(), String> {
    while !commands.is_empty() && !commands.starts_with('#') {
        let (verb, argument) = commands
            .split_once(char::is_whitespace)
            .ok_or("expected reset macro sleep or log")?;
        let argument = argument.trim_start();
        let rest = match verb {
            "sleep" => {
                let end = argument.find(';').unwrap_or(argument.len());
                let seconds = argument[..end]
                    .trim()
                    .parse::<f64>()
                    .map_err(|_| "expected reset macro sleep duration in seconds")?;
                let duration = Duration::try_from_secs_f64(seconds)
                    .map_err(|_| "invalid reset macro sleep duration")?;
                hooks.push(ResetHook::Sleep(duration));
                &argument[end..]
            }
            "log" => {
                let (message, rest) = argument
                    .strip_prefix('"')
                    .and_then(|arg| arg.split_once('"'))
                    .ok_or("expected quoted reset macro log message")?;
                hooks.push(ResetHook::Log(message.into()));
                rest
            }
            _ => return Err(format!("unsupported reset macro operation {verb:?}")),
        };
        commands = rest.trim_start();
        if !commands.is_empty() {
            commands = commands
                .strip_prefix(';')
                .ok_or("expected ';' between reset macro commands")?
                .trim_start();
        }
    }
    Ok(())
}

fn parse_gpio_overlay(source: &str, root: &Path) -> Result<Vec<(String, u8, String, u8)>, String> {
    let unsupported = || "unsupported live REPL overlay: expected GPIO pin connections on existing devices".to_string();
    let entries = crate::repl_parser::parse_inline(source, root)?;
    let mut links = Vec::new();
    for entry in entries {
        if !entry.model.is_empty() || entry.registration.is_some() || entry.body.is_empty() {
            return Err(unsupported());
        }
        for (_, wire) in entry.body {
            let (output, destination) = wire.split_once("->").ok_or_else(unsupported)?;
            let (target, input) = destination.split_once('@').ok_or_else(unsupported)?;
            let output = output.trim().parse::<u8>().map_err(|_| unsupported())?;
            let input = input.trim().parse::<u8>().map_err(|_| unsupported())?;
            links.push((entry.name.clone(), output, target.trim().into(), input));
        }
    }
    if links.is_empty() {
        return Err(unsupported());
    }
    Ok(links)
}

fn execute(
    text: &str,
    words: &[&str],
    variables: &HashMap<String, String>,
    machine: &mut bool,
    boot: &mut Boot,
    script: &Path,
    root: &Path,
    line: usize,
) -> Result<(), String> {
    match words {
        [] => return Ok(()),
        ["using", "sysbus"] => return Ok(()),
        ["numbersMode", mode] => {
            boot.session.number_mode = match *mode {
                "Hexadecimal" => NumberMode::Hexadecimal,
                "Decimal" => NumberMode::Decimal,
                "Both" => NumberMode::Both,
                _ => return Err(format!("unknown numbersMode {mode:?}")),
            };
            return Ok(());
        }
        _ => {}
    }
    let resolve = |value| resolve(variables, value);
    if let ["machine", "UserState", value] = words
        && !boot.started && boot.runtime.is_empty()
    {
        let value = resolve(value)?;
        boot.monitor_user_state = Some(path_value(unquote(value.strip_prefix('@').unwrap_or(value))));
        return Ok(());
    }
    if let ["echo", value] = words
        && (value.starts_with('$') || boot.started && boot.runtime.is_empty())
    {
        let message = unquote(resolve(value)?).trim_start_matches('@').to_owned();
        if boot.started {
            boot.runtime.push(RuntimeCommand { line, source: script.to_owned(), action: Action::Echo(message) });
        } else {
            boot.monitor_output.push(message);
        }
        return Ok(());
    }
    if let ["logLevel", value] = words {
        let value = resolve(value)?;
        let level = match unquote(value) {
            "-1" | "Noisy" => -1,
            "0" | "Debug" => 0,
            "1" | "Info" => 1,
            "2" | "Warning" => 2,
            "3" | "Error" => 3,
            _ => return Err(format!("unsupported monitor log level {value:?}")),
        };
        if boot.started || !boot.runtime.is_empty() {
            boot.runtime.push(RuntimeCommand {
                line,
                source: script.to_owned(),
                action: Action::LogLevel(level),
            });
        } else {
            boot.monitor_log_level = Some(level);
        }
        return Ok(());
    }
    if let [cpu, "WfiAsNop", value] = words
        && !boot.started && boot.runtime.is_empty() && boot.has_platform()
    {
        let enabled = match unquote(resolve(value)?) {
            "true" => true,
            "false" => false,
            _ => return Err("expected true or false for WfiAsNop".into()),
        };
        boot.riscv_wfi_as_nop.push((cpu.trim_start_matches("sysbus.").to_owned(), enabled));
        return Ok(());
    }
    if let ["sysbus", "LogPeripheralAccess", name, options @ ..] = words
        && !boot.started && boot.runtime.is_empty() && boot.has_platform()
    {
        let enabled = match options {
            [] | ["true"] => true,
            ["false"] => false,
            _ => return Err("expected optional true or false for peripheral access logging".into()),
        };
        let name = unquote(resolve(name)?);
        boot.trace_memory.push((name.trim_start_matches("sysbus.").to_owned(), enabled));
        return Ok(());
    }
    if let ["emulation", "SetGlobalQuantum" | "SetQuantum", value] = words
        && !boot.started && boot.runtime.is_empty()
    {
        let quantum = duration_steps(resolve(value)?).map_err(|error| format!("invalid emulation quantum: {error}"))?;
        if quantum == 0 {
            return Err("emulation quantum must be positive".into());
        }
        boot.quantum = Some(quantum);
        return Ok(());
    }
    let action = match words {
        [target, "VendorSpecific1", value] if boot.started => Some(Action::PhyRegister(
            (*target).to_owned(), u16::try_from(crate::parse_address(resolve(value)?)?)
                .map_err(|_| "PHY register exceeds 16 bits")?)),
        [target, axis @ ("AccelerationX" | "AccelerationY" | "AccelerationZ"), value] if boot.started => {
            let value = resolve(value)?.parse::<f64>().map_err(|_| "invalid acceleration")?;
            if !(-200.0..=200.0).contains(&value) {
                return Err("acceleration must be within ±200 g".into());
            }
            let axis = match *axis { "AccelerationX" => 0, "AccelerationY" => 1, _ => 2 };
            let target = target.trim_start_matches("sysbus.").to_owned();
            Some(Action::AdxlAxis(target, axis, (value * 10.0).round() as i16))
        }
        [target, "State"] if boot.started && target.rsplit('.').next().is_some_and(|part| part.starts_with("led")) => {
            Some(Action::LiteXLedState((*target).to_owned()))
        }

        [cpu, "VectorRegisterLength", bits] if boot.started => Some(Action::VectorRegisterLength(
            cpu.trim_start_matches("sysbus.").to_owned(),
            crate::parse_address64(resolve(bits)?)?,
        )),
        ["TraceStackAccess", cpu] if boot.started => Some(Action::TraceStackAccess(
            cpu.trim_start_matches("sysbus.").to_owned(),
        )),
        ["cpu" | "sysbus.cpu", "PerformanceInMips"] if boot.started => Some(Action::ReadCpuMips(boot.session.number_mode)),
        ["cpu" | "sysbus.cpu", "PerformanceInMips", value] if boot.started => {
            let mips = resolve(value)?.parse().map_err(|_| "PerformanceInMips requires a positive integer")?;
            Some(Action::CpuMips(mips))
        }
        ["emulation", "GetTimeSourceInfo"] if boot.started => Some(Action::TimeSourceInfo),
        ["TestMonitorUartIdle", timeout] if boot.started => Some(Action::UartIdle(duration_steps(resolve(timeout)?)?)),

        [target, "Frequency"] if boot.started => Some(Action::TimerProperty(
            target.trim_start_matches("sysbus.").to_owned(), false)),
        ["sysbus", "IsPeripheralEnabled", target] if boot.started => Some(Action::TimerProperty(
            target.trim_start_matches("sysbus.").to_owned(), true)),
        ["sysbus", "LogPeripheralAccess", "rcc" | "sysbus.rcc"] if boot.started => Some(Action::TraceRcc),
        ["rcc" | "sysbus.rcc", "WriteDoubleWord", offset, value] if boot.started => Some(Action::WriteRcc(
            crate::parse_address(resolve(offset)?)?, crate::parse_address(resolve(value)?)?)),

        ["machine", "LoadPlatformDescriptionFromString", value] if boot.started => {
            let source = crate::repl_parser::quoted_value(resolve(value)?)
                .map_err(|_| "expected quoted inline REPL")?;
            Some(Action::GpioOverlay(parse_gpio_overlay(&source, root)?))
        }
        ["ConnectMonitorGpio", source, output, target, input] if boot.started => Some(Action::GpioWire(
            (*source).to_owned(), output.parse().map_err(|_| "invalid source GPIO pin")?,
            (*target).to_owned(), input.parse().map_err(|_| "invalid target GPIO pin")?,
        )),

        [target, "OnGPIO", id, asserted] if boot.started => Some(Action::ClicGpio(
            resolve(target)?.trim_start_matches("sysbus.").to_owned(),
            crate::parse_address(resolve(id)?)?, parse_tester_bool(resolve(asserted)?)?,
        )),
        ["showAnalyzer", target] | ["showAnalyzer", target, "LoggingUartAnalyzer"] if boot.started => {
            Some(Action::LogUart(resolve(target)?.to_owned()))
        }

        [sensor, "Illuminance", value] if boot.started && sensor.starts_with("sysbus.") => {
            let value = resolve(value)?.parse::<f64>().map_err(|_| "invalid illuminance")?;
            Some(Action::Illuminance((*sensor).to_owned(), value))
        }
        [sensor, "FeedIlluminanceSamplesFromRESD", file]
            if boot.started && sensor.starts_with("sysbus.") =>
        {
            let path = resolve(file)?.strip_prefix('@').ok_or("expected @RESD-path")?;
            Some(Action::IlluminanceFeed((*sensor).to_owned(), PathBuf::from(path_value(path))))
        }

        [verb @ ("Save" | "Load"), path] => {
            let path = resolve(path)?
                .strip_prefix('@')
                .ok_or("expected @snapshot-path")?;
            let path = script
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(path_value(unquote(path)));
            Some(if *verb == "Save" {
                Action::SaveMemory(path)
            } else {
                Action::LoadMemory(path)
            })
        }
        ["machine", "UserState", value @ ..] if value.len() <= 1 => {
            let value = value.first().map(|value| {
                let value = resolve(value)?;
                Ok::<_, String>(path_value(unquote(value.strip_prefix('@').unwrap_or(value))))
            }).transpose()?;
            Some(Action::UserState(value))
        }
        ["Clear"] => Some(Action::ClearMemory),
        ["mach" | "machine", "set", name] if *machine => {
            let name = unquote(resolve(name)?);
            if name != "0" && boot.session.machine.as_deref() != Some(name) {
                return Err(format!("unknown machine {name:?}"));
            }
            if !boot.started && boot.runtime.is_empty() {
                return Ok(());
            }
            Some(Action::SelectMachineZero)
        }
        ["sysbus", method, address, rest @ ..]
            if matches!(
                *method,
                "ReadByte"
                    | "ReadWord"
                    | "ReadDoubleWord"
                    | "ReadQuadWord"
                    | "WriteByte"
                    | "WriteWord"
                    | "WriteDoubleWord"
                    | "WriteQuadWord"
            ) =>
        {
            let width = if method.ends_with("Byte") {
                1
            } else if method.ends_with("DoubleWord") {
                4
            } else if method.ends_with("QuadWord") {
                8
            } else {
                2
            };
            let address = crate::parse_address64(resolve(address)?)?;
            let value = match (method.starts_with("Write"), rest) {
                (true, [value]) => Some(crate::memory_setup::write_value(resolve(value)?, width)?),
                (false, []) => None,
                _ => return Err(format!("invalid sysbus {method} arguments")),
            };
            Some(Action::BusScalar(BusScalar {
                address,
                width,
                value,
                number_mode: boot.session.number_mode,
            }))
        }
        ["sysbus", "FindBytes", ..] => Some(Action::FindBytes(FindBytes::parse(text, resolve)?)),
        ["machine", verb @ ("RecordTo" | "PlayFrom"), path, mode @ ..] => {
            let path = resolve(path)?
                .strip_prefix('@')
                .ok_or("expected @event-trace-path")?;
            let path = script.parent().unwrap_or_else(|| Path::new(".")).join(path);
            match (*verb, mode) {
                ("RecordTo", ["DomainExternal"]) => Some(Action::RecordTo(path)),
                ("PlayFrom", []) => Some(Action::PlayFrom(path)),
                _ => {
                    return Err(
                        "expected machine RecordTo @path DomainExternal or machine PlayFrom @path"
                            .into(),
                    );
                }
            }
        }
        ["machine", "RealTimeClockMode", mode] => {
            Some(Action::RtcMode(RtcMode::parse(resolve(mode)?)?))
        }
        ["machine", "RealTimeClockDateTime"] => Some(Action::RtcDateTime),
        [target, "EnableProfilerCollapsedStack", ..] => {
            let cpu = resolve(target)?;
            let args = text
                .split_once("EnableProfilerCollapsedStack")
                .unwrap()
                .1
                .trim();
            let (name, options) =
                quoted_argument(args).ok_or("expected quoted profiler output path")?;
            let path = script.parent().unwrap_or_else(|| Path::new(".")).join(name);
            let mut options = options.split_whitespace();
            let flush = options
                .next()
                .map(parse_tester_bool)
                .transpose()?
                .unwrap_or(false);
            let limit = options
                .next()
                .map(|value| {
                    value
                        .parse::<u64>()
                        .map_err(|_| "invalid profiler file size limit")
                })
                .transpose()?;
            if options.next().is_some() {
                return Err("unsupported collapsed stack profiler options".into());
            }
            Some(Action::StartCollapsedProfile(
                cpu.strip_prefix("sysbus.").unwrap_or(cpu).into(),
                path,
                flush,
                limit,
            ))
        }
        [target, "DisableProfiler"] => {
            let cpu = resolve(target)?;
            Some(Action::StopProfile(
                cpu.strip_prefix("sysbus.").unwrap_or(cpu).into(),
            ))
        }
        ["emulation", "CreateUartPtyTerminal", ..] => {
            let args = text.split_once("CreateUartPtyTerminal").unwrap().1.trim();
            let (name, args) = quoted_argument(args).ok_or("expected quoted terminal name")?;
            let (path, force) =
                quoted_argument(args).ok_or("expected quoted host terminal path")?;
            let force = !force.is_empty() && parse_tester_bool(force)?;
            Some(Action::CreateHostPty(
                name.into(),
                PathBuf::from(path),
                force,
            ))
        }
        ["CreateTerminalTester", target, options @ ..] => {
            let mut binary = None;
            for option in options {
                match option.split_once('=') {
                    Some(("binaryMode", value)) if binary.is_none() => {
                        binary = Some(parse_tester_bool(resolve(value)?)?);
                    }
                    Some(("defaultPauseEmulation", "true" | "True")) => {}
                    _ => {
                        return Err(format!(
                            "unsupported or duplicate UART tester option {option:?}"
                        ));
                    }
                }
            }
            Some(Action::CreateUartTester(
                resolve(target)?.to_owned(),
                binary.unwrap_or(false),
            ))
        }
        ["SetDefaultTester", id] => {
            let id = resolve(id)?;
            Some(Action::DefaultUartTester(
                (id != "null")
                    .then(|| crate::parse_address(id).map(|id| id as usize))
                    .transpose()?,
            ))
        }
        ["WaitForBytesOnUart", ..] => {
            let args = text["WaitForBytesOnUart".len()..].trim_start();
            let (pattern, arguments) = quoted_argument(args)
                .filter(|(pattern, _)| !pattern.is_empty())
                .ok_or("expected WaitForBytesOnUart \"bytes or regex\" [options]")?;
            let options = tester_wait_options(arguments, resolve, TesterOptionsKind::Bytes)?;
            let pattern = if options.regex {
                BinaryUartPattern::Regex(
                    regex::bytes::RegexBuilder::new(pattern)
                        .unicode(false)
                        .dot_matches_new_line(true)
                        .build()
                        .map_err(|e| format!("invalid UART byte regex: {e}"))?,
                )
            } else {
                let bytes = hex_pattern(pattern)?;
                if bytes.is_empty() || bytes.len() > crate::uart::UART_HISTORY_BYTES {
                    return Err("UART byte pattern must contain 1 to 65536 bytes".into());
                }
                BinaryUartPattern::Literal(bytes)
            };
            Some(Action::WaitForBytes(
                options.id,
                pattern,
                options.match_start,
                options.timeout,
            ))
        }
        [command @ ("WaitForLineOnUart" | "WaitForTesterPrompt" | "ShouldNotBeOnUart"), ..] => {
            let (pattern, arguments) = uart_text_argument(text[command.len()..].trim_start())?;
            if pattern.is_empty() {
                return Err("UART wait pattern must be nonempty".into());
            }
            let options = tester_wait_options(arguments, resolve, TesterOptionsKind::Line)?;
            Some(Action::WaitForTester(
                options.id,
                pattern.clone(),
                options.regex.then(|| regex::Regex::new(&pattern))
                    .transpose().map_err(|e| format!("invalid UART text regex: {e}"))?,
                if *command == "WaitForTesterPrompt" { UartWaitMode::Prompt } else { UartWaitMode::Lf },
                options.timeout,
                *command == "ShouldNotBeOnUart",
                options.capture_groups,
            ))
        }
        [
            command @ ("RegisterFailingUartString" | "UnregisterFailingUartString"),
            ..,
        ] => {
            let (pattern, arguments) = uart_text_argument(text[command.len()..].trim_start())?;
            if pattern.is_empty() {
                return Err("failing UART pattern must be nonempty".into());
            }
            let options = tester_wait_options(arguments, resolve, TesterOptionsKind::Failure)?;
            let regex = options
                .regex
                .then(|| {
                    regex::Regex::new(&pattern)
                        .map_err(|e| format!("invalid failing UART regex: {e}"))
                })
                .transpose()?;
            Some(Action::SetUartFailure(
                options.id,
                FailingUartPattern {
                    pattern,
                    regex,
                },
                *command == "RegisterFailingUartString",
            ))
        }
        [command @ ("waitForUart" | "waitForUartPrompt"), ..] => {
            let args = text[command.len()..].trim();
            let expected = format!("expected {command} [USART] \"literal text\" [timeout=seconds]");
            let (target, line) = if args.starts_with('"') {
                (None, args)
            } else {
                let (target, line) = args
                    .split_once(char::is_whitespace)
                    .ok_or_else(|| expected.clone())?;
                (Some(resolve(target)?.to_owned()), line.trim_start())
            };
            let (pattern, options) = quoted_argument(line)
                .filter(|(pattern, _)| !pattern.is_empty())
                .ok_or(expected)?;
            let (ending, timeout) = match options.rsplit_once(char::is_whitespace) {
                Some((ending, timeout)) => (ending.trim_end(), timeout),
                None if options.starts_with("timeout=") => ("", options),
                None => (options, ""),
            };
            let timeout = if timeout.is_empty() {
                None
            } else {
                Some(duration_steps(
                    timeout
                        .strip_prefix("timeout=")
                        .ok_or("expected UART timeout=seconds")?,
                )?)
            };
            let end = match (*command, ending) {
                ("waitForUartPrompt", "") => UartWaitMode::Prompt,
                ("waitForUartPrompt", _) => {
                    return Err("unexpected argument after UART prompt".into());
                }
                (_, "" | "LF") => UartWaitMode::Lf,
                (_, "CR") => UartWaitMode::Cr,
                _ => return Err("expected waitForUart ending LF or CR".into()),
            };
            Some(Action::WaitForUart(
                target,
                pattern.to_owned(),
                end,
                timeout,
            ))
        }
        [target, "WriteLine", ..] => {
            let args = text[target.len()..].trim_start()["WriteLine".len()..].trim();
            let (line, ending) = uart_text_argument(args)?;
            let ending: &[u8] = match ending {
                "" | "CR" | "true" => b"\r",
                "LF" => b"\n",
                "CRLF" => b"\r\n",
                "None" | "false" => b"",
                _ => {
                    return Err(
                        "expected USART WriteLine ending CR, LF, CRLF, None, true or false".into(),
                    );
                }
            };
            Some(Action::WriteUartLine(
                resolve(target)?.to_owned(),
                line,
                ending,
            ))
        }
        ["waitForLed", target, state, timeout, hold @ ..] if hold.len() <= 1 => {
            let state = resolve(state)?
                .parse()
                .map_err(|_| "expected LED state true or false")?;
            let timeout = duration_steps(resolve(timeout)?)?;
            let hold = hold
                .first()
                .map(|value| duration_steps(resolve(value)?))
                .transpose()?
                .unwrap_or(0);
            Some(Action::WaitForLed(
                resolve(target)?.to_owned(),
                state,
                timeout,
                hold,
            ))
        }
        [target, "PC", address] if boot.started || !boot.runtime.is_empty() => {
            let cpu = resolve(target)?;
            Some(Action::CpuPc(
                cpu.strip_prefix("sysbus.").unwrap_or(cpu).into(),
                crate::parse_address(resolve(address)?)?,
            ))
        }
        ["sysbus", "UnhandledAccessBehaviour", policy] => {
            let policy = match resolve(policy)? {
                "DoNotReport" => UnhandledAccess::DoNotReport,
                "ThrowException" => UnhandledAccess::ThrowException,
                "ThrowExceptionIfNotTagged" => UnhandledAccess::ThrowExceptionIfNotTagged,
                other => return Err(format!("unsupported UnhandledAccessBehaviour {other:?}")),
            };
            Some(Action::UnhandledAccess(policy))
        }
        ["sysbus", "Tag", ..] => {
            let args = text["sysbus".len()..].trim_start()["Tag".len()..].trim_start();
            let (range, remainder) = args
                .strip_prefix('<')
                .and_then(|args| args.split_once('>'))
                .ok_or("expected Tag <start size>")?;
            let fields = range
                .split(|c: char| c.is_whitespace() || c == ',')
                .filter(|field| !field.is_empty())
                .collect::<Vec<_>>();
            let [start, size] = fields.as_slice() else {
                return Err("expected Tag <start size>".into());
            };
            let (_, options) =
                quoted_argument(remainder.trim()).ok_or("expected quoted tag name")?;
            let mut default = None;
            let mut throws = None;
            for option in options.split_whitespace() {
                if let Some(value) = option.strip_prefix("throwException=") {
                    if throws.is_some() {
                        return Err("duplicate throwException option".into());
                    }
                    throws = Some(
                        resolve(value)?
                            .to_ascii_lowercase()
                            .parse()
                            .map_err(|_| "invalid throwException value")?,
                    );
                } else if default.is_none() {
                    let value = resolve(option.strip_prefix("defaultValue=").unwrap_or(option))?;
                    default = Some(
                        match value.strip_prefix("0x") {
                            Some(hex) => u64::from_str_radix(hex, 16),
                            None => value.parse(),
                        }
                        .map_err(|_| {
                            format!("unsupported Tag option or invalid default: {option}")
                        })?,
                    );
                } else {
                    return Err(format!("unsupported Tag option {option}"));
                }
            }
            Some(Action::Tag(
                crate::parse_address(resolve(start)?)?,
                crate::parse_address(resolve(size)?)?,
                default.unwrap_or(0),
                throws.unwrap_or(false),
            ))
        }
        [
            "sysbus",
            method @ ("ReadByte" | "ReadWord" | "ReadDoubleWord"),
            address,
        ] => Some(Action::ReadScalar(
            crate::parse_bus_address(resolve(address)?)?,
            match *method {
                "ReadByte" => ScalarWidth::Byte,
                "ReadWord" => ScalarWidth::Word,
                _ => ScalarWidth::DoubleWord,
            },
        )),
        [
            "sysbus",
            method @ ("WriteByte" | "WriteWord" | "WriteDoubleWord"),
            address,
            value,
        ] => {
            let width = match *method {
                "WriteByte" => ScalarWidth::Byte,
                "WriteWord" => ScalarWidth::Word,
                _ => ScalarWidth::DoubleWord,
            };
            let value = crate::parse_address(resolve(value)?)?;
            if (u32::MAX >> (32 - width as usize * 8)) < value {
                return Err(format!("value {value:#x} does not fit {method}"));
            }
            Some(Action::WriteScalar(
                crate::parse_bus_address(resolve(address)?)?,
                width,
                value,
            ))
        }
        ["sysbus", "ReadBytes", address, count] => Some(Action::ReadBytes(
            crate::parse_address(resolve(address)?)?,
            crate::parse_address(resolve(count)?)? as usize,
        )),
        ["sysbus", "WriteBytes", ..] => {
            let args = text["sysbus".len()..].trim_start()["WriteBytes".len()..].trim_start();
            let (bytes, address) = byte_list(args, resolve)?;
            let address = crate::parse_address(resolve(address)?)?;
            Some(Action::WriteBytes(address, bytes))
        }

        [target, "Step", count] => {
            let cpu = resolve(target)?;
            Some(Action::CpuStep(
                cpu.strip_prefix("sysbus.").unwrap_or(cpu).into(),
                u64::from(crate::parse_address(resolve(count)?)?),
            ))
        }
        [target, "EvaluateConditionCode", condition] => {
            let cpu = resolve(target)?;
            let condition = crate::parse_address(resolve(condition)?)?;
            if condition > 15 {
                return Err("expected an ARM condition code from 0 to 15".into());
            }
            Some(Action::CpuCondition(
                cpu.strip_prefix("sysbus.").unwrap_or(cpu).into(),
                condition,
            ))
        }
        [target, "GetRegister", register]
            if matches!(
                resolve(register)?
                    .trim_matches('"')
                    .to_ascii_lowercase()
                    .as_str(),
                "cpsr" | "xpsr"
            ) =>
        {
            let cpu = resolve(target)?;
            Some(Action::CpuXpsr(
                cpu.strip_prefix("sysbus.").unwrap_or(cpu).into(),
            ))
        }
        [target, "SetRegister", register, value] if boot.started || !boot.runtime.is_empty() => {
            Some(Action::CpuRegister(
                resolve(target)?.trim_start_matches("sysbus.").into(),
                unquote(resolve(register)?).into(),
                Some(crate::parse_address64(resolve(value)?)?),
            ))
        }
        [target, "GetRegister", register] if boot.started || !boot.runtime.is_empty() => {
            Some(Action::CpuRegister(
                resolve(target)?.trim_start_matches("sysbus.").into(),
                unquote(resolve(register)?).into(),
                None,
            ))
        }
        [target, "ResetVector", value @ ..]
            if (boot.started || !boot.runtime.is_empty()) && value.len() <= 1 =>
        {
            let value = value.first().map(|value| crate::parse_address64(resolve(value)?)).transpose()?;
            Some(Action::CpuRegister(
                resolve(target)?.trim_start_matches("sysbus.").into(), "ResetVector".into(), value,
            ))
        }
        [target, register @ ("MTVEC" | "STVEC"), value @ ..]
            if (boot.started || !boot.runtime.is_empty()) && value.len() <= 1 =>
        {
            let value = value.first().map(|value| crate::parse_address64(resolve(value)?)).transpose()?;
            Some(Action::CpuRegister(
                resolve(target)?.trim_start_matches("sysbus.").into(), (*register).into(), value,
            ))
        }
        [target, register @ ("MSTATUS" | "MEPC" | "MCAUSE" | "MTVAL")] => {
            Some(Action::CpuRegister(
                resolve(target)?.trim_start_matches("sysbus.").into(), (*register).into(), None,
            ))
        }
        [target, "AddHook", ..] if !text.contains("`sysbus GetSymbolAddress ") => {
            let cpu = resolve(target)?;
            let args = text[target.len()..].trim_start()["AddHook".len()..].trim_start();
            let (address, body) = if let Some(expression) = args.strip_prefix('`') {
                let (expression, body) = expression
                    .split_once('`')
                    .ok_or("unterminated AddHook address expression")?;
                let words: Vec<_> = expression.split_whitespace().collect();
                if !matches!(words.as_slice(), [name, "PC"] if name.strip_prefix("sysbus.").unwrap_or(name) == cpu.strip_prefix("sysbus.").unwrap_or(cpu))
                {
                    return Err("AddHook address expression must read the target CPU PC".into());
                }
                (None, body)
            } else {
                let (address, body) = args
                    .split_once(char::is_whitespace)
                    .ok_or("expected AddHook address and body")?;
                (Some(crate::parse_address(resolve(address)?)?), body)
            };
            Some(Action::AddCpuHook(
                cpu.strip_prefix("sysbus.").unwrap_or(cpu).into(),
                address,
                parse_cpu_hook(body.trim())?,
            ))
        }
        ["logLevel", "-1", target] => Some(Action::NvicLog(resolve(target)?.to_owned())),
        [target, "WriteDoubleWord", offset, value] => Some(Action::NvicWrite(
            resolve(target)?.to_owned(),
            crate::parse_address(resolve(offset)?)?,
            crate::parse_address(resolve(value)?)?,
        )),
        ["emulation", "SynchroniseQuantum"] => Some(Action::SynchroniseQuantum),
        [target, operation @ ("Pause" | "Resume" | "IsPaused")] => {
            let cpu = resolve(target)?;
            let requested = match *operation {
                "Pause" => Some(true),
                "Resume" => Some(false),
                _ => None,
            };
            Some(Action::CpuPaused(
                cpu.strip_prefix("sysbus.").unwrap_or(cpu).into(),
                requested,
            ))
        }
        [target, "ExecutionMode", mode @ ..] if mode.len() <= 1 => {
            let cpu = resolve(target)?;
            let single_step = match mode {
                [] => None,
                ["SingleStep"] => Some(true),
                ["Continuous"] => Some(false),
                _ => return Err("unsupported monitor ExecutionMode value".into()),
            };
            Some(Action::ExecutionMode(cpu.trim_start_matches("sysbus.").into(), single_step))
        }
        [target, "IsHalted", value @ ..] if value.len() <= 1 => {
            let cpu = resolve(target)?;
            let requested = value
                .first()
                .map(|value| resolve(value).and_then(parse_tester_bool))
                .transpose()?;
            Some(Action::CpuHalted(
                cpu.strip_prefix("sysbus.").unwrap_or(cpu).into(),
                requested,
            ))
        }
        [target, "PC"] => {
            let cpu = resolve(target)?;
            Some(Action::ReadCpuPc(
                cpu.strip_prefix("sysbus.").unwrap_or(cpu).into(),
            ))
        }
        ["emulation", "SetGlobalQuantum" | "SetQuantum", duration] => {
            let quantum = duration_steps(resolve(duration)?).map_err(|error| format!("invalid emulation quantum: {error}"))?;
            if quantum == 0 {
                return Err("emulation quantum must be positive".into());
            }
            Some(Action::SetQuantum(quantum))
        }
        ["emulation", "RunToNearestSyncPoint"] => Some(Action::RunToSyncPoint),
        ["machine" | "mach", "Reset"] => Some(Action::MachineReset(Vec::new())),
        [target, "Reset"] => {
            let cpu = resolve(target)?;
            Some(Action::CpuReset(
                cpu.strip_prefix("sysbus.").unwrap_or(cpu).into(),
            ))
        }
        [target, "CpuWaitSignal", state @ ("Set" | "Unset")] => {
            let cpu = resolve(target)?;
            Some(Action::CpuWait(
                cpu.strip_prefix("sysbus.").unwrap_or(cpu).into(),
                *state == "Set",
            ))
        }
        ["emulation", "RunUntil", timestamp] => {
            Some(Action::RunUntil(duration_steps(resolve(timestamp)?)?))
        }
        ["emulation", "RunFor", duration] => {
            Some(Action::RunFor(duration_steps(resolve(duration)?)?))
        }
        ["sysbus", "Unregister", target] | ["machine" | "mach", "UnregisterFromParent", target] => {
            Some(Action::UnregisterGpio(resolve(target)?.to_owned()))
        }
        [device, "PressAndRelease"] => Some(Action::ButtonPulse(resolve(device)?.to_owned())),
        [device, state @ ("Press" | "Release")] => Some(Action::Button(
            resolve(device)?.to_owned(),
            *state == "Press",
        )),
        [target, "Frequency", value] => Some(Action::Frequency(
            resolve(target)?.to_owned(),
            crate::parse_address(resolve(value)?)?,
        )),
        [target, "Clocked", value] => Some(Action::NvicClocked(
            resolve(target)?.to_owned(),
            resolve(value)?
                .to_ascii_lowercase()
                .parse()
                .map_err(|_| "expected NVIC Clocked true or false")?,
        )),
        [target, "SetPendingIRQ", exception] => Some(Action::NvicPending(
            resolve(target)?.to_owned(),
            crate::parse_address(resolve(exception)?)?,
        )),
        [target, "OnGPIO", line, value] => {
            let line = crate::parse_address(resolve(line)?)?;
            if line >= 32 {
                return Err("expected NVIC IRQ line 0 through 31".into());
            }
            let high = resolve(value)?
                .parse()
                .map_err(|_| "expected NVIC level true or false")?;
            Some(Action::NvicGpio(
                resolve(target)?.to_owned(),
                line as u8,
                high,
            ))
        }
        [channel, "Volts", voltage] => {
            let target = resolve(channel)?;
            let target = target.strip_prefix("sysbus.").unwrap_or(target);
            let (name, channel) = target
                .split_once(".adc-channel")
                .ok_or("expected ADC channel path")?;
            Some(Action::Adc(
                name.to_owned(),
                channel.parse().map_err(|_| "invalid ADC channel number")?,
                resolve(voltage)?
                    .parse()
                    .map_err(|_| "invalid ADC voltage")?,
            ))
        }
        _ => None,
    };
    if let Some(action) = action {
        if boot.platform.is_none()
            || (boot.image.is_none()
                && boot.binaries.is_empty()
                && !matches!(
                    action,
                    Action::BusScalar(_)
                        | Action::FindBytes(_)
                        | Action::CreateHostPty(..)
                        | Action::RtcMode(_)
                        | Action::RtcDateTime
                        | Action::UserState(_)
                ))
        {
            return Err("runtime commands require a loaded platform and firmware".into());
        }
        boot.runtime.push(RuntimeCommand {
            line,
            source: script.to_owned(),
            action,
        });
        return Ok(());
    }
    // A terminal tester can attach before start. The next runtime wait drives
    // the machine with the tester already attached.
    if words == ["start"]
        && !boot.started
        && matches!(
            boot.runtime.as_slice(),
            [RuntimeCommand { action: Action::CreateUartTester(..), .. }]
        )
    {
        boot.started = true;
        return Ok(());
    }
    if boot.started || !boot.runtime.is_empty() {
        if matches!(
            boot.runtime.last().map(|entry| &entry.action),
            Some(Action::RunFor(_))
        ) {
            match words {
                ["echo", value] => {
                    let value = unquote(resolve(value)?);
                    boot.post_run.push(PostRun::Echo(path_value(
                        value.strip_prefix('@').unwrap_or(value),
                    )));
                    return Ok(());
                }
                [device, "Dispose"] => {
                    let name = device.strip_prefix("sysbus.").unwrap_or(device);
                    boot.post_run.push(PostRun::DisposePcm(name.into()));
                    return Ok(());
                }
                _ => {}
            }
        }
        return Err("commands after start are unsupported in a boot script".into());
    }
    match words {
        ["cpu" | "sysbus.cpu", "PerformanceInMips", value] if boot.has_platform() => {
            boot.cpu_mips = Some(resolve(value)?.parse().map_err(|_| "invalid PerformanceInMips")?);
        }
        ["mach" | "machine", "create" | "add", name @ ..] if name.len() <= 1 => {
            if *machine {
                return Err("multiple machines are unsupported".into());
            }
            let name = name.first().map(|name| resolve(name)).transpose()?.unwrap_or("machine-0");
            boot.session.machine = Some(unquote(name).to_owned());
            *machine = true;
        }
        ["machine", "LoadPlatformDescription", value] if *machine && boot.platform.is_none() => {
            let file = resolve(value)?
                .strip_prefix('@')
                .ok_or("expected @platform-path")?;
            boot.platform = Some(local_path(&path_value(unquote(file)), script, root));
        }
        ["machine", "SdCardFromFile", args @ ..] if boot.has_platform() => {
            boot.sd_cards
                .push(SdAttachment::parse(args, resolve, script)?);
        }
        ["sysbus", "LoadELF", value, options @ ..]
            if boot.has_platform() && boot.image.is_none() && options.len() <= 1 =>
        {
            boot.use_virtual_elf_addresses = match options {
                [] | ["false" | "False"] => false,
                ["true" | "True"] => true,
                _ => return Err("expected a boolean LoadELF address option".into()),
            };
            let file = resolve(value)?
                .strip_prefix('@')
                .ok_or("expected @firmware-path")?;
            boot.image = Some(Asset::new(file, script));
        }
        ["sysbus", "LoadHEX", value] if boot.has_platform() && boot.image.is_none() => {
            let file = resolve(value)?
                .strip_prefix('@')
                .ok_or("expected @firmware-path")?;
            boot.image = Some(Asset::new(file, script));
            boot.hex = true;
        }
        [cpu, "VectorTableOffset", value] if boot.has_platform() => {
            boot.vector_tables.push((
                cpu.strip_prefix("sysbus.").unwrap_or(cpu).to_string(),
                crate::parse_address(resolve(value)?)?,
            ));
        }
        ["sysbus", "LoadBinary" | "LoadFdt", value, address] if boot.has_platform() => {
            let file = resolve(value)?
                .strip_prefix('@')
                .ok_or("expected @firmware-path")?;
            boot.binaries.push((
                Asset::new(file, script),
                crate::parse_address(resolve(address)?)?,
            ));
        }
        [device, "LoadPCM" | "Output", value] if boot.has_platform() => {
            let device = device.strip_prefix("sysbus.").unwrap_or(device).to_string();
            let path = resolve(value)?
                .strip_prefix('@')
                .ok_or("expected @PCM-path")?;
            if words[1] == "LoadPCM" {
                boot.pcm_inputs.push((device, Asset::new(path, script)));
            } else {
                boot.pcm_outputs.push((device, root.join(path_value(path))));
            }
        }
        ["emulation", "CreateSwitch", name] if *machine && boot.network.switch.is_none() => {
            boot.network.switch = Some(unquote(resolve(name)?).into());
        }
        ["emulation", "CreateNetworkServer", name, ip]
            if *machine && boot.network.server.is_none() =>
        {
            let name = unquote(resolve(name)?).to_string();
            let ip = unquote(resolve(ip)?)
                .parse::<Ipv4Addr>()
                .map_err(|_| "expected IPv4 network server address")?;
            boot.network.server = Some((name, ip));
        }
        ["connector", "Connect", device, switch] if *machine => {
            let switch = unquote(resolve(switch)?);
            let device = unquote(resolve(device)?);
            if boot.network.switch.as_deref() != Some(switch)
                || boot.network.links.iter().any(|name| name == device)
            {
                return Err("invalid or duplicate local switch connection".into());
            }
            boot.network.links.push(device.into());
        }
        [name, "StartTFTP", port]
            if boot.network.is_server(name) && boot.network.port.is_none() =>
        {
            boot.network.port = Some(
                resolve(port)?
                    .parse::<u16>()
                    .ok()
                    .filter(|&port| port != 0)
                    .ok_or("invalid TFTP port")?,
            );
        }
        [module, "ServeFile", file, name]
            if module
                .strip_suffix(".tftp")
                .is_some_and(|name| boot.network.is_server(name))
                && boot.network.port.is_some()
                && boot.network.file.is_none() =>
        {
            let file = resolve(file)?
                .strip_prefix('@')
                .ok_or("expected @TFTP-file-path")?;
            let name = unquote(resolve(name)?);
            boot.network.file = Some((Asset::new(file, script), name.into()));
        }
        ["sysbus", "SilenceRange", start, size] if boot.has_platform() => {
            let start = start
                .strip_prefix('<')
                .ok_or("expected SilenceRange <start size>")?;
            let size = size
                .strip_suffix('>')
                .ok_or("expected SilenceRange <start size>")?;
            boot.silence_ranges
                .push((crate::parse_address(start)?, crate::parse_address(size)?));
        }
        ["sysbus", "LoadSymbolsFrom", value] if boot.has_platform() => {
            let file = resolve(value)?
                .strip_prefix('@')
                .ok_or("expected @symbol-path")?;
            boot.symbols.push(file.to_string());
        }
        [target, "Pressed" | "Released"] if boot.has_platform() && target.contains('.') => {
            let (gpio, button) = target.rsplit_once('.').unwrap();
            boot.mpfs_buttons.push((
                gpio.trim_start_matches("sysbus.").to_string(),
                button.to_string(),
                words[1] == "Pressed",
            ));
        }
        [
            cpu,
            "AddHook",
            "`sysbus",
            "GetSymbolAddress",
            symbol,
            callback,
        ] if boot.has_platform() => {
            let symbol = symbol
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix("\"`"))
                .ok_or("expected quoted hook symbol")?;
            let name = callback.strip_prefix('$').ok_or("expected a named CPU hook")?;
            let hook = match boot.hooks.get(name) {
                Some(hook) => hook,
                None if variables.contains_key(name) => {
                    let file = boot.symbols.last().map(|file| Asset::new(file, script))
                        .or_else(|| boot.image.clone())
                        .ok_or("CPU Python hook requires a loaded ELF symbol table")?;
                    let source = unquote(resolve(callback)?);
                    boot.python_hooks.push((cpu.trim_start_matches("sysbus.").to_owned(),
                        symbol.to_owned(), source.to_owned(), file));
                    return Ok(());
                }
                None => return Err(format!("unknown CPU hook ${name}")),
            };
            let file = boot
                .symbols
                .last()
                .ok_or("hook requires a loaded symbol file")?;
            boot.halt_hooks.push((
                cpu.to_string(),
                symbol.to_string(),
                hook.clone(),
                file.clone(),
            ));
        }
        ["emulation", "SetSeed", value] => {
            // None of the supported devices consume Renode's random seed.
            crate::parse_address(resolve(value)?)?;
        }
        ["emulation", "SetGlobalSerialExecution", "True"] => {
            // The interpreter already executes harts serially.
        }
        [cpu, "PC", arguments @ ..] if boot.has_platform() && arguments.len() <= 1 => {
            let value = arguments
                .first()
                .map(|value| crate::parse_address(resolve(value)?))
                .transpose()?;
            boot.cpu_control.push(CpuControl::Pc(
                cpu.strip_prefix("sysbus.").unwrap_or(cpu).to_string(),
                value,
            ));
        }
        ["cpu" | "sysbus.cpu", "ResetVector", value] if boot.has_platform() => {
            boot.cpu_control
                .push(CpuControl::ResetVector(crate::parse_address(resolve(
                    value,
                )?)?));
        }
        ["cpu" | "sysbus.cpu", "Reset"] if boot.has_platform() => {
            boot.cpu_control.push(CpuControl::Reset);
        }
        [
            "cpu" | "sysbus.cpu",
            register @ ("MTVEC" | "STVEC"),
            arguments @ ..,
        ] if boot.has_platform() && arguments.len() <= 1 => {
            let register = if *register == "MTVEC" {
                TrapVector::Machine
            } else {
                TrapVector::Supervisor
            };
            let value = arguments
                .first()
                .map(|value| crate::parse_address(resolve(value)?))
                .transpose()?;
            boot.cpu_registers
                .push(CpuRegisterCommand::TrapVector(register, value));
        }
        [
            "cpu" | "sysbus.cpu",
            "LogFunctionNames",
            value @ ("true" | "false"),
        ] if boot.has_platform() => {
            boot.log_function_names = *value == "true";
        }
        [
            device,
            command @ ("ConfigCSRRead" | "AddressCSRRead" | "ConfigCSRWrite" | "AddressCSRWrite"),
            index,
            arguments @ ..,
        ] if boot.has_platform()
            && arguments.len() == usize::from(command.ends_with("Write")) =>
        {
            let path = device.strip_prefix("sysbus.").unwrap_or(device);
            let index = crate::parse_address(resolve(index)?)?;
            let value = arguments
                .first()
                .map(|value| crate::parse_address(resolve(value)?))
                .transpose()?;
            boot.cpu_registers.push(CpuRegisterCommand::ExternalPmp(
                path.into(),
                command.starts_with("Config"),
                index,
                value,
            ));
        }
        ["cpu" | "sysbus.cpu", "GetRegister", index] if boot.has_platform() => {
            let index = crate::parse_address(unquote(resolve(index)?))?;
            boot.cpu_registers
                .push(CpuRegisterCommand::Integer(index, None));
        }
        ["cpu" | "sysbus.cpu", "SetRegister", index, value] if boot.has_platform() => {
            let index = crate::parse_address(unquote(resolve(index)?))?;
            let value = crate::parse_address(resolve(value)?)?;
            boot.cpu_registers
                .push(CpuRegisterCommand::Integer(index, Some(value)));
        }
        ["cpu" | "sysbus.cpu", "SetMachineIrqMask", value] if boot.has_platform() => {
            boot.mask = Some(crate::parse_address(resolve(value)?)?);
        }
        ["cpu" | "sysbus.cpu", "SetSupervisorIrqMask", value] if boot.has_platform() => {
            boot.supervisor_mask = Some(crate::parse_address(resolve(value)?)?);
        }
        [button, action @ ("Press" | "Release" | "Toggle")] if boot.has_platform() => {
            let action = crate::litex_panel::ButtonAction::parse(action).unwrap();
            boot.buttons.push((button.to_string(), action));
        }
        ["start"]
            if boot.has_platform() && (boot.image.is_some() || !boot.binaries.is_empty()) =>
        {
            boot.started = true;
        }
        ["showAnalyzer", "sysbus.uart" | "uart"] if boot.has_platform() => {
            // UART output is already connected to stdout.
        }
        ["showAnalyzer", analyzer] if boot.has_platform() => {
            if analyzer
                .rsplit('.')
                .next()
                .is_some_and(|name| name.starts_with("uart") || name.starts_with("usart"))
            {
                boot.serial_analyzer =
                    Some(analyzer.strip_prefix("sysbus.").unwrap_or(analyzer).into());
                return Ok(());
            }
            boot.video_analyzers.push(
                analyzer
                    .strip_prefix("sysbus.")
                    .unwrap_or(analyzer)
                    .to_string(),
            );
        }
        ["echo", ..] => {
            // Monitor output is separate from guest UART output.
        }
        _ => return Err(format!("unsupported monitor command {text:?}")),
    }
    Ok(())
}

pub(crate) struct FindBytes {
    pattern: Vec<u8>,
    start: u64,
    end: u64,
}

impl FindBytes {
    pub(crate) fn parse<'a>(
        text: &'a str,
        resolve: impl Fn(&'a str) -> Result<&'a str, String>,
    ) -> Result<Self, String> {
        let args = text["sysbus".len()..].trim_start()["FindBytes".len()..].trim();
        let (pattern, bounds) = if let Some(args) = args.strip_prefix('[') {
            let (list, bounds) = args
                .split_once(']')
                .ok_or("expected closing byte list bracket")?;
            let pattern = if list.trim().is_empty() {
                Vec::new()
            } else {
                list.split(',')
                    .map(|value| {
                        let value = crate::parse_address(resolve(value.trim())?)?;
                        u8::try_from(value).map_err(|_| format!("byte value out of range: {value}"))
                    })
                    .collect::<Result<_, _>>()?
            };
            (pattern, bounds)
        } else {
            let (hex, bounds) = quoted_argument(args)
                .ok_or("expected sysbus FindBytes \"hex bytes\" or [byte, ...]")?;
            let hex: String = hex
                .chars()
                .filter(|c| !matches!(c, ' ' | '\r' | '\n'))
                .collect();
            if !hex.is_ascii() || hex.len() % 2 != 0 {
                return Err("FindBytes requires complete hexadecimal bytes".into());
            }
            let pattern = (0..hex.len())
                .step_by(2)
                .map(|index| {
                    u8::from_str_radix(&hex[index..index + 2], 16)
                        .map_err(|_| "FindBytes requires complete hexadecimal bytes")
                })
                .collect::<Result<_, _>>()?;
            (pattern, bounds)
        };
        let mut bounds = bounds.split_whitespace();
        let parse_bound = |value| crate::parse_address64(resolve(value)?);
        let start = bounds.next().map(parse_bound).transpose()?.unwrap_or(0);
        let end = bounds
            .next()
            .filter(|&value| value != "null")
            .map(parse_bound)
            .transpose()?
            .unwrap_or(u64::MAX);
        if bounds.next().is_some() {
            return Err("expected sysbus FindBytes pattern [start [end]]".into());
        }
        Ok(Self {
            pattern,
            start,
            end,
        })
    }

    pub(crate) fn write_matches(
        &self,
        bus: &crate::bus::Bus,
        prefix: &str,
        output: &mut impl Write,
    ) -> Result<(), String> {
        let addresses = bus.find_bytes(&self.pattern, self.start, self.end)?;
        let render = || -> io::Result<()> {
            write!(output, "{prefix}[")?;
            for (index, address) in addresses.into_iter().enumerate() {
                let separator = if index == 0 { "" } else { ", " };
                write!(output, "{separator}{address:#010x}")?;
            }
            writeln!(output, "]")
        };
        render().map_err(|error| format!("sysbus search output: {error}"))
    }
}

fn parse_cpu_hook(body: &str) -> Result<CpuHook, String> {
    let body = body
        .strip_prefix('"')
        .and_then(|body| body.strip_suffix('"'))
        .ok_or("expected quoted AddHook body")?;
    if body == "machine.PauseAndRequestEmulationPause()" {
        return Ok(CpuHook::Pause);
    }
    if let Some(arguments) = body
        .strip_prefix("machine.SystemBus.WriteDoubleWord(")
        .and_then(|body| body.strip_suffix(')'))
    {
        let (address, value) = arguments
            .split_once(',')
            .ok_or("expected hook write address and value")?;
        return Ok(CpuHook::WriteDoubleWord(
            crate::parse_address(address.trim())?,
            crate::parse_address(value.trim())?,
        ));
    }
    let log = body
        .strip_prefix("monitor.Parse('log ")
        .and_then(|body| body.strip_suffix("')"))
        .ok_or("unsupported AddHook body")?
        .replace("\\\"", "\"");
    let (message, "") = quoted_argument(&log).ok_or("expected quoted AddHook log message")? else {
        return Err("expected one AddHook log message".into());
    };
    Ok(CpuHook::Log(message.to_owned()))
}

fn parse_tester_bool(value: &str) -> Result<bool, String> {
    value
        .to_ascii_lowercase()
        .parse()
        .map_err(|_| format!("expected true or false, got {value:?}"))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TesterOptionsKind {
    Line,
    Bytes,
    Failure,
}

struct TesterWaitOptions {
    id: Option<usize>,
    timeout: u64,
    match_start: bool,
    regex: bool,
    capture_groups: bool,
}

fn tester_wait_options<'a>(
    options: &'a str,
    resolve: impl Fn(&'a str) -> Result<&'a str, String>,
    kind: TesterOptionsKind,
) -> Result<TesterWaitOptions, String> {
    let mut id = None;
    let mut timeout = None;
    let mut match_start = None;
    let mut regex = None;
    let mut capture_groups = None;
    for option in options.split_whitespace() {
        match option.split_once('=') {
            Some(("testerId", value)) if id.is_none() => {
                id = Some(crate::parse_address(resolve(value)?)? as usize);
            }
            Some(("timeout", value)) if kind != TesterOptionsKind::Failure && timeout.is_none() => {
                timeout = Some(duration_steps(resolve(value)?)?);
            }
            Some(("matchStart", value))
                if kind == TesterOptionsKind::Bytes && match_start.is_none() =>
            {
                match_start = Some(parse_tester_bool(resolve(value)?)?);
            }
            Some(("captureGroups", value))
                if kind == TesterOptionsKind::Line && capture_groups.is_none() =>
            {
                capture_groups = Some(parse_tester_bool(resolve(value)?)?);
            }
            Some(("treatAsRegex", value)) if regex.is_none() => {
                regex = Some(parse_tester_bool(resolve(value)?)?);
            }
            _ => {
                return Err(format!(
                    "unsupported or duplicate UART tester option {option:?}"
                ));
            }
        }
    }
    Ok(TesterWaitOptions {
        id,
        timeout: timeout.unwrap_or(8 * CORTEX_M_STEP_HZ),
        match_start: match_start.unwrap_or(false),
        regex: regex.unwrap_or(false),
        capture_groups: capture_groups.unwrap_or(false),
    })
}

fn byte_list<'a>(
    args: &'a str,
    resolve: impl Fn(&'a str) -> Result<&'a str, String>,
) -> Result<(Vec<u8>, &'a str), String> {
    let (list, remainder) = args
        .strip_prefix('[')
        .and_then(|args| args.split_once(']'))
        .ok_or("expected [byte, ...]")?;
    let bytes = if list.trim().is_empty() {
        Vec::new()
    } else {
        list.split(',')
            .map(|number| {
                let value = crate::parse_address(resolve(number.trim())?)?;
                u8::try_from(value).map_err(|_| format!("byte value out of range: {value}"))
            })
            .collect::<Result<Vec<_>, String>>()?
    };
    Ok((bytes, remainder.trim()))
}

fn hex_pattern(value: &str) -> Result<Vec<u8>, String> {
    let hex: String = value
        .chars()
        .filter(|char| !matches!(char, ' ' | '\r' | '\n'))
        .collect();
    if !hex.is_ascii() || hex.len() % 2 != 0 {
        return Err("FindBytes requires complete hexadecimal bytes".into());
    }
    (0..hex.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&hex[index..index + 2], 16)
                .map_err(|_| "FindBytes requires complete hexadecimal bytes".into())
        })
        .collect()
}

fn uart_text_argument(value: &str) -> Result<(String, &str), String> {
    if let Some(encoded) = value.strip_prefix("hex:") {
        let (text, rest) = encoded.split_once(char::is_whitespace).unwrap_or((encoded, ""));
        if text.len() % 2 != 0 || !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("UART text requires complete hexadecimal bytes".into());
        }
        let text = String::from_utf8(hex_pattern(text)?).map_err(|_| "UART text must be UTF-8")?;
        return Ok((text, rest.trim()));
    }
    quoted_argument(value)
        .map(|(text, rest)| (text.to_owned(), rest))
        .ok_or_else(|| "expected quoted or hex-encoded UART text".into())
}

fn quoted_argument(value: &str) -> Option<(&str, &str)> {
    let (text, rest) = value.strip_prefix('"')?.split_once('"')?;
    (rest.is_empty() || rest.starts_with(char::is_whitespace)).then(|| (text, rest.trim()))
}

fn duration_steps(value: &str) -> Result<u64, String> {
    let seconds = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .or_else(|| {
            value
                .strip_prefix('\'')
                .and_then(|value| value.strip_suffix('\''))
        })
        .unwrap_or(value);
    let seconds = seconds.strip_suffix('s').unwrap_or(seconds);
    let (whole, fraction) = seconds.split_once('.').unwrap_or((seconds, ""));
    let invalid =
        || format!("expected nonnegative decimal seconds at 10 ns resolution, got {value:?}");
    let whole: u64 = whole.parse().map_err(|_| invalid())?;
    let fraction = fraction.trim_end_matches('0');
    if fraction.len() > 8 || !fraction.bytes().all(|digit| digit.is_ascii_digit()) {
        return Err(invalid());
    }
    let ticks = fraction.parse::<u64>().unwrap_or(0)
        * (CORTEX_M_STEP_HZ / 10_u64.pow(fraction.len() as u32));
    whole
        .checked_mul(CORTEX_M_STEP_HZ)
        .and_then(|whole| whole.checked_add(ticks))
        .ok_or_else(invalid)
}

fn path_value(value: &str) -> String {
    value.replace(r"\ ", " ")
}

fn command_words(text: &str) -> Result<Vec<&str>, String> {
    let mut words = Vec::new();
    let mut start = None;
    let mut quote = None;
    let mut escaped = false;
    for (index, character) in text.char_indices() {
        if std::mem::take(&mut escaped) {
            continue;
        }
        if character == '\\'
            && (quote.is_some()
                || start.is_some_and(|start| text[start..].starts_with('@'))
                    && text[index + 1..].starts_with(' '))
        {
            escaped = true;
            continue;
        }
        if quote.is_none() && character == '#' && start.is_none() {
            return Ok(words);
        }
        if quote.is_none() && character.is_whitespace() {
            if let Some(start) = start.take() {
                words.push(&text[start..index]);
            }
            continue;
        }
        let new_word = start.is_none();
        start.get_or_insert(index);
        if quote == Some(character) {
            quote = None;
        } else if (new_word || start.is_some_and(|start| &text[start..index] == "@"))
            && matches!(character, '"' | '\'')
        {
            quote = Some(character);
        }
    }
    if quote.is_some() {
        return Err("unterminated quoted argument".into());
    }
    if let Some(start) = start {
        words.push(&text[start..]);
    }
    Ok(words)
}

impl Asset {
    pub(crate) fn new(value: &str, script: &Path) -> Self {
        Self {
            value: path_value(unquote(value)),
            script: script.into(),
        }
    }

    pub fn input_path(&self, root: &Path, assets: Option<&Path>) -> Result<PathBuf, String> {
        let path = self.resolve(root, assets)?;
        if self.value.contains("://") && !path.is_file() {
            return Err(format!("monitor asset unavailable: {}", self.value));
        }
        Ok(path)
    }

    pub fn resolve(&self, root: &Path, assets: Option<&Path>) -> Result<PathBuf, String> {
        let value = &self.value;
        if let Some((_, url)) = value.split_once("://") {
            let name = url
                .rsplit('/')
                .next()
                .filter(|name| !name.is_empty() && *name != "." && *name != "..")
                .ok_or_else(|| format!("invalid firmware URL {value}"))?;
            let assets = assets.ok_or_else(|| {
                format!("script references {value}; use --asset-dir with local assets")
            })?;
            return Ok(assets.join(name));
        }
        Ok(local_path(value, &self.script, root))
    }
}

fn local_path(value: &str, script: &Path, root: &Path) -> PathBuf {
    let local = script.parent().unwrap_or(Path::new(".")).join(value);
    if local.is_file() {
        local
    } else {
        root.join(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_repl_overlay_uses_the_repl_parser_for_gpio_connections() {
        let root = Path::new(".");
        let links = parse_gpio_overlay("gpioPortA: { 6 -> gpioPortD@12 }", root).unwrap();
        assert_eq!(links, [("gpioPortA".into(), 6, "gpioPortD".into(), 12)]);
        assert!(parse_gpio_overlay("gpioPortA: { someProperty: 1 }", root).is_err());
    }

    #[test]
    fn uart_capture_payload_preserves_utf8_and_unmatched_groups() {
        let regex = regex::Regex::new("(µ)(missing)?()").unwrap();
        assert_eq!(uart_capture_payload("µ", Some(&regex)), "c2b5\tc2b5\t\t");
    }

    #[test]
    fn startup_stream_preserves_bytes_for_the_live_session() {
        let startup = concat!(
            "set global.board @boards/test.repl\n",
            "mach create \"example board\"\n",
            "mach set \"example board\"\n",
            "machine LoadPlatformDescription $board\n",
        );
        let runtime = "PENV-COMMAND 5\npause";
        let input = format!("PENV-STARTUP {}\n{startup}{runtime}", startup.len());
        let mut stream = io::Cursor::new(input.into_bytes());
        let boot = load_session_stream(&mut stream, Path::new(".")).unwrap();
        assert_eq!(boot.session.machine.as_deref(), Some("example board"));
        assert_eq!(resolve(&boot.session.variables, "$board").unwrap(), "@boards/test.repl");
        let mut remaining = String::new();
        stream.read_to_string(&mut remaining).unwrap();
        assert_eq!(remaining, runtime);
    }

    #[test]
    fn monitor_tokens_keep_quoted_include_paths_together() {
        assert_eq!(
            command_words(r#"include @"scripts/a file.resc""#).unwrap(),
            ["include", r#"@"scripts/a file.resc""#],
        );
        assert_eq!(
            command_words(r"i @scripts/a\ file.resc").unwrap(),
            ["i", r"@scripts/a\ file.resc"],
        );
    }

    #[test]
    fn monitor_resolves_aliases_through_global_variables() {
        let mut variables = HashMap::from([
            ("file".into(), "$location".into()),
            ("global.location".into(), "@example.resc".into()),
        ]);
        assert_eq!(resolve(&variables, "$file"), Ok("@example.resc"));
        variables.insert("global.location".into(), "$file".into());
        assert!(resolve(&variables, "$file").is_err());
    }


    #[test]
    fn renode_button_press_and_release_latches_exti_between_steps() {
        use crate::{
            stm32_gpio::Stm32Gpio,
            stm32_gpio_board::{GpioButton, GpioEndpoints},
        };

        // nucleo_wba52cg.robot: Should have Working EXTI (GPIO/EXTI portion).
        const GPIO: u32 = 0x4800_0000;
        const EXTI: u32 = 0x4001_0400;
        const ISER: u32 = 0xe000_e100;
        let mut machine = Machine::without_default_ram();
        machine.bus.map_exti(EXTI, vec![Some(5)], 32).unwrap();
        machine.bus.map_nvic(0xc0, None).unwrap();
        let mut gpio = Stm32Gpio::new(GPIO, 0x400, [0; 3], 8);
        gpio.exti_routes[0] = 1;
        machine.bus.map_stm32_gpio(gpio).unwrap();
        let mut endpoints = GpioEndpoints::default();
        endpoints.connect(GPIO, "gpioPortA", 0, 16).unwrap();
        machine.bus.add_gpio_button(GpioButton::new(
            "button".into(),
            Some("gpioPortA".into()),
            endpoints,
            false,
        ));
        let mut output = io::sink();
        for (address, value) in [(EXTI, 1), (ISER, 1 << 5)] {
            machine.bus.write(address, 4, value, &mut output).unwrap();
        }

        let mut boot = Boot {
            platform: Some("board.repl".into()),
            image: Some(Asset::new("test.hex", Path::new("test.resc"))),
            ..Boot::default()
        };
        let script = Path::new("test.resc");
        execute(
            "sysbus.gpioPortA.button PressAndRelease",
            &["sysbus.gpioPortA.button", "PressAndRelease"],
            &HashMap::new(),
            &mut true,
            &mut boot,
            script,
            Path::new("."),
            1,
        )
        .unwrap();

        for (rising, falling) in [(1, 0), (0, 1)] {
            machine.bus.write(EXTI + 8, 4, rising, &mut output).unwrap();
            machine
                .bus
                .write(EXTI + 12, 4, falling, &mut output)
                .unwrap();
            for _ in 0..2 {
                assert_eq!(
                    run(&mut machine, script, &boot.runtime, 0, &mut output).unwrap(),
                    (Stop::ScriptComplete, 0)
                );
                assert_eq!(machine.bus.read::<4>(GPIO + 0x10).unwrap() & 1, 0);
                assert_eq!(machine.bus.read::<4>(EXTI + 20).unwrap(), 1);
                machine
                    .bus
                    .sample_nvic(&crate::nvic::ActiveExceptions::default(), false);
                assert_eq!(
                    machine
                        .bus
                        .nvic
                        .as_ref()
                        .unwrap()
                        .next_irq(&crate::nvic::ActiveExceptions::default()),
                    Some((5, 0))
                );
                machine.bus.nvic.as_mut().unwrap().acknowledge(5);
                machine.bus.write(EXTI + 20, 4, 1, &mut output).unwrap();
                machine
                    .bus
                    .sample_nvic(&crate::nvic::ActiveExceptions::default(), false);
                assert_eq!(
                    machine
                        .bus
                        .nvic
                        .as_ref()
                        .unwrap()
                        .next_irq(&crate::nvic::ActiveExceptions::default()),
                    None
                );
            }
        }
    }

    #[test]
    fn renode_resc_set_pending_irq_sets_nmi() {
        use crate::systick::ICSR;

        let mut machine = Machine::without_default_ram();
        machine.bus.map_nvic(0xc0, None).unwrap();
        machine.bus.set_nvic_name("nvic".into());
        machine.bus.map_systick().unwrap();
        let mut boot = Boot {
            platform: Some("board.repl".into()),
            image: Some(Asset {
                value: "test.hex".into(),
                script: "test.resc".into(),
            }),
            hex: true,
            ..Boot::default()
        };
        let script = Path::new("test.resc");
        execute(
            "nvic SetPendingIRQ 2",
            &["nvic", "SetPendingIRQ", "2"],
            &HashMap::new(),
            &mut true,
            &mut boot,
            script,
            Path::new("."),
            1,
        )
        .unwrap();
        run(&mut machine, script, &boot.runtime, 0, &mut io::sink()).unwrap();
        assert_eq!(machine.bus.read::<4>(ICSR).unwrap(), 1 << 31);
        assert!(machine.bus.pend_nvic_exception("other", 2).is_err());
        assert!(machine.bus.pend_nvic_exception("nvic", 4).is_err());
    }

    #[test]
    fn led_wait_times_out_and_rejects_a_change_during_hold() {
        use crate::{stm32_gpio::Stm32Gpio, stm32_gpio_board::GpioLed};

        let mut machine = Machine::without_default_ram();
        let gpio = 0x4800_0000;
        machine.bus.map_ram(0, 0x100).unwrap();
        machine
            .bus
            .map_stm32_gpio(Stm32Gpio::new(gpio, 0x400, [1, 0, 0], 8))
            .unwrap();
        machine
            .bus
            .add_gpio_led(gpio, GpioLed::new("led".into(), 0, false));
        let mut output = io::sink();
        let mut budget = 2;
        let error =
            wait_for_led(&mut machine, "led", true, 0, 0, &mut budget, &mut output).unwrap_err();
        assert!(error.contains("did not reach state true"));
        assert_eq!(budget, 2);

        machine.bus.write(gpio + 0x18, 4, 1, &mut output).unwrap(); // set pin
        machine.primary.monitor_paused = true;
        let error =
            wait_for_led(&mut machine, "led", true, 0, 2, &mut budget, &mut output).unwrap_err();
        assert!(error.contains("hold ended before completion (Paused)"));
        assert_eq!(machine.elapsed_steps(), 0);
        machine.primary.monitor_paused = false;

        machine.bus.write(0, 2, 0x6001, &mut output).unwrap(); // str r1, [r0]
        machine.primary.configure_cortex_m(Some(0), false);
        machine.primary.pc = 0;
        machine.primary.registers[..2].copy_from_slice(&[gpio + 0x28, 1]); // clear pin
        let error =
            wait_for_led(&mut machine, "led", true, 0, 2, &mut budget, &mut output).unwrap_err();
        assert!(error.contains("changed while holding state true"));
        assert_eq!(machine.elapsed_steps(), 1);
    }

    #[test]
    fn parses_renode_uart_write_line_endings() {
        let mut boot = Boot {
            platform: Some("board.repl".into()),
            image: Some(Asset::new("firmware.elf", Path::new("test.resc"))),
            ..Boot::default()
        };
        let mut machine = true;
        let script = Path::new("test.resc");
        let root = Path::new(".");
        let variables = HashMap::new();
        for (suffix, expected) in [
            ("", b"\r".as_slice()),
            (" CR", b"\r"),
            (" LF", b"\n"),
            (" CRLF", b"\r\n"),
            (" None", b""),
            (" true", b"\r"),
            (" false", b""),
        ] {
            let command = format!("sysbus.usart1 WriteLine \"with spaces\"{suffix}");
            execute(
                &command,
                &command_words(&command).unwrap(),
                &variables,
                &mut machine,
                &mut boot,
                script,
                root,
                1,
            )
            .unwrap();
            let command = boot.runtime.pop().unwrap();
            assert!(matches!(command.action,
                Action::WriteUartLine(target, line, ending)
                if target == "sysbus.usart1" && line == "with spaces" && ending == expected
            ));
        }
        assert!(
            execute(
                "sysbus.usart1 WriteLine \"x\" invalid",
                &["sysbus.usart1", "WriteLine", "\"x\"", "invalid"],
                &variables,
                &mut machine,
                &mut boot,
                script,
                root,
                1
            )
            .is_err()
        );
    }

    #[test]
    fn uart_match_requires_a_completed_line_and_never_spans_lines() {
        let mut output = Vec::new();
        let mut history = UartHistory::default();
        history.begin_wait("ADC reading: 745", UartWaitMode::Lf);
        let mut uart = ScriptOutput {
            output: &mut output,
            history: Some(history),
        };
        uart.write_all(b"ADC reading: \n745\r\n").unwrap();
        assert!(!uart.history.as_ref().unwrap().matched());
        uart.write_all(b"ADC reading: 7").unwrap();
        uart.write_all(b"45\r").unwrap();
        assert!(!uart.history.as_ref().unwrap().matched());
        uart.write_all(b"\n").unwrap();
        assert!(uart.history.as_ref().unwrap().matched());
        assert_eq!(output, b"ADC reading: \n745\r\nADC reading: 745\r\n");
    }

    #[test]
    fn fire_monitor_resolves_button_and_elf_hook() {
        let directory =
            std::env::temp_dir().join(format!("penv-fire-monitor-{}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        let script = directory.join("boot.resc");
        let source = r#"mach create
machine LoadPlatformDescription @beaglev-fire.repl
emulation SetGlobalQuantum "0.0008"
sysbus LoadSymbolsFrom @u-boot.elf
gpio0.UserButton Pressed
set hook
"""
e51 = machine['sysbus.e51']
e51.IsHalted = True
print("halted management CPU")
"""
u54_4 AddHook `sysbus GetSymbolAddress "_start"` $hook
sysbus LoadELF @hss.elf
"#;
        fs::write(&script, source).unwrap();
        let boot = load(&script, &directory).unwrap();
        assert_eq!(
            boot.mpfs_buttons,
            [("gpio0".into(), "UserButton".into(), true)]
        );
        assert_eq!(
            boot.halt_hooks,
            [(
                "u54_4".into(),
                "_start".into(),
                "e51".into(),
                "u-boot.elf".into()
            )]
        );
        fs::write(&script, source.replace("0.0008", "NaN")).unwrap();
        let result = load(&script, &directory);
        fs::remove_dir_all(directory).unwrap();
        assert!(matches!(result, Err(error) if error.contains("invalid emulation quantum")));
    }

    #[test]
    fn parses_binary_addresses_and_named_cpu_entries() {
        let mut boot = Boot::default();
        let mut created = true;
        let path = Path::new("test.resc");
        let root = Path::new(".");
        let vars = HashMap::new();
        for command in [
            "machine LoadPlatformDescription @board.repl",
            "sysbus LoadBinary @https://example.org/firmware.bin 0x40000000",
            "sysbus.cpu_1 PC 0x40000004",
        ] {
            execute(
                command,
                &command_words(command).unwrap(),
                &vars,
                &mut created,
                &mut boot,
                path,
                root,
                1,
            )
            .unwrap();
        }
        assert_eq!(boot.binaries.len(), 1);
        assert_eq!(boot.binaries[0].1, 0x4000_0000);
        assert_eq!(
            boot.cpu_control,
            [CpuControl::Pc("cpu_1".into(), Some(0x4000_0004))]
        );
        assert_eq!(
            boot.binaries[0]
                .0
                .resolve(root, Some(Path::new("assets")))
                .unwrap(),
            Path::new("assets/firmware.bin")
        );
    }

    #[test]
    fn starlight_monitor_commands_leave_only_guest_assets_to_load() {
        let path = Path::new("beaglev_starlight.resc");
        let root = Path::new("renode");
        let mut boot = Boot::default();
        let mut machine = false;
        let vars = HashMap::from([(
            "symbols".to_owned(),
            "@https://example.org/vmlinux".to_owned(),
        )]);
        for command in [
            "emulation SetSeed 0x1",
            "mach create BeagleV",
            "machine LoadPlatformDescription @platforms/boards/beaglev_starlight.repl",
            "emulation SetGlobalSerialExecution True",
            "showAnalyzer uart3",
            "sysbus LoadELF @fw_payload.elf",
            "sysbus LoadSymbolsFrom $symbols",
            "sysbus LoadBinary @image.fit 0xa0000000",
            "echo \"Script loaded.\"",
        ] {
            execute(
                command,
                &command_words(command).unwrap(),
                &vars,
                &mut machine,
                &mut boot,
                path,
                root,
                1,
            )
            .unwrap();
        }
        assert_eq!(
            boot.image.as_ref().map(|asset| asset.value.as_str()),
            Some("fw_payload.elf")
        );
        assert_eq!(boot.binaries.len(), 1);
        assert_eq!(
            (boot.binaries[0].0.value.as_str(), boot.binaries[0].1),
            ("image.fit", 0xa000_0000)
        );
    }

    #[test]
    fn reads_boot_commands_and_reports_macro_source_lines() {
        let directory = std::env::temp_dir().join(format!("penv-common-resc-{}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        let script = directory.join("boot.resc");
        let content = concat!(
            "$name?=\"Test Machine\"\n",
            "$bin?=@firmware\\ file.elf\n",
            "mach create \"Test Machine\"\n",
            "machine LoadPlatformDescription @board\\ file.repl\n",
            "macro reset\n",
            "\"\"\"\n",
            "  sysbus LoadELF $bin # boot image\n",
            "  sysbus.cpu MTVEC 0x80000020\n",
            "\"\"\"\n",
            "runMacro $reset\n",
            "cpu SetMachineIrqMask 0xffffffff\n",
            "start\n",
        );
        fs::write(&script, content).unwrap();
        let boot = load(&script, &directory).unwrap();
        assert_eq!(boot.platform, Some(directory.join("board file.repl")));
        assert_eq!(
            boot.image.as_ref().map(|image| image.value.as_str()),
            Some("firmware file.elf")
        );
        assert_eq!(
            boot.cpu_registers,
            [CpuRegisterCommand::TrapVector(
                TrapVector::Machine,
                Some(0x8000_0020)
            )]
        );
        assert_eq!(boot.mask, Some(u32::MAX));
        fs::write(&script, content.replace("MTVEC", "unknown")).unwrap();
        let result = load(&script, &directory);
        fs::remove_dir_all(directory).unwrap();
        assert!(
            matches!(result, Err(error) if error.contains("boot.resc:8: unsupported monitor command"))
        );
    }
}
