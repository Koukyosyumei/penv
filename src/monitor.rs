use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

#[derive(Default)]
pub struct Boot {
    pub binaries: Vec<(Asset, u32)>,
    pub boot_pcs: Vec<(String, u32)>,
    pub platform: Option<PathBuf>,
    pub image: Option<Asset>,
    pub mtvec: Option<u32>,
    pub mask: Option<u32>,
    started: bool,
}

#[derive(Debug)]
pub struct Asset {
    value: String,
    script: PathBuf,
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
    let mut state = ScriptState::default();
    load_file(path, root, &mut state)?;
    let boot = state.boot;
    if !state.machine
        || boot.platform.is_none()
        || (boot.image.is_none() && boot.binaries.is_empty())
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
    if state.visiting.len() >= 64 || !state.visiting.insert(canonical.clone()) {
        return Err(format!(
            "{}: recursive or excessively deep script include",
            path.display()
        ));
    }
    let content = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
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
        if let Some(assignment) = text.strip_prefix('$') {
            let (name, value) = assignment
                .split_once('=')
                .ok_or_else(|| error("expected variable assignment"))?;
            let name = name.trim();
            if let Some(name) = name.strip_suffix('?') {
                state
                    .variables
                    .entry(name.trim().into())
                    .or_insert_with(|| value.trim().into());
            } else {
                state.variables.insert(name.into(), value.trim().into());
            }
            continue;
        }
        let words = command_words(text).map_err(|e| error(&e))?;
        match words.as_slice() {
            ["include" | "i", value] => {
                if state.boot.started {
                    return Err(error(
                        "commands after start are unsupported in a boot script",
                    ));
                }
                let file = resolve(&state.variables, value)
                    .map_err(|e| error(&e))?
                    .strip_prefix('@')
                    .ok_or_else(|| error("expected @script-path"))?;
                let included = local_path(&path_value(file), path, root);
                load_file(&included, root, state).map_err(|e| error(&e))?;
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
            )
            .map_err(|e| error(&e))?,
        }
    }
    state.visiting.remove(&canonical);
    Ok(())
}

fn resolve<'a>(variables: &'a HashMap<String, String>, value: &'a str) -> Result<&'a str, String> {
    match value.strip_prefix('$') {
        Some("") => Err("missing variable name".into()),
        Some(name) => variables
            .get(name)
            .map(String::as_str)
            .ok_or_else(|| format!("unknown variable {name}")),
        None => Ok(value),
    }
}

fn execute(
    text: &str,
    words: &[&str],
    variables: &HashMap<String, String>,
    machine: &mut bool,
    boot: &mut Boot,
    script: &Path,
    root: &Path,
) -> Result<(), String> {
    if matches!(words, [] | ["using", "sysbus"]) {
        return Ok(());
    }
    if boot.started {
        return Err("commands after start are unsupported in a boot script".into());
    }
    let resolve = |value| resolve(variables, value);
    match words {
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
            boot.platform = Some(local_path(&path_value(file), script, root));
        }
        ["sysbus", "LoadELF", value] if boot.platform.is_some() && boot.image.is_none() => {
            let file = resolve(value)?
                .strip_prefix('@')
                .ok_or("expected @firmware-path")?;
            boot.image = Some(Asset::new(file, script));
        }
        ["sysbus", "LoadBinary", value, address] if boot.platform.is_some() => {
            let file = resolve(value)?
                .strip_prefix('@')
                .ok_or("expected @firmware-path")?;
            boot.binaries.push((
                Asset::new(file, script),
                crate::parse_address(resolve(address)?)?,
            ));
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
        ["start"]
            if boot.platform.is_some() && (boot.image.is_some() || !boot.binaries.is_empty()) =>
        {
            boot.started = true;
        }
        ["showAnalyzer", "sysbus.uart" | "uart"] if boot.platform.is_some() => {
            // UART output is already connected to stdout.
        }
        _ => return Err(format!("unsupported monitor command {text:?}")),
    }
    Ok(())
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
        } else if new_word && matches!(character, '"' | '\'') {
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
    fn new(value: &str, script: &Path) -> Self {
        Self {
            value: path_value(value),
            script: script.into(),
        }
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
            )
            .unwrap();
        }
        assert_eq!(boot.binaries.len(), 1);
        assert_eq!(boot.binaries[0].1, 0x4000_0000);
        assert_eq!(boot.boot_pcs, [("cpu_1".into(), 0x4000_0004)]);
        assert_eq!(
            boot.binaries[0]
                .0
                .resolve(root, Some(Path::new("assets")))
                .unwrap(),
            Path::new("assets/firmware.bin")
        );
    }

    #[test]
    fn reads_boot_commands_and_reports_macro_source_lines() {
        let directory = std::env::temp_dir().join(format!("penv-resc-{}", std::process::id()));
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
