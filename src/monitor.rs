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
}

pub type Variables = HashMap<String, String>;

pub fn walk(
    path: &Path,
    root: &Path,
    handler: &mut impl FnMut(&str, &Variables, &Path) -> Result<(), String>,
) -> Result<(), String> {
    walk_file(path, root, &mut Variables::new(), &mut Vec::new(), handler)
}

fn walk_file(
    path: &Path,
    root: &Path,
    variables: &mut Variables,
    active: &mut Vec<PathBuf>,
    handler: &mut impl FnMut(&str, &Variables, &Path) -> Result<(), String>,
) -> Result<(), String> {
    let canonical = path
        .canonicalize()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if active.len() >= 64 || active.contains(&canonical) {
        return Err(format!(
            "{}: recursive or excessively deep script include",
            path.display()
        ));
    }
    active.push(canonical);
    let content = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut macros = HashMap::new();
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
                variables
                    .entry(name.trim().into())
                    .or_insert_with(|| value.trim().into());
            } else {
                variables.insert(name.into(), value.trim().into());
            }
            continue;
        }
        if let Some(name) = text.strip_prefix("runMacro $") {
            let body = macros
                .get(name.trim())
                .ok_or_else(|| error("unknown macro"))?;
            for &(index, command) in body {
                dispatch(command.trim(), variables, path, root, active, handler)
                    .map_err(|e| format!("{}:{}: {e}", path.display(), index + 1))?;
            }
            continue;
        }
        dispatch(text, variables, path, root, active, handler).map_err(|e| error(&e))?;
    }
    active.pop();
    Ok(())
}

fn dispatch(
    text: &str,
    variables: &mut Variables,
    path: &Path,
    root: &Path,
    active: &mut Vec<PathBuf>,
    handler: &mut impl FnMut(&str, &Variables, &Path) -> Result<(), String>,
) -> Result<(), String> {
    if let Some((command, argument)) = text.split_once(char::is_whitespace)
        && matches!(command, "i" | "include")
    {
        let filename = resolve(variables, argument.trim())?
            .strip_prefix('@')
            .ok_or("expected @script-path")?;
        let include = local_path(filename.trim_matches('"'), path, root);
        return walk_file(&include, root, variables, active, handler);
    }
    handler(text, variables, path)
}

pub fn words(text: &str) -> Result<Vec<&str>, String> {
    let mut words = Vec::new();
    let mut quoted = false;
    let mut start = None;
    for (index, character) in text.char_indices() {
        if !quoted && character == '#' && start.is_none() {
            break;
        }
        if !quoted && character.is_whitespace() {
            if let Some(start) = start.take() {
                words.push(&text[start..index]);
            }
        } else {
            start.get_or_insert(index);
            if character == '"' {
                quoted = !quoted;
            }
        }
    }
    if quoted {
        return Err("unterminated quoted argument".into());
    }
    if let Some(start) = start {
        words.push(&text[start..]);
    }
    Ok(words)
}

pub fn resolve<'a>(variables: &'a Variables, value: &'a str) -> Result<&'a str, String> {
    match value.strip_prefix('$') {
        Some("") => Err("missing variable name".into()),
        Some(name) => variables
            .get(name)
            .map(String::as_str)
            .ok_or_else(|| format!("unknown variable {name}")),
        None => Ok(value),
    }
}

#[cfg(test)]
fn load(path: &Path, root: &Path) -> Result<Boot, String> {
    let mut boot = Boot::default();
    let mut created = false;
    walk(path, root, &mut |text, variables, source| {
        execute(text, variables, &mut created, &mut boot, source, root)
    })?;
    Ok(boot)
}

pub fn execute(
    text: &str,
    variables: &Variables,
    machine: &mut bool,
    boot: &mut Boot,
    script: &Path,
    root: &Path,
) -> Result<(), String> {
    if text.is_empty() || text.starts_with('#') || text == "using sysbus" {
        return Ok(());
    }
    let words = words(text)?;
    let resolve = |value| resolve(variables, value);
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
            boot.platform = Some(local_path(file.trim_matches('"'), script, root));
        }
        ["sysbus", "LoadELF", value] if boot.platform.is_some() && boot.image.is_none() => {
            let file = resolve(value)?
                .strip_prefix('@')
                .ok_or("expected @firmware-path")?;
            boot.image = Some(firmware_reference(file, script));
        }
        ["sysbus", "LoadBinary", value, address] if boot.platform.is_some() => {
            let file = resolve(value)?
                .strip_prefix('@')
                .ok_or("expected @firmware-path")?;
            boot.binaries.push((
                firmware_reference(file, script),
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
        ["showAnalyzer", "sysbus.uart" | "uart"] if boot.platform.is_some() => {
            // UART output is already connected to stdout.
        }
        _ => return Err(format!("unsupported monitor command {text:?}")),
    }
    Ok(())
}

fn firmware_reference(value: &str, script: &Path) -> String {
    let value = value.trim_matches('"');
    let local = script.parent().unwrap_or(Path::new(".")).join(value);
    if !value.contains("://")
        && local.is_file()
        && let Ok(absolute) = local.canonicalize()
        && let Some(absolute) = absolute.to_str()
    {
        return absolute.to_owned();
    }
    value.to_owned()
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
