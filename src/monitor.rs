use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

#[derive(Default)]
pub struct Boot {
    pub binaries: Vec<(String, u32)>,
    pub boot_pcs: Vec<(String, u32)>,
    pub platform: Option<PathBuf>,
    pub image: Option<String>,
    pub mtvec: Option<u32>,
    pub mask: Option<u32>,
    pub named_analyzers: Vec<String>,
    pub uart_irqs: Vec<UartIrq>,
    pub performance_in_mips: Option<u32>,
    pub fault_on_unmapped_data: bool,
    pub pend_hard_fault: bool,
    pub opcode_counters: Vec<(String, String)>,
    pub opcode_counting: Option<bool>,
    pub opcode_queries: Vec<String>,
    pub opcode_hooks: Vec<(crate::thumb::opcode_counter::HookPhase, u32, u32, String)>,
    pub semihosting_arguments: Option<String>,
    pub semihosting_directory: Option<PathBuf>,
    pub semihosting_temporary_directory: Option<String>,
}

pub enum UartIrq {
    Read(String),
    Set(String, bool),
}

pub fn load(path: &Path, root: &Path) -> Result<Boot, String> {
    let content = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut variables = HashMap::new();
    let mut macros = HashMap::new();
    let mut boot = Boot::default();
    let mut machine = false;
    let mut cursor = content.lines().enumerate();
    while let Some((line, raw)) = cursor.next() {
        let text = raw.trim();
        let error = |message: &str| format!("{}:{}: {message}", path.display(), line + 1);
        if text.is_empty() || text.starts_with('#') || text.starts_with(':') {
            continue;
        }
        if let Some(name) = text.strip_prefix("macro ") {
            let name = name.trim();
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
                body.push((index, command));
            }
            macros.insert(name, body);
            continue;
        }
        if let Some(assignment) = text.strip_prefix('$') {
            let (name, value) = assignment
                .split_once('=')
                .ok_or_else(|| error("expected variable assignment"))?;
            let name = name.trim();
            if let Some(name) = name.strip_suffix('?') {
                variables.entry(name.trim()).or_insert(value.trim());
            } else {
                variables.insert(name, value.trim());
            }
            continue;
        }
        if let Some(name) = text.strip_prefix("runMacro $") {
            let body = macros
                .get(name.trim())
                .ok_or_else(|| error("unknown macro"))?;
            for &(index, command) in body {
                execute(
                    command.trim(),
                    &variables,
                    &mut machine,
                    &mut boot,
                    path,
                    root,
                )
                .map_err(|e| format!("{}:{}: {e}", path.display(), index + 1))?;
            }
            continue;
        }
        execute(text, &variables, &mut machine, &mut boot, path, root).map_err(|e| error(&e))?;
    }
    if !machine || boot.platform.is_none() || (boot.image.is_none() && boot.binaries.is_empty()) {
        return Err(format!(
            "{}: expected machine creation, platform and firmware loading",
            path.display()
        ));
    }
    Ok(boot)
}

fn execute(
    text: &str,
    variables: &HashMap<&str, &str>,
    machine: &mut bool,
    boot: &mut Boot,
    script: &Path,
    root: &Path,
) -> Result<(), String> {
    if text.is_empty() || text.starts_with('#') || text == "using sysbus" {
        return Ok(());
    }
    let words: Vec<_> = text.split_whitespace().collect();
    let resolve = |value| match value {
        "$" => Err("missing variable name".to_string()),
        value => match value.strip_prefix('$') {
            Some(name) => variables
                .get(name)
                .copied()
                .ok_or_else(|| format!("unknown variable {name}")),
            None => Ok(value),
        },
    };
    match words.as_slice() {
        ["mach" | "machine", "create", name @ ..] if name.len() <= 1 => {
            if let Some(name) = name.first() {
                resolve(*name)?;
            }
            if std::mem::replace(machine, true) {
                return Err("multiple machines are unsupported".into());
            }
        }
        ["machine", "LoadPlatformDescription", value] if *machine && boot.platform.is_none() => {
            let file = resolve(value)?
                .strip_prefix('@')
                .ok_or("expected @platform-path")?;
            boot.platform = Some(local_path(file, script, root));
        }
        ["nvic" | "sysbus.nvic", "SetPendingIRQ", value]
            if boot.platform.is_some() && crate::parse_address(resolve(value)?)? == 3 =>
        {
            boot.pend_hard_fault = true;
        }
        [
            "cpu" | "sysbus.cpu",
            "InstallOpcodeCounterPattern",
            name,
            pattern,
        ] if boot.platform.is_some() => {
            boot.opcode_counters.push((
                name.trim_matches('"').into(),
                pattern.trim_matches('"').into(),
            ));
        }
        ["cpu" | "sysbus.cpu", "EnableOpcodesCounting", enabled] if boot.platform.is_some() => {
            boot.opcode_counting = Some(enabled.parse().map_err(|_| "expected true or false")?);
        }
        [
            "cpu" | "sysbus.cpu",
            command @ ("AddPreOpcodeExecutionLogHook" | "AddPostOpcodeExecutionLogHook"),
            mask,
            pattern,
            label,
        ] if boot.platform.is_some() => {
            use crate::thumb::opcode_counter::HookPhase;
            let phase = if *command == "AddPreOpcodeExecutionLogHook" {
                HookPhase::Pre
            } else {
                HookPhase::Post
            };
            boot.opcode_hooks.push((
                phase,
                crate::parse_address(resolve(mask)?)?,
                crate::parse_address(resolve(pattern)?)?,
                label.trim_matches('"').into(),
            ));
        }
        ["cpu" | "sysbus.cpu", "GetOpcodeCounter", name] if boot.platform.is_some() => {
            boot.opcode_queries.push(name.trim_matches('"').into());
        }
        ["sysbus", "UnhandledAccessBehaviour", "ThrowException"] if boot.platform.is_some() => {
            boot.fault_on_unmapped_data = true;
        }
        ["sysbus", "LoadELF", value] if boot.platform.is_some() && boot.image.is_none() => {
            let file = resolve(value)?
                .strip_prefix('@')
                .ok_or("expected @firmware-path")?;
            boot.image = Some(file.to_string());
        }
        ["sysbus", "LoadBinary", value, address] if boot.platform.is_some() => {
            let file = resolve(value)?
                .strip_prefix('@')
                .ok_or("expected @firmware-path")?;
            boot.binaries
                .push((file.to_string(), crate::parse_address(resolve(address)?)?));
        }
        [cpu, "PC", value] if boot.platform.is_some() => {
            boot.boot_pcs.push((
                cpu.strip_prefix("sysbus.").unwrap_or(cpu).to_string(),
                crate::parse_address(resolve(value)?)?,
            ));
        }
        ["cpu" | "sysbus.cpu", "MTVEC", value] if boot.platform.is_some() => {
            boot.mtvec = Some(crate::parse_address(resolve(value)?)?);
        }
        ["cpu" | "sysbus.cpu", "SetMachineIrqMask", value] if boot.platform.is_some() => {
            boot.mask = Some(crate::parse_address(resolve(value)?)?);
        }
        [
            "cpu.semihosting" | "sysbus.cpu.semihosting",
            "TemporaryFilesDirectory",
            value,
        ] if boot.platform.is_some() => {
            let name = resolve(value)?.trim_matches('"');
            if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\', '"']) {
                return Err("expected a single temporary directory name".into());
            }
            boot.semihosting_temporary_directory = Some(name.to_owned());
        }
        [
            "cpu.semihosting" | "sysbus.cpu.semihosting",
            "WorkingDirectory",
            value,
        ] if boot.platform.is_some() => {
            let directory = resolve(value)?
                .strip_prefix('@')
                .ok_or("expected @directory")?;
            let directory = script.parent().unwrap_or(Path::new(".")).join(directory);
            let directory = directory
                .canonicalize()
                .map_err(|error| format!("{}: {error}", directory.display()))?;
            if !directory.is_dir() {
                return Err(format!("{} is not a directory", directory.display()));
            }
            boot.semihosting_directory = Some(directory);
        }
        [
            "cpu.semihosting" | "sysbus.cpu.semihosting",
            "ProgramArguments",
            ..,
        ] if boot.platform.is_some() => {
            let (_, value) = text.split_once("ProgramArguments").unwrap();
            let value = value
                .trim()
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .filter(|s| !s.contains(['"', '\\']))
                .ok_or("expected quoted ProgramArguments without escape sequences")?;
            boot.semihosting_arguments = Some(value.to_owned());
        }
        ["cpu" | "sysbus.cpu", "PerformanceInMips", value] if boot.platform.is_some() => {
            boot.performance_in_mips = Some(
                resolve(value)?
                    .parse::<u32>()
                    .ok()
                    .filter(|&mips| mips > 0)
                    .ok_or("PerformanceInMips requires a positive integer")?,
            );
        }
        ["showAnalyzer", "sysbus.uart" | "uart"] if boot.platform.is_some() => {
            // UART output is already connected to stdout.
        }
        [
            "showAnalyzer",
            name @ ("uart0" | "uart1" | "sysbus.uart0" | "sysbus.uart1"),
        ] if boot.platform.is_some() => {
            boot.named_analyzers
                .push(name.strip_prefix("sysbus.").unwrap_or(name).into());
        }
        [
            name @ ("uart0" | "uart1" | "sysbus.uart0" | "sysbus.uart1"),
            "IRQ",
        ] if boot.platform.is_some() => {
            let name = name.strip_prefix("sysbus.").unwrap_or(name);
            boot.uart_irqs.push(UartIrq::Read(name.into()));
        }
        [
            name @ ("uart0" | "uart1" | "sysbus.uart0" | "sysbus.uart1"),
            "IRQ",
            "Set",
            level,
        ] if boot.platform.is_some() => {
            let name = name.strip_prefix("sysbus.").unwrap_or(name);
            let level = level
                .parse()
                .map_err(|_| "expected IRQ level true or false")?;
            boot.uart_irqs.push(UartIrq::Set(name.into(), level));
        }
        _ => return Err(format!("unsupported monitor command {text:?}")),
    }
    Ok(())
}

pub fn asset_path(
    value: &str,
    script: &Path,
    root: &Path,
    assets: Option<&Path>,
) -> Result<PathBuf, String> {
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
    Ok(local_path(value, script, root))
}

pub fn local_path(value: &str, script: &Path, root: &Path) -> PathBuf {
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
            execute(command, &vars, &mut created, &mut boot, path, root).unwrap();
        }
        assert_eq!(
            boot.binaries,
            [("https://example.org/firmware.bin".into(), 0x4000_0000)]
        );
        assert_eq!(boot.boot_pcs, [("cpu_1".into(), 0x4000_0004)]);
        assert_eq!(
            asset_path(&boot.binaries[0].0, path, root, Some(Path::new("assets"))).unwrap(),
            Path::new("assets/firmware.bin")
        );
    }

    #[test]
    fn reads_boot_commands_and_reports_macro_source_lines() {
        let directory = std::env::temp_dir().join(format!("penv-resc-{}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        let script = directory.join("boot.resc");
        let content = concat!(
            "$name?=\"Test\"\n",
            "$bin?=@firmware.elf\n",
            "mach create $name\n",
            "machine LoadPlatformDescription @board.repl\n",
            "macro reset\n",
            "\"\"\"\n",
            "  sysbus LoadELF $bin\n",
            "  sysbus.cpu MTVEC 0x80000020\n",
            "\"\"\"\n",
            "runMacro $reset\n",
            "cpu SetMachineIrqMask 0xffffffff\n",
        );
        fs::write(&script, content).unwrap();
        let boot = load(&script, &directory).unwrap();
        assert_eq!(boot.platform, Some(directory.join("board.repl")));
        assert_eq!(boot.image.as_deref(), Some("firmware.elf"));
        assert_eq!(boot.mtvec, Some(0x8000_0020));
        assert_eq!(boot.mask, Some(u32::MAX));
        fs::write(&script, content.replace("MTVEC", "unknown")).unwrap();
        let result = load(&script, &directory);
        fs::remove_dir_all(directory).unwrap();
        assert!(
            matches!(result, Err(error) if error.contains("boot.resc:8: unsupported monitor command"))
        );
    }
}
