pub(crate) mod action_arguments;
pub(crate) mod managed_extensions;
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
    pub binaries: Vec<(Asset, u64)>,
    pub network: Network,
    pub video_analyzers: Vec<String>,
    pub pcm_inputs: Vec<(String, Asset)>,
    pub pcm_outputs: Vec<(String, PathBuf)>,
    pub run_steps: Option<u64>,
    pub quantum: Option<u64>,
    pub uart_timeout: Option<u64>,
    pub cpu_mips: Option<u32>,
    pub unhandled_access: Option<UnhandledAccess>,
    pub post_run: Vec<PostRun>,
    pub buttons: Vec<(String, crate::litex_panel::ButtonAction)>,
    pub silence_ranges: Vec<(u32, u32)>,
    pub boot_pcs: Vec<(String, u32)>,
    pub mtvec: Option<u32>,
    pub cpu_control: Vec<CpuControl>,
    pub platform: Option<PathBuf>,
    inline_repl: Vec<(PathBuf, String)>,
    multiple_machines: bool,
    pub generated_repl: Option<GeneratedRepl>,
    pub session: Session,
    pub serial_analyzer: Option<String>,
    pub image: Option<Asset>,
    pub prior_elf_images: Vec<(Asset, bool)>,
    pub hex: bool,
    pub vector_tables: Vec<(String, u32)>,
    pub runtime: Vec<RuntimeCommand>,
    pub use_virtual_elf_addresses: bool,
    pub log_function_names: bool,
    pub monitor_log_level: Option<i8>,
    pub initial_log_capacity: Option<usize>,
    pub trace_memory: Vec<(String, bool)>,
    pub riscv_wfi_as_nop: Vec<(String, bool)>,
    pub monitor_user_state: Option<String>,
    pub monitor_output: Vec<String>,
    pub console_only: bool,
    pub empty_machine: bool,
    pub initial_start: bool,
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
    macros: HashMap<String, Macro>,
    aliases: HashMap<String, String>,
    variables: HashMap<String, String>,
    machine: Option<String>,
    machine_cleared: bool,
    number_mode: NumberMode,
    allow_privates: bool,
    memory_symbols: HashMap<String, std::collections::BTreeSet<(u64, u64)>>,
    named_machines: NamedMachines,
    python: Option<crate::cpu_python_hook::Hook>,
    led_testers: Vec<(Option<String>, String, u64)>,
    led_timeout: u64,
}

#[derive(Default)]
struct NamedMachines {
    order: Vec<String>,
    detached: HashMap<String, Machine>,
    draft: Option<Vec<(PathBuf, String)>>,
    generated: Vec<GeneratedRepl>,
    log_levels: std::collections::BTreeMap<String, i8>,
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
const SESSION_SNAPSHOT_V3: &[u8; 9] = b"PENVSESS\x03";
const SESSION_SNAPSHOT_V4: &[u8; 9] = b"PENVSESS\x04";
const SESSION_SNAPSHOT_V5: &[u8; 9] = b"PENVSESS\x05";
const SESSION_SNAPSHOT_V6: &[u8; 9] = b"PENVSESS\x06";
const SESSION_SNAPSHOT_V7: &[u8; 9] = b"PENVSESS\x07";
const SESSION_SNAPSHOT: &[u8; 9] = b"PENVSESS\x08";
const MAX_MONITOR_PAYLOAD: usize = 8 * 1024 * 1024;

fn write_snapshot(
    path: &Path,
    write: impl FnOnce(&mut fs::File) -> Result<(), String>,
) -> Result<(), String> {
    if let Some(error) = inspect_snapshot_destination(path) {
        return Err(error);
    }
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let temporary = path.with_file_name(format!(
        ".penv-snapshot-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed),
    ));
    let mut file = fs::OpenOptions::new().write(true).create_new(true).open(&temporary)
        .map_err(|error| format!("{}: {error}", temporary.display()))?;
    let result = write(&mut file)
        .and_then(|()| file.sync_all().map_err(|error| error.to_string()));
    drop(file);
    let result = result.and_then(|()| fs::rename(&temporary, path).map_err(|error| error.to_string()));
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result.map_err(|error| format!("{}: {error}", path.display()))
}

struct SnapshotMetadata {
    selected: String,
    user_state: String,
    number_mode: NumberMode,
    machine_cleared: bool,
    quantum: Option<u64>,
    cpu_mips: Option<u32>,
    variables: HashMap<String, String>,
    macros: HashMap<String, Macro>,
    aliases: HashMap<String, String>,
}

fn read_snapshot_metadata(input: &mut impl BufRead) -> Result<Option<SnapshotMetadata>, String> {
    let header = input.fill_buf().map_err(|e| e.to_string())?;
    let version = if header.starts_with(SESSION_SNAPSHOT) {
        8
    } else if header.starts_with(SESSION_SNAPSHOT_V7) {
        7
    } else if header.starts_with(SESSION_SNAPSHOT_V6) {
        6
    } else if header.starts_with(SESSION_SNAPSHOT_V5) {
        5
    } else if header.starts_with(SESSION_SNAPSHOT_V4) {
        4
    } else if header.starts_with(SESSION_SNAPSHOT_V3) {
        3
    } else if header.starts_with(SESSION_SNAPSHOT_V2) {
        2
    } else if header.starts_with(SESSION_SNAPSHOT_V1) {
        1
    } else {
        return Ok(None);
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
    let size = read_size(input)?;
    if size > MAX_MONITOR_PAYLOAD {
        return Err("monitor variable snapshot exceeds size limit".into());
    }
    let mut data = vec![0; size];
    input.read_exact(&mut data).map_err(|e| e.to_string())?;
    let mut data = data.as_slice();
    let selected = read_string(&mut data)?;
    let machine_cleared = if version >= 6 {
        let (&flag, remaining) = data.split_first().ok_or("missing machine selection flag")?;
        data = remaining;
        match flag {
            0 => false,
            1 => true,
            _ => return Err("invalid machine selection flag".into()),
        }
    } else {
        false
    };
    let user_state = if version >= 7 { read_string(&mut data)? } else { String::new() };
    let number_mode = if version >= 8 {
        let (&mode, remaining) = data.split_first().ok_or("missing monitor number format")?;
        data = remaining;
        match mode {
            0 => NumberMode::Hexadecimal,
            1 => NumberMode::Decimal,
            2 => NumberMode::Both,
            _ => return Err("invalid monitor number format".into()),
        }
    } else {
        NumberMode::Hexadecimal
    };
    let mut macros = HashMap::new();
    if version >= 4 {
        let count = read_size(&mut data)?;
        for _ in 0..count {
            let name = read_string(&mut data)?;
            let script = PathBuf::from(read_string(&mut data)?);
            let command_count = read_size(&mut data)?;
            let mut commands = Vec::new();
            for _ in 0..command_count {
                let line = read_size(&mut data)?;
                commands.push((line, read_string(&mut data)?));
            }
            if name.is_empty() || macros.insert(name, Macro { script, commands }).is_some() {
                return Err("invalid monitor macro snapshot".into());
            }
        }
    }
    let mut aliases = HashMap::new();
    if version >= 5 {
        let count = read_size(&mut data)?;
        for _ in 0..count {
            let name = read_string(&mut data)?;
            let command = read_string(&mut data)?;
            if name.is_empty() || command.is_empty() || aliases.insert(name, command).is_some() {
                return Err("invalid monitor alias snapshot".into());
            }
        }
    }
    let mut variables = HashMap::new();
    while !data.is_empty() {
        let key = read_string(&mut data)?;
        let value = read_string(&mut data)?;
        if matches!(key.as_str(), "global.CWD" | "global.ORIGIN") || variables.insert(key, value).is_some() {
            return Err("invalid monitor variable snapshot".into());
        }
    }
    Ok(Some(SnapshotMetadata { selected, user_state, number_mode, machine_cleared, quantum, cpu_mips, variables, macros, aliases }))
}

enum SnapshotPlatform {
    Inline(String),
    File(PathBuf),
}

fn restore_snapshot_platform(path: &Path) -> Result<(Option<String>, Option<SnapshotPlatform>), String> {
    let mut input = open_snapshot_input(path)?;
    let metadata = read_snapshot_metadata(&mut input)?;
    let header = input.fill_buf().map_err(|error| error.to_string())?;
    if header.starts_with(crate::snapshot::CORTEX_MAGIC) {
        let metadata = metadata.ok_or("Cortex-M snapshot requires monitor metadata")?;
        if metadata.cpu_mips.is_none_or(|mips| mips == 0) {
            return Err("invalid Cortex-M snapshot rate".into());
        }
        input.consume(crate::snapshot::CORTEX_MAGIC.len());
        let repl = crate::snapshot::read_text(&mut input).map_err(|error| error.to_string())?;
        return Ok(((!metadata.selected.is_empty()).then_some(metadata.selected), Some(SnapshotPlatform::Inline(repl))));
    }
    if header.starts_with(crate::snapshot::RV64_MAGIC) {
        let metadata = metadata.ok_or("RV64 snapshot requires monitor metadata")?;
        if metadata.cpu_mips.is_none_or(|mips| mips == 0) {
            return Err("invalid RV64 snapshot rate".into());
        }
        input.consume(crate::snapshot::RV64_MAGIC.len());
        let repl = PathBuf::from(crate::snapshot::read_text(&mut input).map_err(|error| error.to_string())?);
        if !repl.is_absolute() || repl.extension().is_none_or(|extension| extension != "repl") || !repl.is_file() {
            return Err("unsupported snapshot platform path".into());
        }
        return Ok(((!metadata.selected.is_empty()).then_some(metadata.selected), Some(SnapshotPlatform::File(repl))));
    }
    let selected = match metadata {
        Some(metadata) if metadata.cpu_mips == Some(0) => {
            (!metadata.selected.is_empty()).then_some(metadata.selected)
        }
        None => None,
        Some(_) => return Err("unsupported snapshot platform format".into()),
    };
    let repl = Machine::read_cpu_less_snapshot_platform(&mut input)?
        .map(|(address, uart)| trivial_uart_snapshot_repl(address, &uart).map(SnapshotPlatform::Inline))
        .transpose()?;
    Ok((selected, repl))
}

impl Session {
    pub(crate) fn egis_reset_entry(&self) -> Option<(&str, u32)> {
        let body = self.macros.get("reset")?;
        let mut commands = body.commands.iter().map(|(_, text)| text.trim())
            .filter(|text| !text.is_empty() && !text.starts_with('#'));
        let words = command_words(commands.next()?).ok()?;
        let [name, "PC", address] = words.as_slice() else { return None };
        if commands.next().is_some() {
            return None;
        }
        let address = crate::parse_address(resolve(&self.variables, address).ok()?).ok()?;
        Some((name.strip_prefix("sysbus.").unwrap_or(name), address))
    }

    fn create_named_machine(&mut self, machine: &mut Machine, requested: Option<&str>) -> Result<(), String> {
        if self.named_machines.order.is_empty() {
            self.named_machines.order.extend(self.machine.clone());
        }
        let name = if let Some(name) = requested {
            name.to_owned()
        } else {
            (0..).map(|index| format!("machine-{index}"))
                .find(|candidate| !self.named_machines.order.contains(candidate)).unwrap()
        };
        if name.is_empty() || self.named_machines.order.contains(&name) {
            return Err(format!("machine {name:?} already exists or has no name"));
        }
        let mut next = Machine::without_default_ram();
        next.cpu_less = true;
        next.empty_monitor = true;
        next.monitor_asset_dir.clone_from(&machine.monitor_asset_dir);
        self.swap_named_machine(machine, next, name.clone());
        self.named_machines.order.push(name);
        self.named_machines.draft = Some(Vec::new());
        Ok(())
    }

    fn swap_named_machine(&mut self, machine: &mut Machine, next: Machine, name: String) {
        let old = std::mem::replace(machine, next);
        if let Some(previous) = self.machine.replace(name.clone()) {
            apply_machine_selection(&mut self.machine_cleared, true, Some(&previous), &mut self.variables);
            self.named_machines.detached.insert(previous, old);
        }
        apply_machine_selection(&mut self.machine_cleared, false, Some(&name), &mut self.variables);
        crate::monitor_log::set_level(self.named_machines.log_levels.get(&name).copied().unwrap_or(1));
    }

    fn select_named_machine(&mut self, machine: &mut Machine, selector: &str) -> Result<(), String> {
        let name = if let Ok(index) = selector.parse::<usize>() {
            self.named_machines.order.get(index).cloned().ok_or("Wrong machine number.")?
        } else {
            selector.to_owned()
        };
        if self.machine.as_deref() == Some(&name) {
            apply_machine_selection(&mut self.machine_cleared, false, Some(&name), &mut self.variables);
            return Ok(());
        }
        let next = self.named_machines.detached.remove(&name)
            .ok_or_else(|| format!("Machine {name} not found."))?;
        self.swap_named_machine(machine, next, name);
        Ok(())
    }

    fn attach_platform_fragment(
        &mut self, machine: &mut Machine, source: &str, origin: &Path, root: &Path,
    ) -> Result<(), String> {
        let draft = self.named_machines.draft.as_mut().ok_or("inline platform requires a new machine")?;
        let mut boot = Boot::default();
        boot.inline_repl.clone_from(draft);
        boot.inline_repl.push((origin.to_owned(), source.to_owned()));
        boot.materialize_platform()?;
        let path = boot.platform.as_deref().expect("materialized REPL");
        let mut next = crate::platform::load_monitor_configuration(path, root, machine.monitor_asset_dir.as_deref())?;
        next.monitor_platform = boot.platform;
        next.monitor_asset_dir.clone_from(&machine.monitor_asset_dir);
        next.bus.use_renode_unmapped_data();
        next.initialize_monitor_reset_pcs();
        self.named_machines.generated.extend(boot.generated_repl);
        *draft = boot.inline_repl;
        *machine = next;
        Ok(())
    }

    fn describe_machine_log_levels(&self, machine: &Machine, output: &mut impl Write) -> Result<(), String> {
        const LEVELS: [&str; 5] = ["NOISY", "DEBUG", "INFO", "WARNING", "ERROR"];
        for (name, &level) in &self.named_machines.log_levels {
            let active = if self.machine.as_deref() == Some(name) {
                machine
            } else if let Some(detached) = self.named_machines.detached.get(name) {
                detached
            } else {
                continue;
            };
            let level = LEVELS[(level + 1) as usize];
            writeln!(output, "{name}:sysbus : {level}").map_err(|error| error.to_string())?;
            for device in active.bus.monitor_bus_device_names() {
                writeln!(output, "{name}:sysbus.{device} : {level}").map_err(|error| error.to_string())?;
            }
        }
        Ok(())
    }

    fn execute_python(&mut self, machine: &mut Machine, source: &str) -> Result<String, String> {
        if self.python.is_none() {
            self.python = Some(crate::cpu_python_hook::Hook::new("", None, None)?);
        }
        self.python.as_mut().unwrap().execute_monitor(&mut machine.bus, source, machine.monitor_firmware.clone())
            .map_err(|error| format!("KeywordException: {error}"))
    }

    fn selected_machine(&self) -> Option<&str> {
        self.machine.as_deref().filter(|_| !self.machine_cleared)
    }

    fn save(&self, machine: &Machine, path: &Path) -> Result<(), String> {
        if self.named_machines.order.len() > 1 {
            return Err("unsupported snapshot platform with multiple machines".into());
        }
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
        let number_mode = match self.number_mode {
            NumberMode::Hexadecimal => 0,
            NumberMode::Decimal => 1,
            NumberMode::Both => 2,
        };
        let has_user_state = !machine.bus.monitor_user_state.is_empty();
        let version_has_user_state = has_user_state || number_mode != 0;
        encode(&mut data, self.machine.as_deref().unwrap_or_default())?;
        if version_has_user_state || self.machine_cleared {
            data.push(u8::from(self.machine_cleared));
        }
        if version_has_user_state {
            encode(&mut data, &machine.bus.monitor_user_state)?;
        }
        if number_mode != 0 {
            data.push(number_mode);
        }
        fn encode_size(data: &mut Vec<u8>, value: usize) -> Result<(), String> {
            if data.len() > MAX_MONITOR_PAYLOAD - 4 {
                return Err("monitor snapshot exceeds size limit".into());
            }
            let value = u32::try_from(value).map_err(|_| "monitor snapshot field exceeds size limit")?;
            data.extend_from_slice(&value.to_le_bytes());
            Ok(())
        }
        if version_has_user_state || self.machine_cleared || !self.macros.is_empty() || !self.aliases.is_empty() {
            let mut macros: Vec<_> = self.macros.iter().collect();
            macros.sort_unstable_by_key(|(name, _)| *name);
            encode_size(&mut data, macros.len())?;
            for (name, body) in macros {
                encode(&mut data, name)?;
                encode(&mut data, body.script.to_str().ok_or("monitor macro path is not UTF-8")?)?;
                encode_size(&mut data, body.commands.len())?;
                for (line, command) in &body.commands {
                    encode_size(&mut data, *line)?;
                    encode(&mut data, command)?;
                }
            }
        }
        if version_has_user_state || self.machine_cleared || !self.aliases.is_empty() {
            let mut aliases: Vec<_> = self.aliases.iter().collect();
            aliases.sort_unstable_by_key(|(name, _)| *name);
            encode_size(&mut data, aliases.len())?;
            for (name, command) in aliases {
                encode(&mut data, name)?;
                encode(&mut data, command)?;
            }
        }
        for (key, value) in entries {
            encode(&mut data, key)?;
            encode(&mut data, value)?;
        }
        write_snapshot(path, |file| {
            let version = if number_mode != 0 {
                SESSION_SNAPSHOT
            } else if has_user_state {
                SESSION_SNAPSHOT_V7
            } else if self.machine_cleared {
                SESSION_SNAPSHOT_V6
            } else if !self.aliases.is_empty() {
                SESSION_SNAPSHOT_V5
            } else if !self.macros.is_empty() {
                SESSION_SNAPSHOT_V4
            } else {
                SESSION_SNAPSHOT_V3
            };
            file.write_all(version).map_err(|e| e.to_string())?;
            file.write_all(&machine.monitor_quantum.to_le_bytes()).map_err(|e| e.to_string())?;
            let mips = if machine.cpu_less { 0 } else { (machine.bus.monitor_step_hz() / 1_000_000 / u64::from(machine.monitor_cpu_rate_divisor())) as u32 };
            file.write_all(&mips.to_le_bytes()).map_err(|e| e.to_string())?;
            file.write_all(&(data.len() as u32).to_le_bytes()).map_err(|e| e.to_string())?;
            file.write_all(&data).map_err(|e| e.to_string())?;
            machine.write_boot_snapshot(file)
        })
    }

    fn load(&mut self, machine: &mut Machine, path: &Path) -> Result<(), String> {
        let mut input = open_snapshot_input(path)?;
        let result = (|| {
            let Some(SnapshotMetadata { selected, user_state, number_mode, machine_cleared, quantum, cpu_mips, mut variables, macros, aliases }) =
                read_snapshot_metadata(&mut input)?
            else {
                return machine.read_boot_snapshot(&mut input);
            };
            if selected != self.machine.as_deref().unwrap_or_default() {
                return Err("snapshot uses a different monitor machine".into());
            }
            let cpu_mips = match cpu_mips {
                None => None,
                Some(0) if machine.cpu_less => None,
                Some(mips) if (machine.is_cortex_m() || machine.primary.rv64.is_some())
                    && mips != 0 && mips.checked_mul(1_000_000)
                        .and_then(|rate| rate.checked_mul(machine.monitor_cpu_rate_divisor())).is_some() => Some(mips),
                _ => return Err("invalid snapshot CPU speed".into()),
            };
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
            machine.bus.monitor_user_state = user_state;
            self.variables = variables;
            self.machine_cleared = machine_cleared;
            self.number_mode = number_mode;
            self.macros = macros;
            self.aliases = aliases;
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

enum AssemblyAddress {
    Literal(u64),
    ProgramCounter(String),
}

enum Action {
    SetEmulationPaused(bool),
    MonitorLog(String),
    Stm32SupplyVoltage(String, f64),
    AssembleBlock(String, AssemblyAddress, String, String, NumberMode),
    RunFor(u64),
    RunUntil(u64),
    RunToSyncPoint,
    SetQuantum(u64),
    LogLevel(i8),
    ScriptLog(i8, String),
    LastLog(i32),
    UserState(Option<String>),
    Echo(String),
    Illuminance(String, f64),
    ClicGpio(String, u32, bool),
    LogUart(String),
    GpioWire(String, u8, String, u8),
    GpioOverlay(Vec<(String, u8, String, u8)>),
    RamOverlay(String, Option<String>, u64, u32),
    PeripheralList,
    ExportPeripherals(PathBuf, PathBuf),
    SubobjectTester(String, u32),
    MonitorCreateMachine(Option<String>),
    MonitorSelectMachine(String),
    PlatformFragment(String, PathBuf, PathBuf),
    ReadLogLevels,
    IlluminanceFeed(String, PathBuf),
    MagneticFeed(String, PathBuf),
    PressureFeed(String, PathBuf, bool),
    VoltageFeed(String, PathBuf, usize, u16),
    PressureValue(String),
    PressurePointer(String, u8),
    PressureIncrement(String, bool),
    FinishI2cSensorTransfer(String),
    EnableZephyrMode(String, Vec<String>),
    RunRemaining,
    TimeSourceInfo,
    EmulationIsStarted,
    SingleStepBlocking(Option<bool>),
    PhyRegister(String, u16),
    AdxlAxis(String, usize, i16),
    LiteXLedState(String),
    UartIdle(u64, Option<usize>, Option<bool>),
    StartCollapsedProfile(String, PathBuf, bool, Option<u64>),
    StopProfile(String),
    DescribeSnapshotPlatform,
    DescribeEnvironment,
    MonitorAssignment(String, PathBuf),
    MonitorAlias(String, String),
    MonitorMacro(String, Macro),
    InvokeMonitorMacro(String, PathBuf),
    InvokeMonitorScript(String, PathBuf, PathBuf),
    MonitorEcho(String, PathBuf),
    SaveMemory(SnapshotPath, bool),
    ClearMemory,
    LoadMemory(SnapshotPath),
    SetMachineCleared(bool),
    FindBytes(FindBytes),
    BusScalar(BusScalar),
    ContextBusScalar(BusScalar, String),
    LoadScopedHex(Asset, String, PathBuf),
    LoadMonitorElf(Asset, bool, PathBuf),
    DisassembleBlock(String, u64, usize),
    BusWhatIsAt(u64, Option<String>),
    ExecutePython(String),
    InvalidPythonSource(String),
    RegisterPyDev(String, u32, u32, bool),
    SetCrashOnAbort(bool),
    InstallInstructionHook(String, u32, u32, String),
    PythonAssignment(String, String),
    PythonHook(String, String, Option<String>),
    RecordTo(PathBuf),
    PlayFrom(PathBuf),
    RtcMode(RtcMode),
    RtcDateTime,
    CreateHostPty(String, PathBuf, bool),
    CpuStep(String, u64),
    ClearTranslationCache(String),
    ExternalPmpRegister(String, bool, u32, Option<u64>),
    CpuMips(u32),
    ReadCsrValidation(String),
    RequireFullCsrValidation(String),
    ReadCpuMips(NumberMode),
    TraceStackAccess(String),
    AddStackAccessHook(String, String),
    AddCsrHook(String, u32, String),
    VectorRegisterLength(String, u64),
    CpuCondition(String, u32),
    CpuBlockSize(String, u32),
    CpuBlockHook(String, bool, String),
    CpuWillExecuteIt(String, u8),
    NvicPriorityBoost(String, bool),
    CpuXpsr(String),
    CpuWait(String, bool),
    CpuPaused(String, Option<bool>),
    CpuHalted(String, Option<bool>),
    ReadCpuPc(String),
    AssertCpuValue(String, Option<String>, u64),
    CpuRegister(String, String, Option<u64>),
    ArmSystemRegister(String, String, Option<u64>),
    ArmExceptionState(String, bool),
    ExecutionMode(String, Option<bool>),
    AddCpuHook(String, Option<u32>, CpuHook),
    PeripheralLog(String, i8),
    PeripheralWrite(String, u32, u32, u32),
    PeripheralRead(String, u32, u32),
    TimerQuadWord(String, u32, Option<u64>),
    WatchdogIrq(String),
    Stm32AdcSample(String, u32, u32, i32),
    EgisSysconProperty(String, String),
    SynchroniseQuantum,
    CpuReset(String),
    CpuPc(String, u32),
    CpuSymbolPc(String, String),
    CpuSymbolVectorTable(String, String),
    CpuVectorTable(String, u32),
    CpuStatus(String, String),
    CpuSecurityState(String, bool),
    CpuPreserveFp(String),
    CpuFpuEnabled(String, Option<bool>),
    CpuMpuBackground(String, Option<bool>),
    CpuTranslateAddress(String, u32, bool, bool),
    CpuSau(String, String, Option<u32>),
    CpuNonsecureVectorTable(String, u32),
    NvicLockup(String, bool),
    NvicSignal(String, String),
    SymbolAddress(String, Option<i32>),
    I2cSensorProperty(String, String, String),
    I2cAcceleration(String, usize, i32, bool),
    I2cAccelerationFeed(String, PathBuf, bool),
    I2cAccelerationRead(String, usize),
    I2cSampleRate(String, u32),
    UsartWriteChar(String, u8),
    BusFaultingProperty(String, String, String),
    AllowPrivates(bool),
    MachineReset(Vec<ResetHook>),
    ReadScalar(u64, ScalarWidth),
    WriteScalar(u64, ScalarWidth, u32),
    Tag(u32, u32, u64, bool, String, bool),
    UnhandledAccess(UnhandledAccess),
    ReadBytes(u32, usize),
    WriteBytes(u32, Vec<u8>),
    Button(String, bool),
    ButtonPulse(String),
    ToggleButton(String),
    UnregisterGpio(String),
    NvicGpio(String, u8, bool),
    NvicPending(String, u32),
    NvicClocked(String, bool),
    Frequency(String, u32),
    TimerProperty(String, bool),
    TraceRcc,
    WriteRcc(u32, u32),
    DefaultLedTimeout(u64),
    CreateLedTester(String, Option<u64>),
    AssertLedState(Option<usize>, bool, Option<u64>, u64, bool),
    WaitForLed(String, bool, u64, u64, bool),
    WaitForMemory(crate::memory_watch::MemoryWatch, u64, bool),
    CheckMemory(crate::memory_watch::MemoryWatch),
    AddMemorySymbol(String, u64, u64),
    ResolveMemorySymbols(String, bool),
    PwmTester(String, String),
    Adc(String, usize, f64),
    WaitForUart(Option<String>, String, UartWaitMode, Option<u64>),
    CreateUartTester(String, Option<String>, bool, Option<u64>, Option<bool>, bool, bool, bool),
    DefaultUartTester(Option<usize>),
    DefaultUartTimeout(u64),
    WaitForTester(Option<usize>, String, Option<regex::Regex>, UartWaitMode, Option<u64>, bool, bool, Option<bool>, Option<bool>),
    WaitForTesterLines(UartLineSequence),
    WaitForBytes(Option<usize>, BinaryUartPattern, bool, Option<u64>, Option<bool>),
    SetUartFailure(Option<usize>, FailingUartPattern, bool),
    ClearUartTesterReport(Option<usize>),
    WriteUartLine(String, String, &'static [u8]),
    WriteTesterUart(Option<usize>, String, &'static [u8], Option<bool>, Option<bool>),
}

fn peripheral_access_width(method: &str) -> u32 {
    if method.ends_with("ByteUsingDoubleWord") { 1 }
    else if method.ends_with("WordUsingDoubleWord") { 2 }
    else { 4 }
}

fn peripheral_word_access(offset: u32, width: u32) -> Result<(u32, u32, u32), String> {
    if offset % width != 0 {
        return Err(format!("misaligned peripheral register {offset:#x}"));
    }
    Ok((offset & !3, (offset & 3) * 8, u32::MAX >> (32 - width * 8)))
}

struct BusScalar {
    address: u64,
    width: usize,
    value: Option<u64>,
    number_mode: NumberMode,
}

impl BusScalar {
    fn read_value(bus: &mut crate::bus::Bus, address: u64, width: usize) -> Result<u64, String> {
        if width == 8 && let Some(result) = bus.access_pydev(address, 8, None) {
            return result.map_err(|error| error.to_string());
        }
        let address32 = u32::try_from(address)
            .ok()
            .filter(|&address| address.checked_add(width as u32 - 1).is_some());
        if let Some(address) = address32
            && width == 8
            && let Some(value) = bus.riscv_machine_timer.as_mut().and_then(|timer| timer.read(address, width))
        {
            return Ok(value);
        }
        match (address32, width) {
            (Some(address), 1) => bus.read_scalar::<1>(address, true),
            (Some(address), 2) => bus.read_scalar::<2>(address, true),
            (Some(address), 4) => bus.read_scalar::<4>(address, true),
            _ => return crate::memory_setup::read_ram(bus, address, width),
        }.map(u64::from).map_err(|error| error.to_string())
    }

    fn apply_cortex_context(&self, bus: &mut crate::bus::Bus, output: &mut impl Write) -> Result<(), String> {
        let address = u32::try_from(self.address).map_err(|_| "Cortex-M bus address exceeds CPU width")?;
        if self.width == 8 {
            return self.apply(bus, output);
        }
        if let Some(value) = self.value {
            return bus.thumb_write(address, self.width, value as u32, output);
        }
        let value = match self.width {
            1 => bus.thumb_read::<1>(address),
            2 => bus.thumb_read::<2>(address),
            _ => bus.thumb_read::<4>(address),
        }?;
        self.number_mode.print(u64::from(value), self.width, output)
    }

    fn apply_rv64_context(
        &self, bus: &mut crate::bus::Bus, hart: usize, output: &mut impl Write,
    ) -> Result<(), String> {
        let address = self.address;
        let width = self.width;
        let value = if let Some(value) = self.value {
            if let Some(memory) = bus.cpu_memory64(hart, address, width) {
                memory.copy_from_slice(&value.to_le_bytes()[..width]);
                bus.observe_memory_store(address, width, value);
            } else if let Ok(address) = u32::try_from(address)
                && width <= 4
                && let Some(result) = bus.write_rv64_peripheral(hart, address, width, value as u32, output)
            {
                result.map_err(|error| error.to_string())?;
            }
            return Ok(());
        } else if let Some(memory) = bus.cpu_ram64(hart, address, width) {
            let mut bytes = [0; 8];
            bytes[..width].copy_from_slice(memory);
            u64::from_le_bytes(bytes)
        } else if let Ok(address) = u32::try_from(address) && width <= 4 {
            u64::from(bus.rv64_devices.read_for_hart(hart, address, width).unwrap_or(0))
        } else {
            0
        };
        self.number_mode.print(value, width, output)
    }

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
            if width == 8 && let Some(result) = bus.access_pydev(address, 8, Some(value)) {
                return result.map(|_| ()).map_err(|error| error.to_string());
            }
            if let Some(address) = address32 && width == 8
                && bus.riscv_machine_timer.as_mut().and_then(|timer| timer.write(address, width, value)).is_some()
            {
                return Ok(());
            }
            match address32 {
                Some(address) if width <= 4 => bus.write_scalar(address, width, value as u32, output, true)
                    .map_err(|error| error.to_string()),
                _ => crate::memory_setup::write_ram(bus, address, width, value),
            }
        } else {
            number_mode.print(Self::read_value(bus, address, width)?, width, output)
        }
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

struct ScriptOutput<'a> {
    output: &'a mut dyn Write,
    history: Option<UartHistory>,
    replies: Option<&'a mut dyn Write>,
}

impl ScriptOutput<'_> {
    fn reply(&mut self) -> &mut dyn Write {
        match self.replies.as_deref_mut() {
            Some(replies) => replies,
            None => self.output,
        }
    }
}

impl Write for ScriptOutput<'_> {
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
                Action::CreateHostPty(..) | Action::RtcMode(_) | Action::RtcDateTime | Action::MonitorLog(_)
                    | Action::AssembleBlock(..) | Action::AddStackAccessHook(..) | Action::AddCsrHook(..) | Action::VectorRegisterLength(..) | Action::BusScalar(_)
                    | Action::MonitorCreateMachine(..) | Action::MonitorSelectMachine(..) | Action::PlatformFragment(..) | Action::ReadLogLevels | Action::ExportPeripherals(..)
                    | Action::LoadScopedHex(..) | Action::CpuBlockSize(..) | Action::CpuBlockHook(..) | Action::CpuWillExecuteIt(..) | Action::CpuCondition(..) | Action::NvicPriorityBoost(..)
                    | Action::Frequency(..) | Action::NvicPending(..) | Action::MachineReset(..) | Action::CpuReset(..)
                    | Action::CpuMips(..) | Action::ReadCpuMips(..) | Action::RequireFullCsrValidation(..) | Action::ReadCsrValidation(..)
                    | Action::CpuStep(..) | Action::ClearTranslationCache(..) | Action::CpuPc(..) | Action::ReadCpuPc(_) | Action::SymbolAddress(..) | Action::ExecutionMode(..)
                    | Action::AssertCpuValue(..) | Action::CpuRegister(..) | Action::SetEmulationPaused(_)
                    | Action::ArmSystemRegister(..) | Action::ArmExceptionState(..)
                    | Action::PeripheralWrite(..) | Action::PeripheralRead(..) | Action::TimerQuadWord(..) | Action::WatchdogIrq(_) | Action::Stm32AdcSample(..) | Action::EgisSysconProperty(..) | Action::CreateUartTester(..)
                    | Action::DefaultUartTester(_) | Action::ClearUartTesterReport(_) | Action::UartIdle(..) | Action::WaitForBytes(..) | Action::WaitForTester(..)
                    | Action::RunFor(_) | Action::RunUntil(_) | Action::RunToSyncPoint | Action::Echo(_) | Action::ScriptLog(..) | Action::LastLog(_) | Action::LogLevel(_) | Action::PeripheralLog(..)
                    | Action::DescribeSnapshotPlatform | Action::DescribeEnvironment
                    | Action::PythonAssignment(..) | Action::ExecutePython(..)
                    | Action::InvalidPythonSource(..) | Action::RegisterPyDev(..)
                    | Action::SetCrashOnAbort(..) | Action::InstallInstructionHook(..) | Action::WaitForTesterLines(..)
                    | Action::DefaultLedTimeout(..) | Action::CreateLedTester(..) | Action::AssertLedState(..)
                    | Action::MonitorAssignment(..) | Action::MonitorEcho(..) | Action::MonitorAlias(..) | Action::MonitorMacro(..) | Action::SetMachineCleared(_) | Action::InvokeMonitorMacro(..) | Action::InvokeMonitorScript(..) | Action::TimeSourceInfo | Action::SetQuantum(_) | Action::EmulationIsStarted | Action::SingleStepBlocking(_) | Action::SaveMemory(..) | Action::ClearMemory | Action::LoadMemory(_)
            )
        })
}

pub fn run(
    machine: &mut Machine,
    session: &mut Session,
    commands: &[RuntimeCommand],
    budget: u64,
    output: &mut impl Write,
) -> Result<(Stop, u64), String> {
    run_commands(machine, commands, budget, output, session, None, 0)
}

pub fn run_startup(
    machine: &mut Machine,
    session: &mut Session,
    commands: &[RuntimeCommand],
    budget: u64,
    guest_output: &mut impl Write,
    replies: &mut impl Write,
) -> Result<(Stop, u64), String> {
    run_commands(machine, commands, budget, guest_output, session, Some(replies), 0)
}

fn run_commands<'a>(
    machine: &mut Machine,
    commands: &[RuntimeCommand],
    mut budget: u64,
    mut output: &'a mut dyn Write,
    session: &mut Session,
    replies: Option<&'a mut dyn Write>,
    depth: usize,
) -> Result<(Stop, u64), String> {
    if commands.is_empty() {
        return machine.run(budget, &mut output);
    }
    let mut output = ScriptOutput {
        output,
        history: None,
        replies,
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
        if !matches!(command.action, Action::PlatformFragment(..)) {
            session.named_machines.draft = None;
        }
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
            Action::InvokeMonitorMacro(name, root) => (|| {
                let path = &session.macros.get(name).ok_or("unknown macro")?.script;
                let mut state = deferred_script_state(path, session);
                expand_macro(name, &mut state, root)?;
                validate_deferred_commands(&state.boot, depth, true)?;
                let start = machine.elapsed_steps();
                let boot = apply_deferred_state(session, state);
                let (stop, count) = run_deferred_commands(machine, boot, budget, &mut output, session, depth)?;
                retired += count;
                budget = budget.saturating_sub(machine.elapsed_steps().wrapping_sub(start));
                Ok((stop != Stop::ScriptComplete).then_some(stop))
            })(),
            Action::InvokeMonitorScript(text, canonical, root) => (|| {
                let mut state = deferred_script_state(&command.source, session);
                load_text(&command.source, canonical.clone(), root, &mut state, text)?;
                let start = machine.elapsed_steps();
                let mut separators = Vec::new();
                let words = command_tokens(text, Some(&mut separators))?;
                let allow_snapshots = !separators.is_empty() || matches!(words.first(), Some(&("include" | "i" | "ExecuteScript")));
                validate_deferred_commands(&state.boot, depth, allow_snapshots)?;
                let boot = apply_deferred_state(session, state);
                let (stop, count) = run_deferred_commands(machine, boot, budget, &mut output, session, depth)?;
                retired += count;
                budget = budget.saturating_sub(machine.elapsed_steps().wrapping_sub(start));
                Ok((stop != Stop::ScriptComplete).then_some(stop))
            })(),
            Action::MonitorAlias(name, command) => {
                session.aliases.insert(name.clone(), command.clone());
                Ok(None)
            }
            Action::MonitorMacro(name, body) => {
                session.macros.insert(name.clone(), body.clone());
                Ok(None)
            }
            Action::MonitorAssignment(text, origin) => command_words(text).and_then(|words| {
                assign_variable(text, &words, session.machine.as_deref().filter(|_| !session.machine_cleared), &mut session.variables, Some(origin))
            }).map(|_| None),
            Action::MonitorEcho(value, origin) => monitor_echo(value, &session.variables, origin).and_then(|message| {
                writeln!(output.reply(), "{message}").map_err(|error| error.to_string())
            }).map(|()| None),
            Action::DescribeEnvironment => writeln!(
                output.reply(), "{}", describe_environment(&session.variables, &session.macros, &session.aliases),
            ).map(|()| None).map_err(|error| error.to_string()),
            Action::DescribeSnapshotPlatform => describe_snapshot_platform(machine, session)
                .and_then(|script| output.output.write_all(script.as_bytes()).map_err(|error| error.to_string()))
                .map(|()| None),
            Action::SaveMemory(path, restorable) => path.resolve(&session.variables).and_then(|path| {
                if *restorable {
                    snapshot_platform(machine)?;
                }
                session.save(machine, &path)
            }).map(|()| None),
            Action::ClearMemory => machine.clear_memory_snapshot().map(|()| {
                cleared = true;
                None
            }),
            Action::LoadMemory(path) => path.resolve(&session.variables).and_then(|path| session.load(machine, &path)).map(|()| {
                machine.monitor_single_step_blocking = true;
                cleared = false;
                None
            }),
            Action::SetMachineCleared(cleared) => {
                apply_machine_selection(&mut session.machine_cleared, *cleared, session.machine.as_deref(), &mut session.variables);
                Ok(None)
            }
            Action::LoadMonitorElf(asset, virtual_addresses, root) => {
                let path = asset.input_path(root, machine.monitor_asset_dir.as_deref())?;
                load_monitor_elf(machine, path, *virtual_addresses).map(|()| None)
            }
            Action::LoadScopedHex(asset, context, root) => {
                let path = asset.input_path(root, machine.monitor_asset_dir.as_deref())?;
                let file = fs::File::open(&path).map_err(|error| format!("{}: {error}", path.display()))?;
                crate::image::load_hex_in_context(machine, file, context).map(|()| None)
            }
            Action::ContextBusScalar(access, context) => {
                let readback = if machine.is_cortex_m() {
                    machine.validate_monitor_bus_context(context)?;
                    access.apply_cortex_context(&mut machine.bus, &mut output.reply())
                } else if let Ok(hart) = machine.monitor_bus_hart(context) {
                    access.apply_rv64_context(&mut machine.bus, hart, &mut output.reply())
                } else {
                    machine.validate_monitor_bus_context(context)?;
                    access.apply(&mut machine.bus, &mut output.reply())
                };
                readback.map(|()| None)
            }
            Action::DisassembleBlock(cpu, address, length) => {
                let (hart, triple, features) = machine.monitor_assembly_target(cpu)?;
                let bytes = match hart {
                    Some(hart) => machine.bus.cpu_ram64(hart, *address, *length),
                    None => machine.bus.ram64(*address, *length),
                }.ok_or_else(|| format!("unmapped instruction memory at {cpu}@{address:#x}"))?;
                use std::fmt::Write as _;
                let mut input = String::with_capacity(5 * bytes.len());
                for byte in bytes {
                    write!(input, "0x{byte:02x} ").unwrap();
                }
                input.push('\n');
                let result = crate::llvm_mc_for(triple, &features, "--disassemble", &input)?;
                if !result.status.success() {
                    return Err(String::from_utf8_lossy(&result.stderr).trim().to_owned());
                }
                output.reply().write_all(&result.stdout).map_err(|error| error.to_string())?;
                Ok(None)
            }
            Action::BusWhatIsAt(address, context) => {
                let registration = if let Some(context) = context {
                    let hart = machine.monitor_bus_hart(context)?;
                    if let Ok(address) = u32::try_from(*address)
                        && let Some(index) = machine.bus.rv64_devices.uart_for_hart(hart, address)
                    {
                        Some(machine.bus.rv64_devices.uarts[index].name())
                    } else {
                        machine.bus.cpu_ram64(hart, *address, 1).map(|_| "Memory")
                    }
                } else {
                    machine.bus.ram64(*address, 1).map(|_| "Memory")
                };
                if let Some(registration) = registration {
                    writeln!(output.reply(), "{registration}").map_err(|error| error.to_string())?;
                }
                Ok(None)
            }
            Action::ExecutePython(source) => {
                let result = session.execute_python(machine, source)?;
                output.reply().write_all(result.as_bytes()).map_err(|error| error.to_string())?;
                Ok(None)
            }
            Action::InvalidPythonSource(command) => Err(format!(
                "KeywordException: There was an error when executing command '{command}': Variable type has to be a string."
            )),
            Action::RegisterPyDev(source, address, size, initable) => machine.bus
                .map_pydev(source, *address, *size, *initable)
                .map(|()| None),
            Action::SetCrashOnAbort(enabled) => {
                machine.bus.suppress_cpu_abort = !enabled;
                Ok(None)
            }
            Action::InstallInstructionHook(cpu, mask, opcode, source) => machine
                .install_instruction_hook(cpu, *mask, *opcode, source)
                .map(|()| None),
            Action::PythonAssignment(name, source) => {
                let result = session.execute_python(machine, source)?;
                let result = result.trim_end();
                if result.is_empty() {
                    return Err("Python command substitution produced no value".into());
                }
                let assignment = format!("${name}={result}");
                assign_variable(&assignment, &[], session.selected_machine().map(str::to_owned).as_deref(),
                    &mut session.variables, None)?;
                Ok(None)
            }
            Action::PythonHook(cpu, symbol, source) => {
                let address = machine.monitor_symbol_address(symbol)?;
                if let Some(source) = source {
                    machine.install_monitor_python_hook(cpu, address, source)?;
                } else {
                    machine.remove_monitor_hooks(cpu, address)?;
                }
                Ok(None)
            }
            Action::CpuStatus(cpu, property) => {
                let thumb = machine.cortex_m_cpu(cpu)?;
                match property.as_str() {
                    "IsLockedUp" => writeln!(output.reply(), "{}", if thumb.locked_up() { "True" } else { "False" }),
                    "ExecutedInstructions" => writeln!(output.reply(), "{}", thumb.executed_instructions()),
                    "SecureState" => writeln!(output.reply(), "{}", if thumb.secure_state() { "True" } else { "False" }),
                    "GetItState" => writeln!(output.reply(), "{}", thumb.it_state()),
                    "FPCAR" | "FPCCR" => {
                        let address = if property == "FPCAR" { 0xe000_ef38 } else { 0xe000_ef34 };
                        let value = machine.bus.read_scalar::<4>(address, true).map_err(|error| error.to_string())?;
                        writeln!(output.reply(), "0x{value:08x}")
                    }
                    _ => unreachable!(),
                }.map(|()| None).map_err(|error| error.to_string())
            }
            Action::CpuPreserveFp(cpu) => machine.preserve_cortex_m_fp(cpu).map(|()| None),
            Action::CpuFpuEnabled(cpu, enabled) => {
                machine.cortex_m_cpu(cpu)?;
                let nvic = machine.bus.nvic.as_mut().filter(|nvic| nvic.has_fpu())
                    .ok_or("CPU has no floating-point unit")?;
                if let Some(enabled) = enabled {
                    nvic.set_fpu_access(*enabled);
                } else {
                    writeln!(output.reply(), "{}", if nvic.fpu_access() { "True" } else { "False" })
                        .map_err(|error| error.to_string())?;
                }
                Ok(None)
            }
            Action::CpuMpuBackground(cpu, enabled) => {
                machine.cortex_m_cpu(cpu)?;
                let nvic = machine.bus.nvic.as_mut().ok_or("CPU has no configurable MPU")?;
                let background = nvic.mpu_privileged_background(*enabled)?;
                if enabled.is_none() {
                    writeln!(output.reply(), "{}", if background { "True" } else { "False" })
                        .map_err(|error| error.to_string())?;
                }
                Ok(None)
            }
            Action::CpuTranslateAddress(cpu, address, write, execute) => {
                let unprivileged = machine.cortex_m_cpu(cpu)?.unprivileged();
                let nvic = machine.bus.nvic.as_ref().ok_or("CPU has no NVIC")?;
                let private = (0xe000_0000..0xe010_0000).contains(address);
                let allowed = if unprivileged && private {
                    *write && *address == 0xe000_ef00 && nvic.user_can_pend_interrupt()
                } else {
                    nvic.thumb_mpu_permits(*address, 1, *write, *execute, unprivileged, false)
                };
                if unprivileged && private && !allowed {
                    let message = format!("{cpu}: Using incorrect MMU mode for translation of address 0x{address:08X}, current MMU mode is 1, mapping is for MMU mode 0");
                    crate::monitor_log::record(2, &message);
                    if machine.bus.monitor_log_enabled(2) {
                        eprintln!("{message}");
                    }
                }
                let translated = if allowed { *address } else { u32::MAX };
                writeln!(output.reply(), "0x{translated:08X}").map_err(|error| error.to_string())?;
                Ok(None)
            }
            Action::CpuSecurityState(cpu, secure) => {
                machine.set_cortex_m_security(cpu, *secure)?;
                Ok(None)
            }
            Action::CpuSau(cpu, register, value) => {
                machine.cortex_m_cpu(cpu)?;
                if let Some(value) = machine.bus.nvic.as_mut().ok_or("CPU requires NVIC")?
                    .configure_sau(register, *value)?
                {
                    writeln!(output.reply(), "0x{value:08x}").map_err(|error| error.to_string())?;
                }
                Ok(None)
            }
            Action::CpuNonsecureVectorTable(cpu, address) => {
                machine.cortex_m_cpu(cpu)?;
                machine.bus.nvic.as_mut().ok_or("CPU requires NVIC")?.set_nonsecure_vector_base(*address)?;
                Ok(None)
            }
            Action::NvicLockup(target, state) => {
                machine.bus.monitor_nvic_signal(target, "Lockup", false)?;
                machine.bus.nvic.as_mut().unwrap().lockup = *state;
                Ok(None)
            }
            Action::NvicSignal(target, signal) => {
                let high = machine.bus.monitor_nvic_signal(target, signal, machine.cortex_m_sleeping())?;
                writeln!(output.reply(), "{}", if high { "True" } else { "False" })
                    .map(|()| None).map_err(|error| error.to_string())
            }
            Action::BusScalar(access) => {
                if access.value.is_none() {
                    access.apply(&mut machine.bus, &mut output.reply())
                } else {
                    access.apply(&mut machine.bus, &mut output)
                }
            }.map(|()| None),
            Action::FindBytes(search) => search
                .write_matches(&machine.bus, "", &mut output.output)
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
            Action::ExternalPmpRegister(path, config, index, value) => {
                if let Some(value) = machine.monitor_external_pmp(path, *config, *index, *value)? {
                    writeln!(output.reply(), "0x{value:x}").map_err(|error| error.to_string())?;
                }
                Ok(None)
            }
            Action::ClearTranslationCache(cpu) => machine.clear_monitor_translation_cache(cpu).map(|()| None),
            Action::CpuStep(cpu, steps) if !machine.is_cortex_m() => {
                let chunk = (*steps).min(budget);
                let start = machine.elapsed_steps();
                machine.monitor_step_native_cpu(cpu, chunk, &mut output).and_then(|(stop, count)| {
                    retired += count;
                    budget = budget.saturating_sub(machine.elapsed_steps().wrapping_sub(start));
                    if stop != Stop::Limit || chunk < *steps {
                        return Ok(Some(stop));
                    }
                    let pc = machine.monitor_named_pc(cpu)?;
                    writeln!(output.reply(), "0x{pc:08x}").map_err(|error| error.to_string())?;
                    Ok(None)
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
            Action::VectorRegisterLength(cpu, bits) => machine.configure_monitor_vlen(cpu, *bits).map(|()| None),
            Action::AddCsrHook(cpu, csr, source) => machine.install_csr_hook(cpu, *csr, source).map(|()| None),
            Action::AddStackAccessHook(cpu, source) => machine.install_stack_access_hook(cpu, source).map(|()| None),
            Action::TraceStackAccess(cpu) => machine.enable_rv64_stack_access_trace(cpu).map(|()| None),
            Action::ReadCsrValidation(cpu) | Action::RequireFullCsrValidation(cpu) => {
                machine.require_riscv_cpu(cpu)?;
                if matches!(command.action, Action::ReadCsrValidation(_)) {
                    writeln!(output.reply(), "Full").map_err(|error| error.to_string())?;
                }
                Ok(None)
            }
            Action::CpuMips(mips) => machine.set_monitor_cpu_rate(*mips).map(|()| None),
            Action::ReadCpuMips(mode) => {
                if machine.cpu_less {
                    return Err("the platform has no CPU".into());
                }
                mode.print(machine.bus.monitor_step_hz() / 1_000_000 / u64::from(machine.monitor_cpu_rate_divisor()), 4, &mut output.output).map(|()| None)
            }
            Action::SingleStepBlocking(value) => {
                if let Some(value) = value {
                    machine.monitor_single_step_blocking = *value;
                    Ok(None)
                } else {
                    writeln!(output.reply(), "{}", if machine.monitor_single_step_blocking { "True" } else { "False" })
                        .map(|()| None).map_err(|error| error.to_string())
                }
            }
            Action::EmulationIsStarted => {
                write!(output.reply(), "{}", emulation_started_reply(machine.monitor_is_active()))
                    .map(|()| None).map_err(|error| error.to_string())
            }
            Action::TimeSourceInfo => {
                writeln!(output.reply(), "Elapsed Virtual Time: {}\nQuantum: {}",
                    monitor_timestamp(machine.monitor_elapsed_ns()),
                    monitor_timestamp(u128::from(machine.monitor_quantum) * 10))
                    .map(|()| None).map_err(|error| error.to_string())
            }
            Action::AssembleBlock(cpu, address, triple, source, mode) => {
                let address = match address {
                    AssemblyAddress::Literal(address) => *address,
                    AssemblyAddress::ProgramCounter(cpu) => machine.monitor_named_pc(cpu)?,
                };
                crate::monitor_asm::assemble_source(machine, cpu, address, triple, source)
                    .and_then(|size| mode.print(size as u64, 4, &mut output.reply()))
                    .map(|()| None)
            }
            Action::MonitorLog(command) => {
                let start = machine.elapsed_steps();
                let (status, message) = crate::monitor_log::handle(machine, command, budget, &mut output)
                    .ok_or("invalid log monitor command")??;
                budget -= machine.elapsed_steps().wrapping_sub(start);
                match status {
                    "OK" => Ok(None),
                    "MATCH" => writeln!(output.reply(), "{message}")
                        .map(|()| None).map_err(|error| error.to_string()),
                    _ => Err(format!("log monitor {status}: {message}")),
                }
            }
            Action::SetEmulationPaused(paused) => {
                let message = if *paused {
                    machine.set_monitor_active(false);
                    "Pausing emulation..."
                } else if machine.monitor_is_active() {
                    "Emulation is already started. Ignoring command"
                } else {
                    machine.set_monitor_active(true);
                    "Starting emulation..."
                };
                writeln!(output.reply(), "{message}").map(|()| None).map_err(|error| error.to_string())
            }
            Action::UartIdle(timeout, tester, pause) => {
                let cycles = duration_cycles(*timeout, CORTEX_M_STEP_HZ, machine.bus.monitor_step_hz())?;
                let start = machine.elapsed_steps();
                let limit = cycles.min(budget);
                let (stop, count) = wait_for_uart_idle(machine, *tester, cycles, *pause, budget, &mut output)?;
                retired += count;
                budget -= machine.elapsed_steps().wrapping_sub(start);
                match stop {
                    Stop::Limit if limit == cycles => Ok(None),
                    Stop::OutputMatch => Err("UART was not idle".into()),
                    _ => Err(format!("UART idle interval ended before completion ({stop:?})")),
                }
            }
            Action::SetQuantum(duration) => {
                machine.monitor_quantum = *duration;
                Ok(None)
            }
            Action::CpuStep(cpu, count) if machine.has_thumb2_cpu() => {
                machine.prepare_monitor_cortex_step(cpu)?;
                let before = machine.cortex_m_cpu(cpu)?.executed_instructions();
                let start = machine.elapsed_steps();
                let was_paused = machine.primary.monitor_paused;
                machine.primary.monitor_paused = false;
                let result = machine.monitor_step_cortex_m((*count).min(budget), &mut output);
                machine.resume_cpu();
                machine.set_monitor_active(false);
                machine.primary.monitor_paused = was_paused;
                result?;
                retired += machine.cortex_m_cpu(cpu)?.executed_instructions().wrapping_sub(before);
                budget = budget.saturating_sub(machine.elapsed_steps().wrapping_sub(start));
                Ok(None)
            }
            Action::RunToSyncPoint | Action::RunFor(_) | Action::RunUntil(_) | Action::CpuStep(..) => {
                let valid_cpu = match &command.action {
                    Action::CpuStep(cpu, _) => machine.prepare_monitor_cortex_step(cpu),
                    _ => machine.prepare_monitor_cortex_run(),
                };
                valid_cpu.and_then(|()| {
                    let was_paused = machine.primary.monitor_paused;
                    if was_paused && matches!(command.action, Action::CpuStep(..)) {
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
                    machine.primary.monitor_paused = false;
                    let result = TimeHandle::run_grant(chunk, |granted| {
                        let result = machine.run(granted, &mut output)?;
                        Ok((result, machine.elapsed_steps().wrapping_sub(start)))
                    });
                    let paused_during_run = machine.primary.monitor_paused;
                    machine.set_monitor_active(false);
                    machine.primary.monitor_paused = paused_during_run || was_paused;
                    result.map(|((stop, count), used)| {
                        retired += count;
                        budget -= used;
                        (stop != Stop::Limit || (chunk < steps && !paused_during_run))
                            .then_some(stop)
                    })
                })
            }
            Action::CpuBlockSize(cpu, count) => {
                if *count == 0 {
                    return Err("MaximumBlockSize requires a positive instruction count".into());
                }
                machine.cortex_m_cpu(cpu)?;
                Ok(None)
            }
            Action::CpuBlockHook(cpu, end, source) => {
                machine.cortex_m_cpu(cpu)?;
                let hook = crate::cpu_python_hook::Hook::with_cpu(
                    source, machine.monitor_firmware.clone(), Some(cpu.clone()), cpu,
                )?;
                machine.bus.thumb_block_hooks[usize::from(*end)] = Some(hook);
                Ok(None)
            }
            Action::CpuWillExecuteIt(cpu, it) => {
                let thumb = machine.cortex_m_cpu(cpu)?;
                let execute = it & 15 == 0 || thumb.condition(u32::from(it >> 4));
                writeln!(output.reply(), "{}", if execute { "True" } else { "False" })
                    .map_err(|error| error.to_string())?;
                Ok(None)
            }
            Action::NvicPriorityBoost(target, ignore_primask) => {
                let boost = machine.monitor_nvic_priority_boost(target, *ignore_primask)?;
                writeln!(output.reply(), "{boost}").map_err(|error| error.to_string())?;
                Ok(None)
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
                        writeln!(output.reply(), "{mode}").map_err(|error| error.to_string())?;
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
            Action::ClicGpio(name, id, asserted) if machine.is_cortex_m() => {
                let line = u8::try_from(*id).ok().filter(|&line| line < 32).ok_or("expected NVIC IRQ line 0 through 31")?;
                machine.bus.drive_nvic_line(name, line, *asserted).map(|()| None)
            }
            Action::ClicGpio(name, id, asserted) => {
                if let Some(result) = machine.drive_arm_cpu_interrupt(name, *id, *asserted) {
                    result.map(|()| None)
                } else {
                    machine.bus.clic.as_mut().filter(|clic| clic.name == *name)
                        .ok_or_else(|| format!("unknown GPIO target {name:?}"))
                        .and_then(|clic| clic.gpio(*id, *asserted)).map(|()| None)
                }
            }
            Action::RamOverlay(name, cpu, address, size) => {
                let hart = cpu.as_deref().map(|cpu| machine.monitor_bus_hart(cpu)).transpose()?;
                machine.bus.map_named_monitor_ram(name, hart, *address, *size)
                    .map_err(|error| format!("KeywordException: Error E39: Exception was thrown during registration: {error}"))?;
                Ok(None)
            }
            Action::SubobjectTester(name, address) => machine.bus.map_subobject_tester(*address, name).map(|()| None),
            Action::ExportPeripherals(path, root) => {
                let platform = machine.monitor_platform.as_deref().ok_or("peripherals export requires a platform")?;
                crate::network_repl::write_peripheral_map(platform, Some(root), path).map(|()| None)
            }
            Action::PeripheralList => {
                for name in machine.bus.monitor_bus_device_names() {
                    writeln!(output.reply(), "{name}").map_err(|error| error.to_string())?;
                }
                Ok(None)
            }
            Action::GpioOverlay(links) => machine.bus.connect_stm32_gpios(
                links.iter().map(|(from, output, to, input)| (from.as_str(), *output, to.as_str(), *input)),
            ).map(|()| None),
            Action::GpioWire(from, output_pin, to, input_pin) => machine.bus
                .connect_stm32_gpio(from, *output_pin, to, *input_pin).map(|()| None),
            Action::LogUart(target) => machine.bus.log_uart(target).map(|()| None),
            Action::Illuminance(sensor, value) => machine.bus.set_illuminance(sensor, *value).map(|()| None),
            Action::IlluminanceFeed(sensor, path) => machine.bus.feed_illuminance(sensor, path).map(|()| None),
            Action::MagneticFeed(sensor, path) => machine.bus.feed_magnetic(sensor, path).map(|()| None),
            Action::PressureFeed(sensor, path, temperature) => machine.bus.feed_pressure(sensor, path, *temperature).map(|()| None),
            Action::VoltageFeed(sensor, path, input, channel) => machine.bus.feed_ina228(sensor, path, *input, *channel).map(|()| None),
            Action::PressureValue(sensor) => {
                let pressure = machine.bus.find_pressure(sensor)?.pressure();
                writeln!(output.reply(), "{pressure}").map_err(|error| error.to_string())?;
                Ok(None)
            }
            Action::PressurePointer(sensor, register) => {
                if !session.allow_privates {
                    return Err("setting I²C subaddress requires allowPrivates".into());
                }
                machine.bus.find_pressure(sensor)?.set_register_pointer(*register);
                Ok(None)
            }
            Action::PressureIncrement(sensor, enabled) => {
                if !session.allow_privates {
                    return Err("setting I²C auto-increment requires allowPrivates".into());
                }
                machine.bus.find_pressure(sensor)?.set_auto_increment(*enabled);
                Ok(None)
            }
            Action::FinishI2cSensorTransfer(sensor) => {
                machine.bus.find_pressure(sensor)?.stop();
                Ok(None)
            }
            Action::EnableZephyrMode(cpu, excluded) => {
                let config = crate::zephyr_boot::BootOptions {
                    busy_wait: Some((cpu.clone(), excluded.clone())),
                    ..Default::default()
                };
                let firmware = machine.monitor_firmware.clone();
                crate::zephyr_boot::configure(machine, &config, firmware.as_deref()).map(|()| None)
            }
            Action::Echo(message) => writeln!(output.reply(), "{message}")
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
            Action::ScriptLog(level, message) => {
                crate::monitor_log::record(*level, message);
                Ok(None)
            }
            Action::LastLog(count) => write_last_log(*count, &mut output.reply()).map(|()| None),
            Action::MonitorCreateMachine(name) => session.create_named_machine(machine, name.as_deref()).map(|()| None),
            Action::MonitorSelectMachine(name) => session.select_named_machine(machine, name).map(|()| None),
            Action::PlatformFragment(source, origin, root) => session.attach_platform_fragment(machine, source, origin, root).map(|()| None),
            Action::ReadLogLevels => session.describe_machine_log_levels(machine, &mut output.reply()).map(|()| None),
            Action::LogLevel(level) => {
                machine.bus.set_monitor_log_level(*level);
                Ok(None)
            }
            Action::PeripheralLog(target, level) if session.machine.as_ref() == Some(target)
                || session.named_machines.detached.contains_key(target) =>
            {
                session.named_machines.log_levels.insert(target.clone(), *level);
                if session.machine.as_ref() == Some(target) {
                    machine.bus.set_monitor_log_level(*level);
                }
                Ok(None)
            }
            Action::PeripheralLog(target, level) => {
                let name = target.strip_prefix("sysbus.").unwrap_or(target);
                if machine.monitor_named_pc(name).is_ok() {
                    crate::monitor_log::set_target_level(name, *level);
                    Ok(None)
                } else {
                    machine.bus.set_peripheral_log_level(target, *level).map(|()| None)
                }
            }
            Action::EgisSysconProperty(target, property) => {
                let value = machine.bus.egis_syscon_property(target, property)
                    .ok_or_else(|| format!("unknown AOSMU property {target} {property}"))?;
                writeln!(output.reply(), "{value}").map(|()| None).map_err(|error| error.to_string())
            }
            Action::Stm32AdcSample(target, value, channel, repeat) => {
                machine.bus.feed_stm32_adc_sample(target, *value, *channel, *repeat).map(|()| None)
            }
            Action::WatchdogIrq(target) => {
                let value = machine.bus.andes_watchdog_irq(target)
                    .ok_or_else(|| format!("unknown Andes watchdog {target:?}"))?;
                writeln!(output.reply(), "{}", if value { "True" } else { "False" })
                    .map(|()| None).map_err(|error| error.to_string())
            }
            Action::TimerQuadWord(target, offset, value) => {
                let address = machine.bus.machine_timer_register(target, *offset)
                    .ok_or_else(|| format!("unknown machine timer register {target} {offset:#x}"))?;
                BusScalar { address: u64::from(address), width: 8, value: *value, number_mode: session.number_mode }
                    .apply(&mut machine.bus, &mut output.reply()).map(|()| None)
            }
            Action::PeripheralRead(target, offset, width) => {
                let (aligned, shift, mask) = peripheral_word_access(*offset, *width)?;
                let address = machine.bus.named_word_register(target, aligned)
                    .ok_or_else(|| format!("unknown peripheral register {target} {offset:#x}"))?;
                let value = machine.bus.read_scalar::<4>(address, true).map_err(|error| error.to_string())?;
                writeln!(output.reply(), "0x{:01$x}", (value >> shift) & mask, (*width * 2) as usize)
                    .map(|()| None).map_err(|error| error.to_string())
            }
            Action::PeripheralWrite(target, offset, value, width) => {
                let (aligned, shift, mask) = peripheral_word_access(*offset, *width)?;
                if let Some(address) = machine.bus.named_word_register(target, aligned) {
                    let value = if *width == 4 {
                        *value
                    } else {
                        let prior = machine.bus.read_scalar::<4>(address, true).map_err(|error| error.to_string())?;
                        (prior & !(mask << shift)) | ((*value & mask) << shift)
                    };
                    machine.bus.write_scalar(address, 4, value, &mut output, true)
                        .map(|()| None).map_err(|error| error.to_string())
                } else if *width != 4 {
                    Err(format!("unknown peripheral register {target} {offset:#x}"))
                } else if let Some(address) = machine.bus.trivial_uart_register(target, *offset) {
                    machine.bus.write_scalar(address, 4, *value, &mut io::sink(), true)
                        .map(|()| None).map_err(|error| error.to_string())
                } else {
                    let address = machine.bus.nvic_register_address(target, *offset)?;
                    machine.bus.write_scalar(address, 4, *value, &mut output, true)
                        .map(|()| None).map_err(|error| error.to_string())
                }
            }
            Action::AssertCpuValue(cpu, register, expected) => {
                let cpu = if cpu == "@default" { machine.monitor_single_cpu_name()?.to_owned() } else { cpu.clone() };
                let actual = match register {
                    Some(register) => machine.monitor_named_register(&cpu, register, None)?
                        .ok_or("register has no readable value")?,
                    None => machine.monitor_named_pc(&cpu)?,
                };
                if actual == *expected {
                    Ok(None)
                } else {
                    let target = register.as_ref().map_or_else(|| "PC".into(), |name| format!("Register {name}"));
                    Err(format!("{target} value assertion failed, actual: {actual:#x}, expected: {expected:#x}"))
                }
            }
            Action::ReadCpuPc(cpu) => machine.monitor_named_pc(cpu).and_then(|pc| {
                writeln!(output.reply(), "0x{pc:08x}")
                    .map_err(|error| error.to_string()).map(|()| None)
            }),
            Action::CpuRegister(cpu, register, value) => {
                machine.monitor_named_register(cpu, register, *value).and_then(|value| {
                    if let Some(value) = value {
                        writeln!(output.reply(), "0x{value:x}").map_err(|error| error.to_string())?;
                    }
                    Ok(None)
                })
            }
            Action::ArmSystemRegister(cpu, register, value) => {
                if let Some(value) = machine.monitor_arm_system_register(cpu, register, *value)? {
                    writeln!(output.reply(), "0x{value:x}").map_err(|error| error.to_string())?;
                }
                Ok(None)
            }
            Action::ArmExceptionState(cpu, security) => {
                let state = machine.monitor_arm_exception_state(cpu, *security)?;
                writeln!(output.reply(), "{state}").map_err(|error| error.to_string())?;
                Ok(None)
            }
            // Synchronous script actions run at completed instruction boundaries.
            Action::SynchroniseQuantum => Ok(None),
            Action::CpuWait(cpu, asserted) => {
                machine.set_cortex_m_cpu_wait(cpu, *asserted).map(|()| None)
            }
            Action::I2cAcceleration(target, axis, value, default) => {
                if let Ok(sensor) = machine.bus.lis2dw12(target) {
                    sensor.set_axis(*axis, *value, *default);
                } else if !default {
                    let property = ["AccelerationX", "AccelerationY", "AccelerationZ"][*axis];
                    let value = (f64::from(*value) / 1_000_000.0).to_string();
                    machine.bus.set_i2c_sensor_property(target, property, &value)?;
                } else {
                    return Err(format!("unknown LIS2DW12 sensor {target:?}"));
                }
                Ok(None)
            }
            Action::I2cAccelerationFeed(target, path, multi_frequency) => {
                let now_ns = machine.monitor_elapsed_ns().min(u128::from(u64::MAX)) as u64;
                machine.bus.lis2dw12(target).and_then(|sensor| sensor.feed(path, now_ns, *multi_frequency))
                    .map(|()| None)
            }
            Action::I2cAccelerationRead(target, axis) => {
                let now_ns = machine.monitor_elapsed_ns().min(u128::from(u64::MAX)) as u64;
                let value = machine.bus.lis2dw12(target)?.axis(*axis, now_ns);
                writeln!(output.reply(), "{value}").map_err(|error| error.to_string())?;
                Ok(None)
            }
            Action::BusFaultingProperty(target, property, value) =>
                machine.bus.set_bus_faulting_property(target, property, value).map(|()| None),
            Action::UsartWriteChar(target, value) =>
                machine.bus.write_usart_char(target, *value).map(|()| None),
            Action::I2cSampleRate(target, rate) => {
                if !session.allow_privates {
                    return Err("setting the sensor sample rate requires allowPrivates".into());
                }
                let now_ns = machine.monitor_elapsed_ns().min(u128::from(u64::MAX)) as u64;
                machine.bus.lis2dw12(target)?.set_rate(*rate, now_ns);
                Ok(None)
            }
            Action::AllowPrivates(allow) => {
                session.allow_privates = *allow;
                Ok(None)
            }
            Action::Stm32SupplyVoltage(target, voltage) =>
                machine.bus.set_stm32l0_voltage(target, *voltage).map(|()| None),
            Action::I2cSensorProperty(target, property, value) =>
                machine.bus.set_i2c_sensor_property(target, property, value).map(|()| None),
            Action::SymbolAddress(symbol, index) => (|| {
                let path = machine.monitor_firmware.as_deref().ok_or("no firmware ELF for symbol lookup")?;
                let address = crate::cpu_python_hook::symbol_at(path, symbol, *index)?;
                writeln!(output.reply(), "0x{address:x}").map_err(|error| error.to_string())?;
                Ok(None)
            })(),
            Action::CpuSymbolVectorTable(cpu, symbol) => {
                let address = u32::try_from(machine.monitor_symbol_address(symbol)?)
                    .map_err(|_| "vector table address exceeds Cortex-M address width")?;
                machine.set_named_vector_table(cpu, address)?;
                if machine.elapsed_steps() == 0 {
                    machine.validate_entry()?;
                }
                Ok(None)
            }
            Action::CpuSymbolPc(cpu, symbol) => machine.set_monitor_symbol_pc(cpu, symbol).map(|()| None),
            Action::CpuPc(cpu, pc) if machine.is_cortex_m() => {
                machine.set_cortex_m_monitor_pc(cpu, *pc, &mut output.reply()).map(|()| None)
            }
            Action::CpuPc(cpu, pc) => machine.set_named_entry(cpu, *pc).map(|()| None),
            Action::CpuVectorTable(cpu, address) => {
                machine.set_named_vector_table(cpu, *address)?;
                machine.prepare_monitor_cortex_step(cpu)?;
                Ok(None)
            }
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
            Action::Tag(start, size, default, throws, name, silent) => machine
                .bus
                .tag_named(*start, *size, *default, *throws, name, *silent)
                .map(|()| None),
            Action::UnhandledAccess(policy) => {
                machine.bus.unhandled_access = Some(*policy);
                Ok(None)
            }
            Action::ReadBytes(address, count) => {
                read_bus_bytes(machine, *address, *count, &mut output.output).map(|()| None)
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
            Action::ToggleButton(name) => machine.bus
                .set_miv_button(name, crate::litex_panel::ButtonAction::Toggle)
                .or_else(|_| machine.bus.set_litex_button(name, crate::litex_panel::ButtonAction::Toggle))
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
                machine.bus.write_scalar(address, 4, *value, &mut output.output, true)
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
            Action::CreateUartTester(target, requested_machine, binary, timeout, echo, carriage_return, next_line, pause) => {
                let available = session.machine.as_deref().unwrap_or("machine-0");
                if let Some(name) = requested_machine && name != available {
                    return Err(format!("{}:{}: Machine with name {name} not found. Available machines: [{available}]", command.source.display(), command.line));
                }
                machine.bus.default_uart_pause = *pause;
                machine.bus.set_default_uart_echo(*echo);
                machine.bus.set_default_uart_next_line(*next_line);
                machine.bus.create_uart_tester(target, *binary, *timeout, *carriage_return)
                    .and_then(|id| writeln!(output.output, "{id}").map_err(|e| e.to_string()))
                    .map(|()| None)
            }
            Action::DefaultUartTimeout(timeout) => {
                machine.bus.set_default_uart_timeout(*timeout);
                Ok(None)
            }
            Action::DefaultUartTester(id) => {
                machine.bus.set_default_uart_tester(*id).map(|()| None)
            }
            Action::ClearUartTesterReport(id) => machine.bus.clear_uart_tester_report(*id).map(|()| None),
            Action::SetUartFailure(id, pattern, register) => machine
                .bus
                .set_uart_failure(*id, pattern.clone(), *register)
                .map(|()| None),
            Action::WaitForBytes(id, pattern, match_start, timeout, pause) => {
                let cycles = uart_wait_cycles(machine, *id, *timeout)?.min(budget);
                let start = machine.elapsed_steps();
                wait_for_uart_bytes(machine, *id, pattern, *match_start, cycles, *pause, &mut output)
                    .and_then(|(stop, count, matched)| {
                        retired += count;
                        budget -= machine.elapsed_steps().wrapping_sub(start);
                        let matched = matched.ok_or_else(|| format!("Terminal tester failed! Byte pattern was not received ({stop:?})"))?;
                        write_hex_list(matched.content, 2, &mut output.output).map_err(|e| e.to_string())?;
                        for group in matched.groups {
                            match group {
                                Some(bytes) => write_hex_list(bytes, 2, &mut output.output),
                                None => writeln!(output.output, "null"),
                            }.map_err(|e| e.to_string())?;
                        }
                        Ok(None)
                    })
            }
            Action::WaitForTester(id, pattern, regex, mode, timeout, absent, _, first_only, pause) => {
                let cycles = uart_wait_cycles(machine, *id, *timeout)?;
                let limit = cycles.min(budget);
                let start = machine.elapsed_steps();
                machine.bus.watch_uart_tester_text(*id, pattern, regex.clone(), *mode, *first_only).and_then(|watch| {
                    let pause = !*absent && pause.unwrap_or(machine.bus.default_uart_pause);
                    run_uart_watch(machine, watch, limit, Some(pause), &mut output)
                }).and_then(|(stop, count, line)| {
                    retired += count;
                    budget -= machine.elapsed_steps().wrapping_sub(start);
                    if pattern.is_empty() && let Some(line) = line {
                        writeln!(output.output, "{line}").map_err(|error| error.to_string())?;
                    }
                    if (*absent && stop == Stop::Limit && limit == cycles)
                        || (!*absent && stop == Stop::OutputMatch) {
                        Ok(None)
                    } else if *absent && stop != Stop::OutputMatch {
                        Err(format!("UART absence interval ended before completion ({stop:?})"))
                    } else if *absent {
                        Err(format!("Unexpected UART entry matching {pattern:?}"))
                    } else {
                        Err(format!("Terminal tester failed! Line containing {pattern:?} was not received ({stop:?})"))
                    }
                })
            }
            Action::WaitForTesterLines(sequence) => {
                let elapsed = machine.elapsed_steps();
                let (stop, count, matched) = sequence.wait(machine, budget, &mut output)?;
                retired += count;
                budget = budget.saturating_sub(machine.elapsed_steps().wrapping_sub(elapsed));
                if stop == Stop::OutputMatch && matched.is_some() {
                    Ok(None)
                } else {
                    Err(format!("Terminal tester failed! UART line sequence was not received ({stop:?})"))
                }
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
            Action::AddMemorySymbol(name, address, size) => {
                session.memory_symbols.entry(name.clone()).or_default().insert((*address, *size));
                Ok(None)
            }
            Action::ResolveMemorySymbols(name, single) => {
                if let Some(symbols) = session.memory_symbols.get(name) {
                    if *single && symbols.len() != 1 {
                        return Err(format!("Expected a single symbol for {name}. There are multiple symbols."));
                    }
                    for (address, _) in symbols {
                        writeln!(output.reply(), "{address:#x}").map_err(|error| error.to_string())?;
                    }
                } else {
                    let address = machine.monitor_symbol_address(name)
                        .map_err(|_| format!("Symbol {name} not found"))?;
                    writeln!(output.reply(), "{address:#x}").map_err(|error| error.to_string())?;
                }
                Ok(None)
            }
            Action::CheckMemory(watch) => {
                if watch.check(|address, width| BusScalar::read_value(&mut machine.bus, address, width))? {
                    Ok(None)
                } else {
                    Err("memory assertion did not match the current value".into())
                }
            }
            Action::WaitForMemory(watch, timeout, pause) => {
                let limit = duration_cycles(*timeout, CORTEX_M_STEP_HZ, machine.bus.monitor_step_hz())?.min(budget);
                machine.bus.memory_watch = Some(watch.clone());
                machine.set_monitor_active(true);
                let start = machine.elapsed_steps();
                let result = run_until_deadline(machine, limit, &mut output, |bus, _| {
                    bus.memory_watch.as_ref().is_some_and(crate::memory_watch::MemoryWatch::matched)
                });
                machine.bus.memory_watch = None;
                machine.set_monitor_active(!pause);
                let (stop, count) = result?;
                retired += count;
                budget -= machine.elapsed_steps().wrapping_sub(start);
                if stop == Stop::OutputMatch {
                    Ok(None)
                } else {
                    Err(format!("memory assertion did not observe a matching write ({stop:?})"))
                }
            }
            Action::PwmTester(target, property) => machine.bus.pwm_tester(target, property).and_then(|value| {
                if let Some(value) = value {
                    writeln!(output.reply(), "{value}").map_err(|error| error.to_string())?;
                }
                Ok(None)
            }),
            Action::DefaultLedTimeout(timeout) => {
                session.led_timeout = *timeout;
                Ok(None)
            }
            Action::CreateLedTester(target, timeout) => {
                machine.bus.gpio_led_index(target)?;
                let name = session.selected_machine().map(str::to_owned);
                let id = session.led_testers.iter()
                    .position(|(owner, path, _)| *owner == name && path == target)
                    .unwrap_or_else(|| {
                        let id = session.led_testers.len();
                        session.led_testers.push((name, target.clone(), timeout.unwrap_or(session.led_timeout)));
                        id
                    });
                writeln!(output.reply(), "{id}").map_err(|error| error.to_string())?;
                Ok(None)
            }
            Action::AssertLedState(..) | Action::WaitForLed(..) => {
                let (target, state, timeout, hold, pause) = match &command.action {
                    Action::WaitForLed(target, state, timeout, hold, pause) =>
                        (target.as_str(), *state, *timeout, *hold, *pause),
                    Action::AssertLedState(id, state, timeout, hold, pause) => {
                        let id = match (id, session.led_testers.len()) {
                            (Some(id), _) => *id,
                            (None, 1) => 0,
                            (None, 0) => return Err("There are no testers available.".into()),
                            (None, _) => return Err("There is more than one tester available - please specify ID of the desired tester.".into()),
                        };
                        let (owner, target, default) = session.led_testers.get(id)
                            .ok_or_else(|| format!("Tester for given ID={id} was not found."))?;
                        if owner.as_deref() != session.selected_machine() {
                            return Err("LED tester belongs to a different machine".into());
                        }
                        (target.as_str(), *state, timeout.unwrap_or(*default), *hold, *pause)
                    }
                    _ => unreachable!(),
                };
                let rate = machine.bus.monitor_step_hz();
                let timeout = duration_cycles(timeout, CORTEX_M_STEP_HZ, rate)?;
                let hold = duration_cycles(hold, CORTEX_M_STEP_HZ, rate)?;
                let index = machine.bus.gpio_led_index(target)?;
                let needs_time = hold.min(budget) != 0
                    || (timeout.min(budget) != 0 && machine.bus.gpio_led(index).state != state);
                if needs_time {
                    machine.set_monitor_active(true);
                }
                let count = wait_for_led(machine, target, state, timeout, hold, &mut budget, &mut output)
                    .map_err(|error| if error.contains("did not reach state") {
                        format!("LED assertion not met. {error}")
                    } else {
                        error
                    })?;
                if pause && needs_time {
                    machine.set_monitor_active(false);
                }
                retired += count;
                Ok(None)
            }
            Action::WriteTesterUart(id, text, ending, echo, pause) => {
                let target = machine.bus.uart_tester_target(*id)?.to_owned();
                let cycles = if echo.unwrap_or_else(|| machine.bus.default_uart_echo()) && !text.is_empty() {
                    Some(uart_wait_cycles(machine, *id, None)?)
                } else {
                    None
                };
                let quantum = duration_cycles(
                    machine.monitor_quantum, CORTEX_M_STEP_HZ, machine.bus.monitor_step_hz(),
                )?.max(1);
                let now = machine.elapsed_steps();
                let next_sync = now.saturating_add(quantum - now % quantum);
                machine.bus.write_uart_tester_input(&target, text, ending, next_sync)?;
                if let Some(cycles) = cycles {
                    let watch = machine.bus.watch_uart_tester_text(*id, text, None, UartWaitMode::Prompt, Some(false))?;
                    let start = machine.elapsed_steps();
                    let limit = cycles.min(budget);
                    let pause = pause.unwrap_or(machine.bus.default_uart_pause);
                    let (stop, count, _) = run_uart_watch(machine, watch, limit, Some(pause), &mut output)
                        .map_err(|error| format!("{}:{}: {error}", command.source.display(), command.line))?;
                    retired += count;
                    budget -= machine.elapsed_steps().wrapping_sub(start);
                    match stop {
                        Stop::OutputMatch => Ok(None),
                        Stop::Limit if cycles == limit => Err(format!("Terminal tester failed! UART echo {text:?} was not received")),
                        _ => Err(format!("UART echo wait ended before completion ({stop:?})")),
                    }
                } else {
                    Ok(None)
                }
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

fn live_command_boot(path: &Path, session: &Session) -> Boot {
    let mut boot = Boot {
        platform: Some(path.to_owned()),
        image: Some(Asset::new("", path)),
        started: true,
        multiple_machines: session.named_machines.order.len() > 1 || session.named_machines.draft.is_some(),
        ..Boot::default()
    };
    boot.session.machine.clone_from(&session.machine);
    boot.session.machine_cleared = session.machine_cleared;
    boot.session.number_mode = session.number_mode;
    boot
}

fn deferred_script_state(path: &Path, session: &Session) -> ScriptState {
    ScriptState {
        boot: live_command_boot(path, session),
        machine: true,
        variables: session.variables.clone(),
        macros: session.macros.clone(),
        aliases: session.aliases.clone(),
        ..ScriptState::default()
    }
}

fn apply_deferred_state(session: &mut Session, state: ScriptState) -> Boot {
    session.machine_cleared = state.boot.session.machine_cleared;
    session.variables = state.variables;
    session.macros = state.macros;
    session.aliases = state.aliases;
    state.boot
}

fn validate_deferred_commands(boot: &Boot, depth: usize, allow_snapshots: bool) -> Result<(), String> {
    if depth >= 32 {
        return Err("recursive or excessively deep deferred monitor script".into());
    }
    if !boot.post_run.is_empty() || (!allow_snapshots && boot.runtime.iter().any(|entry| matches!(
        entry.action, Action::SaveMemory(..) | Action::LoadMemory(_) | Action::ClearMemory
    ))) {
        return Err("snapshot and post-run commands are unavailable in this deferred invocation".into());
    }
    Ok(())
}

fn run_deferred_commands(
    machine: &mut Machine, boot: Boot, budget: u64, output: &mut ScriptOutput<'_>, session: &mut Session,
    depth: usize,
) -> Result<(Stop, u64), String> {
    session.number_mode = boot.session.number_mode;
    if boot.runtime.is_empty() {
        return Ok((Stop::ScriptComplete, 0));
    }
    let replies = output.replies.as_mut().map(|reply| &mut **reply as &mut dyn Write);
    run_commands(machine, &boot.runtime, budget, &mut *output.output, session, replies, depth + 1)
}

fn write_last_log(count: i32, output: &mut impl Write) -> Result<(), String> {
    for entry in crate::monitor_log::tail(count) {
        writeln!(output, "{entry}").map_err(|error| error.to_string())?;
    }
    Ok(())
}

pub fn frame(output: &mut impl Write, status: &str, bytes: &[u8]) -> io::Result<()> {
    writeln!(output, "{status} {}", bytes.len())?;
    output.write_all(bytes)?;
    output.flush()
}

pub(crate) struct AutoLoadCommand {
    pub(crate) name: &'static str,
    pub(crate) description: &'static str,
    pub(crate) method_signatures: &'static [&'static str],
    pub(crate) execute: Box<dyn FnMut(&mut Machine, &[&str]) -> Result<String, String>>,
}

impl AutoLoadCommand {
    pub(crate) fn register(self, monitor: &mut MonitorBuiltins) -> Result<(), String> {
        if self.name.split_whitespace().collect::<Vec<_>>().as_slice() != [self.name]
            || matches!(self.name, "help" | "next_value" | "sleep" | "echo" | "dump" | "dump_file")
            || monitor.commands.contains_key(self.name)
        {
            return Err(format!("invalid or duplicate monitor command {:?}", self.name));
        }
        monitor.commands.insert(self.name, self);
        Ok(())
    }
}

/// Execute one runtime command at a time on the same machine. Separate command
/// status from UART/monitor bytes so firmware output cannot create a reply frame.
#[derive(Default)]
pub(crate) struct MonitorBuiltins {
    next_value: i64,
    seed: u32,
    extensions: managed_extensions::Session,
    commands: std::collections::BTreeMap<&'static str, AutoLoadCommand>,
}

impl MonitorBuiltins {
    pub(crate) fn execute_registered_script(&mut self, machine: &mut Machine, text: &str) -> Result<String, String> {
        let mut output = String::new();
        for command in action_arguments::commands(text) {
            let command = command.trim();
            if command.is_empty() {
                continue;
            }
            let response = self.execute(machine, command)
                .ok_or_else(|| format!("unknown monitor command {command}"))??;
            output.push_str(&response);
        }
        Ok(output)
    }

    pub(crate) fn execute(&mut self, machine: &mut Machine, text: &str) -> Option<Result<String, String>> {
        let command = text.split_whitespace().next()?;
        let object = command.split_once('[').map_or(command, |(name, _)| name);
        if let Some(registered) = self.commands.get_mut(object) {
            return Some(
                action_arguments::words(text).and_then(|mut words| {
                    let indexer = words[0].strip_prefix(object).filter(|tail| !tail.is_empty());
                    if let Some(indexer) = indexer {
                        words[0] = indexer;
                        (registered.execute)(machine, &words)
                    } else {
                        (registered.execute)(machine, &words[1..])
                    }
                }).map_err(|error| {
                    if registered.method_signatures.is_empty() {
                        error
                    } else {
                        format!("{error}\nThe following methods are available:\n{}", registered.method_signatures.join("\n"))
                    }
                }),
            );
        }
        if matches!(command, "i" | "include") {
            let words = match command_words(text) {
                Ok(words) => words,
                Err(error) => return Some(Err(error)),
            };
            if let [_, source] = words.as_slice()
                && let Some(path) = source.strip_prefix('@')
                && Path::new(&path_value(path)).extension().is_some_and(|ext| ext == "cs")
            {
                return Some(self.extensions.include(Path::new(&path_value(path))).map(|()| String::new()));
            }
        }
        if command == "emulation" {
            let words = match command_words(text) {
                Ok(words) => words,
                Err(error) => return Some(Err(error)),
            };
            match words.as_slice() {
                ["emulation", "GetSeed"] => return Some(Ok(format!("{}\n", self.seed))),
                ["emulation", "SetSeed", seed] => return Some(crate::parse_address(seed).map(|seed| {
                    self.seed = seed;
                    String::new()
                })),
                ["emulation", method] => {
                    if let Some(result) = self.extensions.invoke(method) {
                        return Some(result);
                    }
                }
                _ => {}
            }
        }
        if !matches!(command, "help" | "next_value" | "sleep" | "echo" | "dump" | "dump_file") {
            return None;
        }
        Some(command_words(text).and_then(|words| match words.as_slice() {
            ["help"] => Ok(self.commands.values().map(|command| {
                format!("{}: {}\n", command.name, command.description)
            }).collect()),
            ["next_value"] | ["next_value", _] => {
                let offset = words.get(1).map_or(Ok(0), |value| value.parse::<i64>())
                    .map_err(|_| "expected a next_value offset")?;
                let current = self.next_value.checked_add(offset).ok_or("next_value overflow")?;
                self.next_value = self.next_value.checked_add(1).ok_or("next_value overflow")?;
                Ok(format!("{current}\n"))
            }
            ["sleep", seconds] => {
                let seconds = seconds.parse::<f64>().map_err(|_| "invalid sleep duration")?;
                let duration = Duration::try_from_secs_f64(seconds).map_err(|_| "invalid sleep duration")?;
                std::thread::sleep(duration);
                Ok(String::new())
            }
            ["echo"] => Ok("\n".to_owned()),
            ["echo", value] => Ok(format!("{}\n", unquote(value))),
            ["echo", "-n", value] => Ok(unquote(value).to_owned()),
            ["echo", ..] => Ok("usage: echo [-n] [string]\n".to_owned()),
            ["dump", address, count] | ["dump", address, count, _] => {
                let width = words.get(3).map_or(Ok(16), |value| value.parse::<usize>())
                    .map_err(|_| "invalid dump row width")?;
                let address = crate::parse_address(address)?;
                let count = count.parse::<usize>().map_err(|_| "invalid dump length")?;
                dump_monitor_rows(machine, address, count, width)
            }
            ["dump_file", address, count, path] => {
                let address = crate::parse_address(address)?;
                let count = count.parse::<usize>().map_err(|_| "invalid dump length")?;
                let path = unquote(path);
                let path = path_value(path.strip_prefix('@').unwrap_or(path));
                let bytes = machine.bus.read_bytes(address, count).map_err(|error| error.to_string())?;
                let mut file = io::BufWriter::new(fs::File::create(path).map_err(|error| error.to_string())?);
                for byte in bytes {
                    file.write_all(&[byte.map_err(|error| error.to_string())?])
                        .map_err(|error| error.to_string())?;
                }
                file.flush().map_err(|error| error.to_string())?;
                Ok(String::new())
            }
            _ => Err(format!("invalid {command} arguments")),
        }))
    }
}

fn dump_monitor_rows(machine: &mut Machine, address: u32, count: usize, width: usize) -> Result<String, String> {
    use std::fmt::Write as _;

    if width == 0 {
        return Err("dump row width must be positive".into());
    }
    let mut output = String::new();
    for offset in (0..count).step_by(width) {
        let row = u32::try_from(offset).ok().and_then(|offset| address.checked_add(offset))
            .ok_or("dump address overflow")?;
        let values = machine.bus.read_bytes(row, width).map_err(|error| error.to_string())?;
        write!(output, "0x{row:08X} |").unwrap();
        let mut preview = String::new();
        for value in values {
            let value = value.map_err(|error| error.to_string())?;
            write!(output, " {value:02X}").unwrap();
            preview.push(if (0x20..=0x7f).contains(&value) { char::from(value) } else { '.' });
        }
        writeln!(output, " | {preview}").unwrap();
    }
    Ok(output)
}

fn read_monitor_command(input: &mut impl BufRead) -> Result<Option<String>, String> {
    let mut line = String::new();
    if input.read_line(&mut line).map_err(|error| error.to_string())? == 0 {
        return Ok(None);
    }
    if let Some(length) = line.strip_prefix("PENV-COMMAND ") {
        read_monitor_text(input, length).map(Some)
    } else {
        Ok(Some(line))
    }
}

fn live_load_elf(
    machine: &mut Machine,
    command: &str,
    session: &Session,
    root: &Path,
    assets: Option<&Path>,
) -> Option<Result<(), String>> {
    let mut words = command.split_ascii_whitespace();
    if words.next() != Some("sysbus") || words.next() != Some("LoadELF") {
        return None;
    }
    Some((|| {
        let words = command_words(command)?;
        let ["sysbus", "LoadELF", value, options @ ..] = words.as_slice() else {
            return Err("expected sysbus LoadELF @file [useVirtualAddress]".into());
        };
        let virtual_addresses = load_elf_address_mode(options)?;
        let value = resolve(&session.variables, value)?;
        let name = monitor_path_token(value).ok_or("expected @firmware-path")?;
        let origin = monitor_origin(&session.variables).join("<monitor>");
        let asset = Asset::new(name, &origin);
        let path = asset.input_path(root, assets)?;
        load_monitor_elf(machine, path, virtual_addresses)
    })())
}

fn load_monitor_elf(machine: &mut Machine, path: PathBuf, virtual_addresses: bool) -> Result<(), String> {
    let file = fs::File::open(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    crate::image::load(machine, file, false, Some(virtual_addresses), false)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    if machine.is_cortex_m() && machine.monitor_firmware.is_none() && machine.elapsed_steps() == 0 {
        machine.validate_entry()?;
    }
    machine.monitor_firmware = Some(path);
    Ok(())
}

pub fn serve(
    machine: &mut Machine,
    mut output: impl Write,
    startup_output: &[u8],
    mut session: Session,
    root: &Path,
    assets: Option<&Path>,
) -> Result<(), String> {
    use std::sync::mpsc::{RecvTimeoutError, sync_channel};

    let initial: Vec<_> = (0..machine.hart_count()).map(|id| machine.hart_mut(id).clone()).collect();
    let path = Path::new("<monitor>");
    let mut builtins = MonitorBuiltins::default();
    frame(&mut output, "READY", startup_output).map_err(|error| error.to_string())?;

    // Keep the machine on the monitor thread. The reader only delivers complete
    // commands so a partial frame does not stop an active emulation.
    let (send, commands) = sync_channel(1);
    std::thread::Builder::new().name("monitor-input".into()).spawn(move || {
        let mut input = io::stdin().lock();
        loop {
            match read_monitor_command(&mut input) {
                Ok(Some(command)) => {
                    if send.send(Ok(command)).is_err() {
                        break;
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    let _ = send.send(Err(error));
                    break;
                }
            }
        }
    }).map_err(|error| error.to_string())?;

    let mut background_stop = None;
    loop {
        let command = if machine.monitor_is_active() {
            match commands.recv_timeout(Duration::from_millis(5)) {
                Ok(command) => command,
                Err(RecvTimeoutError::Disconnected) => return Ok(()),
                Err(RecvTimeoutError::Timeout) => {
                    let steps = (machine.bus.monitor_step_hz() / 1_000).max(1);
                    let start = machine.elapsed_steps();
                    let result = run_until_deadline(machine, steps, &mut io::sink(), |_, _| false);
                    match result {
                        Ok((Stop::Limit, _)) if machine.elapsed_steps() != start => {}
                        Ok((Stop::Limit | Stop::Paused, _)) => machine.set_monitor_active(false),
                        Ok((stop, _)) => {
                            machine.set_monitor_active(false);
                            background_stop = Some(format!("emulation stopped: {stop:?}"));
                        }
                        Err(error) => {
                            machine.set_monitor_active(false);
                            background_stop = Some(error);
                        }
                    }
                    continue;
                }
            }
        } else {
            match commands.recv() {
                Ok(command) => command,
                Err(_) => return Ok(()),
            }
        }?;
        if let Some(error) = background_stop.take() {
            frame(&mut output, "ERROR", error.as_bytes()).map_err(|error| error.to_string())?;
            continue;
        }
        let text = command.trim();
        if let Some(result) = live_load_elf(machine, text, &session, root, assets) {
            let reply = match result {
                Ok(()) => frame(&mut output, "OK", &[]),
                Err(error) if error.starts_with("monitor asset unavailable: ") => {
                    frame(&mut output, "ASSET", error.trim_start_matches("monitor asset unavailable: ").as_bytes())
                }
                Err(error) => frame(&mut output, "ERROR", error.as_bytes()),
            };
            reply.map_err(|error| error.to_string())?;
            continue;
        }
        if let Some(result) = builtins.execute(machine, text) {
            let reply = match result {
                Ok(body) => frame(&mut output, "OK", body.as_bytes()),
                Err(error) => frame(&mut output, "ERROR", error.as_bytes()),
            };
            reply.map_err(|error| error.to_string())?;
            continue;
        }
        if let Some(result) = crate::monitor_asm::handle(machine, text) {
            match result {
                Ok(size) => frame(&mut output, "OK", size.to_string().as_bytes()),
                Err(error) => frame(&mut output, "UNSUPPORTED", error.as_bytes()),
            }.map_err(|error| error.to_string())?;
            continue;
        }
        let log_result = if text.split_ascii_whitespace().next().is_some_and(crate::monitor_log::is_command) {
            match resolve_log_command(text, &session.variables) {
                Ok(command) => crate::monitor_log::handle(machine, &command, u64::MAX, &mut io::sink()),
                Err(error) => Some(Err(error)),
            }
        } else {
            None
        };
        if let Some(result) = log_result {
            match result {
                Ok((status, value)) => frame(&mut output, status, value.as_bytes()),
                Err(error) if error.starts_with("unsupported") => {
                    frame(&mut output, "UNSUPPORTED", error.as_bytes())
                }
                Err(error) => frame(&mut output, "ERROR", error.as_bytes()),
            }.map_err(|error| error.to_string())?;
            continue;
        }
        let mut boot = live_command_boot(path, &session);
        let mut existing_machine = true;
        let mut included_state = None;
        let parsed = (|| {
            let mut separators = Vec::new();
            let words = if text.contains(';') {
                command_tokens(text, Some(&mut separators))?
            } else {
                command_words(text)?
            };
            if !separators.is_empty() || matches!(words.as_slice(), ["include" | "i" | "ExecuteScript", _]) {
                let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
                boot.runtime.push(RuntimeCommand {
                    line: 1, source: path.to_owned(),
                    action: Action::InvokeMonitorScript(text.to_owned(), cwd.join(path), root.to_owned()),
                });
                return Ok(());
            }
            if let ["runMacro", name] = words.as_slice()
                && let Some(name) = name.strip_prefix('$')
            {
                boot.runtime.push(RuntimeCommand {
                    line: 1, source: path.to_owned(),
                    action: Action::InvokeMonitorMacro(name.to_owned(), root.to_owned()),
                });
                return Ok(());
            }
            if words.first().is_some_and(|verb|
                matches!(*verb, "alias" | "macro" | "printenv")
                    || matches!(words.as_slice(), ["mach" | "machine", "set" | "clear", ..])
                    || monitor_alias(&session.aliases, session.selected_machine(), verb).is_some())
            {
                let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
                let mut state = ScriptState {
                    boot: std::mem::take(&mut boot),
                    machine: true,
                    variables: session.variables.clone(),
                    macros: session.macros.clone(),
                    aliases: session.aliases.clone(),
                    ..ScriptState::default()
                };
                load_text(path, cwd.join(path), root, &mut state, text)?;
                if !state.boot.post_run.is_empty() {
                    return Err("post-run script actions are unavailable in a live include".into());
                }
                if state.boot.runtime.iter().any(|command| matches!(command.action,
                    Action::SaveMemory(..) | Action::LoadMemory(_)))
                {
                    return Err("use live Save and Load as separate monitor commands".into());
                }
                boot = state.boot;
                included_state = Some((state.variables, state.macros, state.aliases));
                return Ok(());
            }
            if let Some((name, source)) = python_assignment(text, &session.variables)? {
                boot.runtime.push(RuntimeCommand {
                    line: 1, source: path.to_owned(), action: Action::PythonAssignment(name, source),
                });
                return Ok(());
            }
            if assign_variable(text, &words, session.machine.as_deref().filter(|_| !session.machine_cleared), &mut session.variables, None)? {
                return Ok(());
            }
            execute(text, &words, &session.variables, &mut existing_machine, &mut boot, path, root, 1)
        })();
        let mut bytes = Vec::new();
        let reply = match (parsed, boot.runtime.as_slice()) {
            (Ok(()), []) => {
                session.number_mode = boot.session.number_mode;
                session.machine_cleared = boot.session.machine_cleared;
                if let Some((variables, macros, aliases)) = included_state.take() {
                    session.variables = variables;
                    session.macros = macros;
                    session.aliases = aliases;
                }
                frame(&mut output, "OK", &[])
            }
            (Ok(()), [RuntimeCommand { action: Action::UartIdle(steps, tester, pause), .. }]) => {
                let result = duration_cycles(*steps, CORTEX_M_STEP_HZ, machine.bus.monitor_step_hz())
                    .and_then(|cycles| wait_for_uart_idle(machine, *tester, cycles, *pause, u64::MAX, &mut bytes));

                match result {
                    Ok((Stop::Limit, _)) => frame(&mut output, "MATCH", &[]),
                    Ok((Stop::OutputMatch, _)) => frame(&mut output, "FAIL", b"UART was not idle"),
                    Ok((stop, _)) => frame(&mut output, "STOP", format!("{stop:?}").as_bytes()),
                    Err(error) => frame(&mut output, "ERROR", error.as_bytes()),
                }
            }

            (Ok(()), [RuntimeCommand { action: Action::WaitForBytes(id, pattern, match_start, timeout, pause), .. }]) => {
                let result = uart_wait_cycles(machine, *id, *timeout)
                    .and_then(|cycles| wait_for_uart_bytes(machine, *id, pattern, *match_start, cycles, *pause, &mut io::sink()));
                match result {
                    Ok((_, _, Some(matched))) => frame(&mut output, "MATCHBYTES", uart_byte_payload(matched).as_bytes()),
                    Ok((Stop::Limit, _, None)) => frame(&mut output, "FAIL", b"Terminal tester failed! Byte pattern was not received (Limit)"),
                    Ok((stop, _, None)) => frame(&mut output, "STOP", format!("{stop:?}").as_bytes()),
                    Err(error) => frame(&mut output, "ERROR", error.as_bytes()),
                }
            }
            (Ok(()), [RuntimeCommand { action: Action::WaitForTesterLines(sequence), .. }]) => {
                match sequence.wait(machine, u64::MAX, &mut bytes) {
                    Ok((Stop::OutputMatch, _, Some((line, Some(timestamp))))) =>
                        frame(&mut output, "MATCHTIMEDUART", format!("{timestamp}\t{}", uart_capture_payload(&line, None)).as_bytes()),
                    Ok((Stop::OutputMatch, _, Some((line, None)))) =>
                        frame(&mut output, "MATCH", line.as_bytes()),
                    Ok((Stop::Limit, _, _)) =>
                        frame(&mut output, "FAIL", b"Terminal tester failed! UART line sequence was not received (Limit)"),
                    Ok((stop, _, _)) => frame(&mut output, "STOP", format!("{stop:?}").as_bytes()),
                    Err(error) => frame(&mut output, "ERROR", error.as_bytes()),
                }
            }
            (Ok(()), [RuntimeCommand { action: Action::WaitForTester(id, expected, regex, mode, timeout, absent, capture_groups, first_only, pause), .. }]) => {
                let result = uart_wait_cycles(machine, *id, *timeout).and_then(|cycles| {
                    let watch = machine.bus.watch_uart_tester_text(*id, expected, regex.clone(), *mode, *first_only)?;
                    let pause = !*absent && pause.unwrap_or(machine.bus.default_uart_pause);
                    let started_at = machine.monitor_elapsed_ns();
                    let stop = drive_tester_watch(machine, watch, cycles, pause, &mut bytes);
                    let failure = machine.bus.uart_watch_failure(watch).map(str::to_owned);
                    let matched = machine.bus.uart_tester_matched_line(watch);
                    let timestamp = if machine.bus.uart_tester_matched_unfinished_line(watch) {
                        Some(machine.monitor_elapsed_ns())
                    } else {
                        machine.bus.uart_tester_matched_timestamp(watch)
                    };
                    let captures = if machine.bus.uart_tester_suppresses_captures(watch) {
                        None
                    } else {
                        regex.as_ref()
                    };
                    machine.bus.finish_stm32_uart_watch(watch);
                    if let Some(line) = failure {
                        return Ok(("FAIL", format!("Terminal tester failed! Test failing entry has been found on UART: {line}")));
                    }
                    match (stop?, matched) {
                        ((Stop::OutputMatch, _), Some(line)) if *absent => Ok(("FAIL", format!("Unexpected UART entry: {line}"))),
                        ((Stop::Limit, _), _) if *absent => Ok(("MATCH", String::new())),
                        ((Stop::OutputMatch, _), Some(line)) => {
                            let regex = if *capture_groups { captures } else { None };
                            let payload = uart_capture_payload(&line, regex);
                            if let Some(timestamp) = timestamp {
                                Ok(("MATCHTIMEDUART", format!("{timestamp}\t{payload}")))
                            } else if *capture_groups {
                                Ok(("MATCHCAPTURES", payload))
                            } else {
                                Ok(("MATCH", line))
                            }
                        }
                        ((Stop::Limit, _), _) => {
                            let buffered = machine.bus.uart_tester_buffered_tail(watch).unwrap_or_default();
                            Ok(("FAIL", format!(
                                "InvalidOperationException: Terminal tester failed! UART line containing {expected:?} was not received (Limit)\nVirtual time: {} to {}\nBuffered UART tail: {buffered:?}",
                                monitor_timestamp(started_at), monitor_timestamp(machine.monitor_elapsed_ns()),
                            )))
                        }
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
            (Ok(()), commands) if included_state.is_some() || commands.len() == 1 => {
                let echo_assertion = matches!(commands,
                    [RuntimeCommand { action: Action::WriteTesterUart(_, text, _, echo, _), .. }]
                        if !text.is_empty() && echo.unwrap_or_else(|| machine.bus.default_uart_echo()));
                match run_commands(machine, commands, u64::MAX, &mut bytes, &mut session, None, 0) {
                    Ok((Stop::ScriptComplete, _)) => {
                        if let Some((variables, macros, aliases)) = included_state.take() {
                            session.number_mode = boot.session.number_mode;
                            session.machine_cleared = boot.session.machine_cleared;
                            session.variables = variables;
                            session.macros = macros;
                            session.aliases = aliases;
                        }
                        frame(&mut output, if echo_assertion { "MATCH" } else { "OK" }, &bytes)
                    }
                    Ok((stop, _)) => {
                        frame(&mut output, "STOP", format!("{stop:?}").as_bytes())
                    }
                    Err(error) if (error.starts_with("<monitor>:1: PC ")
                        || error.starts_with("<monitor>:1: Register ")) && error.contains(" value assertion failed,") => {
                        frame(&mut output, "FAIL", error.as_bytes())
                    }
                    Err(error) if matches!(commands,
                        [RuntimeCommand { action: Action::CpuRegister(_, _, Some(_)), .. }])
                        && error.ends_with("register is read-only.") =>
                    {
                        frame(&mut output, "REJECTED", error.as_bytes())
                    }
                    Err(error) if matches!(commands, [RuntimeCommand { action: Action::ResolveMemorySymbols(..), .. }])
                        && (error.contains("Expected a single symbol") || (error.starts_with("Symbol ") && error.ends_with(" not found"))) =>
                    {
                        frame(&mut output, "REJECTED", error.as_bytes())
                    }
                    Err(error) if matches!(commands, [RuntimeCommand { action: Action::SymbolAddress(..), .. }])
                        && error.strip_prefix("<monitor>:1: ").is_some_and(|message| {
                            message.starts_with("Wrong index ")
                                || message.starts_with("Could not find any address for symbol:")
                                || message.starts_with("Found ") && message.contains(" possible addresses for the symbol")
                        }) =>
                    {
                        frame(&mut output, "REJECTED", error.as_bytes())
                    }
                    Err(error) if error.starts_with("KeywordException: ") || error.starts_with("<monitor>:1: KeywordException: ") => {
                        let message = error.strip_prefix("<monitor>:1: ").unwrap_or(&error);
                        frame(&mut output, "REJECTED", message.as_bytes())
                    }
                    Err(error) if error.starts_with("<monitor>:1: memory assertion did not match") => {
                        frame(&mut output, "FAIL", error.as_bytes())
                    }
                    Err(error) if error.starts_with("<monitor>:1: Terminal tester failed!") => {
                        frame(&mut output, "FAIL", error.as_bytes())
                    }
                    Err(error) if error.starts_with("<monitor>:1: unsupported ADXL372 target ")
                        || error.starts_with("<monitor>:1: unsupported snapshot platform") => {
                        frame(&mut output, "UNSUPPORTED", error.as_bytes())
                    }
                    Err(error) => {
                        frame(&mut output, "ERROR", error.as_bytes())
                    }
                }
            }
            (Err(error), _) if missing_include(&error) || invalid_command_parameters(&error) => {
                frame(&mut output, "REJECTED", error.as_bytes())
            }
            (Err(error), _) if error.starts_with("Wrong machine number.")
                || (error.starts_with("Machine ") && error.ends_with(" not found.")) => {
                frame(&mut output, "ERROR", error.as_bytes())
            }
            (result, _) => frame(&mut output, "UNSUPPORTED",
                                result.err().as_deref().unwrap_or("not a runtime command").as_bytes()),
        };
        reply.map_err(|error| error.to_string())?;
    }
}

fn describe_snapshot_platform(machine: &Machine, session: &Session) -> Result<String, String> {
    let repl = snapshot_platform(machine)?;
    let name = session.machine.as_deref().unwrap_or("machine-0");
    if name.chars().any(|c| c.is_control() || matches!(c, '"' | '\\')) {
        return Err("unsupported snapshot platform name for monitor bootstrap".into());
    }
    let mut script = format!("mach create \"{name}\"\n");
    match repl {
        Some(SnapshotPlatform::Inline(repl)) => {
            if repl.contains('\n') {
                script.push_str(&format!("machine LoadPlatformDescriptionFromString \"\"\"{repl}\"\"\"\n"));
            } else {
                script.push_str(&format!("machine LoadPlatformDescriptionFromString \"{repl}\"\n"));
            }
        }
        Some(SnapshotPlatform::File(repl)) => {
            let repl = repl.to_str().filter(|path| !path.chars().any(|c| c.is_control() || matches!(c, '"' | '\\')))
                .ok_or("unsupported snapshot platform path")?;
            script.push_str(&format!("machine LoadPlatformDescription \"{repl}\"\n"));
        }
        None => {}
    }
    Ok(script)
}

fn snapshot_platform(machine: &Machine) -> Result<Option<SnapshotPlatform>, String> {
    if machine.is_cortex_m() && machine.elapsed_steps() != 0 {
        return machine.cortex_snapshot_platform().map(|repl| Some(SnapshotPlatform::Inline(repl)));
    }
    if !machine.cpu_less {
        return machine.rv64_snapshot_platform().map(|path| Some(SnapshotPlatform::File(path.to_owned())));
    }
    if machine.empty_monitor {
        return Ok(None);
    }
    let (address, uart) = machine.bus.snapshot_trivial_uart()
        .map_err(|error| format!("unsupported snapshot platform: {error}"))?;
    trivial_uart_snapshot_repl(address, uart).map(|repl| Some(SnapshotPlatform::Inline(repl)))
}

fn trivial_uart_snapshot_repl(address: u32, uart: &str) -> Result<String, String> {
    if uart.is_empty() || !uart.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err("unsupported snapshot platform name for monitor bootstrap".into());
    }
    Ok(format!("{uart}: UART.TrivialUart @ sysbus {address:#x}"))
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

fn uart_byte_payload(matched: crate::uart::BinaryUartMatch) -> String {
    use std::fmt::Write;
    let mut payload = String::new();
    for (index, field) in std::iter::once(Some(matched.content)).chain(matched.groups).enumerate() {
        if index != 0 {
            payload.push('\t');
        }
        for byte in field.unwrap_or_default() {
            write!(payload, "{byte:02x}").expect("writing to String");
        }
    }
    payload
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

fn uart_wait_cycles(machine: &Machine, id: Option<usize>, timeout: Option<u64>) -> Result<u64, String> {
    let timeout = match timeout {
        Some(timeout) => timeout,
        None => machine.bus.uart_tester_timeout(id)?,
    };
    duration_cycles(timeout, CORTEX_M_STEP_HZ, machine.bus.monitor_step_hz())
}

#[derive(Clone)]
struct UartLineSequence {
    tester: Option<usize>,
    patterns: Vec<crate::uart::FailingUartPattern>,
    timeout: Option<u64>,
    unfinished: bool,
    first_only: Option<bool>,
    pause: Option<bool>,
}

impl UartLineSequence {
    fn wait(
        &self, machine: &mut Machine, budget: u64, output: &mut impl Write,
    ) -> Result<(Stop, u64, Option<(String, Option<u128>)>), String> {
        let cycles = uart_wait_cycles(machine, self.tester, self.timeout)?.min(budget);
        let watch = machine.bus.watch_uart_tester_lines(
            self.tester, self.patterns.clone(), self.unfinished, self.first_only,
        )?;
        let pause = self.pause.unwrap_or(machine.bus.default_uart_pause);
        let result = drive_tester_watch(machine, watch, cycles, pause, output);
        let failure = machine.bus.uart_watch_failure(watch).map(str::to_owned);
        let timestamp = if machine.bus.uart_tester_matched_unfinished_line(watch) {
            Some(machine.monitor_elapsed_ns())
        } else {
            machine.bus.uart_tester_matched_timestamp(watch)
        };
        let matched = machine.bus.uart_tester_matched_line(watch).map(|line| (line, timestamp));
        machine.bus.finish_stm32_uart_watch(watch);
        if let Some(line) = failure {
            return Err(format!("Terminal tester failed! Test failing entry has been found on UART: {line}"));
        }
        result.map(|(stop, count)| (stop, count, matched))
    }
}

fn wait_for_uart_bytes(
    machine: &mut Machine,
    id: Option<usize>,
    pattern: &BinaryUartPattern,
    match_start: bool,
    cycles: u64,
    pause: Option<bool>,
    output: &mut impl Write,
) -> Result<(Stop, u64, Option<crate::uart::BinaryUartMatch>), String> {
    let watch = machine.bus.watch_uart_bytes(id, pattern, match_start)?;
    let pause = pause.unwrap_or(machine.bus.default_uart_pause);
    let result = drive_tester_watch(machine, watch, cycles, pause, output);
    let matched = machine.bus.take_uart_byte_match(watch);
    machine.bus.finish_stm32_uart_watch(watch);
    result.map(|(stop, count)| (stop, count, matched))
}

fn wait_for_uart_idle(
    machine: &mut Machine,
    tester: Option<usize>,
    cycles: u64,
    pause: Option<bool>,
    budget: u64,
    output: &mut impl Write,
) -> Result<(Stop, u64), String> {
    let (id, previous) = machine.bus.uart_activity(tester)?;
    let limit = cycles.min(budget);
    if limit != 0 {
        machine.set_monitor_active(true);
    }
    let result = run_until_deadline(machine, limit, output, |bus, _| bus.uart_activity_changed(id, previous))?;
    if limit == cycles && result.0 == Stop::Limit && pause.unwrap_or(machine.bus.default_uart_pause) {
        machine.set_monitor_active(false);
    }
    Ok(result)
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

fn drive_tester_watch(
    machine: &mut Machine,
    watch: UartWatch,
    budget: u64,
    pause: bool,
    output: &mut impl Write,
) -> Result<(Stop, u64), String> {
    if budget != 0 && !machine.bus.stm32_uart_matched(watch) {
        machine.set_monitor_active(true);
    }
    let result = drive_uart_watch(machine, watch, budget, output)?;
    if pause && result.0 == Stop::OutputMatch {
        machine.set_monitor_active(false);
    }
    Ok(result)
}

fn run_uart_watch(
    machine: &mut Machine,
    watch: UartWatch,
    budget: u64,
    pause: Option<bool>,
    output: &mut impl Write,
) -> Result<(Stop, u64, Option<String>), String> {
    let result = if let Some(pause) = pause {
        drive_tester_watch(machine, watch, budget, pause, output)
    } else {
        drive_uart_watch(machine, watch, budget, output)
    };
    let result = result.and_then(|(stop, count)| {
        if let Some(line) = machine.bus.uart_watch_failure(watch) {
            return Err(format!(
                "Terminal tester failed! Test failing entry has been found on UART:\n{line}"
            ));
        }
        Ok((stop, count, machine.bus.uart_tester_matched_line(watch)))
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
    output: &mut ScriptOutput<'_>,
) -> Result<(Stop, u64), String> {
    if let Some(target) = target {
        let watch = machine.bus.watch_stm32_uart(target, pattern, end)?;
        run_uart_watch(machine, watch, budget, None, output).map(|(stop, count, _)| (stop, count))
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
    TrapVector(TrapVector, Option<u64>),
    Integer(u32, Option<u64>),
    ExternalPmp(String, bool, u32, Option<u64>),
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
    pub fdt_bootargs: Option<(String, bool)>,
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

fn monitor_string(token: &str) -> Option<String> {
    if let Some(body) = token.strip_prefix("\"\"\"").and_then(|body| body.strip_suffix("\"\"\"")) {
        return Some(body.to_owned());
    }
    for (quote, escaped) in [('"', r#"\""#), ('\'', r"\'")] {
        if let Some(body) = token.strip_prefix(quote).and_then(|body| body.strip_suffix(quote)) {
            return Some(body.replace(escaped, if quote == '"' { "\"" } else { "'" }));
        }
    }
    None
}

fn unquote(text: &str) -> &str {
    text.strip_prefix('"')
        .and_then(|text| text.strip_suffix('"'))
        .unwrap_or(text)
}

#[derive(Clone, Debug)]
struct Macro {
    script: PathBuf,
    commands: Vec<(usize, String)>,
}

#[derive(Default)]
struct ScriptState {
    variables: HashMap<String, String>,
    macros: HashMap<String, Macro>,
    aliases: HashMap<String, String>,
    visiting: HashSet<PathBuf>,
    boot: Boot,
    machine: bool,
    deferred_variables: HashSet<String>,
}

fn describe_environment(
    variables: &HashMap<String, String>,
    macros: &HashMap<String, Macro>,
    aliases: &HashMap<String, String>,
) -> String {
    use std::fmt::Write;
    let mut output = String::from("Variables:\n");
    let mut variables: Vec<_> = variables.iter().collect();
    variables.sort_unstable_by_key(|(name, _)| *name);
    for (name, token) in variables {
        if !name.contains('.') || name == "global.ORIGIN" {
            continue;
        }
        let (kind, value) = describe_monitor_token(token);
        let _ = writeln!(output, "\t{kind} {name} = {value} ({token})");
    }
    output.push_str("Macros:\n");
    let mut macros: Vec<_> = macros.iter().collect();
    macros.sort_unstable_by_key(|(name, _)| *name);
    for (name, body) in macros {
        let body = body.commands.iter().map(|(_, text)| text.as_str()).collect::<Vec<_>>().join("\n\t");
        let prefix = if name.contains('.') { "" } else { "global." };
        let _ = writeln!(output, "{prefix}{name} \"{body}\"");
    }
    output.push_str("Aliases:");
    let mut aliases: Vec<_> = aliases.iter().collect();
    aliases.sort_unstable_by_key(|(name, _)| *name);
    for (name, command) in aliases {
        let _ = write!(output, "\n\t{name} = {command}");
    }
    output
}

fn describe_monitor_token(token: &str) -> (&'static str, String) {
    if let Some(path) = token.strip_prefix('@') {
        return ("PathToken", path_value(unquote(path)));
    }
    if let Some(value) = monitor_string(token) {
        return ("StringToken", value);
    }
    if token.eq_ignore_ascii_case("true") {
        return ("BooleanToken", "True".into());
    }
    if token.eq_ignore_ascii_case("false") {
        return ("BooleanToken", "False".into());
    }
    if let Some(value) = token.strip_prefix("0x").and_then(|value| u64::from_str_radix(value, 16).ok()) {
        return ("HexToken", value.to_string());
    }
    if let Ok(value) = token.parse::<i64>() {
        return ("DecimalIntegerToken", value.to_string());
    }
    ("LiteralToken", token.to_owned())
}

pub fn load(path: &Path, root: &Path) -> Result<Boot, String> {
    load_script(path, root, false, None)
}

pub fn load_for_session(path: &Path, root: &Path) -> Result<Boot, String> {
    load_script(path, root, true, None)
}

pub enum StartupInput {
    Boot(Box<Boot>),
    Reply(&'static str, String),
}

pub fn load_session_stream(mut input: impl BufRead, root: &Path) -> Result<StartupInput, String> {
    let mut header = String::new();
    input.by_ref().take(64).read_line(&mut header).map_err(|e| e.to_string())?;
    let source = std::env::current_dir().map_err(|e| e.to_string())?.join("<monitor-startup>");
    if let Some(length) = header.strip_prefix("PENV-STARTUP ") {
        let body = read_monitor_text(&mut input, length)?;
        return load_script(&source, root, true, Some(&body)).map(|boot| StartupInput::Boot(Box::new(boot)));
    }
    let chain_length = header.strip_prefix("PENV-INSPECT-CHAIN ");
    let length = chain_length.or_else(|| header.strip_prefix("PENV-INSPECT "))
        .ok_or("expected a monitor startup or inspection frame followed by the byte count")?;
    let inspect_chain = chain_length.is_some();
    let body = read_monitor_text(&mut input, length)?;
    header.clear();
    input.by_ref().take(64).read_line(&mut header).map_err(|e| e.to_string())?;
    let length = header.strip_prefix("PENV-COMMAND ").ok_or("expected PENV-COMMAND")?;
    let command = read_monitor_text(&mut input, length)?;
    let mut state = script_state()?;
    if inspect_chain {
        let mut separators = Vec::new();
        if command_tokens(command.trim(), Some(&mut separators)).is_ok()
            && !separators.is_empty()
            && load_text(&source, source.clone(), root, &mut state, &body).is_ok()
            && !state.machine && state.boot.runtime.iter().all(|entry| is_console_action(&entry.action))
        {
            state.boot.monitor_output.clear();
            state.boot.runtime.retain(|entry| !matches!(entry.action, Action::Echo(_) | Action::LastLog(_)));
            if load_text(&source, source.clone(), root, &mut state, &command).is_ok()
                && !state.machine && !state.boot.has_platform() && !state.boot.started
                && state.boot.runtime.iter().all(|entry| is_console_action(&entry.action))
            {
                let mut output = Vec::new();
                run_console_script(&state.boot, &mut output)?;
                return Ok(StartupInput::Reply("OK", String::from_utf8(output).map_err(|e| e.to_string())?));
            }
        }
        return Ok(StartupInput::Reply("DEFER", String::new()));
    }
    if let Ok(["mach" | "machine", "set", selector]) = command_words(command.trim()).as_deref() {
        if load_text(&source, source.clone(), root, &mut state, &body).is_ok()
            && !state.boot.multiple_machines
            && let Err(error) = validate_machine_selection(
                selector, &state.variables, state.boot.session.machine.as_deref(), state.machine,
            )
        {
            return Ok(StartupInput::Reply("REJECTED", error));
        }
        return Ok(StartupInput::Reply("DEFER", String::new()));
    }
    if let Ok([verb @ ("include" | "i" | "ExecuteScript"), value]) = command_words(command.trim()).as_deref() {
        if load_text(&source, source.clone(), root, &mut state, &body).is_ok() {
            let origin = source.parent().unwrap_or(Path::new("."));
            if let Err(error) = include_path(verb, value, &source, origin, root, &state.variables) {
                let status = if missing_include(&error) { "REJECTED" } else { "ERROR" };
                return Ok(StartupInput::Reply(status, error));
            }
            if !state.machine && state.boot.runtime.is_empty() {
                state.boot.monitor_output.clear();
                if load_text(&source, source.clone(), root, &mut state, &command).is_ok()
                    && !state.machine && !state.boot.has_platform() && state.boot.runtime.is_empty()
                {
                    let mut output = Vec::new();
                    run_console_script(&state.boot, &mut output)?;
                    return Ok(StartupInput::Reply("OK", String::from_utf8(output).map_err(|e| e.to_string())?));
                }
            }
        }
        return Ok(StartupInput::Reply("DEFER", String::new()));
    }
    if let Ok(["machine", "LoadPlatformDescription", value]) = command_words(command.trim()).as_deref() {
        if load_text(&source, source.clone(), root, &mut state, &body).is_ok()
            && state.machine
            && !state.boot.has_platform()
            && !state.boot.started
            && state.boot.runtime.is_empty()
            && let Ok(path) = platform_path(value, &source, root, &state.variables)
            && !path.to_string_lossy().contains("://")
            && platform_file_missing(&path)
        {
            return Ok(StartupInput::Reply("REJECTED", format!("Could not find file '{}'.", path.display())));
        }
        return Ok(StartupInput::Reply("DEFER", String::new()));
    }
    if let Ok([verb @ ("Load" | "Save"), value]) = command_words(command.trim()).as_deref() {
        let mut pending = script_state()?;
        if load_text(&source, source.clone(), root, &mut pending, &body).is_ok()
            && pending.boot.runtime.is_empty()
            && let Ok(path) = snapshot_path(value, source.parent().unwrap_or(Path::new(".")), &pending.variables)
            && let Some(error) = if *verb == "Load" {
                inspect_snapshot_input(&path)
            } else {
                inspect_snapshot_destination(&path)
            }
        {
            return Ok(StartupInput::Reply("REJECTED", error));
        }
    }
    if let Ok(words) = command_words(command.trim())
        && (matches!(words.first(), Some(&("log" | "Save" | "Load")))
            || matches!(words.as_slice(), ["logLevel", _]))
    {
        let uses_variables = words[1..].iter().any(|word| word.starts_with('$') || word.starts_with("@$"));
        if (!uses_variables || load_text(&source, source.clone(), root, &mut state, &body).is_ok())
            && let Err(error) = execute(
                command.trim(), &words, &state.variables, &mut state.machine,
                &mut state.boot, &source, root, 1,
            )
            && invalid_command_parameters(&error)
        {
            return Ok(StartupInput::Reply("REJECTED", error));
        }
        return Ok(StartupInput::Reply("DEFER", String::new()));
    }
    if matches!(command_words(command.trim()).as_deref(), Ok(["runMacro", _])) {
        if load_text(&source, source.clone(), root, &mut state, &body).is_ok()
            && !state.machine && state.boot.runtime.is_empty()
        {
            state.boot.monitor_output.clear();
            if let Err(error) = load_text(&source, source.clone(), root, &mut state, &command) {
                return Ok(StartupInput::Reply("ERROR", error));
            }
            if !state.machine && !state.boot.has_platform() && state.boot.runtime.is_empty() {
                let mut output = Vec::new();
                run_console_script(&state.boot, &mut output)?;
                return Ok(StartupInput::Reply("OK", String::from_utf8(output).map_err(|e| e.to_string())?));
            }
        }
        return Ok(StartupInput::Reply("DEFER", String::new()));
    }
    if matches!(command_words(command.trim()).as_deref(), Ok(["emulation", "IsStarted"])) {
        if load_text(&source, source.clone(), root, &mut state, &body).is_ok()
            && !state.boot.started
            && !state.boot.has_platform()
            && state.boot.runtime.iter().all(|entry| is_console_action(&entry.action))
        {
            return Ok(StartupInput::Reply("OK", emulation_started_reply(state.boot.started).into()));
        }
        return Ok(StartupInput::Reply("DEFER", String::new()));
    }
    // Inspect only read-only commands that resolve in startup state.
    // A queued runtime operation may change that state, so use the live monitor then.
    if !matches!(command_words(command.trim()).as_deref(), Ok(["echo", _] | ["lastLog"] | ["lastLog", _] | ["printenv"]))
        || load_text(&source, source.clone(), root, &mut state, &body).is_err()
        || state.boot.has_platform()
        || state.boot.started
        || !state.boot.runtime.iter().all(|command| matches!(command.action,
            Action::ScriptLog(..) | Action::LogLevel(_)))
    {
        return Ok(StartupInput::Reply("DEFER", String::new()));
    }
    state.boot.monitor_output.clear();
    if let Err(error) = load_text(&source, source.clone(), root, &mut state, &command) {
        let status = if error.ends_with(LAST_LOG_PARAMETERS) { "REJECTED" } else { "ERROR" };
        return Ok(StartupInput::Reply(status, error));
    }
    let mut output = Vec::new();
    run_console_script(&state.boot, &mut output)?;
    if matches!(command_words(command.trim()).as_deref(), Ok(["echo", _])) {
        output.pop(); // The inspection protocol returns the echo value without a trailing newline.
    }
    Ok(StartupInput::Reply("OK", String::from_utf8(output).map_err(|e| e.to_string())?))
}

fn emulation_started_reply(started: bool) -> &'static str {
    if started { "True\n" } else { "False\n" }
}

fn is_console_action(action: &Action) -> bool {
    matches!(action, Action::ReadLogLevels | Action::ScriptLog(..) | Action::LogLevel(_) | Action::LastLog(_) | Action::Echo(_))
}

fn is_log_capture_setup(action: &Action) -> bool {
    matches!(action, Action::MonitorLog(text) if
        text.split_ascii_whitespace().next().is_some_and(|verb|
            matches!(verb, "AttachMonitorLog" | "ClearMonitorLog")))
}

fn is_monitor_snapshot_action(action: &Action) -> bool {
    matches!(action, Action::SaveMemory(..) | Action::LoadMemory(_) | Action::DescribeEnvironment
        | Action::MonitorAssignment(..) | Action::MonitorEcho(..) | Action::MonitorAlias(..) | Action::MonitorMacro(..) | Action::SetMachineCleared(_) | Action::InvokeMonitorMacro(..) | Action::InvokeMonitorScript(..))
}

fn is_empty_machine_action(action: &Action) -> bool {
    is_console_action(action) || is_monitor_snapshot_action(action) || matches!(action,
        Action::BusScalar(_) | Action::MonitorLog(_) | Action::UnhandledAccess(_)
        | Action::TimeSourceInfo | Action::SetQuantum(_) | Action::EmulationIsStarted | Action::SingleStepBlocking(_) | Action::RunFor(_) | Action::RunUntil(_)
        | Action::RunToSyncPoint | Action::SetEmulationPaused(_)
        | Action::ClearMemory | Action::DescribeSnapshotPlatform | Action::InvalidPythonSource(..)
        | Action::RegisterPyDev(..) | Action::SetCrashOnAbort(..))
}

pub fn run_console_script(boot: &Boot, output: &mut impl Write) -> Result<(), String> {
    crate::monitor_log::enable_history();
    if let Some(level) = boot.monitor_log_level {
        crate::monitor_log::set_level(level);
    }
    for message in &boot.monitor_output {
        writeln!(output, "{message}").map_err(|error| error.to_string())?;
    }
    for command in &boot.runtime {
        match &command.action {
            Action::ReadLogLevels => {}
            Action::ScriptLog(level, message) => crate::monitor_log::record(*level, message),
            Action::LogLevel(level) => crate::monitor_log::set_level(*level),
            Action::LastLog(count) => write_last_log(*count, output)?,
            Action::Echo(message) => writeln!(output, "{message}").map_err(|error| error.to_string())?,
            Action::EmulationIsStarted => output.write_all(emulation_started_reply(boot.started).as_bytes())
                .map_err(|error| error.to_string())?,
            _ => return Err("command requires a booted monitor".into()),
        }
    }
    Ok(())
}

fn read_monitor_text(input: &mut impl Read, length: &str) -> Result<String, String> {
    let length = length.strip_suffix('\n').and_then(|length| length.trim_end().parse::<usize>().ok())
        .filter(|length| *length <= MAX_MONITOR_PAYLOAD)
        .ok_or("invalid monitor frame byte count")?;
    let mut body = vec![0; length];
    input.read_exact(&mut body).map_err(|e| format!("reading monitor frame: {e}"))?;
    String::from_utf8(body).map_err(|e| e.to_string())
}

fn script_state() -> Result<ScriptState, String> {
    let mut state = ScriptState::default();
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    state.variables.insert("global.CWD".into(), format!("@\"{}\"", cwd.display()));
    state.variables.insert("global.ORIGIN".into(), format!("@\"{}\"", cwd.display()));
    Ok(state)
}

fn load_script(path: &Path, root: &Path, session: bool, source: Option<&str>) -> Result<Boot, String> {
    let mut state = script_state()?;
    if let Some(source) = source {
        load_text(path, path.to_owned(), root, &mut state, source)?;
    } else {
        load_file(path, root, &mut state)?;
    }
    let mut boot = state.boot;
    boot.session.variables = state.variables;
    boot.session.macros = state.macros;
    boot.session.aliases = state.aliases;
    if !session && !state.machine && !boot.has_platform() && !boot.started
        && (!boot.monitor_output.is_empty() || !boot.runtime.is_empty())
        && boot.runtime.iter().all(|command|
            is_console_action(&command.action) || matches!(command.action, Action::EmulationIsStarted))
    {
        boot.console_only = true;
        return Ok(boot);
    }
    // Keep monitor-only state in the same CPU-less backing store in both entry points.
    if !state.machine && !boot.has_platform() && !boot.started
        && (session || boot.runtime.iter().any(|command|
            is_monitor_snapshot_action(&command.action) || matches!(command.action, Action::MonitorLog(_))))
        && boot.runtime.iter().all(|command|
            is_console_action(&command.action) || is_monitor_snapshot_action(&command.action)
                || matches!(command.action, Action::MonitorLog(_) | Action::EmulationIsStarted))
    {
        boot.empty_machine = true;
        return Ok(boot);
    }
    if state.machine && !boot.has_platform() && (session || !boot.runtime.is_empty())
        && boot.runtime.iter().all(|command| is_empty_machine_action(&command.action))
    {
        boot.empty_machine = true;
        return Ok(boot);
    }
    boot.materialize_platform()?;
    if let [
        RuntimeCommand {
            action: Action::RunFor(steps),
            ..
        },
    ] = boot.runtime.as_slice()
        && (boot.image.is_some() || !boot.binaries.is_empty())
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

fn expand_macro(name: &str, state: &mut ScriptState, root: &Path) -> Result<(), String> {
    let body = state.macros.get(name).ok_or("unknown macro")?.clone();
    let canonical = if body.script.is_absolute() {
        body.script.clone()
    } else {
        std::env::current_dir().map_err(|error| error.to_string())?.join(&body.script)
    };
    // Retain the script directory without mistaking a macro call for an include of its defining file.
    let canonical = canonical.with_file_name(format!("<macro:{}>", name.replace(['/', '\\'], "_")));
    for (index, text) in &body.commands {
        load_text_at(&body.script, canonical.clone(), root, state, text, *index)?;
    }
    Ok(())
}

fn has_snapshot_boundary(boot: &Boot) -> bool {
    boot.runtime.iter().any(|command| matches!(
        command.action, Action::SaveMemory(..) | Action::LoadMemory(_)
    ))
}

fn load_text(path: &Path, canonical: PathBuf, root: &Path, state: &mut ScriptState, content: &str) -> Result<(), String> {
    load_text_at(path, canonical, root, state, content, 0)
}

fn load_text_at(path: &Path, canonical: PathBuf, root: &Path, state: &mut ScriptState, content: &str, offset: usize) -> Result<(), String> {
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
    let mut cursor = content.lines().enumerate().map(|(line, text)| (offset.saturating_add(line), text));
    while let Some((line, raw)) = cursor.next() {
        let mut text = std::borrow::Cow::Borrowed(raw.trim());
        let error = |message: &str| format!("{}:{}: {message}", path.display(), line + 1);
        if text.is_empty() || text.starts_with('#') || text.starts_with(':') {
            continue;
        }
        // A Robot frame can contain a quoted argument spanning physical lines.
        // Read it as one command when the same text is supplied in a RESC file.
        while command_words(&text).as_ref().is_err_and(|error| error == UNTERMINATED_ARGUMENT) {
            let (_, next) = cursor.next().ok_or_else(|| error(UNTERMINATED_ARGUMENT))?;
            let command = text.to_mut();
            command.push('\n');
            command.push_str(next);
        }
        let text = text.as_ref();
        let mut separators = Vec::new();
        if text.contains(';') {
            command_tokens(text, Some(&mut separators)).map_err(|e| error(&e))?;
        }
        if !separators.is_empty() {
            let mut chain_name = canonical.file_name().unwrap_or_default().to_os_string();
            chain_name.push(".<command-chain>");
            let chain_source = canonical.with_file_name(chain_name);
            let mut start = 0;
            for end in separators.into_iter().chain(std::iter::once(text.len())) {
                load_text_at(path, chain_source.clone(), root, state, &text[start..end], line)?;
                start = end + 1;
            }
            continue;
        }
        if let Some(value) = text.strip_prefix("machine LoadPlatformDescriptionFromString ") {
            if !state.machine || state.boot.session.machine_cleared {
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
                let action = if state.boot.multiple_machines {
                    Action::PlatformFragment(source, origin.to_owned(), root.to_owned())
                } else {
                    parse_live_overlay(&source, root).map_err(|message| error(&message))?
                };
                state.boot.runtime.push(RuntimeCommand {
                    line: line + 1, source: path.to_owned(), action,
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
            let (name, commands) = if let Some((name, body)) = declaration.split_once(char::is_whitespace) {
                let body = monitor_string(body.trim()).ok_or_else(|| error("expected quoted macro body"))?;
                (name, vec![(line, body)])
            } else {
                if cursor.next().is_none_or(|(_, line)| line.trim() != "\"\"\"") {
                    return Err(error("expected triple-quoted macro body"));
                }
                let mut commands = Vec::new();
                loop {
                    let (index, command) = cursor.next().ok_or_else(|| error("unterminated macro"))?;
                    if command.trim() == "\"\"\"" {
                        break;
                    }
                    commands.push((index, command.trim().to_owned()));
                }
                (declaration, commands)
            };
            let body = Macro { script: path.into(), commands };
            if has_snapshot_boundary(&state.boot) {
                state.boot.runtime.push(RuntimeCommand {
                    line: line + 1, source: path.to_owned(), action: Action::MonitorMacro(name.into(), body),
                });
            } else {
                state.macros.insert(name.into(), body);
            }
            continue;
        }
        let words = command_words(text).map_err(|e| error(&e))?;
        if let Some((name, source)) = python_assignment(text, &state.variables).map_err(|message| error(&message))? {
            state.deferred_variables.insert(name.clone());
            state.boot.runtime.push(RuntimeCommand {
                line: line + 1, source: path.to_owned(), action: Action::PythonAssignment(name, source),
            });
            continue;
        }
        if words.iter().any(|word| word.trim_start_matches('@').strip_prefix('$')
            .is_some_and(|name| state.deferred_variables.contains(name)))
        {
            state.boot.runtime.push(RuntimeCommand {
                line: line + 1, source: path.to_owned(),
                action: Action::InvokeMonitorScript(text.to_owned(), canonical.clone(), root.to_owned()),
            });
            continue;
        }
        // A restore can change both machine selection and variable bindings.
        if (matches!(words.as_slice(),
                ["mach" | "machine", "set", _]
                | ["numbersMode", _]
                | ["SetDefaultUartTimeout" | "CreateTerminalTester" | "TestMonitorUartIdle"
                    | "SetDefaultLedTimeout" | "CreateLedTester" | "AssertLedState"
                    | "WaitForLineOnUart" | "WaitForTesterPrompt" | "WaitForNextLineOnUart"
                    | "WaitForBytesOnUart" | "ShouldNotBeOnUart" | "WriteTesterUart"
                    | "RegisterFailingUartString" | "UnregisterFailingUartString", ..]
                | ["WaitForMonitorLog" | "AssertAbsentMonitorLog" | "RegisterFailingMonitorLog"
                    | "UnregisterFailingMonitorLog" | "log" | "logLevel" | "lastLog", ..]
                | ["emulation", "SetGlobalQuantum" | "SetQuantum" | "RunFor" | "RunUntil", _]
                | ["machine", "UserState" | "GetTimeSourceInfo" | "SetQuantum" | "Reset", ..])
            || words.first().is_some_and(|word| requires_selected_machine(word) && *word != "machine"))
            && has_snapshot_boundary(&state.boot)
        {
            state.boot.runtime.push(RuntimeCommand {
                line: line + 1, source: path.to_owned(),
                action: Action::InvokeMonitorScript(text.to_owned(), canonical.clone(), root.to_owned()),
            });
            continue;
        }
        let assignment = text.starts_with('$') || matches!(words.as_slice(), ["set", _, _]);
        let follows_snapshot = (assignment || matches!(words.as_slice(), ["echo", _] | ["printenv"] | ["alias", _, _]))
            && has_snapshot_boundary(&state.boot);
        if follows_snapshot && assignment {
            state.boot.runtime.push(RuntimeCommand {
                line: line + 1, source: path.to_owned(),
                action: Action::MonitorAssignment(text.to_owned(), origin.to_owned()),
            });
            continue;
        }
        if assign_variable(text, &words, state.boot.session.selected_machine(), &mut state.variables, None)
            .map_err(|e| error(&e))?
        {
            continue;
        }
        match words.as_slice() {
            ["mach" | "machine", "set", selector] if !state.boot.multiple_machines => {
                validate_machine_selection(selector, &state.variables, state.boot.session.machine.as_deref(), state.machine)
                    .map_err(|e| error(&e))?;
                set_script_machine_selection(state, false, path, line + 1);
            }
            ["mach" | "machine", "clear"] => set_script_machine_selection(state, true, path, line + 1),
            ["RestoreMonitorSnapshot", value] => {
                if state.machine || state.boot.has_platform() || state.boot.started || !state.boot.runtime.is_empty() {
                    return Err(error("snapshot bootstrap requires a fresh monitor"));
                }
                let snapshot = snapshot_path(value, origin, &state.variables).map_err(|e| error(&e))?;
                let (selected, repl) = restore_snapshot_platform(&snapshot)
                    .map_err(|e| error(&format!("{}: {e}", snapshot.display())))?;
                state.machine = true;
                state.boot.session.machine = selected;
                match repl {
                    Some(SnapshotPlatform::Inline(repl)) => state.boot.inline_repl.push((origin.to_owned(), repl)),
                    Some(SnapshotPlatform::File(repl)) => state.boot.platform = Some(repl),
                    None => {}
                }
                state.boot.runtime.push(RuntimeCommand {
                    line: line + 1, source: path.to_owned(), action: Action::LoadMemory(SnapshotPath::Fixed(snapshot)),
                });
            }
            ["include" | "i" | "ExecuteScript", value]
                if has_snapshot_boundary(&state.boot)
                    && (value.starts_with('$') || value.starts_with("@$")
                        || unquote(value.trim_start_matches('@')).ends_with(".resc")) =>
            {
                state.boot.runtime.push(RuntimeCommand {
                    line: line + 1, source: path.to_owned(),
                    action: Action::InvokeMonitorScript(text.to_owned(), canonical.clone(), root.to_owned()),
                });
            }
            [verb @ ("include" | "i" | "ExecuteScript"), value] => {
                let included = include_path(verb, value, path, origin, root, &state.variables)
                    .map_err(|e| error(&e))?;
                match included.extension().and_then(|suffix| suffix.to_str()) {
                    Some("repl") if !state.boot.has_platform() => {
                        state.machine = true;
                        state.boot.platform = Some(included);
                    }
                    Some("cs") if ["BusFaultingPeripheral.cs", "UnimplementedRegistersPeripheral.cs", "SubobjectTester.cs"]
                        .iter().any(|file| fs::canonicalize(root.join("tests/unit-tests").join(file))
                            .is_ok_and(|native| fs::canonicalize(&included).is_ok_and(|source| source == native))) => {}
                    Some("repl" | "cs" | "py") => {
                        return Err(error("unsupported command: include needs an available loader"));
                    }
                    _ => load_file(&included, root, state).map_err(|e| error(&e))?,
                }
            }
            ["echo", value] if follows_snapshot => state.boot.runtime.push(RuntimeCommand {
                line: line + 1, source: path.to_owned(),
                action: Action::MonitorEcho((*value).to_owned(), origin.to_owned()),
            }),
            ["printenv"] => {
                if follows_snapshot {
                    state.boot.runtime.push(RuntimeCommand {
                        line: line + 1, source: path.to_owned(), action: Action::DescribeEnvironment,
                    });
                } else {
                    let message = describe_environment(&state.variables, &state.macros, &state.aliases);
                    if state.boot.started || !state.boot.runtime.is_empty() {
                        state.boot.runtime.push(RuntimeCommand {
                            line: line + 1, source: path.to_owned(), action: Action::Echo(message),
                        });
                    } else {
                        state.boot.monitor_output.push(message);
                    }
                }
            }
            ["alias", name, command] => {
                let command = monitor_string(command).unwrap_or_else(|| (*command).to_owned());
                if name.is_empty() || command.trim().is_empty()
                    || !name.chars().all(|c| c.is_alphanumeric() || matches!(c, '_' | '.'))
                {
                    return Err(error("invalid monitor alias"));
                }
                let scope = state.boot.session.selected_machine().map(normalized_monitor_scope);
                let key = if name.contains('.') { (*name).to_owned() }
                    else { format!("{}.{name}", scope.as_deref().unwrap_or("global")) };
                if follows_snapshot {
                    state.boot.runtime.push(RuntimeCommand {
                        line: line + 1, source: path.to_owned(), action: Action::MonitorAlias(key, command),
                    });
                } else {
                    state.aliases.insert(key, command);
                }
            }
            ["alias", ..] => return Err(error("invalid monitor alias parameters")),
            ["runMacro", name] if name.starts_with('$') => {
                if has_snapshot_boundary(&state.boot) {
                    state.boot.runtime.push(RuntimeCommand {
                        line: line + 1, source: path.to_owned(),
                        action: Action::InvokeMonitorMacro(name[1..].into(), root.to_owned()),
                    });
                } else {
                    expand_macro(&name[1..], state, root).map_err(|e| error(&e))?;
                }
            }
            _ => {
                if let Err(failure) = execute(
                    text, &words, &state.variables, &mut state.machine, &mut state.boot,
                    path, root, line + 1,
                ) {
                    let Some(verb) = words.first().filter(|_| {
                        failure.starts_with("unsupported monitor command")
                            || failure == "commands after start are unsupported in a boot script"
                    }) else {
                        return Err(error(&failure));
                    };
                    if has_snapshot_boundary(&state.boot) {
                        state.boot.runtime.push(RuntimeCommand {
                            line: line + 1, source: path.to_owned(),
                            action: Action::InvokeMonitorScript(text.to_owned(), canonical.clone(), root.to_owned()),
                        });
                        continue;
                    }
                    let Some(command) = monitor_alias(&state.aliases, state.boot.session.selected_machine(), verb) else {
                        return Err(error(&failure));
                    };
                    let expanded = format!("{command}{}", &text[verb.len()..]);
                    let alias_source = canonical.with_file_name(format!("<alias:{verb}>"));
                    load_text(path, alias_source, root, state, &expanded).map_err(|e| error(&e))?;
                }
            }
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

const LOG_LEVEL_PARAMETERS: &str = "Bad parameters for command logLevel";
const LAST_LOG_PARAMETERS: &str = "Bad parameters for command lastLog";
const LOG_PARAMETERS: &str = "Bad parameters for command log";
const SNAPSHOT_PARAMETERS: &str = "Parameters did not match the signature: expected @snapshot-path or quoted path";

const NEGATIVE_MEMORY_COMPARISON: &str = "Negative values require signed memory comparisons.";

fn parse_memory_watch(
    group: &str, comparison: &str, width: &str, signed: &str, expected: &str, addresses: &[&str],
) -> Result<crate::memory_watch::MemoryWatch, String> {
    let width = width.parse().map_err(|_| "expected a memory access width in bytes")?;
    let signed = parse_tester_bool(signed)?;
    let expected = if let Some(magnitude) = expected.strip_prefix('-') {
        if !signed {
            return Err(NEGATIVE_MEMORY_COMPARISON.into());
        }
        0u64.wrapping_sub(crate::parse_address64(magnitude)?)
    } else {
        crate::parse_address64(expected.strip_prefix('+').unwrap_or(expected))?
    };
    let addresses = addresses.iter().map(|address| crate::parse_address64(address))
        .collect::<Result<Vec<_>, _>>()?;
    crate::memory_watch::MemoryWatch::new(addresses, width, signed, comparison.parse()?, expected, group.parse()?)
        .map_err(str::to_owned)
}

fn invalid_command_parameters(error: &str) -> bool {
    matches!(error, LOG_PARAMETERS | LOG_LEVEL_PARAMETERS | LAST_LOG_PARAMETERS | SNAPSHOT_PARAMETERS | NEGATIVE_MEMORY_COMPARISON)
}

fn inspect_snapshot_destination(path: &Path) -> Option<String> {
    if path.is_dir() {
        return Some(format!("Cannot save snapshot to '{}': destination is a directory.", path.display()));
    }
    let parent = path.parent().filter(|parent| !parent.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let missing = match fs::metadata(parent) {
        Ok(metadata) => !metadata.is_dir(),
        Err(error) => matches!(error.kind(), io::ErrorKind::NotFound | io::ErrorKind::NotADirectory),
    };
    missing.then(|| format!("Could not find a part of the path '{}'.", path.display()))
}

fn inspect_snapshot_input(path: &Path) -> Option<String> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => metadata,
        Ok(_) => return Some(nonregular_snapshot(path)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Some(missing_snapshot(path)),
        _ => return None,
    };
    // Header inspection never constructs or modifies an emulation.
    let file = fs::File::open(path).ok()?;
    let opened = file.metadata().ok()?;
    if !opened.is_file() || metadata.len() != opened.len() {
        return None;
    }
    let mut input = io::BufReader::new(file);
    read_snapshot_metadata(&mut input)
        .and_then(|_| Machine::validate_boot_snapshot_header(&mut input))
        .err()
        .map(|reason| format!(
            "This snapshot is incompatible or the emulation's state is corrupted: {}: {reason}",
            path.display(),
        ))
}

fn nonregular_snapshot(path: &Path) -> String {
    format!("Parameters did not match the signature: snapshot input is not a regular file: {}", path.display())
}

fn open_snapshot_input(path: &Path) -> Result<io::BufReader<fs::File>, String> {
    let describe_error = |error: io::Error| {
        if error.kind() == io::ErrorKind::NotFound {
            missing_snapshot(path)
        } else {
            format!("{}: {error}", path.display())
        }
    };
    if !fs::metadata(path).map_err(describe_error)?.is_file() {
        return Err(nonregular_snapshot(path));
    }
    let file = fs::File::open(path).map_err(describe_error)?;
    if !file.metadata().map_err(describe_error)?.is_file() {
        return Err(nonregular_snapshot(path));
    }
    Ok(io::BufReader::new(file))
}

fn missing_snapshot(path: &Path) -> String {
    format!("Parameters did not match the signature: snapshot file does not exist: {}", path.display())
}

fn missing_include(message: &str) -> bool {
    message.starts_with("File does not exist: ") || message.starts_with("Could not find file: ")
}

fn include_path(
    verb: &str, value: &str, path: &Path, origin: &Path, root: &Path, variables: &HashMap<String, String>,
) -> Result<PathBuf, String> {
    let file = if let Some(path) = anchored_path(value, origin, variables)? {
        path
    } else {
        let value = resolve(variables, value)?;
        let file = path_value(unquote(value.trim_start_matches('@')));
        let candidate = root.join(&file);
        if candidate.is_file() { candidate } else { local_path(&file, path, root) }
    };
    match fs::metadata(&file) {
        Ok(_) => Ok(file),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let message = if verb == "ExecuteScript" { "Could not find file" } else { "File does not exist" };
            Err(format!("{message}: {}", file.display()))
        }
        Err(error) => Err(format!("{}: {error}", file.display())),
    }
}

fn monitor_origin(variables: &HashMap<String, String>) -> &Path {
    let origin = resolve(variables, "$global.ORIGIN").unwrap_or(".");
    Path::new(unquote(origin.trim_start_matches('@')))
}

enum SnapshotPath {
    Fixed(PathBuf),
    WorkingDirectory(PathBuf),
}

impl SnapshotPath {
    fn resolve(&self, variables: &HashMap<String, String>) -> Result<PathBuf, String> {
        match self {
            Self::Fixed(path) => Ok(path.clone()),
            Self::WorkingDirectory(relative) => Ok(monitor_working_directory(variables)?.join(relative)),
        }
    }
}

fn monitor_working_directory(variables: &HashMap<String, String>) -> Result<&Path, String> {
    let cwd = resolve(variables, "$global.CWD")?;
    Ok(Path::new(unquote(cwd.trim_start_matches('@'))))
}

fn snapshot_path(
    value: &str, script_dir: &Path, variables: &HashMap<String, String>,
) -> Result<PathBuf, String> {
    if let Some(path) = anchored_path(value, monitor_origin(variables), variables)? {
        return Ok(path);
    }
    if let Some(path) = monitor_string(value) {
        return Ok(script_dir.join(path));
    }
    let path = monitor_path_token(resolve(variables, value)?).ok_or(SNAPSHOT_PARAMETERS)?;
    Ok(script_dir.join(path_value(unquote(path))))
}

fn path_anchor(value: &str) -> Option<(&str, &str)> {
    let token = value.strip_prefix('$')?;
    let (anchor, relative) = token.strip_prefix("global.").unwrap_or(token).split_once('/')?;
    matches!(anchor, "ORIGIN" | "CWD").then_some((anchor, relative))
}

fn anchored_path(
    value: &str, origin: &Path, variables: &HashMap<String, String>,
) -> Result<Option<PathBuf>, String> {
    let Some((anchor, relative)) = path_anchor(value) else {
        return Ok(None);
    };
    let base = if anchor == "ORIGIN" {
        origin
    } else {
        monitor_working_directory(variables)?
    };
    Ok(Some(base.join(path_value(relative))))
}

fn python_assignment(text: &str, variables: &HashMap<String, String>) -> Result<Option<(String, String)>, String> {
    let Some((name, value)) = text.strip_prefix('$').and_then(|text| text.split_once('=')) else {
        return Ok(None);
    };
    let Some(command) = value.trim().strip_prefix('`').and_then(|command| command.strip_suffix('`')) else {
        return Ok(None);
    };
    let words = command_words(command)?;
    let ["py" | "python", source] = words.as_slice() else { return Ok(None) };
    let name = name.trim();
    if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || matches!(c, '_' | '.')) {
        return Err("invalid Python command substitution variable".into());
    }
    let source = monitor_string(resolve(variables, source)?).ok_or("expected quoted Python source")?;
    Ok(Some((name.to_owned(), source)))
}

fn assign_variable(
    text: &str,
    words: &[&str],
    machine: Option<&str>,
    variables: &mut HashMap<String, String>,
    origin: Option<&Path>,
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
    let scope = machine.map(normalized_monitor_scope);
    let local = scope.as_ref().and_then(|scope| name.strip_prefix(scope)?.strip_prefix('.'));
    if !conditional || variable_binding(variables, name).is_none() {
        let anchored = anchored_path(value, origin.unwrap_or_else(|| monitor_origin(variables)), variables)?
            .map(|path| format!("@{}", path.display()));
        let value = anchored.as_deref().unwrap_or(value);
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

fn apply_machine_selection(
    cleared: &mut bool, next: bool, machine: Option<&str>, variables: &mut HashMap<String, String>,
) {
    if *cleared == next {
        return;
    }
    *cleared = next;
    if let Some(name) = machine {
        let prefix = format!("{}.", normalized_monitor_scope(name));
        let locals: Vec<_> = variables.iter().filter_map(|(key, value)| {
            key.strip_prefix(&prefix).map(|name| (name.to_owned(), value.clone()))
        }).collect();
        for (name, value) in locals {
            if next {
                variables.remove(&name);
            } else {
                variables.insert(name, value);
            }
        }
    }
}

fn set_script_machine_selection(state: &mut ScriptState, cleared: bool, path: &Path, line: usize) {
    if has_snapshot_boundary(&state.boot) {
        state.boot.runtime.push(RuntimeCommand {
            line, source: path.to_owned(), action: Action::SetMachineCleared(cleared),
        });
    } else {
        apply_machine_selection(&mut state.boot.session.machine_cleared, cleared,
            state.boot.session.machine.as_deref(), &mut state.variables);
    }
}

fn validate_machine_selection(
    selector: &str, variables: &HashMap<String, String>, machine: Option<&str>, exists: bool,
) -> Result<(), String> {
    let selector = resolve(variables, selector)?;
    if let Ok(index) = crate::parse_address64(selector) {
        if exists && index == 0 {
            return Ok(());
        }
        return Err("Wrong machine number. Type mach to show a list of available machines.".into());
    }
    let name = unquote(selector);
    if machine == Some(name) {
        Ok(())
    } else {
        Err(format!("Machine {name} not found."))
    }
}

fn normalized_monitor_scope(name: &str) -> String {
    name.replace(|c: char| !c.is_alphanumeric() && c != '_', "_")
}

fn monitor_alias<'a>(aliases: &'a HashMap<String, String>, machine: Option<&str>, verb: &str) -> Option<&'a String> {
    aliases.get(verb)
        .or_else(|| machine.and_then(|name| aliases.get(&format!("{}.{verb}", normalized_monitor_scope(name)))))
        .or_else(|| aliases.get(&format!("global.{verb}")))
}

fn variable_binding<'a>(variables: &'a HashMap<String, String>, name: &str) -> Option<&'a String> {
    variables.get(name).or_else(|| {
        if name.starts_with("global.") {
            return None;
        }
        variables.get(&format!("global.{name}"))
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

fn parse_live_overlay(source: &str, root: &Path) -> Result<Action, String> {
    let entries = crate::repl_parser::parse_inline(source, root)?;
    if let [entry] = entries.as_slice()
        && entry.model == "Dynamic.SubobjectTester" && entry.body.is_empty()
    {
        return Ok(Action::SubobjectTester(entry.name.clone(), entry.address()?));
    }
    if let [entry] = entries.as_slice()
        && matches!(entry.model.as_str(), "Memory.ArrayMemory" | "Memory.MappedMemory")
        && let Some(registration) = entry.registration.as_deref()
    {
        let [(_, property)] = entry.body.as_slice() else {
            return Err("memory overlay requires a size".into());
        };
        let size = crate::parse_address(property.strip_prefix("size:").ok_or("expected memory size")?.trim())?;
        if let Some(fields) = registration.strip_prefix("sysbus new Bus.BusPointRegistration")
            .and_then(|fields| fields.trim().strip_prefix('{'))
            .and_then(|fields| fields.strip_suffix('}'))
        {
            let mut address = None;
            let mut cpu = None;
            for field in fields.split_terminator(';') {
                let (name, value) = field.trim().split_once(':').ok_or("invalid scoped memory registration")?;
                match name.trim() {
                    "address" if address.is_none() => address = Some(crate::parse_address64(value.trim())?),
                    "cpu" if cpu.is_none() => cpu = Some(value.trim().to_owned()),
                    _ => return Err("unknown or duplicate scoped memory registration field".into()),
                }
            }
            return Ok(Action::RamOverlay(entry.name.clone(), cpu, address.ok_or("missing memory address")?, size));
        }
        let address = registration.strip_prefix("sysbus ").ok_or("memory overlay requires a sysbus address")?;
        return Ok(Action::RamOverlay(entry.name.clone(), None, crate::parse_address64(address.trim())?, size));
    }
    parse_gpio_overlay(source, root).map(Action::GpioOverlay)
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

fn monitor_echo(value: &str, variables: &HashMap<String, String>, origin: &Path) -> Result<String, String> {
    if let Some(path) = anchored_path(value, origin, variables)? {
        return Ok(path.display().to_string());
    }
    let value = resolve(variables, value)?;
    if let Some(path) = value.strip_prefix('@') {
        return Ok(path_value(path));
    }
    Ok(monitor_string(value).unwrap_or_else(|| value.to_owned()))
}

fn resolve_log_command(text: &str, variables: &HashMap<String, String>) -> Result<String, String> {
    if !text.contains('$') {
        return Ok(text.to_owned());
    }
    let words = command_words(text)?;
    let (timeout_index, pattern_index) = match words.as_slice() {
        ["WaitForMonitorLog" | "AssertAbsentMonitorLog", _, _, _, _, _, ..] => (Some(1), 5),
        ["RegisterFailingMonitorLog" | "UnregisterFailingMonitorLog", _, _] => (None, 2),
        _ => return Ok(text.to_owned()),
    };
    let mut command = String::new();
    for (index, word) in words.into_iter().enumerate() {
        if index != 0 {
            command.push(' ');
        }
        if word.starts_with('$') && Some(index) == timeout_index {
            let timeout = unquote(resolve(variables, word)?);
            if timeout.is_empty() || timeout.chars().any(char::is_whitespace) {
                return Err("expected a single log timeout value".into());
            }
            command.push_str(timeout);
        } else if word.starts_with('$') && index == pattern_index {
            use std::fmt::Write;
            let pattern = monitor_string(resolve(variables, word)?).ok_or("expected a log string variable")?;
            if pattern.is_empty() {
                command.push('-');
            }
            for byte in pattern.bytes() {
                write!(command, "{byte:02x}").unwrap();
            }
        } else {
            command.push_str(word);
        }
    }
    Ok(command)
}

fn requires_selected_machine(target: &str) -> bool {
    matches!(target, "machine" | "cpu" | "sysbus") || target.starts_with("sysbus.")
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
    if boot.session.machine_cleared && words.first().is_some_and(|word| requires_selected_machine(word))
    {
        return Err(format!("No such command or device: {}", words[0]));
    }
    match words {
        [] => return Ok(()),
        ["using", "sysbus"] => return Ok(()),
        ["mach" | "machine", "set", selector] if !boot.session.machine_cleared && !boot.multiple_machines => {
            return validate_machine_selection(selector, variables, boot.session.machine.as_deref(), *machine);
        }
        ["AttachMonitorLog", capacity] if !boot.started && boot.runtime.is_empty() => {
            boot.initial_log_capacity = Some(crate::monitor_log::parse_capacity(capacity)?);
            return Ok(());
        }
        ["SetDefaultUartTimeout", value] if !boot.started && boot.runtime.is_empty() => {
            boot.uart_timeout = Some(duration_steps(resolve(variables, value)?)?);
            return Ok(());
        }
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
    if let ["EnsureTypeIsLoaded", identifier] = words {
        let identifier = unquote(resolve(identifier)?);
        return match identifier {
            "Antmicro.Renode.Peripherals.Dynamic.SubobjectTester"
            | "Antmicro.Renode.Peripherals.UnimplementedRegistersPeripheral"
            | "Antmicro.Renode.Peripherals.Mocks.BusFaultingPeripheral" => Ok(()),
            _ => Err(format!("unsupported peripheral type {identifier:?}")),
        };
    }
    if let ["emulation", "SetGlobalSerialExecution", value] | ["machine", "SetSerialExecution", value] = words {
        if parse_tester_bool(resolve(value)?)? {
            // This scheduler executes harts serially.
            return Ok(());
        }
        return Err("unsupported monitor parallel execution".into());
    }
    if let ["machine", "UserState", value] = words
        && !boot.started && boot.runtime.is_empty()
    {
        let value = resolve(value)?;
        boot.monitor_user_state = Some(path_value(unquote(value.strip_prefix('@').unwrap_or(value))));
        return Ok(());
    }
    if let ["echo", value] = words {
        let message = monitor_echo(value, variables, monitor_origin(variables))?;
        if boot.started || !boot.runtime.is_empty() {
            boot.runtime.push(RuntimeCommand { line, source: script.to_owned(), action: Action::Echo(message) });
        } else {
            boot.monitor_output.push(message);
        }
        return Ok(());
    }
    if let ["logLevel", value] = words {
        let level = crate::monitor_log::parse_level(unquote(resolve(value)?)).map_err(|_| LOG_LEVEL_PARAMETERS)?;
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
    if let ["emulation", "SetGlobalQuantum" | "SetQuantum", value] | ["machine", "SetQuantum", value] = words
        && !boot.started && boot.runtime.is_empty()
    {
        let quantum = duration_steps(resolve(value)?).map_err(|error| format!("invalid emulation quantum: {error}"))?;
        if quantum == 0 {
            return Err("emulation quantum must be positive".into());
        }
        boot.quantum = Some(quantum);
        return Ok(());
    }
    if let ["allowPrivates", allowed] = words
        && !boot.started && boot.runtime.is_empty()
    {
        boot.session.allow_privates = parse_tester_bool(resolve(allowed)?)?;
        return Ok(());
    }
    let action = match words {
        ["DescribeMonitorSnapshotPlatform"] => Some(Action::DescribeSnapshotPlatform),
        ["pause" | "p"] => Some(Action::SetEmulationPaused(true)),
        ["start" | "s"]
            if boot.started || !boot.runtime.is_empty() || (*machine && !boot.has_platform()) =>
        {
            boot.started = true;
            Some(Action::SetEmulationPaused(false))
        }
        [cpu, "AssembleBlock", address, source, options @ ..] => {
            let source = resolve(source)?;
            let source = source.strip_prefix("\"\"\"").and_then(|s| s.strip_suffix("\"\"\""))
                .or_else(|| source.strip_prefix('"').and_then(|s| s.strip_suffix('"')))
                .ok_or("expected quoted AssembleBlock source")?;
            let triple = match options {
                [] | ["false"] | ["null"] | ["null", "false"] => "default",
                [triple] | [triple, "false"] if triple.starts_with('"') && triple.ends_with('"') => unquote(triple),
                [triple] if triple.starts_with("triple=") => unquote(&triple["triple=".len()..]),
                _ => return Err("unsupported AssembleBlock triple or alternate dialect".into()),
            };
            let token = resolve(address)?;
            let address = if let Some(expression) = token.strip_prefix('`').and_then(|s| s.strip_suffix('`')) {
                let words = command_words(expression)?;
                let [cpu, "PC"] = words.as_slice() else {
                    return Err("unsupported AssembleBlock address expression".into());
                };
                AssemblyAddress::ProgramCounter(cpu.trim_start_matches("sysbus.").to_owned())
            } else {
                AssemblyAddress::Literal(crate::parse_address64(token)?)
            };
            Some(Action::AssembleBlock(
                cpu.trim_start_matches("sysbus.").to_owned(), address,
                triple.to_owned(), source.to_owned(), boot.session.number_mode,
            ))
        }
        [command, ..] if crate::monitor_log::is_command(command) => {
            Some(Action::MonitorLog(resolve_log_command(text, variables)?))
        }
        [target, "VendorSpecific1", value] if boot.started => Some(Action::PhyRegister(
            (*target).to_owned(), u16::try_from(crate::parse_address(resolve(value)?)?)
                .map_err(|_| "PHY register exceeds 16 bits")?)),
        [target, axis @ ("AccelerationX" | "AccelerationY" | "AccelerationZ"
            | "DefaultAccelerationX" | "DefaultAccelerationY" | "DefaultAccelerationZ"), value]
            if target.trim_start_matches("sysbus.").contains('.') =>
        {
            let value = resolve(value)?.parse::<f64>().map_err(|_| "invalid acceleration")?;
            if !(-200.0..=200.0).contains(&value) {
                return Err("acceleration must be within ±200 g".into());
            }
            let index = match axis.as_bytes().last() { Some(b'X') => 0, Some(b'Y') => 1, _ => 2 };
            Some(Action::I2cAcceleration(resolve(target)?.into(), index, (value * 1_000_000.0).round() as i32, axis.starts_with("Default")))
        }
        ["allowPrivates", allowed] => Some(Action::AllowPrivates(parse_tester_bool(resolve(allowed)?)?)),
        [target, "SampleRate", rate] if target.trim_start_matches("sysbus.").contains('.') => {
            Some(Action::I2cSampleRate(resolve(target)?.into(), crate::parse_address(resolve(rate)?)?))
        }
        [target, axis @ ("AccelerationX" | "AccelerationY" | "AccelerationZ")]
            if target.trim_start_matches("sysbus.").contains('.') =>
        {
            let axis = match *axis { "AccelerationX" => 0, "AccelerationY" => 1, _ => 2 };
            Some(Action::I2cAccelerationRead(resolve(target)?.into(), axis))
        }
        [target, "FeedAccelerationSamplesFromRESD", file, options @ ..] => {
            let path = resolve(file)?.strip_prefix('@').ok_or("expected @RESD-path")?;
            let multi = match options {
                [] | ["type=SingleFrequency"] => false,
                ["type=MultiFrequency"] => true,
                _ => return Err("invalid acceleration RESD options".into()),
            };
            Some(Action::I2cAccelerationFeed(resolve(target)?.into(), PathBuf::from(path_value(path)), multi))
        }
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

        [cpu, "VectorRegisterLength", bits] if boot.has_platform() => Some(Action::VectorRegisterLength(
            cpu.trim_start_matches("sysbus.").to_owned(),
            crate::parse_address64(resolve(bits)?)?,
        )),
        [cpu, "RegisterCSRHandlerFromString", csr, source] => Some(Action::AddCsrHook(
            resolve(cpu)?.trim_start_matches("sysbus.").into(),
            crate::parse_address(resolve(csr)?)?,
            monitor_string(resolve(source)?).ok_or("expected quoted CSR hook source")?,
        )),
        [cpu, "AddPreStackAccessHook", source] => Some(Action::AddStackAccessHook(
            resolve(cpu)?.trim_start_matches("sysbus.").to_owned(),
            monitor_string(resolve(source)?).ok_or("expected quoted stack access hook source")?,
        )),
        ["TraceStackAccess", cpu] if boot.started => Some(Action::TraceStackAccess(
            cpu.trim_start_matches("sysbus.").to_owned(),
        )),
        [target, "CSRValidation", value @ ..] if value.len() <= 1 => {
            let cpu = resolve(target)?.trim_start_matches("sysbus.").to_owned();
            match value {
                [] => Some(Action::ReadCsrValidation(cpu)),
                ["Full"] => Some(Action::RequireFullCsrValidation(cpu)),
                _ => return Err("unsupported monitor CSR validation level".into()),
            }
        }
        ["cpu" | "sysbus.cpu", "PerformanceInMips"] if boot.started => Some(Action::ReadCpuMips(boot.session.number_mode)),
        ["cpu" | "sysbus.cpu", "PerformanceInMips", value] if boot.started || !boot.runtime.is_empty() => {
            let mips = resolve(value)?.parse().map_err(|_| "PerformanceInMips requires a positive integer")?;
            Some(Action::CpuMips(mips))
        }
        ["log", value, level @ ..] if level.len() <= 1 => {
            let value = resolve(value)?;
            let message = monitor_string(value).ok_or(LOG_PARAMETERS)?;
            let level = match level {
                [] => 1,
                [level] => {
                    let token = resolve(level)?;
                    match monitor_string(token) {
                        Some(name) if matches!(name.as_str(), "Noisy" | "Debug" | "Info" | "Warning" | "Error") =>
                            crate::monitor_log::parse_level(&name).map_err(|_| LOG_PARAMETERS)?,
                        None if matches!(token, "-1" | "0" | "1" | "2" | "3") =>
                            crate::monitor_log::parse_level(token).map_err(|_| LOG_PARAMETERS)?,
                        _ => return Err(LOG_PARAMETERS.into()),
                    }
                }
                _ => unreachable!(),
            };
            Some(Action::ScriptLog(level, format!("Script: {message}")))
        }
        ["log", ..] => return Err(LOG_PARAMETERS.into()),
        ["lastLog", count @ ..] if count.len() <= 1 => {
            let count = match count {
                [] => crate::monitor_log::DEFAULT_TAIL,
                [count] => {
                    let token = resolve(count)?;
                    let value = if let Some(hex) = token.strip_prefix("0x") {
                        u64::from_str_radix(hex, 16).map(|value| value as i64)
                    } else {
                        token.parse::<i64>()
                    }.map_err(|_| LAST_LOG_PARAMETERS)?;
                    value as i32
                }
                _ => unreachable!(),
            };
            if count > crate::monitor_log::HISTORY_LIMIT as i32 {
                return Err(format!("The number of entries cannot be larger than {}", crate::monitor_log::HISTORY_LIMIT));
            }
            Some(Action::LastLog(count))
        }
        ["emulation" | "machine", "GetTimeSourceInfo"] => Some(Action::TimeSourceInfo),
        ["emulation", "SingleStepBlocking", value @ ..] if value.len() <= 1 => {
            Some(Action::SingleStepBlocking(value.first()
                .map(|value| resolve(value).and_then(parse_tester_bool)).transpose()?))
        }
        ["emulation", "IsStarted"] => Some(Action::EmulationIsStarted),
        ["TestMonitorUartIdle", timeout, args @ ..] => {
            let mut tester = None;
            let mut pause = None;
            for option in args {
                match option.split_once('=') {
                    Some(("testerId", value)) if tester.is_none() => {
                        tester = Some(crate::parse_address(resolve(value)?)? as usize);
                    }
                    Some(("pauseEmulation", value)) if pause.is_none() => {
                        pause = Some(parse_tester_bool(resolve(value)?)?);
                    }
                    _ => return Err(format!("unsupported or duplicate UART idle option {option:?}")),
                }
            }
            Some(Action::UartIdle(duration_steps(resolve(timeout)?)?, tester, pause))
        }

        [target, "Frequency"] if boot.started => Some(Action::TimerProperty(
            target.trim_start_matches("sysbus.").to_owned(), false)),
        ["sysbus", "IsPeripheralEnabled", target] if boot.started => Some(Action::TimerProperty(
            target.trim_start_matches("sysbus.").to_owned(), true)),
        ["sysbus", "LogPeripheralAccess", "rcc" | "sysbus.rcc"] if boot.started => Some(Action::TraceRcc),
        ["rcc" | "sysbus.rcc", "WriteDoubleWord", offset, value] if boot.started => Some(Action::WriteRcc(
            crate::parse_address(resolve(offset)?)?, crate::parse_address(resolve(value)?)?)),

        ["mach" | "machine", "create" | "add", name @ ..] if *machine && name.len() <= 1 => {
            let name = name.first().map(|name| resolve(name).map(|value| unquote(value).to_owned())).transpose()?;
            boot.multiple_machines = true;
            Some(Action::MonitorCreateMachine(name))
        }
        ["mach" | "machine", "set", name] if boot.multiple_machines => {
            Some(Action::MonitorSelectMachine(unquote(resolve(name)?).to_owned()))
        }
        ["logLevel"] => Some(Action::ReadLogLevels),
        ["machine", "LoadPlatformDescription", value] if boot.started && boot.multiple_machines => {
            let path = platform_path(value, script, root, variables)?;
            let source = std::fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
            let origin = path.parent().unwrap_or(Path::new("."));
            Some(Action::PlatformFragment(source, origin.to_owned(), root.to_owned()))
        }
        ["machine", "LoadPlatformDescriptionFromString", value] if boot.started => {
            let source = monitor_string(resolve(value)?)
                .ok_or("expected quoted inline REPL")?;
            Some(if boot.multiple_machines {
                Action::PlatformFragment(source, monitor_origin(variables).to_owned(), root.to_owned())
            } else {
                parse_live_overlay(&source, root)?
            })
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
        [sensor, method @ ("FeedIlluminanceSamplesFromRESD" | "FeedMagneticSamplesFromRESD"
            | "FeedPressureSamplesFromRESD" | "FeedTemperatureSamplesFromRESD"), file]
            if boot.started && sensor.starts_with("sysbus.") =>
        {
            let path = snapshot_path(file, monitor_origin(variables), variables)?;
            let sensor = (*sensor).to_owned();
            Some(match *method {
                "FeedIlluminanceSamplesFromRESD" => Action::IlluminanceFeed(sensor, path),
                "FeedMagneticSamplesFromRESD" => Action::MagneticFeed(sensor, path),
                _ => Action::PressureFeed(sensor, path, *method == "FeedTemperatureSamplesFromRESD"),
            })
        }
        [sensor, method @ ("FeedShuntVoltageSamplesFromRESD" | "FeedBusVoltageSamplesFromRESD"), file, channel @ ..]
            if boot.started && sensor.starts_with("sysbus.") && channel.len() <= 1 =>
        {
            let path = snapshot_path(file, monitor_origin(variables), variables)?;
            let channel = channel.first().map(|channel| {
                u16::try_from(crate::parse_address(resolve(channel)?)?)
                    .map_err(|_| "RESD channel exceeds 16 bits".to_owned())
            }).transpose()?.unwrap_or(0);
            Some(Action::VoltageFeed((*sensor).to_owned(), path, usize::from(*method == "FeedBusVoltageSamplesFromRESD"), channel))
        }

        [verb @ ("Save" | "SaveRestorableMonitorSnapshot" | "Load"), path] => {
            let directory = script.parent().unwrap_or_else(|| Path::new("."));
            let path = if let Some(("CWD", relative)) = path_anchor(path) {
                SnapshotPath::WorkingDirectory(PathBuf::from(path_value(relative)))
            } else {
                SnapshotPath::Fixed(snapshot_path(path, directory, variables)?)
            };
            Some(if *verb == "Load" {
                Action::LoadMemory(path)
            } else {
                Action::SaveMemory(path, *verb == "SaveRestorableMonitorSnapshot")
            })
        }
        ["Save" | "Load", ..] => return Err(SNAPSHOT_PARAMETERS.into()),
        ["machine", "UserState", value @ ..] if value.len() <= 1 => {
            let value = value.first().map(|value| {
                let value = resolve(value)?;
                Ok::<_, String>(path_value(unquote(value.strip_prefix('@').unwrap_or(value))))
            }).transpose()?;
            Some(Action::UserState(value))
        }
        ["Clear"] => Some(Action::ClearMemory),
        ["peripherals", "export", value] => Some(Action::ExportPeripherals(
            snapshot_path(value, monitor_origin(variables), variables)?, root.to_owned(),
        )),
        ["peripherals"] => Some(Action::PeripheralList),
        ["py" | "python", source] => {
            Some(match monitor_string(resolve(source)?) {
                Some(source) => Action::ExecutePython(source),
                None => Action::InvalidPythonSource(text.to_owned()),
            })
        }
        ["machine", "PyDevFromString", source, address, size, initable] => Some(Action::RegisterPyDev(
            monitor_string(resolve(source)?).ok_or("expected quoted PyDev source")?,
            crate::parse_address(resolve(address)?)?,
            crate::parse_address(resolve(size)?)?,
            parse_tester_bool(resolve(initable)?)?,
        )),
        ["SetCrashOnAbort", value] => Some(Action::SetCrashOnAbort(parse_tester_bool(resolve(value)?)?)),
        [cpu, "InstallCustomInstructionHandlerFromString", pattern, source] => {
            let pattern = monitor_string(resolve(pattern)?).ok_or("expected quoted opcode pattern")?;
            if pattern.len() != 32 {
                return Err("expected a 32-bit custom instruction pattern".into());
            }
            let mut mask = 0;
            let mut opcode = 0;
            for bit in pattern.bytes() {
                mask <<= 1;
                opcode <<= 1;
                match bit {
                    b'0' => mask |= 1,
                    b'1' => {
                        mask |= 1;
                        opcode |= 1;
                    }
                    b'x' | b'X' | b'?' => {}
                    _ => return Err("invalid custom instruction pattern character".into()),
                }
            }
            let source = monitor_string(resolve(source)?).ok_or("expected quoted Python source")?;
            Some(Action::InstallInstructionHook(resolve(cpu)?.trim_start_matches("sysbus.").into(), mask, opcode, source))
        }
        [target, method @ ("AddHook" | "RemoveHooksAt"), ..]
            if text.contains("`sysbus GetSymbolAddress ")
                && (*method == "RemoveHooksAt" || text.rsplit_once('`').is_some_and(|(_, body)| body.trim_start().starts_with('"'))) =>
        {
            let cpu = resolve(target)?.trim_start_matches("sysbus.").to_owned();
            let args = text[target.len()..].trim_start()[method.len()..].trim_start();
            let (expression, code) = args.strip_prefix('`').and_then(|args| args.split_once('`'))
                .ok_or("expected a hook symbol address")?;
            let words = command_words(expression)?;
            let ["sysbus", "GetSymbolAddress", symbol] = words.as_slice() else {
                return Err("expected a hook symbol address".into());
            };
            let symbol = monitor_string(resolve(symbol)?).ok_or("expected a quoted hook symbol")?;
            let source = if *method == "AddHook" {
                Some(monitor_string(resolve(code.trim())?).ok_or("expected quoted Python source")?)
            } else {
                if !code.trim().is_empty() {
                    return Err("unexpected RemoveHooksAt argument".into());
                }
                None
            };
            Some(Action::PythonHook(cpu, symbol, source))
        }
        [target, "DisassembleBlock", address, length] => {
            let length = crate::parse_address(resolve(length)?)? as usize;
            if !(1..=65536).contains(&length) {
                return Err("DisassembleBlock requires a length of 1 to 65536 bytes".into());
            }
            Some(Action::DisassembleBlock(
                resolve(target)?.trim_start_matches("sysbus.").into(),
                crate::parse_address64(resolve(address)?)?, length,
            ))
        }
        ["sysbus", "WhatIsAt", address, context @ ..] if context.len() <= 1 => {
            Some(Action::BusWhatIsAt(
                crate::parse_address64(resolve(address)?)?,
                context.first().map(|name| resolve(name).map(|name| unquote(name).to_owned())).transpose()?,
            ))
        }
        ["sysbus", "LoadELF", value, options @ ..]
            if boot.started || boot.has_platform() && !boot.runtime.is_empty()
                && boot.runtime.iter().any(|entry| matches!(entry.action,
                    Action::ExecutionMode(..) | Action::ArmSystemRegister(..) | Action::CpuStep(..))) =>
        {
            let file = monitor_path_token(resolve(value)?).ok_or("expected @firmware-path")?;
            Some(Action::LoadMonitorElf(Asset::new(file, script), load_elf_address_mode(options)?, root.to_owned()))
        }
        ["sysbus", "LoadHEX", value, context] => {
            let file = monitor_path_token(resolve(value)?).ok_or("expected @firmware-path")?;
            let context = resolve(context)?;
            let context = context.strip_prefix("context=").unwrap_or(context);
            Some(Action::LoadScopedHex(Asset::new(file, script), unquote(context).into(), root.to_owned()))
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
            let (rest, context) = match rest.split_last() {
                Some((last, before)) if last.starts_with("context=") => {
                    (before, Some(unquote(resolve(&last["context=".len()..])?).to_owned()))
                }
                Some((last, before)) if rest.len() == 1 + usize::from(method.starts_with("Write")) => {
                    (before, Some(unquote(resolve(last)?).to_owned()))
                }
                _ => (rest, None),
            };
            let value = match (method.starts_with("Write"), rest) {
                (true, [value]) => Some(crate::memory_setup::write_value(resolve(value)?, width)?),
                (false, []) => None,
                _ => return Err(format!("invalid sysbus {method} arguments")),
            };
            let access = BusScalar { address, width, value, number_mode: boot.session.number_mode };
            Some(match context {
                Some(context) => Action::ContextBusScalar(access, context),
                None => Action::BusScalar(access),
            })
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
            let mut requested_machine = None;
            let mut binary = None;
            let mut timeout = None;
            let mut echo = None;
            let mut carriage_return = None;
            let mut next_line = None;
            let mut pause = None;
            for option in options {
                match option.split_once('=') {
                    Some(("timeout", value)) if timeout.is_none() => {
                        timeout = Some(duration_steps(resolve(value)?)?);
                    }
                    Some(("machine", value)) if requested_machine.is_none() => {
                        let name = resolve(value)?;
                        requested_machine = Some(monitor_string(name).unwrap_or_else(|| name.to_owned()));
                    }
                    Some(("endLineOption", value)) if carriage_return.is_none() => {
                        carriage_return = Some(match resolve(value)? {
                            "TreatLineFeedAsEndLine" => false,
                            "TreatCarriageReturnAsEndLine" => true,
                            _ => return Err("unsupported UART endLineOption".into()),
                        });
                    }
                    Some(("defaultMatchNextLine", value)) if next_line.is_none() => {
                        next_line = Some(parse_tester_bool(resolve(value)?)?);
                    }
                    Some(("defaultWaitForEcho", value)) if echo.is_none() => {
                        echo = Some(parse_tester_bool(resolve(value)?)?);
                    }
                    Some(("binaryMode", value)) if binary.is_none() => {
                        binary = Some(parse_tester_bool(resolve(value)?)?);
                    }
                    Some(("defaultPauseEmulation", value)) if pause.is_none() => {
                        pause = Some(parse_tester_bool(resolve(value)?)?);
                    }
                    _ => {
                        return Err(format!(
                            "unsupported or duplicate UART tester option {option:?}"
                        ));
                    }
                }
            }
            Some(Action::CreateUartTester(
                resolve(target)?.to_owned(),
                requested_machine,
                binary.unwrap_or(false),
                timeout,
                echo,
                carriage_return.unwrap_or(false),
                next_line.unwrap_or(false),
                pause.unwrap_or(false),
            ))
        }
        ["SetDefaultUartTimeout", value] => Some(Action::DefaultUartTimeout(duration_steps(resolve(value)?)?)),
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
            let (pattern, arguments) = uart_text_argument(args, variables)?;
            if pattern.is_empty() {
                return Err("UART byte wait pattern must be nonempty".into());
            }
            let options = tester_wait_options(arguments, resolve, TesterOptionsKind::Bytes)?;
            let pattern = if options.regex {
                BinaryUartPattern::Regex(
                    regex::bytes::RegexBuilder::new(&pattern)
                        .unicode(false)
                        .dot_matches_new_line(true)
                        .build()
                        .map_err(|e| format!("invalid UART byte regex: {e}"))?,
                )
            } else {
                let bytes = hex_pattern(&pattern)?;
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
                options.pause,
            ))
        }
        ["WaitForLinesOnUart", count, ..] => {
            let prefix = text["WaitForLinesOnUart".len()..].trim_start();
            let mut arguments = prefix[count.len()..].trim_start();
            let count = count.parse::<usize>().ok().filter(|&count| count <= 1024)
                .ok_or("invalid UART line sequence count")?;
            let mut patterns = Vec::with_capacity(count);
            for _ in 0..count {
                let (pattern, remaining) = uart_text_argument(arguments, variables)?;
                patterns.push(pattern);
                arguments = remaining;
            }
            let options = tester_wait_options(arguments, resolve, TesterOptionsKind::Line)?;
            let patterns = patterns.into_iter().map(|pattern| {
                let regex = options.regex.then(|| regex::Regex::new(&pattern)).transpose()
                    .map_err(|error| format!("invalid UART text regex: {error}"))?;
                Ok::<_, String>(crate::uart::FailingUartPattern { pattern, regex })
            }).collect::<Result<_, _>>()?;
            Some(Action::WaitForTesterLines(UartLineSequence {
                tester: options.id, patterns, timeout: options.timeout,
                unfinished: options.unfinished, first_only: options.first_only, pause: options.pause,
            }))
        }
        ["WaitForNextLineOnUart", ..] => {
            let args = text["WaitForNextLineOnUart".len()..].trim_start();
            let options = tester_wait_options(args, resolve, TesterOptionsKind::Line)?;
            if options.regex || options.capture_groups || options.unfinished || options.first_only.is_some() {
                return Err("next UART line accepts only testerId, timeout and pauseEmulation".into());
            }
            Some(Action::WaitForTester(
                options.id, String::new(), None, UartWaitMode::Lf,
                options.timeout, false, false, Some(true), options.pause,
            ))
        }
        [command @ ("WaitForLineOnUart" | "WaitForTesterPrompt" | "ShouldNotBeOnUart"), ..] => {
            let (pattern, arguments) = uart_text_argument(text[command.len()..].trim_start(), variables)?;
            if pattern.is_empty() {
                return Err("UART wait pattern must be nonempty".into());
            }
            let options = tester_wait_options(arguments, resolve, TesterOptionsKind::Line)?;
            Some(Action::WaitForTester(
                options.id,
                pattern.clone(),
                options.regex.then(|| regex::Regex::new(&pattern))
                    .transpose().map_err(|e| format!("invalid UART text regex: {e}"))?,
                if *command == "WaitForTesterPrompt" || options.unfinished { UartWaitMode::Prompt } else { UartWaitMode::Lf },
                options.timeout,
                *command == "ShouldNotBeOnUart",
                options.capture_groups,
                if *command == "ShouldNotBeOnUart" { Some(false) } else { options.first_only },
                options.pause,
            ))
        }
        ["ClearTerminalTesterReport", id @ ..] if id.len() <= 1 => {
            let id = id.first().map(|id| {
                id.strip_prefix("testerId=").ok_or("expected testerId=N")?
                    .parse::<usize>().map_err(|_| "invalid UART tester ID")
            }).transpose()?;
            Some(Action::ClearUartTesterReport(id))
        }
        [
            command @ ("RegisterFailingUartString" | "UnregisterFailingUartString"),
            ..,
        ] => {
            let (pattern, arguments) = uart_text_argument(text[command.len()..].trim_start(), variables)?;
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
        ["WriteTesterUart", ..] => {
            let argument = text["WriteTesterUart".len()..].trim_start();
            let (value, options) = uart_text_argument(argument, variables)?;
            let args: Vec<_> = options.split_whitespace().collect();
            let [ending, echo, selector @ ..] = args.as_slice() else {
                return Err("expected UART ending and echo option".into());
            };
            let ending: &[u8] = match *ending {
                "CR" => b"\r",
                "None" => b"",
                _ => return Err("expected UART ending CR or None".into()),
            };
            let echo = (*echo != "default").then(|| parse_tester_bool(resolve(echo)?)).transpose()?;
            let mut id = None;
            let mut pause = None;
            for option in selector {
                match option.split_once('=') {
                    Some(("testerId", value)) if id.is_none() => {
                        id = Some(crate::parse_address(resolve(value)?)? as usize);
                    }
                    Some(("pauseEmulation", value)) if pause.is_none() => {
                        pause = Some(parse_tester_bool(resolve(value)?)?);
                    }
                    _ => return Err(format!("unsupported or duplicate UART write option {option:?}")),
                }
            }
            Some(Action::WriteTesterUart(id, value, ending, echo, pause))
        }
        [target, "WriteLine", ..] => {
            let args = text[target.len()..].trim_start()["WriteLine".len()..].trim();
            let (line, ending) = uart_text_argument(args, variables)?;
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
        [target, property @ ("FaultOffset" | "FaultOnWrites"), value] => {
            Some(Action::BusFaultingProperty(resolve(target)?.into(), (*property).into(), resolve(value)?.into()))
        }
        [target, "WriteChar", value] => {
            let value = u8::try_from(crate::parse_address(resolve(value)?)?)
                .map_err(|_| "expected a byte for WriteChar")?;
            Some(Action::UsartWriteChar(resolve(target)?.into(), value))
        }
        ["addMemorySymbol", name, address, size] => {
            let name = unquote(resolve(name)?);
            let address = crate::parse_address64(resolve(address)?)?;
            let size = crate::parse_address64(resolve(size)?)?;
            if name.is_empty() || (size != 0 && address.checked_add(size - 1).is_none()) {
                return Err("invalid memory symbol range".into());
            }
            Some(Action::AddMemorySymbol(name.to_owned(), address, size))
        }
        ["resolveMemorySymbols", name, mode @ ("single" | "all")] => {
            Some(Action::ResolveMemorySymbols(unquote(resolve(name)?).to_owned(), *mode == "single"))
        }
        ["checkMemory", group, comparison, width, signed, expected, addresses @ ..] => {
            Some(Action::CheckMemory(parse_memory_watch(group, comparison, width, signed, resolve(expected)?, addresses)?))
        }
        ["waitForMemory", group, comparison, width, signed, expected, timeout, pause, addresses @ ..] => {
            let watch = parse_memory_watch(group, comparison, width, signed, resolve(expected)?, addresses)?;
            Some(Action::WaitForMemory(watch, duration_steps(resolve(timeout)?)?, parse_tester_bool(pause)?))
        }
        ["SetDefaultLedTimeout", timeout] => {
            Some(Action::DefaultLedTimeout(duration_steps(resolve(timeout)?)?))
        }
        ["CreateLedTester", target, options @ ..] => {
            let timeout = match options {
                [] => None,
                [option] => Some(duration_steps(resolve(option.strip_prefix("timeout=")
                    .ok_or("expected LED tester timeout")?)?)?),
                _ => return Err("invalid LED tester options".into()),
            };
            Some(Action::CreateLedTester(resolve(target)?.to_owned(), timeout))
        }
        ["AssertLedState", state, options @ ..] => {
            let state = parse_tester_bool(resolve(state)?)?;
            let mut id = None;
            let mut timeout = None;
            let mut hold = None;
            let mut pause = None;
            for option in options {
                let (key, value) = option.split_once('=').ok_or("expected named LED assertion option")?;
                let value = resolve(value)?;
                match key {
                    "testerId" if id.is_none() =>
                        id = Some(value.parse().map_err(|_| "invalid LED tester ID")?),
                    "timeout" if timeout.is_none() => timeout = Some(duration_steps(value)?),
                    "hold" if hold.is_none() => hold = Some(duration_steps(value)?),
                    "pauseEmulation" if pause.is_none() => pause = Some(parse_tester_bool(value)?),
                    _ => return Err(format!("unsupported or duplicate LED assertion option {key:?}")),
                }
            }
            Some(Action::AssertLedState(id, state, timeout, hold.unwrap_or(0), pause.unwrap_or(false)))
        }
        ["waitForLed", target, state, timeout, options @ ..] => {
            let state = resolve(state)?.parse().map_err(|_| "expected LED state true or false")?;
            let timeout = duration_steps(resolve(timeout)?)?;
            let mut options = options;
            let hold = if let Some((duration, remaining)) = options.split_first()
                && !duration.contains('=')
            {
                options = remaining;
                duration_steps(resolve(duration)?)?
            } else {
                0
            };
            let pause = match options {
                [] => false,
                [option] => parse_tester_bool(option.strip_prefix("pauseEmulation=")
                    .ok_or("expected LED pauseEmulation option")?)?,
                _ => return Err("invalid LED wait options".into()),
            };
            Some(Action::WaitForLed(resolve(target)?.to_owned(), state, timeout, hold, pause))
        }
        [target, "Voltage", value] => {
            let voltage = resolve(value)?.parse::<f64>().ok().filter(|voltage| voltage.is_finite() && *voltage >= 0.0)
                .ok_or("expected a nonnegative supply voltage")?;
            Some(Action::Stm32SupplyVoltage(resolve(target)?.into(), voltage))
        }
        [target, property @ ("Temperature" | "Humidity" | "Pressure" | "SerialNumber"
            | "UncompensatedPressure" | "AngularRateX" | "AngularRateY" | "AngularRateZ"
            | "MagneticFieldX" | "MagneticFieldY" | "MagneticFieldZ"), value]
            if target.trim_start_matches("sysbus.").contains('.') =>
        {
            Some(Action::I2cSensorProperty(resolve(target)?.into(), (*property).into(), resolve(value)?.into()))
        }
        [target, "Pressure"] if target.starts_with("sysbus.") => {
            Some(Action::PressureValue(resolve(target)?.into()))
        }
        [target, "currentAddress", value] if target.starts_with("sysbus.") => {
            let address = u8::try_from(crate::parse_address(resolve(value)?)?)
                .map_err(|_| "I²C subaddress exceeds 8 bits")?;
            Some(Action::PressurePointer(resolve(target)?.into(), address))
        }
        [target, "autoIncrement", value] if target.starts_with("sysbus.") => {
            Some(Action::PressureIncrement(resolve(target)?.into(), parse_tester_bool(resolve(value)?)?))
        }
        [target, "FinishTransmission"] if target.starts_with("sysbus.") => {
            Some(Action::FinishI2cSensorTransfer(resolve(target)?.into()))
        }
        [cpu, "EnableZephyrMode", excluded @ ..] => {
            let cpu = resolve(cpu)?.trim_start_matches("sysbus.");
            let excluded = excluded.iter().map(|symbol| {
                monitor_string(resolve(symbol)?).ok_or_else(|| "expected a quoted ELF symbol name".to_owned())
            }).collect::<Result<_, _>>()?;
            Some(Action::EnableZephyrMode(cpu.into(), excluded))
        }
        ["sysbus", "GetSymbolAddress", symbol, index @ ..] if index.len() <= 1 => {
            let index = index.first().map(|value| resolve(value)?.parse()
                .map_err(|_| "invalid symbol index".to_owned())).transpose()?;
            Some(Action::SymbolAddress(unquote(resolve(symbol)?).to_owned(), index))
        }
        [target, "VectorTableOffset", expression] if expression.starts_with('`') && expression.ends_with('`') => {
            let expression = &expression[1..expression.len() - 1];
            let words = command_words(expression)?;
            let ["sysbus", "GetSymbolAddress", symbol] = words.as_slice() else {
                return Err("expected a vector table symbol expression".into());
            };
            Some(Action::CpuSymbolVectorTable(
                resolve(target)?.trim_start_matches("sysbus.").into(), unquote(resolve(symbol)?).into(),
            ))
        }
        [target, "VectorTableOffset", value] if boot.started || !boot.runtime.is_empty() => {
            let target = resolve(target)?.trim_start_matches("sysbus.").to_owned();
            Some(Action::CpuVectorTable(target, crate::parse_address(resolve(value)?)?))
        }
        [target, "PC", address] if address.starts_with('`') && address.ends_with('`') => {
            let expression = &address[1..address.len() - 1];
            let words = command_words(expression)?;
            let ["sysbus", "GetSymbolAddress", symbol] = words.as_slice() else {
                return Err("unsupported program counter expression".into());
            };
            Some(Action::CpuSymbolPc(
                resolve(target)?.trim_start_matches("sysbus.").into(), unquote(resolve(symbol)?).into(),
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
                "Report" => UnhandledAccess::Report,
                "ThrowException" => UnhandledAccess::ThrowException,
                "ThrowExceptionIfNotTagged" => UnhandledAccess::ThrowExceptionIfNotTagged,
                other => return Err(format!("unsupported UnhandledAccessBehaviour {other:?}")),
            };
            if *machine && !boot.started && boot.runtime.is_empty() {
                boot.unhandled_access = Some(policy);
                return Ok(());
            }
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
            let (name, options) =
                quoted_argument(remainder.trim()).ok_or("expected quoted tag name")?;
            let mut default = None;
            let mut throws = None;
            let mut silent = None;
            for option in options.split_whitespace() {
                if let Some(value) = option.strip_prefix("silent=") {
                    if silent.replace(parse_tester_bool(resolve(value)?)?).is_some() {
                        return Err("duplicate silent option".into());
                    }
                } else if let Some(value) = option.strip_prefix("throwException=") {
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
            let start = crate::parse_address(resolve(start)?)?;
            let end_or_size = crate::parse_address(resolve(size)?)?;
            let size = if range.contains(',') && end_or_size >= start {
                end_or_size.checked_sub(start).and_then(|span| span.checked_add(1)).ok_or("tag range is too large")?
            } else {
                end_or_size
            };
            Some(Action::Tag(start, size, default.unwrap_or(0), throws.unwrap_or(false), name.into(), silent.unwrap_or(false)))
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

        ["AssertMonitorPC", cpu, value] => Some(Action::AssertCpuValue(
            resolve(cpu)?.trim_start_matches("sysbus.").into(), None,
            crate::parse_address64(resolve(value)?)?,
        )),
        ["AssertMonitorRegister", cpu, register, value] => Some(Action::AssertCpuValue(
            resolve(cpu)?.trim_start_matches("sysbus.").into(), Some(unquote(resolve(register)?).into()),
            crate::parse_address64(resolve(value)?)?,
        )),
        [device, command @ ("ConfigCSRRead" | "AddressCSRRead" | "ConfigCSRWrite" | "AddressCSRWrite"), index, operands @ ..]
            if boot.started && operands.len() == usize::from(command.ends_with("Write")) =>
        {
            Some(Action::ExternalPmpRegister(
                device.trim_start_matches("sysbus.").into(),
                command.starts_with("Config"),
                crate::parse_address(resolve(index)?)?,
                operands.first().map(|value| crate::parse_address64(resolve(value)?)).transpose()?,
            ))
        }
        [target, "ClearTranslationCache"] => Some(Action::ClearTranslationCache(
            resolve(target)?.trim_start_matches("sysbus.").into(),
        )),
        [target, "Step", count @ ..] if count.len() <= 1 => {
            let cpu = resolve(target)?;
            let count = count.first().map(|count| crate::parse_address64(resolve(count)?))
                .transpose()?.unwrap_or(1);
            Some(Action::CpuStep(cpu.trim_start_matches("sysbus.").into(), count))
        }
        [target, "MaximumBlockSize", count] => Some(Action::CpuBlockSize(
            resolve(target)?.trim_start_matches("sysbus.").into(), crate::parse_address(resolve(count)?)?,
        )),
        [target, property @ ("SetHookAtBlockBegin" | "SetHookAtBlockEnd"), source] => {
            let source = monitor_string(resolve(source)?).ok_or("expected quoted block hook")?;
            Some(Action::CpuBlockHook(
                resolve(target)?.trim_start_matches("sysbus.").into(),
                *property == "SetHookAtBlockEnd", source,
            ))
        }
        [target, "WillNextItInstructionExecute", state] => Some(Action::CpuWillExecuteIt(
            resolve(target)?.trim_start_matches("sysbus.").into(), u8::try_from(crate::parse_address(resolve(state)?)?)
                .map_err(|_| "ITSTATE exceeds eight bits")?,
        )),
        [target, "GetPriorityBoost", ignore_primask] => Some(Action::NvicPriorityBoost(
            resolve(target)?.into(), parse_tester_bool(resolve(ignore_primask)?)?,
        )),
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
        [target, "SetRegister", register, value]
            if boot.started || !boot.runtime.is_empty() || boot.image.is_some() || !boot.binaries.is_empty()
                || (boot.has_platform() && crate::parse_address(unquote(resolve(register)?)).is_err()) =>
        {
            Some(Action::CpuRegister(
                resolve(target)?.trim_start_matches("sysbus.").into(),
                unquote(resolve(register)?).into(),
                Some(crate::parse_address64(resolve(value)?)?),
            ))
        }
        [target, "GetRegister", register]
            if boot.started || !boot.runtime.is_empty() || boot.image.is_some() || !boot.binaries.is_empty()
                || (boot.has_platform() && crate::parse_address(unquote(resolve(register)?)).is_err()) =>
        {
            Some(Action::CpuRegister(
                resolve(target)?.trim_start_matches("sysbus.").into(),
                unquote(resolve(register)?).into(),
                None,
            ))
        }
        [target, "GetSystemRegisterValue", register] | [target, "SetSystemRegisterValue", register, _] => {
            let value = words.get(3).map(|value| crate::parse_address64(resolve(value)?)).transpose()?;
            Some(Action::ArmSystemRegister(
                resolve(target)?.trim_start_matches("sysbus.").into(),
                unquote(resolve(register)?).into(), value,
            ))
        }
        [target, property @ ("ExceptionLevel" | "SecurityState")] => {
            Some(Action::ArmExceptionState(resolve(target)?.trim_start_matches("sysbus.").into(), *property == "SecurityState"))
        }
        [target, property @ ("SP" | "CPSR"), value @ ..]
            if (boot.started || boot.has_platform()) && value.len() <= 1 =>
        {
            let value = value.first()
                .map(|value| crate::parse_address64(resolve(value)?)).transpose()?;
            Some(Action::CpuRegister(
                resolve(target)?.trim_start_matches("sysbus.").into(), (*property).into(), value,
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
        [target, register @ ("MSTATUS" | "MEPC" | "MCAUSE" | "MTVAL" | "SEPC"), value @ ..]
            if value.is_empty() || (matches!(*register, "MEPC" | "MTVAL" | "SEPC") && value.len() == 1) =>
        {
            let value = value.first().map(|value| crate::parse_address64(resolve(value)?)).transpose()?;
            Some(Action::CpuRegister(
                resolve(target)?.trim_start_matches("sysbus.").into(), (*register).into(), value,
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
        ["logLevel", level, target] => Some(Action::PeripheralLog(resolve(target)?.to_owned(), crate::monitor_log::parse_level(resolve(level)?)?)),
        [target, "ReadQuadWord", offset] => Some(Action::TimerQuadWord(
            resolve(target)?.into(), crate::parse_address(resolve(offset)?)?, None,
        )),
        [target, "WriteQuadWord", offset, value] => Some(Action::TimerQuadWord(
            resolve(target)?.into(), crate::parse_address(resolve(offset)?)?, Some(crate::parse_address64(resolve(value)?)?),
        )),
        [target, method @ ("ReadDoubleWord" | "ReadByteUsingDoubleWord" | "ReadWordUsingDoubleWord"), offset] =>
            Some(Action::PeripheralRead(resolve(target)?.into(), crate::parse_address(resolve(offset)?)?, peripheral_access_width(method))),
        [target, method @ ("WriteDoubleWord" | "WriteByteUsingDoubleWord" | "WriteWordUsingDoubleWord"), offset, value] =>
            Some(Action::PeripheralWrite(
                resolve(target)?.into(), crate::parse_address(resolve(offset)?)?,
                crate::parse_address(resolve(value)?)?, peripheral_access_width(method),
            )),
        [target, property @ ("APBClockFrequency" | "RootClockFrequency")] if !matches!(*target, "cpu" | "sysbus.cpu") =>
            Some(Action::EgisSysconProperty(resolve(target)?.into(), (*property).into())),
        [target, "FeedSample", sample, channel, repeat @ ..]
            if repeat.len() <= 1 && crate::parse_address(resolve(sample)?).is_ok() =>
        {
            let repeat = repeat.first().map(|value| resolve(value)?.parse::<i32>().map_err(|_| String::from("invalid ADC sample repeat")))
                .transpose()?.unwrap_or(1);
            Some(Action::Stm32AdcSample(
                resolve(target)?.into(), crate::parse_address(resolve(sample)?)?, crate::parse_address(resolve(channel)?)?, repeat,
            ))
        }
        [target, "IRQ", "IsSet"] => Some(Action::WatchdogIrq(resolve(target)?.into())),
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
        ["emulation", "SetGlobalQuantum" | "SetQuantum", duration] | ["machine", "SetQuantum", duration] => {
            let quantum = duration_steps(resolve(duration)?).map_err(|error| format!("invalid emulation quantum: {error}"))?;
            if quantum == 0 {
                return Err("emulation quantum must be positive".into());
            }
            Some(Action::SetQuantum(quantum))
        }
        ["emulation", "RunToNearestSyncPoint"] => Some(Action::RunToSyncPoint),
        ["machine" | "mach", "Reset"] => Some(Action::MachineReset(Vec::new())),
        [target, property @ ("Reset" | "LowPercentage" | "HighPercentage")]
            if target.trim_start_matches("sysbus.").contains('.') =>
        {
            Some(Action::PwmTester(resolve(target)?.into(), (*property).into()))
        }
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
        [device, "Toggle"] if boot.started => Some(Action::ToggleButton(resolve(device)?.to_owned())),
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
        [target, property @ ("IsLockedUp" | "ExecutedInstructions" | "GetItState" | "FPCAR" | "FPCCR" | "SecureState")] => {
            Some(Action::CpuStatus(resolve(target)?.trim_start_matches("sysbus.").into(), (*property).into()))
        }
        [target, "DoLazyFloatingPointStatePreservation"] => Some(Action::CpuPreserveFp(
            resolve(target)?.trim_start_matches("sysbus.").into(),
        )),
        [target, "FpuEnabled", values @ ..] if values.len() <= 1 => Some(Action::CpuFpuEnabled(
            resolve(target)?.trim_start_matches("sysbus.").into(),
            values.first().map(|value| parse_tester_bool(resolve(value)?)).transpose()?,
        )),
        [target, "MPUPrivdefena", values @ ..] if values.len() <= 1 => Some(Action::CpuMpuBackground(
            resolve(target)?.trim_start_matches("sysbus.").into(),
            values.first().map(|value| parse_tester_bool(resolve(value)?)).transpose()?,
        )),
        [target, "TranslateAddress", address, access @ ("Read" | "Write" | "Execute")] => Some(Action::CpuTranslateAddress(
            resolve(target)?.trim_start_matches("sysbus.").into(),
            crate::parse_address(resolve(address)?)?, *access == "Write", *access == "Execute",
        )),
        [target, "SecureState", value] => Some(Action::CpuSecurityState(
            resolve(target)?.trim_start_matches("sysbus.").into(), parse_tester_bool(resolve(value)?)?,
        )),
        [target, register @ ("SAUControl" | "SAURegionNumber" | "SAURegionBaseAddress" | "SAURegionLimitAddress"), values @ ..]
            if values.len() <= 1 =>
        {
            Some(Action::CpuSau(
                resolve(target)?.trim_start_matches("sysbus.").into(), (*register).into(),
                values.first().map(|value| crate::parse_address(resolve(value)?)).transpose()?,
            ))
        }
        [target, "VectorTableOffsetNonSecure", value] => Some(Action::CpuNonsecureVectorTable(
            resolve(target)?.trim_start_matches("sysbus.").into(), crate::parse_address(resolve(value)?)?,
        )),
        [target, "Lockup", "Set", value] => Some(Action::NvicLockup(
            resolve(target)?.into(), parse_tester_bool(resolve(value)?)?,
        )),
        [target, signal @ ("Lockup" | "InSleep" | "InDeepSleep"), "IsSet"] => {
            Some(Action::NvicSignal(resolve(target)?.into(), (*signal).into()))
        }
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
        let empty_machine_command = *machine && !boot.has_platform() && is_empty_machine_action(&action);
        if !empty_machine_command && !is_console_action(&action) && !is_monitor_snapshot_action(&action)
            && !matches!(action, Action::MonitorLog(_) | Action::EmulationIsStarted
                | Action::AllowPrivates(_) | Action::SetCrashOnAbort(_))
            && (!boot.has_platform() || (boot.image.is_none()
                && boot.binaries.is_empty()
                && !matches!(
                    action,
                    Action::BusScalar(_) | Action::ContextBusScalar(..) | Action::CpuVectorTable(..) | Action::CpuStatus(..) | Action::NvicSignal(..)
                        | Action::DisassembleBlock(..) | Action::BusWhatIsAt(..) | Action::ExecutePython(..) | Action::PythonHook(..)
                        | Action::InvalidPythonSource(..) | Action::RegisterPyDev(..)
                        | Action::SetCrashOnAbort(..) | Action::InstallInstructionHook(..)
                        | Action::MonitorCreateMachine(..) | Action::MonitorSelectMachine(..) | Action::PlatformFragment(..) | Action::ReadLogLevels
                        | Action::RamOverlay(..) | Action::PeripheralList | Action::ExportPeripherals(..) | Action::SubobjectTester(..) | Action::LoadScopedHex(..) | Action::LoadMonitorElf(..)
                        | Action::CpuSecurityState(..) | Action::CpuPreserveFp(..) | Action::CpuFpuEnabled(..) | Action::CpuSau(..) | Action::CpuNonsecureVectorTable(..) | Action::NvicLockup(..)
                        | Action::CpuMpuBackground(..) | Action::CpuTranslateAddress(..)
                        | Action::AssembleBlock(..)
                        | Action::AddStackAccessHook(..) | Action::AddCsrHook(..) | Action::VectorRegisterLength(..)
                        | Action::AssertCpuValue(..)
                        | Action::ReadCpuPc(_)
                        | Action::ExecutionMode(..)
                        | Action::CpuPc(..)
                        | Action::CpuStep(..) | Action::ClearTranslationCache(..) | Action::CpuMips(..)
                        | Action::CpuBlockSize(..) | Action::CpuBlockHook(..) | Action::CpuWillExecuteIt(..) | Action::CpuCondition(..) | Action::NvicPriorityBoost(..)
                        | Action::Frequency(..) | Action::NvicPending(..) | Action::MachineReset(..) | Action::CpuReset(..) | Action::ReadCpuMips(..)
                        | Action::RequireFullCsrValidation(..) | Action::ReadCsrValidation(..)
                        | Action::CpuRegister(..) | Action::ArmSystemRegister(..) | Action::ArmExceptionState(..)
                        | Action::SetEmulationPaused(_)
                        | Action::DefaultUartTimeout(_)
                        | Action::PeripheralWrite(..) | Action::PeripheralRead(..) | Action::TimerQuadWord(..) | Action::WatchdogIrq(_) | Action::Stm32AdcSample(..) | Action::EgisSysconProperty(..)
                        | Action::CreateUartTester(..)
                        | Action::DefaultLedTimeout(..) | Action::CreateLedTester(..) | Action::AssertLedState(..)
                        | Action::DefaultUartTester(_)
                        | Action::ClearUartTesterReport(_)
                        | Action::UartIdle(..)
                        | Action::WaitForBytes(..)
                        | Action::WaitForTester(..)
                        | Action::WaitForTesterLines(..)
                        | Action::UsartWriteChar(..) | Action::WaitForMemory(..) | Action::BusFaultingProperty(..)
                        | Action::CheckMemory(..) | Action::AddMemorySymbol(..) | Action::ResolveMemorySymbols(..)
                        | Action::PeripheralLog(..)
                        | Action::I2cAccelerationFeed(..) | Action::I2cAccelerationRead(..) | Action::I2cSampleRate(..) | Action::I2cAcceleration(..)
                        | Action::I2cSensorProperty(..)
                        | Action::PressureValue(..) | Action::PressurePointer(..) | Action::PressureIncrement(..)
                        | Action::FinishI2cSensorTransfer(..) | Action::EnableZephyrMode(..)
                        | Action::RunFor(_)
                        | Action::RunUntil(_)
                        | Action::RunToSyncPoint
                        | Action::TimeSourceInfo | Action::SetQuantum(_) | Action::EmulationIsStarted | Action::SingleStepBlocking(_)
                        | Action::DescribeSnapshotPlatform
                        | Action::SaveMemory(..)
                        | Action::ClearMemory
                        | Action::LoadMemory(_)
                        | Action::UnhandledAccess(_) | Action::Tag(..)
                        | Action::FindBytes(_)
                        | Action::CreateHostPty(..)
                        | Action::RtcMode(_)
                        | Action::RtcDateTime
                        | Action::UserState(_)
                )))
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
    if boot.started || !matches!(words, ["showAnalyzer", _]) && boot.runtime.iter().any(|entry|
        !is_console_action(&entry.action) && !is_log_capture_setup(&entry.action)
            && !matches!(entry.action, Action::AllowPrivates(_) | Action::ExecutePython(_)
                | Action::PythonAssignment(..) | Action::SetCrashOnAbort(_)))
    {
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
            boot.session.machine_cleared = false;
            *machine = true;
        }
        ["machine", "LoadPlatformDescription", value] if *machine && boot.platform.is_none() => {
            boot.platform = Some(platform_path(value, script, root, variables)?);
        }
        ["machine", "SdCardFromFile", args @ ..] if boot.has_platform() => {
            boot.sd_cards
                .push(SdAttachment::parse(args, resolve, script)?);
        }
        ["sysbus", "LoadELF", value, options @ ..]
            if boot.has_platform() && !boot.hex && options.len() <= 1 =>
        {
            let virtual_addresses = load_elf_address_mode(options)?;
            let file = monitor_path_token(resolve(value)?).ok_or("expected @firmware-path")?;
            if let Some(previous) = boot.image.replace(Asset::new(file, script)) {
                boot.prior_elf_images.push((previous, boot.use_virtual_elf_addresses));
            }
            boot.use_virtual_elf_addresses = virtual_addresses;
        }
        ["sysbus", "LoadHEX", value] if boot.has_platform() && boot.image.is_none() => {
            let file = monitor_path_token(resolve(value)?).ok_or("expected @firmware-path")?;
            boot.image = Some(Asset::new(file, script));
            boot.hex = true;
        }
        [cpu, "VectorTableOffset", value] if boot.has_platform() => {
            boot.vector_tables.push((
                cpu.strip_prefix("sysbus.").unwrap_or(cpu).to_string(),
                crate::parse_address(resolve(value)?)?,
            ));
        }
        ["sysbus", "LoadFdt", value, address, args, append @ ..]
            if boot.has_platform() && append.len() <= 1 =>
        {
            let file = monitor_path_token(resolve(value)?).ok_or("expected @firmware-path")?;
            let mut asset = Asset::new(file, script);
            let args = monitor_string(resolve(args)?).ok_or("expected quoted FDT boot arguments")?;
            let append = append.first().map(|value| parse_tester_bool(resolve(value)?)).transpose()?.unwrap_or(true);
            asset.fdt_bootargs = Some((args, append));
            boot.binaries.push((asset, crate::parse_address64(resolve(address)?)?));
        }
        ["sysbus", "LoadBinary" | "LoadFdt", value, address] if boot.has_platform() => {
            let file = monitor_path_token(resolve(value)?).ok_or("expected @firmware-path")?;
            boot.binaries.push((
                Asset::new(file, script),
                crate::parse_address64(resolve(address)?)?,
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
        [cpu, "AddHook", address, callback]
            if boot.has_platform() && address.starts_with("`sysbus GetSymbolAddress ") =>
        {
            let expression = address.strip_prefix('`').and_then(|s| s.strip_suffix('`'))
                .ok_or("expected quoted hook address")?;
            let words = command_words(expression)?;
            let ["sysbus", "GetSymbolAddress", symbol] = words.as_slice() else {
                return Err("expected a hook symbol address".into());
            };
            let symbol = symbol.strip_prefix('"').and_then(|s| s.strip_suffix('"'))
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
            crate::parse_address64(resolve(value)?)?;
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
                .map(|value| crate::parse_address64(resolve(value)?))
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
                .map(|value| crate::parse_address64(resolve(value)?))
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
            let value = crate::parse_address64(resolve(value)?)?;
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
        ["start" | "s"]
            if boot.has_platform() && (boot.image.is_some() || !boot.binaries.is_empty()) =>
        {
            boot.started = true;
            boot.initial_start = true;
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
    timeout: Option<u64>,
    match_start: bool,
    regex: bool,
    capture_groups: bool,
    first_only: Option<bool>,
    unfinished: bool,
    pause: Option<bool>,
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
    let mut first_only = None;
    let mut unfinished = None;
    let mut pause = None;
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
            Some(("includeUnfinishedLine", value))
                if kind == TesterOptionsKind::Line && unfinished.is_none() =>
            {
                unfinished = Some(parse_tester_bool(resolve(value)?)?);
            }
            Some(("matchNextLine", value))
                if kind == TesterOptionsKind::Line && first_only.is_none() =>
            {
                first_only = Some(parse_tester_bool(resolve(value)?)?);
            }
            Some(("captureGroups", value))
                if kind == TesterOptionsKind::Line && capture_groups.is_none() =>
            {
                capture_groups = Some(parse_tester_bool(resolve(value)?)?);
            }
            Some(("pauseEmulation", value)) if kind != TesterOptionsKind::Failure && pause.is_none() => {
                pause = Some(parse_tester_bool(resolve(value)?)?);
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
        timeout,
        match_start: match_start.unwrap_or(false),
        regex: regex.unwrap_or(false),
        capture_groups: capture_groups.unwrap_or(false),
        first_only,
        unfinished: unfinished.unwrap_or(false),
        pause,
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

fn uart_text_argument<'a>(value: &'a str, variables: &HashMap<String, String>) -> Result<(String, &'a str), String> {
    if value.starts_with('$') {
        let (variable, rest) = value.split_once(char::is_whitespace).unwrap_or((value, ""));
        let text = monitor_string(resolve(variables, variable)?).ok_or("expected a UART string variable")?;
        return Ok((text, rest.trim()));
    }
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
    let invalid = || format!("expected a nonnegative time interval at 10 ns resolution, got {value:?}");
    let mut parts = seconds.rsplit(':');
    let last = parts.next().ok_or_else(invalid)?;
    let (whole, fraction) = last.split_once('.').unwrap_or((last, ""));
    let mut whole: u64 = whole.parse().map_err(|_| invalid())?;
    for (part, multiplier) in parts.by_ref().take(2).zip([60, 3600]) {
        let field: u64 = part.parse().map_err(|_| invalid())?;
        whole = field.checked_mul(multiplier).and_then(|field| whole.checked_add(field))
            .ok_or_else(invalid)?;
    }
    if parts.next().is_some() {
        return Err(invalid());
    }
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

const UNTERMINATED_ARGUMENT: &str = "unterminated quoted argument";

fn command_words(text: &str) -> Result<Vec<&str>, String> {
    command_tokens(text, None)
}

fn command_tokens<'a>(text: &'a str, mut separators: Option<&mut Vec<usize>>) -> Result<Vec<&'a str>, String> {
    let mut words = Vec::new();
    let mut start = None;
    let mut quote = None;
    let mut escaped = false;
    let mut triple = false;
    let mut chars = text.char_indices();
    while let Some((index, character)) = chars.next() {
        if std::mem::take(&mut escaped) {
            continue;
        }
        if character == '\\'
            && (quote.is_some()
                || start.is_some_and(|start| {
                    let token = &text[start..index];
                    token.starts_with('@') || path_anchor(token).is_some()
                })
                    && text[index + 1..].starts_with(' '))
        {
            escaped = true;
            continue;
        }
        if quote.is_none() && character == '#' && start.is_none() {
            return Ok(words);
        }
        if quote.is_none() && character == ';'
            && let Some(separators) = separators.as_deref_mut()
        {
            if let Some(start) = start.take() {
                words.push(&text[start..index]);
            }
            separators.push(index);
            continue;
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
            if triple && !text[index..].starts_with("\"\"\"") {
                continue;
            }
            if triple {
                chars.next();
                chars.next();
            }
            quote = None;
            triple = false;
        } else if quote.is_none()
            && (new_word || start.is_some_and(|start| {
                let prefix = &text[start..index];
                prefix == "@" || prefix.ends_with('=')
            }))
            && matches!(character, '"' | '\'' | '`')
        {
            quote = Some(character);
            triple = text[index..].starts_with("\"\"\"");
            if triple {
                chars.next();
                chars.next();
            }
        }
    }
    if quote.is_some() {
        return Err(UNTERMINATED_ARGUMENT.into());
    }
    if let Some(start) = start {
        words.push(&text[start..]);
    }
    Ok(words)
}

impl Asset {
    pub(crate) fn new(value: &str, script: &Path) -> Self {
        Self {
            fdt_bootargs: None,
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

fn platform_file_missing(path: &Path) -> bool {
    match path.metadata() {
        Ok(metadata) => metadata.is_dir(),
        Err(error) => error.kind() == io::ErrorKind::NotFound,
    }
}

fn load_elf_address_mode(options: &[&str]) -> Result<bool, String> {
    match options {
        [] | ["false" | "False"] => Ok(false),
        ["true" | "True"] => Ok(true),
        _ => Err("expected a boolean LoadELF address option".into()),
    }
}

fn monitor_path_token(value: &str) -> Option<&str> {
    value.strip_prefix('@').map(|path| path.trim_start_matches('@'))
}

fn platform_path(
    value: &str, script: &Path, root: &Path, variables: &HashMap<String, String>,
) -> Result<PathBuf, String> {
    if let Some(path) = anchored_path(value, monitor_origin(variables), variables)? {
        return Ok(path);
    }
    let value = resolve(variables, value)?;
    if let Some(path) = monitor_string(value) {
        return Ok(PathBuf::from(path));
    }
    let file = monitor_path_token(value).ok_or("expected a platform path")?;
    Ok(local_path(&path_value(unquote(file)), script, root))
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
    fn platform_and_snapshot_paths_accept_repeated_at_signs() {
        let root = Path::new("/tmp/penv-path-token");
        let variables = HashMap::from([("global.board".into(), "@@@board.repl".into())]);
        assert_eq!(
            platform_path("$board", Path::new("boot.resc"), root, &variables).unwrap(),
            root.join("board.repl"),
        );
        assert_eq!(
            snapshot_path("@@state.save", root, &variables).unwrap(),
            root.join("state.save"),
        );
        assert!(snapshot_path("state.save", root, &variables).is_err());
    }

    #[test]
    fn live_repl_overlay_uses_the_repl_parser_for_gpio_connections() {
        let root = Path::new(".");
        let links = parse_gpio_overlay("gpioPortA: { 6 -> gpioPortD@12 }", root).unwrap();
        assert_eq!(links, [("gpioPortA".into(), 6, "gpioPortD".into(), 12)]);
        assert!(parse_gpio_overlay("gpioPortA: { someProperty: 1 }", root).is_err());
    }

    #[test]
    fn log_command_resolves_utf8_pattern_and_duration_separately() {
        let variables = HashMap::from([
            ("global.pattern".into(), r#""µ original""#.into()),
            ("global.timeout".into(), r#""0.002s""#.into()),
        ]);
        assert_eq!(
            resolve_log_command("WaitForMonitorLog $timeout literal keep any $pattern", &variables).unwrap(),
            "WaitForMonitorLog 0.002s literal keep any c2b5206f726967696e616c",
        );
        assert_eq!(
            resolve_log_command("RegisterFailingMonitorLog regex $pattern", &variables).unwrap(),
            "RegisterFailingMonitorLog regex c2b5206f726967696e616c",
        );
    }

    #[test]
    fn uart_argument_resolves_quoted_variables_without_consuming_options() {
        let variables = HashMap::from([
            ("global.pattern".into(), r#""a b""#.into()),
            ("global.invalid".into(), "123".into()),
        ]);
        assert_eq!(
            uart_text_argument("$pattern timeout=0", &variables).unwrap(),
            ("a b".into(), "timeout=0"),
        );
        assert!(uart_text_argument("$invalid timeout=0", &variables).is_err());
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
        let StartupInput::Boot(boot) = load_session_stream(&mut stream, Path::new(".")).unwrap() else {
            panic!("expected a startup script");
        };
        assert_eq!(boot.session.machine.as_deref(), Some("example board"));
        assert_eq!(resolve(&boot.session.variables, "$board").unwrap(), "@boards/test.repl");
        let mut remaining = String::new();
        stream.read_to_string(&mut remaining).unwrap();
        assert_eq!(remaining, runtime);
    }

    #[test]
    fn monitor_alias_expands_arguments_and_rejects_cycles() {
        let script = r#"alias say echo
alias greet say
greet "hello world"
"#;
        let boot = load_script(Path::new("aliases.resc"), Path::new("."), false, Some(script)).unwrap();
        let mut output = Vec::new();
        run_console_script(&boot, &mut output).unwrap();
        assert_eq!(output, b"hello world\n");

        let cycle = "alias first second\nalias second first\nfirst";
        let result = load_script(Path::new("aliases.resc"), Path::new("."), false, Some(cycle));
        assert!(result.err().unwrap().contains("recursive"));
    }

    #[test]
    fn command_chains_preserve_quoted_separators_and_observe_updates() {
        let script = r#"set marker "one;two";echo $marker;set marker "end";echo $marker"#;
        let boot = load_script(Path::new("chain.resc"), Path::new("."), false, Some(script)).unwrap();
        let mut output = Vec::new();
        run_console_script(&boot, &mut output).unwrap();
        assert_eq!(output, b"one;two\nend\n");
    }

    #[test]
    fn preboot_inspection_uses_monitor_state_without_a_machine() {
        let inspect = |startup: &str, command: &str| {
            let input = format!(
                "PENV-INSPECT {}\n{startup}PENV-COMMAND {}\n{command}",
                startup.len(), command.len()
            );
            let StartupInput::Reply(status, value) =
                load_session_stream(io::Cursor::new(input), Path::new(".")).unwrap()
            else {
                panic!("expected an inspection reply");
            };
            (status, value)
        };
        assert_eq!(inspect("$var=1234\n$var?=5678\n", "echo $var"), ("OK", "1234".into()));
        assert_eq!(inspect("mach create\nstart\n", "echo $var").0, "DEFER");
        let (status, output) = inspect("log \"marker\"\n", "lastLog 0x1");
        assert_eq!(status, "OK");
        assert!(output.contains("[INFO] Script: marker"));
        assert_eq!(inspect("", r#"lastLog "1""#).0, "REJECTED");
        assert_eq!(inspect("", "set var 1234").0, "DEFER");
        let machine = "mach create \"bench board\"\n";
        assert_eq!(inspect("", "emulation IsStarted"), ("OK", "False\n".into()));
        assert_eq!(inspect(machine, "emulation IsStarted"), ("OK", "False\n".into()));
        assert_eq!(inspect("mach create\nstart\n", "emulation IsStarted").0, "DEFER");
        assert_eq!(inspect(machine, "mach set 0").0, "DEFER");
        assert_eq!(inspect(machine, "mach set 9").0, "REJECTED");
        assert_eq!(inspect(machine, "logLevel 4").0, "REJECTED");
        assert_eq!(
            inspect(&format!("{machine}set marker ready\n"), "echo $marker"),
            ("OK", "ready".into()),
        );
        let missing_repl = format!("/tmp/penv-missing-platform-{}.repl", std::process::id());
        let startup = format!("{machine}$board=@{missing_repl}\n");
        assert_eq!(
            inspect(&startup, "machine LoadPlatformDescription $board"),
            ("REJECTED", format!("Could not find file '{missing_repl}'.")),
        );
        assert_eq!(inspect("", "machine LoadPlatformDescription @missing.repl").0, "DEFER");
        assert_eq!(inspect(machine, "machine LoadPlatformDescription @.").0, "REJECTED");
        assert_eq!(inspect(machine, r#"machine LoadPlatformDescription ".""#).0, "REJECTED");
        assert_eq!(platform_path(r#""relative board.repl""#, Path::new("script/boot.resc"),
            Path::new("root"), &HashMap::new()).unwrap(), Path::new("relative board.repl"));
        let missing = format!("@/tmp/penv-missing-snapshot-{}.save", std::process::id());
        assert_eq!(inspect(machine, &format!("Load {missing}")).0, "REJECTED");
        let no_directory = format!("@/tmp/penv-missing-snapshot-directory-{}/state.save", std::process::id());
        assert_eq!(inspect(machine, &format!("Save {no_directory}")).0, "REJECTED");
        assert_eq!(inspect(machine, "Save @.").0, "REJECTED");
        assert_eq!(inspect(machine, "Load @.").0, "REJECTED");
        let pending = format!("{machine}Save {missing}\n");
        assert_eq!(inspect(&pending, &format!("Load {missing}")).0, "DEFER");
        assert_eq!(inspect(machine, "logLevel 2").0, "DEFER");
        assert_eq!(inspect(machine, "mach set \"missing\"").0, "REJECTED");
        assert_eq!(inspect("", "echo $var").0, "ERROR");
        let macro_setup = "macro greet \"echo $label\"\nset label ready\n";
        assert_eq!(inspect(macro_setup, "runMacro $greet"), ("OK", "ready\n".into()));
        assert_eq!(inspect(macro_setup, "runMacro $missing").0, "ERROR");
        let nested = "macro inner \"set label changed\"\nmacro outer \"runMacro $inner\"\nrunMacro $outer\n";
        assert_eq!(inspect(nested, "echo $label"), ("OK", "changed".into()));
        let recursive = "macro repeat \"runMacro $repeat\"\n";
        let (status, message) = inspect(recursive, "runMacro $repeat");
        assert_eq!(status, "ERROR");
        assert!(message.contains("recursive"));
        assert_eq!(inspect("mach create\n", "runMacro $greet").0, "DEFER");
        let (status, value) = inspect("log \"hello world\"\n", "lastLog 1");
        assert_eq!(status, "OK");
        assert!(value.ends_with("[INFO] Script: hello world\n"));

    }

    #[test]
    fn uart_pause_on_buffered_match_and_resume_on_following_wait() {
        let mut machine = Machine::without_default_ram();
        machine.cpu_less = true;
        machine.bus.map_trivial_uart(0x1000, "uart0").unwrap();
        let id = machine.bus.create_uart_tester("uart0", false, None, false).unwrap();
        let mut output = io::sink();
        for byte in b"A\n" {
            machine.bus.write(0x1000, 4, u32::from(*byte), &mut output).unwrap();
        }
        let watch = machine.bus.watch_uart_tester_text(Some(id), "A", None, UartWaitMode::Lf, None).unwrap();
        assert_eq!(run_uart_watch(&mut machine, watch, 0, Some(true), &mut output).unwrap().0, Stop::OutputMatch);
        assert!(machine.primary.monitor_paused);
        assert_eq!(machine.elapsed_steps(), 0);

        let watch = machine.bus.watch_uart_tester_text(Some(id), "B", None, UartWaitMode::Lf, None).unwrap();
        assert_eq!(run_uart_watch(&mut machine, watch, 10, Some(false), &mut output).unwrap().0, Stop::Limit);
        assert!(!machine.primary.monitor_paused);
        assert_eq!(machine.elapsed_steps(), 10);

        machine.bus.default_uart_pause = true;
        assert_eq!(wait_for_uart_idle(&mut machine, Some(id), 10, None, 5, &mut output).unwrap().0, Stop::Limit);
        assert!(!machine.primary.monitor_paused);
        assert_eq!(wait_for_uart_idle(&mut machine, Some(id), 10, None, 10, &mut output).unwrap().0, Stop::Limit);
        assert!(machine.primary.monitor_paused);
        assert_eq!(machine.elapsed_steps(), 25);
    }

    #[test]
    fn virtual_time_commands_use_restored_arguments() {
        let path = std::env::temp_dir().join(format!("penv-restored-time-{}", std::process::id()));
        fs::create_dir(&path).unwrap();
        let directory = GeneratedRepl(path);
        let script = directory.0.join("clock.resc");
        for verb in ["RunFor", "RunUntil"] {
            let content = format!(r#"mach create
set interval "0.001"
Save @clock.save
set interval "0.003"
emulation SetGlobalQuantum $interval
emulation {verb} $interval
emulation GetTimeSourceInfo
Load @clock.save
emulation SetGlobalQuantum $interval
emulation {verb} $interval
emulation GetTimeSourceInfo
"#);
            let mut boot = load_script(&script, &directory.0, false, Some(&content)).unwrap();
            let mut machine = Machine::without_default_ram();
            machine.cpu_less = true;
            machine.empty_monitor = boot.empty_machine;
            let mut output = Vec::new();
            run(&mut machine, &mut boot.session, &boot.runtime, u64::MAX, &mut output).unwrap();
            assert_eq!(output, concat!(
                "Elapsed Virtual Time: 00:00:00.003000000\nQuantum: 00:00:00.003000000\n",
                "Elapsed Virtual Time: 00:00:00.001000000\nQuantum: 00:00:00.001000000\n",
            ).as_bytes());
        }
    }

    #[test]
    fn restorable_snapshot_reconstructs_named_uart_platform() {
        let directory = std::env::temp_dir().join(format!("penv-restorable-snapshot-{}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        let dir = GeneratedRepl(directory);
        let snapshot = dir.0.join("clock state.save");
        let script = dir.0.join("restore.resc");
        fs::write(&script, "RestoreMonitorSnapshot $ORIGIN/clock\\ state.save\n").unwrap();

        let mut source = Machine::without_default_ram();
        source.cpu_less = true;
        source.bus.map_trivial_uart(0x2340, "uart1").unwrap();
        source.bus.monitor_user_state = "saved state".into();
        source.run(9, &mut io::sink()).unwrap();
        let session = Session {
            machine: Some("clock board".into()),
            machine_cleared: true,
            number_mode: NumberMode::Both,
            variables: HashMap::from([("global.marker".into(), "42".into())]),
            ..Session::default()
        };
        session.save(&source, &snapshot).unwrap();
        assert_eq!(inspect_snapshot_input(&snapshot), None);
        let invalid = dir.0.join("invalid.save");
        fs::write(&invalid, b"invalid snapshot").unwrap();
        assert!(inspect_snapshot_input(&invalid).unwrap().contains("unknown snapshot format"));

        let mut boot = load(&script, &dir.0).unwrap();
        assert_eq!(boot.session.machine.as_deref(), Some("clock board"));
        assert_eq!(fs::read_to_string(boot.platform.as_ref().unwrap()).unwrap(),
            "uart1: UART.TrivialUart @ sysbus 0x2340\n");

        let mut restored = Machine::without_default_ram();
        restored.cpu_less = true;
        restored.bus.map_trivial_uart(0x2340, "uart1").unwrap();
        run(&mut restored, &mut boot.session, &boot.runtime, u64::MAX, &mut io::sink()).unwrap();
        assert_eq!(restored.elapsed_steps(), 9);
        assert_eq!(restored.bus.monitor_user_state, "saved state");
        assert!(matches!(boot.session.number_mode, NumberMode::Both));
        assert_eq!(boot.session.selected_machine(), None);
        assert_eq!(boot.session.machine.as_deref(), Some("clock board"));
        assert_eq!(resolve(&boot.session.variables, "$marker").unwrap(), "42");
    }

    #[test]
    fn platform_paths_use_the_nested_script_origin() {
        let directory = std::env::temp_dir().join(format!("penv-nested-platform-{}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        let dir = GeneratedRepl(directory);
        let child = dir.0.join("nested");
        fs::create_dir(&child).unwrap();
        let board = child.join("board.repl");
        fs::write(&board, "uart0: UART.TrivialUart @ sysbus 0x1000\n").unwrap();
        fs::write(child.join("child.resc"), "set board $ORIGIN/board.repl\n").unwrap();
        let script = dir.0.join("boot.resc");
        fs::write(&script, concat!(
            "mach create\n",
            "include $ORIGIN/nested/child.resc\n",
            "machine LoadPlatformDescription $board\n",
        )).unwrap();

        let boot = load_for_session(&script, &dir.0).unwrap();
        assert_eq!(boot.platform, Some(board.canonicalize().unwrap()));
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
    fn explicit_global_lookup_does_not_fall_back_to_machine_variables() {
        let mut variables = HashMap::new();
        assign_variable("set value 41", &["set", "value", "41"], None, &mut variables, None).unwrap();
        assign_variable("set value 42", &["set", "value", "42"], Some("demo"), &mut variables, None).unwrap();
        assign_variable("set local 43", &["set", "local", "43"], Some("demo"), &mut variables, None).unwrap();

        assert_eq!(resolve(&variables, "$value"), Ok("42"));
        assert_eq!(resolve(&variables, "$demo.value"), Ok("42"));
        assert_eq!(resolve(&variables, "$global.value"), Ok("41"));
        assert!(resolve(&variables, "$global.local").is_err());
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
                    run(&mut machine, &mut Session::default(), &boot.runtime, 0, &mut output).unwrap(),
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
                fdt_bootargs: None,
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
        run(&mut machine, &mut Session::default(), &boot.runtime, 0, &mut io::sink()).unwrap();
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
            replies: None,
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
