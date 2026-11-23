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
    pub buttons: Vec<(String, String, bool)>,
    pub halt_hooks: Vec<(String, String, String, String)>,
    symbols: Vec<String>,
    hooks: HashMap<String, String>,
    pub sd_cards: Vec<(String, String, u64)>,
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
        if let Some(name) = text.strip_prefix("set ") {
            if cursor
                .next()
                .is_none_or(|(_, line)| line.trim() != "\"\"\"")
            {
                return Err(error("expected triple-quoted hook body"));
            }
            let mut body = Vec::new();
            loop {
                let (_, command) = cursor.next().ok_or_else(|| error("unterminated hook"))?;
                let command = command.trim();
                if command == "\"\"\"" {
                    break;
                }
                if !command.is_empty() && !command.starts_with('#') {
                    body.push(command);
                }
            }
            let [assign, halt, ..] = body.as_slice() else {
                return Err(error("unsupported hook body"));
            };
            let (variable, hart) = assign
                .split_once(" = machine['sysbus.")
                .and_then(|(variable, suffix)| Some((variable, suffix.strip_suffix("']")?)))
                .ok_or_else(|| error("unsupported hook machine lookup"))?;
            if variable.is_empty()
                || hart.is_empty()
                || *halt != format!("{variable}.IsHalted = True")
                || !matches!(
                    body.get(2..),
                    Some([] | ["print(\"halted management CPU\")"])
                )
            {
                return Err(error("unsupported hook body"));
            }
            boot.hooks.insert(name.to_string(), hart.to_string());
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
        ["machine", "SdCardFromFile", value, controller, capacity]
        | [
            "machine",
            "SdCardFromFile",
            value,
            controller,
            capacity,
            "False" | "false",
        ] if boot.platform.is_some() => {
            let file = resolve(value)?
                .strip_prefix('@')
                .ok_or("expected @SD-image-path")?;
            let controller = controller.strip_prefix("sysbus.").unwrap_or(controller);
            let capacity = crate::parse_address64(resolve(capacity)?)?;
            boot.sd_cards
                .push((file.to_string(), controller.to_string(), capacity));
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
        ["sysbus", "LoadSymbolsFrom", value] if boot.platform.is_some() => {
            let file = resolve(value)?
                .strip_prefix('@')
                .ok_or("expected @symbol-path")?;
            boot.symbols.push(file.to_string());
        }
        ["emulation", "SetGlobalQuantum", value] => {
            let seconds: f64 = resolve(value)?
                .trim_matches('"')
                .parse()
                .map_err(|_| "invalid emulation quantum")?;
            if !seconds.is_finite() || seconds <= 0.0 {
                return Err("invalid emulation quantum".into());
            }
            // Single-machine execution already synchronizes after each instruction.
        }
        [target, "Pressed" | "Released"] if boot.platform.is_some() && target.contains('.') => {
            let (gpio, button) = target.rsplit_once('.').unwrap();
            boot.buttons.push((
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
        ] if boot.platform.is_some() => {
            let symbol = symbol
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix("\"`"))
                .ok_or("expected quoted hook symbol")?;
            let hook = callback
                .strip_prefix('$')
                .and_then(|name| boot.hooks.get(name))
                .ok_or("unknown halt hook")?;
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
        ["showAnalyzer", _] if boot.platform.is_some() => {
            // UART output is already connected to stdout.
        }
        ["echo", ..] => {
            // Monitor output is separate from guest UART output.
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
        assert_eq!(boot.buttons, [("gpio0".into(), "UserButton".into(), true)]);
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
    fn starlight_monitor_commands_leave_only_guest_assets_to_load() {
        let path = Path::new("beaglev_starlight.resc");
        let root = Path::new("renode");
        let mut boot = Boot::default();
        let mut machine = false;
        let vars = HashMap::from([("symbols", "@https://example.org/vmlinux")]);
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
            execute(command, &vars, &mut machine, &mut boot, path, root).unwrap();
        }
        assert_eq!(boot.image.as_deref(), Some("fw_payload.elf"));
        assert_eq!(boot.binaries, [("image.fit".into(), 0xa000_0000)]);
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
