use crate::repl_syntax::{quoted_string, unquoted_delimiters, without_comment};

use crate::platform::{property_key, validate_entry_properties};

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    rc::Rc,
};

use crate::{
    constant_mmio::ConstantMmio,
    machine::{MAX_HARTS, Machine},
    nvic::IRQ_LINES,
    stm32_can::StmCan,
    stm32_iwdg::Iwdg,
    stm32_uart::{Stm32Irq, UsartLayout},
    stm32f1_i2c::F1I2c,
    stm32f4_flash::F4Flash,
    stm32f4_power::F4Power,
    stm32f4_rcc::F4Rcc,
    stm32f4_spi::{Ad4011, F4Spi},
    stm32h7_adc_platform,
    stm32h7_exti::H7Exti,
    stm32h7_flash::FlashController,
    stm32h7_gpio_platform, stm32h7_rtc_platform,
};

type Properties = Vec<(SourceLine, String)>;

#[derive(Clone, Debug)]
pub(crate) struct SourceLine {
    path: Rc<Path>,
    line: usize,
}

impl std::fmt::Display for SourceLine {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}:{}", self.path.display(), self.line)
    }
}

#[derive(Debug)]
pub(crate) struct Entry {
    pub(crate) name: String,
    pub(crate) model: String,
    pub(crate) registration: Option<String>,
    pub(crate) body: Properties,
    source: PathBuf,
    line: usize,
}

impl Entry {
    pub(crate) fn sized_sysbus_address(&self, size: u32) -> Result<u32, String> {
        let registration = self
            .registration
            .as_deref()
            .ok_or("missing sysbus registration")?;
        crate::repl_parser::sized_sysbus_address(registration, size)
    }

    pub(crate) fn from_parsed(
        name: String,
        model: String,
        registration: Option<String>,
        body: Vec<(Rc<Path>, usize, String)>,
        source: PathBuf,
        line: usize,
    ) -> Self {
        Self {
            name,
            model,
            registration,
            body: body
                .into_iter()
                .map(|(path, line, value)| (SourceLine { path, line }, value))
                .collect(),
            source,
            line,
        }
    }

    pub(crate) fn error(&self, detail: &str) -> String {
        format!(
            "{}:{}: {}: {detail}",
            self.source.display(),
            self.line,
            self.name
        )
    }

    pub(crate) fn sysbus(&self) -> Result<Option<u32>, String> {
        let registration = self
            .registration
            .as_deref()
            .ok_or_else(|| self.error("missing sysbus registration"))?;
        if registration == "sysbus" {
            return Ok(None);
        }
        let address = registration
            .strip_prefix("sysbus")
            .filter(|s| s.starts_with(char::is_whitespace))
            .ok_or_else(|| self.error("unsupported bus registration"))?;
        number(address.trim()).map(Some).map_err(|e| self.error(&e))
    }

    fn bus_range(&self) -> Result<(u32, u32), String> {
        let (base, size) = self
            .registration
            .as_deref()
            .and_then(|text| text.strip_prefix("sysbus <"))
            .and_then(|text| text.strip_suffix('>'))
            .and_then(|text| text.split_once(','))
            .ok_or_else(|| self.error("expected sysbus range"))?;
        let size = size
            .trim()
            .strip_prefix('+')
            .ok_or_else(|| self.error("expected +size in sysbus range"))?;
        Ok((
            number(base.trim()).map_err(|e| self.error(&e))?,
            number(size).map_err(|e| self.error(&e))?,
        ))
    }

    fn address_with_optional_range(&self, size: u32) -> Result<u32, String> {
        let Some(range) = self
            .registration
            .as_deref()
            .and_then(|text| text.strip_prefix("sysbus <"))
            .and_then(|text| text.strip_suffix('>'))
        else {
            return self.address();
        };
        let (address, length) = range
            .split_once(',')
            .ok_or_else(|| self.error("invalid sysbus range"))?;
        let length = length
            .trim()
            .strip_prefix('+')
            .ok_or_else(|| self.error("expected +size in sysbus range"))?;
        if number(length).map_err(|e| self.error(&e))? != size {
            return Err(self.error("invalid sysbus range size"));
        }
        number(address.trim()).map_err(|e| self.error(&e))
    }

    pub(crate) fn address(&self) -> Result<u32, String> {
        self.sysbus()?
            .ok_or_else(|| self.error("expected a sysbus address"))
    }
}

pub(crate) fn number(value: &str) -> Result<u32, String> {
    let digits = value.replace('_', "");
    match digits.strip_prefix("0x") {
        Some(hex) => u32::from_str_radix(hex, 16),
        None => digits.parse(),
    }
    .map_err(|_| format!("invalid 32-bit number {value:?}"))
}

fn inline_body<'a>(
    declaration: &'a str,
    source: &SourceLine,
) -> Result<(&'a str, Properties), &'static str> {
    let mut braces = unquoted_delimiters(declaration, &['{', '}']);
    let Some((begin, '{')) = braces.next() else {
        return if unquoted_delimiters(declaration, &['}']).next().is_some() {
            Err("unexpected closing brace")
        } else {
            Ok((declaration, Vec::new()))
        };
    };
    // A brace immediately after @ begins a registration expression.
    if declaration[..begin].trim_end().ends_with('@') {
        return Ok((declaration, Vec::new()));
    }
    let Some((end, '}')) = braces.next() else {
        return Err("expected a closing brace for inline properties");
    };
    if braces.next().is_some() || !declaration[end + 1..].trim().is_empty() {
        return Err("unexpected text after inline properties");
    }
    let text = &declaration[begin + 1..end];
    let mut body = Vec::new();
    let mut start = 0;
    for end in unquoted_delimiters(text, &[';'])
        .map(|(index, _)| index)
        .chain([text.len()])
    {
        let item = text[start..end].trim();
        if !item.is_empty() {
            body.push((source.clone(), item.into()));
        }
        start = end + 1;
    }
    Ok((&declaration[..begin], body))
}

fn parse(
    path: &Path,
    root: &Path,
    visiting: &mut HashSet<PathBuf>,
    entries: &mut Vec<Entry>,
) -> Result<(), String> {
    let canonical = path
        .canonicalize()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if visiting.len() >= 64 || !visiting.insert(canonical.clone()) {
        return Err(format!(
            "{}: recursive or excessively deep platform include",
            path.display()
        ));
    }
    let content = fs::read_to_string(&canonical).map_err(|e| format!("{}: {e}", path.display()))?;
    let source_path = Rc::<Path>::from(path);
    let mut current: Option<Entry> = None;
    let mut registration_depth = 0;
    let mut lines = content.lines().enumerate();
    while let Some((index, raw)) = lines.next() {
        let line = without_comment(raw);
        let text = line.trim();
        let lineno = index + 1;
        let source = SourceLine {
            path: Rc::clone(&source_path),
            line: lineno,
        };
        let error = |detail: &str| format!("{}:{lineno}: {detail}", path.display());
        if text.is_empty() {
            continue;
        }
        if registration_depth != 0 {
            let registration = current.as_mut().unwrap().registration.as_mut().unwrap();
            registration.push(' ');
            registration.push_str(text);
            registration_depth += brace_depth(text);
            if registration_depth < 0 {
                return Err(error("unexpected closing registration brace"));
            }
            continue;
        }
        if text == "script: '''"
            && let Some(entry) = current
                .as_mut()
                .filter(|entry| entry.model == "Python.PythonPeripheral")
        {
            let mut property = String::from("script:");
            loop {
                let (_, raw) = lines
                    .next()
                    .ok_or_else(|| error("unterminated Python peripheral script"))?;
                if raw.trim() == "'''" {
                    break;
                }
                property.push('\n');
                property.push_str(raw);
            }
            entry.body.push((source, property));
            continue;
        }
        if line.starts_with(char::is_whitespace) {
            current
                .as_mut()
                .ok_or_else(|| error("property has no declaration"))?
                .body
                .push((source, text.into()));
            continue;
        }
        entries.extend(current.take());
        if let Some(include) = text.strip_prefix("using ") {
            let filename = quoted_string(include)
                .filter(|filename| !filename.is_empty())
                .ok_or_else(|| error("expected using \"path.repl\""))?;
            let local = canonical.parent().unwrap().join(&filename);
            let include_path = if local.is_file() {
                local
            } else {
                root.join(filename)
            };
            parse(&include_path, root, visiting, entries).map_err(|e| error(&e))?;
            continue;
        }
        let (name, declaration) = text
            .split_once(':')
            .ok_or_else(|| error("expected a platform declaration"))?;
        let name = name.trim();
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(error("unsupported declaration name"));
        }
        let (declaration, body) = inline_body(declaration, &source).map_err(error)?;
        let (model, registration) = match declaration.split_once('@') {
            Some((model, registration)) => (model.trim(), Some(registration.trim().to_string())),
            None => (declaration.trim(), None),
        };
        if (crate::network_platform::has_regions(model)
            || model == F4Flash::MODEL
            || stm32h7_adc_platform::has_regions(model)
            || model == stm32h7_gpio_platform::GPIO
            || model == stm32h7_rtc_platform::RTC
            || model == crate::stm32h7_spi::SPI
            || crate::stm32h7_board_platform::has_regions(model))
            && let Some(registration) = &registration
            && registration.starts_with('{')
        {
            registration_depth = brace_depth(registration);
        }
        current = Some(Entry {
            name: name.into(),
            model: model.into(),
            registration,
            body,
            source: path.into(),
            line: lineno,
        });
    }
    if registration_depth != 0 {
        return Err(current
            .unwrap()
            .error("unterminated peripheral registration"));
    }
    entries.extend(current);
    visiting.remove(&canonical);
    Ok(())
}

fn brace_depth(text: &str) -> i32 {
    unquoted_delimiters(text, &['{', '}'])
        .map(|(_, brace)| if brace == '{' { 1 } else { -1 })
        .sum()
}

fn apply_overlays(entries: Vec<Entry>) -> Vec<Entry> {
    let mut definitions: Vec<Entry> = Vec::new();
    let mut names: HashMap<String, usize> = HashMap::new();
    for entry in entries {
        if entry.model.is_empty()
            && entry.registration.is_none()
            && let Some(&index) = names.get(&entry.name)
        {
            let body = &mut definitions[index].body;
            // Replace a property from an earlier entry.
            for (_, text) in &entry.body {
                if let Some(key) = property_key(text) {
                    let mut previous = body
                        .iter()
                        .enumerate()
                        .filter(|(_, (_, property))| property_key(property) == Some(key));
                    if let Some((index, _)) = previous.next()
                        && previous.next().is_none()
                    {
                        body.remove(index);
                    }
                }
            }
            let insertion = body
                .iter()
                .position(|(_, text)| text == "init:")
                .unwrap_or(body.len());
            body.splice(insertion..insertion, entry.body);
        } else {
            if !entry.model.is_empty() {
                names.entry(entry.name.clone()).or_insert(definitions.len());
            }
            definitions.push(entry);
        }
    }
    definitions
}

pub fn load(path: &Path, root: Option<&Path>) -> Result<Machine, String> {
    load_with_timing(path, root, false)
}

pub fn load_with_timing(
    path: &Path,
    root: Option<&Path>,
    f4_repl_timing: bool,
) -> Result<Machine, String> {
    load_basic(path, root, f4_repl_timing).or_else(|original| {
        let root = root.unwrap_or(Path::new("."));
        crate::repl_parser::load(path, root)
            .and_then(|entries| build_machine_with_timing(entries, path, f4_repl_timing))
            .map_err(|error| format!("{original}; REPL syntax: {error}"))
    })
}

fn load_basic(path: &Path, root: Option<&Path>, f4_repl_timing: bool) -> Result<Machine, String> {
    let mut entries = Vec::new();
    parse(
        path,
        root.unwrap_or(Path::new(".")),
        &mut HashSet::new(),
        &mut entries,
    )?;
    for entry in &entries {
        validate_entry_properties(&entry.body, |source, key| {
            format!("{source}: {}: duplicate REPL property {key:?}", entry.name)
        })?;
    }
    build_machine_with_timing(apply_overlays(entries), path, f4_repl_timing)
}

pub(crate) fn load_parsed(path: &Path, root: &Path) -> Result<Machine, String> {
    build_machine(crate::repl_parser::load(path, root)?, path)
}

pub fn export_peripherals(path: &Path, root: Option<&Path>, output: &Path) -> Result<(), String> {
    load(path, root)?;
    write_peripheral_map(path, root, output)
}

pub(crate) fn write_peripheral_map(path: &Path, root: Option<&Path>, output: &Path) -> Result<(), String> {
    use std::io::Write;
    let entries = crate::repl_parser::load(path, root.unwrap_or(Path::new(".")))?;
    let mut nodes = Vec::new();
    for entry in &entries {
        let addresses = match entry.registration.as_deref() {
            Some("sysbus") => Vec::new(),
            Some(registration) if registration.starts_with("sysbus <") => {
                vec![entry.bus_range()?.0]
            }
            Some(registration) if registration.starts_with("sysbus ") => vec![entry.address()?],
            Some(registration)
                if registration.starts_with('{') && entry.model == F4Flash::MODEL =>
            {
                let (registers, options, _) = f4_flash_registrations(entry)?;
                vec![registers, options]
            }
            None | Some("none") => continue,
            Some(_) => return Err(entry.error("unsupported peripheral export registration")),
        };
        let model = entry.model.rsplit('.').next().unwrap();
        let bindings = addresses
            .iter()
            .map(|address| format!(r#"{{"Type":"Bus","Value":{address}}}"#))
            .collect::<Vec<_>>()
            .join(",");
        nodes.push(format!(
            r#"{{"Name":{},"Alias":{},"Bindings":[{bindings}],"Children":[]}}"#,
            json_string(model),
            json_string(&entry.name),
        ));
    }
    let mut file =
        fs::File::create(output).map_err(|err| format!("{}: {err}", output.display()))?;
    writeln!(
        file,
        r#"{{"Header":"Peripheral Map 1.0","Root":{{"Name":"SystemBus","Alias":"sysbus","Bindings":[],"Children":[{}]}}}}"#,
        nodes.join(",")
    )
    .map_err(|err| format!("{}: {err}", output.display()))
}

fn json_string(value: &str) -> String {
    use std::fmt::Write;
    let mut output = String::from("\"");
    for ch in value.chars() {
        match ch {
            '\\' | '"' => {
                output.push('\\');
                output.push(ch);
            }
            '\u{0}'..='\u{1f}' => write!(output, "\\u{:04x}", u32::from(ch)).unwrap(),
            _ => output.push(ch),
        }
    }
    output.push('"');
    output
}


pub(crate) fn check_text(content: &str, root: Option<&Path>) -> Result<(), String> {
    let entries = crate::repl_parser::parse_inline(content, root.unwrap_or(Path::new(".")))?;
    if entries.is_empty() {
        return Ok(());
    }
    build_machine(entries, Path::new("<inline REPL>")).map(|_| ())
}

fn build_machine(entries: Vec<Entry>, path: &Path) -> Result<Machine, String> {
    build_machine_with_timing(entries, path, false)
}

fn build_machine_with_timing(
    entries: Vec<Entry>,
    path: &Path,
    f4_repl_timing: bool,
) -> Result<Machine, String> {
    let mut machine = Machine::without_default_ram();
    machine.bus.f4_repl_timing = f4_repl_timing;
    let mut cpus = Vec::new();
    let mut clint = None;
    let mut dwt = None;
    let mut nvic = None;
    let mut exti = None;
    let mut flash_controller = None;
    let mut combiners = Vec::new();
    let mut ram = false;
    let mut uart = None;
    let mut stm32_uarts = Vec::new();
    let mut stm32_can = Vec::new();
    let mut watchdogs = Vec::new();
    let mut f4_rcc = None;
    let mut plic = None;
    let mut timer = None;
    let mut names = HashSet::new();
    for entry in &entries {
        if !names.insert(&entry.name) {
            return Err(entry.error("duplicate declarations and overrides are unsupported"));
        }
        if entry.model.is_empty() {
            if entry.name == "sysbus" {
                configure_bus_init(entry, &mut machine)?;
                continue;
            }
            let detail = if entry.registration.is_some() {
                "REPL registration overrides are unsupported"
            } else {
                "REPL overlay target has no earlier declaration"
            };
            return Err(entry.error(detail));
        }
        match entry.model.as_str() {
            "Miscellaneous.BitBanding" => {
                let properties = keys(entry, &["peripheralBase"])?;
                let [(_, target)] = properties.as_slice() else {
                    return Err(entry.error("expected bit-band target base"));
                };
                let target = number(target).map_err(|e| entry.error(&e))?;
                let (base, size) = entry.bus_range()?;
                machine
                    .bus
                    .map_bitband(base, size, target)
                    .map_err(|e| entry.error(&e))?;
            }
            "CPU.RiscV32" | "CPU.VexRiscv" | "CPU.CortexM" => {
                if entry.sysbus()?.is_some() {
                    return Err(entry.error("expected CPU registered on sysbus without an address"));
                }
                cpus.push(entry);
            }
            "Python.PythonPeripheral" => {
                let peripheral =
                    ConstantMmio::from_platform(entry).map_err(|error| entry.error(&error))?;
                machine
                    .bus
                    .map_constant_mmio(peripheral)
                    .map_err(|error| entry.error(&error))?;
            }
            "Memory.MappedMemory" | "Memory.ArrayMemory" => {
                let init = entry.body.iter().position(|(_, text)| text == "init:");
                let (properties, commands) = init
                    .map_or((entry.body.as_slice(), &[][..]), |index| {
                        (&entry.body[..index], &entry.body[index + 1..])
                    });
                let size = match properties {
                    [(_, text)] => text
                        .split_once(':')
                        .filter(|(key, _)| key.trim() == "size")
                        .map(|(_, value)| value.trim()),
                    _ => None,
                }
                .ok_or_else(|| entry.error("expected one memory size property before init"))?;
                let size = number(size).map_err(|e| entry.error(&e))?;
                let address = if let Some(range) = entry
                    .registration
                    .as_deref()
                    .and_then(|registration| registration.strip_prefix("sysbus <"))
                    .and_then(|range| range.strip_suffix('>'))
                {
                    let (address, length) = range
                        .split_once(',')
                        .ok_or_else(|| entry.error("invalid memory range"))?;
                    let length = length
                        .trim()
                        .strip_prefix('+')
                        .ok_or_else(|| entry.error("expected +size in memory range"))?;
                    if number(length).map_err(|e| entry.error(&e))? != size {
                        return Err(entry.error("memory range size disagrees with size property"));
                    }
                    number(address.trim()).map_err(|e| entry.error(&e))?
                } else {
                    entry.address()?
                };
                if entry.model == "Memory.ArrayMemory" {
                    machine.bus.map_array_memory(address, size as usize)
                } else {
                    machine.bus.map_ram(address, size as usize)
                }.map_err(|e| entry.error(&e))?;
                for (line, command) in commands {
                    let error = |detail: &str| format!("{line}: {}: {detail}", entry.name);
                    let (offset, width, value) =
                        memory_init(command, address).map_err(|e| error(&e))?;
                    if offset
                        .checked_add(width as u32)
                        .is_none_or(|end| end > size)
                    {
                        return Err(error("memory init write exceeds the declared region"));
                    }
                    let target = address + offset;
                    machine
                        .bus
                        .memory(target, width)
                        .map_err(|e| error(&e.to_string()))?
                        .copy_from_slice(&value.to_le_bytes()[..width]);
                }
                ram = true;
            }
            "GPIOPort.Murax_GPIO" => {
                keys(entry, &[])?;
                machine
                    .bus
                    .map_murax_gpio(entry.address()?)
                    .map_err(|e| entry.error(&e))?;
            }
            "Timers.Murax_Timer" => {
                if timer.replace(entry).is_some() {
                    return Err(entry.error("only one Murax timer is currently supported"));
                }
            }
            "IRQControllers.PlatformLevelInterruptController" => {
                if plic.replace(entry).is_some() {
                    return Err(entry.error("only one PLIC is currently supported"));
                }
            }
            StmCan::MODEL => stm32_can.push(entry),
            Iwdg::MODEL => watchdogs.push(entry),
            "UART.STM32F7_USART" | "UART.STM32_UART" => stm32_uarts.push(entry),
            "UART.Murax_UART" | "UART.NS16550" | "UART.LiteX_UART" => {
                if uart.replace(entry).is_some() {
                    return Err(entry.error("only one UART is currently supported"));
                }
            }
            "IRQControllers.STM32F4_EXTI" | "IRQControllers.STM32H7_EXTI" => {
                if exti.replace(entry).is_some() {
                    return Err(entry.error("only one STM32 EXTI is supported"));
                }
            }
            "Miscellaneous.CombinedInput" => combiners.push(entry),
            F4Rcc::MODEL => {
                if f4_rcc.replace(entry).is_some() {
                    return Err(entry.error("only one STM32F4 RCC is supported"));
                }
            }
            F4Power::MODEL => {
                keys(entry, &[])?;
                machine
                    .bus
                    .map_f4_power(entry.address()?)
                    .map_err(|error| entry.error(&error))?;
            }
            F4Flash::MODEL | "MTD.STM32H7_FlashController" => {
                if flash_controller.replace(entry).is_some() {
                    return Err(entry.error("only one STM32 flash controller is supported"));
                }
            }
            "Miscellaneous.DWT" => {
                if dwt.replace(entry).is_some() {
                    return Err(entry.error("only one DWT is supported"));
                }
            }
            "IRQControllers.NVIC" => {
                if nvic.replace(entry).is_some() {
                    return Err(entry.error("only one NVIC is supported"));
                }
            }
            "IRQControllers.CoreLevelInterruptor" => {
                if clint.replace(entry).is_some() {
                    return Err(entry.error("only one CLINT is currently supported"));
                }
            }
            _ if crate::network_platform::is_network(entry)
                || stm32h7_adc_platform::is_device(&entry.model)
                || stm32h7_gpio_platform::is_device(&entry.model)
                || entry.model == stm32h7_rtc_platform::RTC
                || matches!(entry.model.as_str(), F1I2c::MODEL | F1I2c::ECHO_MODEL)
                || matches!(entry.model.as_str(), F4Spi::MODEL | F4Spi::LOOPBACK_MODEL)
                || entry.model == crate::stm32h7_spi::SPI
                || crate::stm32h7_board_platform::is_device(&entry.model) => {}
            _ => return Err(entry.error(&format!("unsupported platform model {:?}", entry.model))),
        }
    }
    if cpus.is_empty() || !ram {
        return Err(format!(
            "{}: platform requires at least one CPU and mapped RAM",
            path.display()
        ));
    }
    if let Some(entry) = cpus.iter().find(|entry| entry.model == "CPU.CortexM") {
        if cpus.len() != 1 || clint.is_some() || plic.is_some() || timer.is_some() {
            return Err(
                entry.error("expected a single Cortex-M CPU without RISC-V interrupt controllers")
            );
        }
        let mut cpu_type = None;
        let mut mpu_regions = None;
        let mut vectors = None;
        for (key, value) in keys(
            entry,
            &[
                "cpuType",
                "vectorTableOffset",
                "InitVectorTableOffset",
                "numberOfMPURegions",
                "nvic",
            ],
        )? {
            match key {
                "cpuType" => cpu_type = Some(value),
                "nvic" if nvic.is_some_and(|controller| controller.name == value) => {}
                "nvic" => return Err(entry.error("unknown NVIC reference")),
                "numberOfMPURegions" => {
                    let count = number(value).map_err(|e| entry.error(&e))?;
                    if count > 16 {
                        return Err(entry.error("invalid MPU region count"));
                    }
                    mpu_regions = Some(count as u8);
                }
                "vectorTableOffset" | "InitVectorTableOffset" if vectors.is_none() => {
                    vectors = Some(number(value).map_err(|e| entry.error(&e))?);
                }
                "vectorTableOffset" | "InitVectorTableOffset" => {
                    return Err(entry.error("duplicate vector table offset"));
                }
                _ => unreachable!(),
            }
        }
        let (thumb2, default_mpu_regions, _has_cache) = match cpu_type {
            Some(r#""cortex-m0""# | r#""cortex-m0+""#) => (false, 0, false),
            Some(r#""cortex-m4""#) => (true, 8, false),
            Some(r#""cortex-m7""#) => (true, 16, true),
            _ => {
                return Err(
                    entry.error("expected Cortex-M0, Cortex-M0+, Cortex-M4 or Cortex-M7 cpuType")
                );
            }
        };
        if !thumb2 && mpu_regions.is_some() {
            return Err(entry.error("MPU regions require Cortex-M4 or Cortex-M7"));
        }
        if let Some(dwt) = dwt {
            let (_, frequency) = keys(dwt, &["frequency"])?
                .into_iter()
                .next()
                .ok_or_else(|| dwt.error("missing DWT frequency"))?;
            let frequency = number(frequency).map_err(|error| dwt.error(&error))?;
            if frequency == 0 {
                return Err(dwt.error("DWT frequency must be positive"));
            }
            machine
                .bus
                .map_dwt(dwt.address()?, frequency)
                .map_err(|error| dwt.error(&error))?;
        }
        machine.bus.map_systick().map_err(|e| entry.error(&e))?;
        machine
            .bus
            .set_step_hz(crate::machine::VIRTUAL_STEPS_PER_SECOND);
        let mut priority = None;
        let mut frequency = None;
        if let Some(controller) = nvic {
            if controller.address()? != crate::nvic::BASE {
                return Err(controller.error("NVIC must be mapped at 0xe000e000"));
            }
            let expected = format!("->{}@0", entry.name);
            let mut connected = false;
            for (_, text) in &controller.body {
                let wire: String = text.chars().filter(|c| !c.is_whitespace()).collect();
                if !connected && wire.strip_prefix("IRQ").unwrap_or(&wire) == expected {
                    connected = true;
                } else if let Some((key, value)) = text.split_once(':') {
                    let value = number(value.trim()).map_err(|e| controller.error(&e))?;
                    match key.trim() {
                        "priorityMask" if priority.is_none() && value <= 255 => {
                            priority = Some(value as u8)
                        }
                        "systickFrequency" if frequency.is_none() && value > 0 => {
                            frequency = Some(value)
                        }
                        _ => return Err(controller.error("unsupported or duplicate NVIC property")),
                    }
                } else {
                    return Err(controller.error("unsupported NVIC wiring"));
                }
            }
        }
        if let Some(frequency) = frequency {
            machine
                .bus
                .systick
                .as_mut()
                .unwrap()
                .set_frequency(frequency);
        }
        if let Some(flash) = flash_controller {
            if flash.model == F4Flash::MODEL {
                configure_f4_flash(flash, &entries, &mut machine)?;
            } else {
                configure_h7_flash(flash, &entries, nvic, &mut machine)?;
            }
        }
        configure_stm32_can(&stm32_can, nvic, &mut machine)?;
        configure_f4_spi(&entries, nvic, &mut machine)?;
        configure_f1_i2c(&entries, nvic, &mut machine)?;
        let mut uart_irq = None;
        if let Some(uart) = uart.filter(|uart| uart.model == "UART.NS16550") {
            keys(uart, &[])?;
            machine
                .bus
                .map_named_ns16550(&uart.name, uart.address()?, None, false)
                .map_err(|error| uart.error(&error))?;
        }
        if let Some(uart) = uart.filter(|uart| uart.model != "UART.NS16550") {
            if uart.model != "UART.LiteX_UART" {
                return Err(uart.error("unsupported Cortex-M UART model"));
            }
            if let [(_, wire)] = uart.body.as_slice()
                && let Some(controller) = nvic
                && let Some((pin, destination)) = wire.split_once("->")
                && matches!(pin.trim(), "IRQ" | "")
                && let Some((name, irq)) = destination.split_once('@')
                && name.trim() == controller.name
            {
                let irq = number(irq.trim()).map_err(|e| uart.error(&e))?;
                if irq >= IRQ_LINES as u32 {
                    return Err(uart.error("NVIC IRQ must be in 0–239"));
                }
                uart_irq = Some(irq as u8);
            } else if !uart.body.is_empty() {
                return Err(uart.error("expected UART IRQ wired to NVIC"));
            }
            machine
                .bus
                .map_litex_uart(vec![uart.address()?], None)
                .map_err(|e| uart.error(&e))?;
        }
        let mut combined_routes = HashMap::new();
        for combiner in combiners {
            let mut count = None;
            let mut output = None;
            for (_, text) in &combiner.body {
                if let Some(value) = text.strip_prefix("numberOfInputs:")
                    && count.is_none()
                {
                    count = Some(number(value.trim()).map_err(|e| combiner.error(&e))?);
                } else if let Some(wire) = text.strip_prefix("->")
                    && let Some((target, index)) = wire.split_once('@')
                    && nvic.is_some_and(|controller| controller.name == target.trim())
                    && output.is_none()
                {
                    output =
                        Some(irq_index(index, IRQ_LINES as u32).map_err(|e| combiner.error(&e))?);
                } else {
                    return Err(combiner.error("unsupported combined input property or wiring"));
                }
            }
            match (count, output) {
                (Some(count), Some(output)) if count > 0 => {
                    combined_routes.insert(combiner.name.as_str(), (count, output));
                }
                _ => return Err(combiner.error("expected input count and an NVIC output")),
            }
        }
        let exti_lines = if let Some(controller) = exti {
            if controller.model == "IRQControllers.STM32H7_EXTI" {
                if controller.body.iter().any(|(_, text)| !text.contains("->")) {
                    return Err(controller.error("unsupported STM32H7 EXTI property"));
                }
                let mut outputs = [None; H7Exti::LINES];
                exti_outputs(controller, nvic, &combined_routes, &mut outputs)?;
                machine
                    .bus
                    .map_h7_exti(controller.address()?, outputs)
                    .map_err(|e| controller.error(&e))?;
                H7Exti::LINES as u32
            } else {
                let mut count = None;
                let mut direct = None;
                for (_, text) in &controller.body {
                    if text.contains("->") {
                        continue;
                    }
                    let (key, value) = text
                        .split_once(':')
                        .ok_or_else(|| controller.error("invalid EXTI property"))?;
                    let target = match key.trim() {
                        "numberOfOutputLines" if count.is_none() => &mut count,
                        "firstDirectLine" if direct.is_none() => &mut direct,
                        _ => return Err(controller.error("unsupported or duplicate EXTI property")),
                    };
                    *target = Some(number(value.trim()).map_err(|e| controller.error(&e))?);
                }
                let count = count.unwrap_or(14);
                let direct = direct.unwrap_or(23);
                if !(1..=32).contains(&count) || direct > 32 {
                    return Err(controller.error("EXTI supports 1–32 output lines"));
                }
                let mut outputs = vec![None; count as usize];
                exti_outputs(controller, nvic, &combined_routes, &mut outputs)?;
                machine
                    .bus
                    .map_exti(controller.address()?, outputs, direct)
                    .map_err(|e| controller.error(&e))?;
                count
            }
        } else {
            0
        };
        stm32h7_adc_platform::configure(&entries, nvic, exti, exti_lines, &mut machine)?;
        crate::stm32h7_board_platform::configure(&entries, nvic, exti, exti_lines, &mut machine)?;
        stm32h7_gpio_platform::configure(&entries, exti, exti_lines, &mut machine)?;
        stm32h7_rtc_platform::configure(&entries, nvic, exti, exti_lines, &mut machine)?;
        for uart in stm32_uarts {
            let legacy = uart.model == "UART.STM32_UART";
            let mut irq = None;
            let mut low_power = None;
            let mut frequency = None;
            for (_, text) in &uart.body {
                if let Some((pin, destination)) = text.split_once("->")
                    && matches!(pin.trim(), "IRQ" | "")
                    && let Some((name, line)) = destination.split_once('@')
                    && irq.is_none()
                {
                    let name = name.trim();
                    let route = if nvic.is_some_and(|controller| controller.name == name) {
                        Stm32Irq::Nvic(
                            irq_index(line, IRQ_LINES as u32).map_err(|e| uart.error(&e))?,
                        )
                    } else if exti.is_some_and(|controller| controller.name == name) {
                        Stm32Irq::Exti(irq_index(line, exti_lines).map_err(|e| uart.error(&e))?)
                    } else if let Some(&(pins, output)) = combined_routes.get(name) {
                        irq_index(line, pins).map_err(|e| uart.error(&e))?;
                        Stm32Irq::Nvic(output)
                    } else {
                        return Err(uart.error("unsupported USART IRQ target"));
                    };
                    irq = Some(route);
                } else if let Some((key, value)) = text.split_once(':') {
                    let value = value.trim();
                    match key.trim() {
                        "lowPowerMode" if !legacy && low_power.is_none() => {
                            low_power = Some(
                                value
                                    .parse::<bool>()
                                    .map_err(|_| uart.error("invalid lowPowerMode"))?,
                            );
                        }
                        "frequency" if frequency.is_none() => {
                            frequency = Some(number(value).map_err(|e| uart.error(&e))?);
                            if frequency == Some(0) {
                                return Err(uart.error("USART frequency must be positive"));
                            }
                        }
                        _ => return Err(uart.error("unsupported or duplicate USART property")),
                    }
                } else {
                    return Err(uart.error("unsupported USART IRQ wiring"));
                }
            }
            let layout = if legacy {
                UsartLayout::F4
            } else {
                UsartLayout::F7 {
                    low_power: low_power.unwrap_or(false),
                }
            };
            let (base, alias) = if uart
                .registration
                .as_deref()
                .is_some_and(|text| text.starts_with('{'))
            {
                crate::stm32h7_board_platform::registration(uart, &entries)
                    .map_err(|error| uart.error(&error))?
            } else {
                (uart.address_with_optional_range(layout.size())?, None)
            };
            let mut device =
                crate::stm32_uart::Stm32Usart::with_layout(uart.name.clone(), base, irq, layout);
            device.frequency = frequency;
            if let Some(alias) = alias {
                device.clock = Some(
                    crate::stm32_uart::Stm32UsartClock::from_alias(alias)
                        .map_err(|e| uart.error(&e))?,
                );
            }
            machine
                .bus
                .map_stm32_usart(device)
                .map_err(|e| uart.error(&e))?;
        }
        if let Some(rcc) = f4_rcc {
            configure_f4_rcc(rcc, &entries, &mut machine)?;
        }
        machine
            .bus
            .map_nvic(priority.unwrap_or(0xc0), uart_irq)
            .map_err(|e| entry.error(&e))?;
        if let Some(controller) = nvic {
            machine.bus.set_nvic_name(controller.name.clone());
        }
        machine.primary.name = Some(entry.name.clone());
        if cpu_type == Some(r#""cortex-m4""#) {
            machine.bus.nvic.as_mut().unwrap().enable_fpu();
        }
        machine.primary.configure_cortex_m(vectors, thumb2);
        if thumb2 {
            machine
                .bus
                .nvic
                .as_mut()
                .unwrap()
                .configure_mpu(mpu_regions.unwrap_or(default_mpu_regions));
        }
        crate::network_platform::configure(
            &entries,
            &mut machine,
            None,
            nvic.map(|entry| entry.name.as_str()),
        )?;
        return Ok(machine);
    }
    if let Some(entry) = entries
        .iter()
        .find(|entry| matches!(entry.model.as_str(), F1I2c::MODEL | F1I2c::ECHO_MODEL))
    {
        return Err(entry.error("STM32F1 I2C requires a Cortex-M CPU"));
    }
    if let Some(entry) = entries
        .iter()
        .find(|entry| matches!(entry.model.as_str(), F4Spi::MODEL | F4Spi::LOOPBACK_MODEL))
    {
        return Err(entry.error("STM32F4 SPI requires a Cortex-M CPU"));
    }
    if let Some(entry) = entries.iter().find(|entry| {
        stm32h7_adc_platform::is_device(&entry.model)
            || stm32h7_gpio_platform::is_device(&entry.model)
            || entry.model == stm32h7_rtc_platform::RTC
            || entry.model == crate::stm32h7_spi::SPI
            || crate::stm32h7_board_platform::is_device(&entry.model)
    }) {
        return Err(entry.error("STM32H7 peripherals require a Cortex-M CPU"));
    }
    if let Some(flash) = flash_controller {
        return Err(flash.error("STM32 flash controller requires a Cortex-M CPU"));
    }
    if let Some(dwt) = dwt {
        return Err(dwt.error("DWT requires a Cortex-M CPU"));
    }
    if let Some(nvic) = nvic {
        return Err(nvic.error("NVIC requires a Cortex-M CPU"));
    }
    if let Some(rcc) = f4_rcc {
        return Err(rcc.error("STM32F4 RCC requires a Cortex-M CPU"));
    }
    if let Some(watchdog) = watchdogs.first() {
        return Err(watchdog.error("IWDG requires a Cortex-M CPU"));
    }
    if let Some(can) = stm32_can.first() {
        return Err(can.error("STM32 CAN requires a Cortex-M CPU"));
    }
    if let Some(uart) = stm32_uarts.first() {
        return Err(uart.error("STM32 USART requires a Cortex-M platform"));
    }
    if let Some(controller) = exti.or(combiners.first().copied()) {
        return Err(controller.error("STM32 interrupt routing requires a Cortex-M platform"));
    }
    let mut configs = Vec::new();
    for entry in cpus {
        let mut isa = None;
        let mut id = 0;
        let mut pmp = entry.model == "CPU.RiscV32";
        for (key, value) in keys(
            entry,
            &[
                "cpuType",
                "hartId",
                "timeProvider",
                "privilegedArchitecture",
                "pmpEntryCount",
            ],
        )? {
            match (key, value) {
                ("cpuType", value) => {
                    isa = match value.trim_matches('"') {
                        "rv32i" => Some((false, false, false)),
                        "rv32im" | "rv32im_zicsr_zifencei" => Some((true, false, false)),
                        "rv32ima" | "rv32ima_zicsr_zifencei" => Some((true, true, false)),
                        "rv32imac" | "rv32imac_zicsr_zifencei" => Some((true, true, true)),
                        "rv32imc" | "rv32imc_zicsr_zifencei" => Some((true, false, true)),
                        _ => return Err(entry.error(&format!("unsupported CPU type {value}"))),
                    };
                }
                ("hartId", value) => id = number(value).map_err(|e| entry.error(&e))? as usize,
                ("privilegedArchitecture", "PrivilegedArchitecture.Priv1_10") => {}
                ("timeProvider", name) if clint.is_some_and(|clint| clint.name == name) => {}
                ("pmpEntryCount", "0") => pmp = false,
                ("pmpEntryCount", "16") => pmp = true,
                _ => return Err(entry.error(&format!("unsupported CPU property {key} = {value}"))),
            }
        }
        configs.push((
            id,
            entry,
            isa.ok_or_else(|| entry.error("missing cpuType"))?,
            pmp,
        ));
    }
    configs.sort_unstable_by_key(|config| config.0);
    if configs.len() > MAX_HARTS
        || configs
            .iter()
            .enumerate()
            .any(|(index, config)| index != config.0)
    {
        return Err(format!(
            "{}: hart IDs must be unique and contiguous from zero, up to {MAX_HARTS} harts",
            path.display()
        ));
    }
    machine.configure_harts(configs.len());
    for &(id, entry, (multiply, atomic, compressed), pmp) in &configs {
        let hart = machine.hart_mut(id);
        hart.name = Some(entry.name.clone());
        hart.set_integer_extensions(multiply, atomic, compressed);
        if entry.model == "CPU.VexRiscv" {
            hart.enable_vex_core(true);
        }
        hart.enable_user_mode();
        hart.enable_supervisor_mode();
        if pmp {
            hart.enable_pmp(16);
        }
    }
    let cpu = configs[0].1;
    if let Some(entry) = clint {
        let mut target_count = None;
        let mut frequency = None;
        let mut wires = HashSet::new();
        for (lineno, line) in &entry.body {
            if line.contains("->") {
                let wire: String = line.chars().filter(|c| !c.is_whitespace()).collect();
                if wires.insert(wire) {
                    continue;
                }
            } else if let Some((key, value)) = line.split_once(':') {
                let slot = match key.trim() {
                    "frequency" => Some(&mut frequency),
                    "numberOfTargets" => Some(&mut target_count),
                    _ => None,
                };
                if let Some(slot) = slot
                    && slot.is_none()
                {
                    *slot = Some(number(value.trim()).map_err(|e| entry.error(&e))?);
                    continue;
                }
            }
            return Err(format!(
                "{}:{lineno}: unsupported or duplicate CLINT property",
                entry.source.display()
            ));
        }
        let targets = target_count.unwrap_or(1) as usize;
        if frequency == Some(0) || targets == 0 || targets > configs.len() || wires.len() != targets
        {
            return Err(entry.error("invalid CLINT frequency or target wiring"));
        }
        for &(id, target, _, _) in &configs[..targets] {
            let (software, timer) = if target.model == "CPU.VexRiscv" {
                (101, 100)
            } else {
                (3, 7)
            };
            let expected = format!(
                "[{},{}]->{}@[{software},{timer}]",
                2 * id,
                2 * id + 1,
                target.name
            );
            if !wires.contains(&expected) {
                return Err(entry.error(&format!("expected CLINT connection {expected}")));
            }
        }
        machine
            .bus
            .map_clint(entry.address()?, targets)
            .map_err(|e| entry.error(&e))?;
    }
    let plic_config = plic
        .map(|entry| {
            let mut outputs = [0; 2];
            let mut sources = None;
            let mut contexts = None;
            for (_, line) in &entry.body {
                if let Some((context, target)) = line.split_once("->") {
                    let context = number(context.trim()).map_err(|e| entry.error(&e))? as usize;
                    let (name, line) = target
                        .split_once('@')
                        .ok_or_else(|| entry.error("invalid PLIC connection"))?;
                    let line = number(line.trim()).map_err(|e| entry.error(&e))?;
                    if name.trim() != cpu.name
                        || !matches!(line, 9 | 11)
                        || context >= outputs.len()
                        || outputs[context] != 0
                        || outputs.contains(&(1 << line))
                    {
                        return Err(entry.error("invalid or duplicate PLIC connection"));
                    }
                    outputs[context] = 1 << line;
                } else if let Some((key, value)) = line.split_once(':') {
                    match key.trim() {
                        "numberOfSources" if sources.is_none() => {
                            sources = Some(number(value.trim()).map_err(|e| entry.error(&e))?)
                        }
                        "numberOfContexts" if contexts.is_none() => {
                            contexts = Some(number(value.trim()).map_err(|e| entry.error(&e))?)
                        }
                        _ => return Err(entry.error("unsupported PLIC property")),
                    }
                } else {
                    return Err(entry.error("unsupported PLIC wiring"));
                }
            }
            let count = contexts
                .filter(|count| (1..=2).contains(count))
                .ok_or_else(|| entry.error("PLIC requires one or two contexts"))?
                as usize;
            if outputs[..count].iter().all(|&line| line == 0)
                || outputs[count..].iter().any(|&line| line != 0)
                || cpu.model != "CPU.RiscV32"
            {
                return Err(entry.error("expected PLIC context outputs wired to the RiscV32 CPU"));
            }
            let sources = sources.ok_or_else(|| entry.error("missing PLIC numberOfSources"))?;
            machine
                .bus
                .map_plic(entry.address()?, sources, &outputs[..count], true)
                .map_err(|e| entry.error(&e))?;
            Ok((entry, sources))
        })
        .transpose()?;
    if let Some(entry) = uart {
        if entry.model == "UART.Murax_UART" {
            let connected = direct_irq(entry, &cpu.name, 11)?;
            machine
                .bus
                .map_uart(entry.address()?, connected)
                .map_err(|e| entry.error(&e))?;
        } else if entry.model == "UART.LiteX_UART" {
            keys(entry, &[])?;
            machine
                .bus
                .map_litex_uart(vec![entry.address()?], None)
                .map_err(|e| entry.error(&e))?;
        } else {
            let (plic, sources) =
                plic_config.ok_or_else(|| entry.error("NS16550 requires a PLIC"))?;
            let mut source = None;
            let mut wide = None;
            for (_, line) in &entry.body {
                if let Some(target) = line
                    .split_once("->")
                    .filter(|(pin, _)| pin.trim() == "IRQ")
                    .and_then(|(_, target)| target.trim().split_once('@'))
                    .filter(|(name, _)| name.trim() == plic.name)
                    && source.is_none()
                {
                    source = Some(number(target.1.trim()).map_err(|e| entry.error(&e))?);
                } else if let Some(("wideRegisters", value)) = line
                    .split_once(':')
                    .map(|(key, value)| (key.trim(), value.trim()))
                    && wide.is_none()
                {
                    wide = Some(
                        value
                            .parse::<bool>()
                            .map_err(|_| entry.error("invalid wideRegisters value"))?,
                    );
                } else {
                    return Err(entry.error("unsupported NS16550 property or wiring"));
                }
            }
            let source = source
                .filter(|source| (1..=sources).contains(source))
                .ok_or_else(|| entry.error("expected a valid NS16550 IRQ connection to PLIC"))?;
            machine
                .bus
                .map_named_ns16550(&entry.name, entry.address()?, Some(source), wide.unwrap_or(false))
                .map_err(|e| entry.error(&e))?;
        }
    }
    if let Some(entry) = timer {
        let connected = direct_irq(entry, &cpu.name, 7)?;
        machine
            .bus
            .map_murax_timer(entry.address()?, connected)
            .map_err(|e| entry.error(&e))?;
    }
    let vex_cpu = (cpu.model == "CPU.VexRiscv").then_some(cpu.name.as_str());
    crate::network_platform::configure(&entries, &mut machine, vex_cpu, None)?;
    Ok(machine)
}

fn configure_bus_init(entry: &Entry, machine: &mut Machine) -> Result<(), String> {
    let Some(((_, init), commands)) = entry.body.split_first() else {
        return Err(entry.error("missing sysbus init"));
    };
    if init != "init:" || entry.registration.is_some() {
        return Err(entry.error("expected sysbus init"));
    }
    for (_, command) in commands {
        if let Some(path) = command.strip_prefix("ApplySVD @")
            && !path.is_empty()
            && !path.chars().any(char::is_whitespace)
        {
            if path == "https://dl.antmicro.com/projects/renode/svd/STM32F40x.svd.gz" {
                map_f40x_otg_svd_defaults(machine).map_err(|e| entry.error(&e))?;
            }
            continue;
        }
        let (range, rest) = command
            .strip_prefix("Tag <")
            .and_then(|text| text.split_once('>'))
            .ok_or_else(|| entry.error("unsupported sysbus init command"))?;
        let (start, end) = range
            .split_once(',')
            .ok_or_else(|| entry.error("invalid Tag range"))?;
        let (_, value) = rest
            .trim()
            .strip_prefix('"')
            .and_then(|text| text.split_once('"'))
            .ok_or_else(|| entry.error("expected quoted Tag name"))?;
        let mut arguments = value.split_whitespace().collect::<Vec<_>>();
        if matches!(arguments.last(), Some(&"silent=True" | &"silent=False")) {
            arguments.pop();
        }
        let parse = |text: &str| number(text.trim()).map_err(|e| entry.error(&e));
        let value = match arguments.as_slice() {
            [] => 0,
            [value] => parse(value)?,
            _ => return Err(entry.error("invalid Tag value")),
        };
        machine
            .bus
            .map_bus_tag(parse(start)?, parse(end)?, value)
            .map_err(|e| entry.error(&e))?;
    }
    Ok(())
}

fn map_f40x_otg_svd_defaults(machine: &mut Machine) -> Result<(), String> {
    // Unmodeled OTG FS registers accessed during CubeMX host initialization.
    // The REPL supplies an explicit USB:RESET tag for the global reset window.
    const BASE: u32 = 0x5000_0000;
    for (offset, reset) in [
        (0x008, 0),          // GAHBCFG
        (0x00c, 0x00000a00), // GUSBCFG
        (0x100, 0x02000600), // HPTXFSIZ
        (0x400, 0),          // HCFG
        (0x408, 0x00003fff), // HFNUM
        (0x440, 0),          // HPRT
        (0xe00, 0),          // PCGCCTL
    ] {
        machine
            .bus
            .map_bus_tag(BASE + offset, BASE + offset + 3, reset)?;
    }
    for channel in 0..8 {
        let address = BASE + 0x508 + channel * 0x20; // HCINT and HCINTMSK
        machine.bus.map_bus_tag(address, address + 7, 0)?;
    }
    Ok(())
}

fn configure_f4_rcc(rcc: &Entry, entries: &[Entry], machine: &mut Machine) -> Result<(), String> {
    let mut rtc = None;
    let mut timer9 = None;
    for (_, text) in &rcc.body {
        if let Some(name) = text.strip_prefix("rtcPeripheral:")
            && rtc.is_none()
        {
            let target = entries
                .iter()
                .find(|target| {
                    target.name == name.trim() && target.model == stm32h7_rtc_platform::RTC
                })
                .ok_or_else(|| rcc.error("expected an STM32F4 RTC reference"))?;
            rtc = Some(target.address()?);
        } else if let Ok(("Timer9Reset", name, "255")) = crate::stm32h7_board_platform::wire(text)
            && timer9.is_none()
        {
            let target = entries
                .iter()
                .find(|target| {
                    target.name == name && target.model == crate::stm32h7_timer::Timer::MODEL
                })
                .ok_or_else(|| rcc.error("expected Timer9Reset connected to an STM32 timer"))?;
            timer9 = Some(target.address()?);
        } else {
            return Err(rcc.error("unsupported or duplicate STM32F4 RCC property or wiring"));
        }
    }
    machine
        .bus
        .map_f4_rcc(F4Rcc::new(rcc.address()?, rtc, timer9))
        .map_err(|error| rcc.error(&error))
}

fn configure_f1_i2c(
    entries: &[Entry],
    nvic: Option<&Entry>,
    machine: &mut Machine,
) -> Result<(), String> {
    let mut controllers = Vec::new();
    for entry in entries.iter().filter(|entry| entry.model == F1I2c::MODEL) {
        let mut i2c = F1I2c::new(entry.address()?);
        for (_, text) in &entry.body {
            let (output, target, input) =
                crate::stm32h7_board_platform::wire(text).map_err(|error| entry.error(error))?;
            let route = match output {
                "EventInterrupt" if i2c.event.is_none() => &mut i2c.event,
                "ErrorInterrupt" if i2c.error.is_none() => &mut i2c.error,
                _ => return Err(entry.error("unsupported or duplicate I2C interrupt output")),
            };
            if !nvic.is_some_and(|nvic| nvic.name == target) {
                return Err(entry.error("expected I2C interrupt connected to NVIC"));
            }
            *route = Some(Stm32Irq::Nvic(
                irq_index(input, IRQ_LINES as u32).map_err(|error| entry.error(&error))?,
            ));
        }
        controllers.push((entry, i2c));
    }
    for entry in entries.iter().filter(|entry| {
        matches!(
            entry.model.as_str(),
            F1I2c::ECHO_MODEL | F1I2c::HS3001_MODEL
        )
    }) {
        keys(entry, &[])?;
        let (parent, address) = entry
            .registration
            .as_deref()
            .and_then(|registration| registration.split_once(char::is_whitespace))
            .ok_or_else(|| entry.error("expected I2C controller and target address"))?;
        let (_, controller) = controllers
            .iter_mut()
            .find(|(entry, _)| entry.name == parent)
            .ok_or_else(|| entry.error("expected target attached to STM32F1 I2C"))?;
        let address = irq_index(address, 128).map_err(|error| entry.error(&error))?;
        let attached = if entry.model == F1I2c::HS3001_MODEL {
            controller.attach_hs3001(address)
        } else {
            controller.attach_echo(address)
        };
        if !attached {
            return Err(entry.error("duplicate I2C target address"));
        }
    }
    for (entry, controller) in controllers {
        machine
            .bus
            .map_f1_i2c(controller)
            .map_err(|error| entry.error(&error))?;
    }
    Ok(())
}

fn adc_registration(entry: &Entry) -> Result<(&str, &str), String> {
    entry
        .registration
        .as_deref()
        .and_then(|registration| registration.split_once(char::is_whitespace))
        .ok_or_else(|| entry.error("expected ADC source parent and channel"))
}

fn configure_f4_spi(
    entries: &[Entry],
    nvic: Option<&Entry>,
    machine: &mut Machine,
) -> Result<(), String> {
    use crate::{
        stm32h7_adc_dma::{AdcDma, StreamTarget},
        stm32h7_board_platform::{registration, wire},
    };

    let mut loopbacks = HashSet::new();
    for entry in entries
        .iter()
        .filter(|entry| entry.model == F4Spi::LOOPBACK_MODEL)
    {
        keys(entry, &[])?;
        let parent = entry
            .registration
            .as_deref()
            .filter(|name| {
                entries
                    .iter()
                    .any(|spi| spi.name == *name && spi.model == F4Spi::MODEL)
            })
            .ok_or_else(|| entry.error("expected loopback attached to STM32F4 SPI"))?;
        if !loopbacks.insert(parent) {
            return Err(entry.error("duplicate SPI loopback"));
        }
    }
    let mut inputs = HashMap::new();
    let mut wire_sources = HashMap::<_, BTreeMap<_, _>>::new();
    for source in entries
        .iter()
        .filter(|entry| entry.model == Ad4011::SOURCE_MODEL)
    {
        let (parent, channel) = adc_registration(source)?;
        let channel = number(channel).map_err(|error| source.error(&error))?;
        let target = entries
            .iter()
            .find(|entry| entry.name == parent)
            .ok_or_else(|| source.error("missing ADC source parent"))?;
        let mut voltage = 0;
        let mut floating = false;
        for (key, value) in keys(source, &["defaultValueMicroVolts", "IsFloating"])? {
            if key == "IsFloating" {
                floating = value
                    .parse()
                    .map_err(|_| source.error("expected IsFloating boolean"))?;
            } else {
                voltage = number(value).map_err(|error| source.error(&error))?;
            }
        }
        let sample = (!floating).then_some(voltage);
        match target.model.as_str() {
            Ad4011::WIRE_MODEL => {
                if wire_sources
                    .entry(parent)
                    .or_default()
                    .insert(channel, sample)
                    .is_some()
                {
                    return Err(source.error("duplicate analog wire input"));
                }
            }
            Ad4011::MUX_INPUT | Ad4011::MUX_OUTPUT
                if channel < 16 && (target.model == Ad4011::MUX_INPUT || channel == 0) =>
            {
                if wire_sources
                    .entry(parent)
                    .or_default()
                    .insert(channel, sample)
                    .is_some()
                {
                    return Err(source.error("duplicate analog multiplexer input"));
                }
            }
            Ad4011::MODEL if channel == 0 => {
                if inputs.insert(parent, voltage).is_some() {
                    return Err(source.error("duplicate AD4011 ADC channel 0"));
                }
            }
            _ => {
                return Err(
                    source.error("expected AD4011 channel 0, analog wire or multiplexer input")
                );
            }
        }
    }
    for wire in entries
        .iter()
        .filter(|entry| entry.model == Ad4011::WIRE_MODEL)
    {
        keys(wire, &[])?;
        let (parent, channel) = adc_registration(wire)?;
        if number(channel).ok() != Some(0)
            || !entries
                .iter()
                .any(|adc| adc.name == parent && adc.model == Ad4011::MODEL)
        {
            return Err(wire.error("expected analog wire attached to AD4011 channel 0"));
        }
        let voltage = wire_sources
            .get(wire.name.as_str())
            .and_then(|sources| sources.values().copied().flatten().next())
            .unwrap_or(0);
        if inputs.insert(parent, voltage).is_some() {
            return Err(wire.error("duplicate AD4011 ADC channel 0"));
        }
    }
    for mux in entries
        .iter()
        .filter(|entry| matches!(entry.model.as_str(), Ad4011::MUX_INPUT | Ad4011::MUX_OUTPUT))
    {
        let output_mux = mux.model == Ad4011::MUX_OUTPUT;
        let properties = if output_mux {
            &["selectedChannel", "enabled", "outputAdc", "outputChannel"][..]
        } else {
            &["selectedChannel", "enabled"][..]
        };
        let props: HashMap<_, _> = keys(mux, properties)?.into_iter().collect();
        let selected = number(props.get("selectedChannel").copied().unwrap_or("0"))
            .ok()
            .filter(|index| *index < 16)
            .ok_or_else(|| mux.error("expected multiplexer channel 0 through 15"))?;
        let enabled = props
            .get("enabled")
            .copied()
            .unwrap_or("false")
            .parse::<bool>()
            .map_err(|_| mux.error("expected multiplexer enabled boolean"))?;
        let (adc, input_channel) = if output_mux {
            if mux.registration.as_deref() != Some("sysbus") {
                return Err(mux.error("expected output multiplexer on sysbus"));
            }
            let Some(adc) = props.get("outputAdc").copied() else {
                if props.contains_key("outputChannel") {
                    return Err(mux.error("outputChannel requires outputAdc"));
                }
                continue;
            };
            let channel = props
                .get("outputChannel")
                .and_then(|channel| number(channel).ok())
                .filter(|index| *index < 16)
                .ok_or_else(|| mux.error("expected outputChannel from 0 through 15"))?;
            (adc, (enabled && selected == channel).then_some(0))
        } else {
            let (adc, channel) = adc_registration(mux)?;
            if number(channel).ok() != Some(0) {
                return Err(mux.error("expected AD4011 ADC channel 0"));
            }
            (adc, enabled.then_some(selected))
        };
        if !entries
            .iter()
            .any(|entry| entry.name == adc && entry.model == Ad4011::MODEL)
        {
            return Err(mux.error("expected output connected to AD4011"));
        }
        let voltage = input_channel
            .and_then(|channel| {
                wire_sources
                    .get(mux.name.as_str())?
                    .get(&channel)
                    .copied()
                    .flatten()
            })
            .unwrap_or(0);
        if inputs.insert(adc, voltage).is_some() {
            return Err(mux.error("duplicate AD4011 ADC channel 0"));
        }
    }
    let mut converters = HashMap::new();
    for adc in entries.iter().filter(|entry| entry.model == Ad4011::MODEL) {
        let parent = adc
            .registration
            .as_deref()
            .filter(|name| {
                entries
                    .iter()
                    .any(|spi| spi.name == *name && spi.model == F4Spi::MODEL)
            })
            .ok_or_else(|| adc.error("expected AD4011 attached to STM32F4 SPI"))?;
        let props = keys(adc, &["referenceVoltage"])?;
        let reference = props
            .first()
            .and_then(|(_, value)| value.parse::<f64>().ok())
            .filter(|volts| (2.4..=5.1).contains(volts))
            .ok_or_else(|| adc.error("expected referenceVoltage between 2.4 and 5.1 V"))?;
        let input = inputs.remove(adc.name.as_str()).unwrap_or(0);
        if loopbacks.contains(parent)
            || converters
                .insert(parent, Ad4011::new((reference * 1e6).round() as u32, input))
                .is_some()
        {
            return Err(adc.error("duplicate SPI target"));
        }
    }
    for entry in entries.iter().filter(|entry| entry.model == F4Spi::MODEL) {
        let mut spi = F4Spi::new(entry.address()?);
        spi.loopback = loopbacks.contains(entry.name.as_str());
        spi.adc = converters.remove(entry.name.as_str());
        let mut series = false;
        for (_, text) in &entry.body {
            if text.split_once(':').is_some_and(|(key, value)| {
                key.trim() == "series" && value.trim() == "STM32Series.F4"
            }) && !series
            {
                series = true;
                continue;
            }
            let (output, target, input) = wire(text).map_err(|error| entry.error(error))?;
            match output {
                "IRQ" if spi.irq.is_none() && nvic.is_some_and(|nvic| nvic.name == target) => {
                    let irq =
                        irq_index(input, IRQ_LINES as u32).map_err(|error| entry.error(&error))?;
                    spi.irq = Some(Stm32Irq::Nvic(irq));
                }
                "DMAReceive" if spi.dma.is_none() => {
                    let dma = entries
                        .iter()
                        .find(|dma| dma.name == target && dma.model == "DMA.STM32DMA")
                        .ok_or_else(|| {
                            entry.error("expected SPI receive DMA connected to STM32 DMA")
                        })?;
                    let (base, _) =
                        registration(dma, entries).map_err(|error| dma.error(&error))?;
                    let stream = irq_index(input, AdcDma::STREAMS as u32)
                        .map_err(|error| entry.error(&error))?;
                    spi.dma = Some(StreamTarget {
                        base,
                        stream: usize::from(stream),
                    });
                }
                _ => return Err(entry.error("unsupported or duplicate STM32F4 SPI output")),
            }
        }
        if !series {
            return Err(entry.error("expected series: STM32Series.F4"));
        }
        machine
            .bus
            .map_f4_spi(spi)
            .map_err(|error| entry.error(&error))?;
    }
    Ok(())
}

fn configure_stm32_can(
    entries: &[&Entry],
    nvic: Option<&Entry>,
    machine: &mut Machine,
) -> Result<(), String> {
    let mut devices = Vec::new();
    for entry in entries {
        let base = entry.address_with_optional_range(StmCan::SIZE)?;
        let mut master = None;
        let mut irq = [None; 4];
        for (_, text) in &entry.body {
            if let Some(name) = text.strip_prefix("master:")
                && master.is_none()
            {
                master = Some(name.trim());
                continue;
            }
            let Some((outputs, target)) = text.split_once("->") else {
                return Err(entry.error("expected CAN master or NVIC wiring"));
            };
            let Some((name, inputs)) = target.split_once('@') else {
                return Err(entry.error("missing CAN interrupt input"));
            };
            if !nvic.is_some_and(|nvic| nvic.name == name.trim()) {
                return Err(entry.error("expected CAN interrupt routed to NVIC"));
            }
            let outputs = irq_list(outputs, 4).map_err(|error| entry.error(&error))?;
            let inputs = irq_list(inputs, IRQ_LINES as u32).map_err(|error| entry.error(&error))?;
            if outputs.len() != inputs.len() {
                return Err(entry.error("mismatched CAN interrupt wiring widths"));
            }
            for (output, input) in outputs.into_iter().zip(inputs) {
                if irq[usize::from(output)].replace(input).is_some() {
                    return Err(entry.error("duplicate CAN interrupt output"));
                }
            }
        }
        devices.push((*entry, base, master, irq));
    }

    for &(entry, base, master, irq) in &devices {
        let master = master
            .map(|name| {
                devices
                    .iter()
                    .find(|(candidate, _, parent, _)| candidate.name == name && parent.is_none())
                    .map(|(_, address, _, _)| *address)
                    .ok_or_else(|| entry.error("expected CAN master without its own master"))
            })
            .transpose()?;
        machine
            .bus
            .map_f4_can(StmCan::new(base, master, irq))
            .map_err(|error| entry.error(&error))?;
    }
    Ok(())
}

fn configure_f4_flash(
    controller: &Entry,
    entries: &[Entry],
    machine: &mut Machine,
) -> Result<(), String> {
    let properties = keys(controller, &["flash"])?;
    let [(_, name)] = properties.as_slice() else {
        return Err(controller.error("expected a flash memory reference"));
    };
    let flash = entries
        .iter()
        .find(|entry| entry.name == *name && entry.model == "Memory.MappedMemory")
        .ok_or_else(|| controller.error("flash must reference mapped memory"))?;
    let start = flash.address()?;
    let end = start
        .checked_add(F4Flash::BANK_SIZE)
        .ok_or_else(|| flash.error("flash bank range overflow"))?;
    machine
        .bus
        .memory(start, F4Flash::BANK_SIZE as usize)
        .map_err(|error| flash.error(&error.to_string()))?;

    let (base, option_base, size) = f4_flash_registrations(controller)?;
    machine
        .bus
        .map_f4_flash(F4Flash::new(base, option_base, size, start..end))
        .map_err(|error| controller.error(&error))
}

fn f4_flash_registrations(controller: &Entry) -> Result<(u32, u32, u32), String> {
    let parse = || -> Option<(u32, u32, u32)> {
        let registration = controller
            .registration
            .as_deref()?
            .strip_prefix('{')?
            .strip_suffix('}')?
            .trim();
        let (registers, options) = registration.split_once(';')?;
        let base = number(registers.strip_prefix("sysbus ")?.trim()).ok()?;
        let options = options
            .trim()
            .strip_prefix("sysbus new Bus.BusMultiRegistration")?
            .trim()
            .strip_prefix('{')?
            .strip_suffix('}')?;
        let mut option_base = None;
        let mut size = None;
        let mut region = false;
        for field in options.split(';') {
            let (key, value) = field.trim().split_once(':')?;
            let value = value.trim();
            match key {
                "address" if option_base.is_none() => option_base = Some(number(value).ok()?),
                "size" if size.is_none() => size = Some(number(value).ok()?),
                "region" if !region && value == r#""optionBytes""# => region = true,
                _ => return None,
            }
        }
        let size = size.filter(|size| *size >= 12 && size % 4 == 0)?;
        region.then_some((base, option_base?, size))
    };
    parse().ok_or_else(|| controller.error("expected register and optionBytes sysbus registrations"))
}


fn configure_h7_flash(
    flash: &Entry,
    entries: &[Entry],
    nvic: Option<&Entry>,
    machine: &mut Machine,
) -> Result<(), String> {
    let mut banks = [None, None];
    let mut irq = None;
    for (_, text) in &flash.body {
        if let Some((pin, target)) = text.split_once("->")
            && pin.trim().is_empty()
            && let Some((name, line)) = target.split_once('@')
            && nvic.is_some_and(|nvic| nvic.name == name.trim())
            && irq.is_none()
        {
            irq = Some(irq_index(line, IRQ_LINES as u32).map_err(|error| flash.error(&error))?);
            continue;
        }
        let (key, name) = text
            .split_once(':')
            .ok_or_else(|| flash.error("invalid flash controller property or IRQ"))?;
        let bank = match key.trim() {
            "flash1" if banks[0].is_none() => &mut banks[0],
            "flash2" if banks[1].is_none() => &mut banks[1],
            _ => return Err(flash.error("unsupported or duplicate flash bank")),
        };
        let entry = entries
            .iter()
            .find(|entry| entry.name == name.trim() && entry.model == "Memory.MappedMemory")
            .ok_or_else(|| flash.error("flash bank must reference mapped memory"))?;
        let start = entry.address()?;
        let (_, size) = keys(entry, &["size"])?
            .into_iter()
            .next()
            .ok_or_else(|| entry.error("missing memory size"))?;
        let size = number(size).map_err(|error| entry.error(&error))?;
        let end = start
            .checked_add(size)
            .ok_or_else(|| entry.error("flash bank range overflow"))?;
        *bank = Some(start..end);
    }
    let [Some(first), Some(second)] = banks else {
        return Err(flash.error("expected two flash banks"));
    };
    if first == second {
        return Err(flash.error("flash banks must reference distinct memory regions"));
    }
    let irq = irq.ok_or_else(|| flash.error("expected flash interrupt connected to NVIC"))?;
    machine
        .bus
        .map_h7_flash(FlashController::new(flash.address()?, irq, [first, second]))
        .map_err(|error| flash.error(&error))
}

fn exti_outputs(
    controller: &Entry,
    nvic: Option<&Entry>,
    combined_routes: &HashMap<&str, (u32, u8)>,
    outputs: &mut [Option<u8>],
) -> Result<(), String> {
    for (_, text) in &controller.body {
        let Some((from, to)) = text.split_once("->") else {
            continue;
        };
        let (target, pins) = to
            .split_once('@')
            .ok_or_else(|| controller.error("invalid EXTI output"))?;
        let direct_nvic = nvic.is_some_and(|nvic| nvic.name == target.trim());
        let combined = combined_routes.get(target.trim());
        let pin_count = if direct_nvic {
            IRQ_LINES as u32
        } else if let Some((count, _)) = combined {
            *count
        } else {
            return Err(controller.error("unknown EXTI output target"));
        };
        let from = irq_list(from, outputs.len() as u32).map_err(|e| controller.error(&e))?;
        let to = irq_list(pins, pin_count).map_err(|e| controller.error(&e))?;
        if from.len() != to.len() {
            return Err(controller.error("mismatched EXTI wiring widths"));
        }
        for (line, pin) in from.into_iter().zip(to) {
            let output = combined.map_or(pin, |&(_, output)| output);
            if outputs[usize::from(line)].replace(output).is_some() {
                return Err(controller.error("duplicate EXTI output"));
            }
        }
    }
    Ok(())
}

pub(crate) fn irq_index(value: &str, count: u32) -> Result<u8, String> {
    let index = number(value.trim())?;
    if index >= count || index > u32::from(u8::MAX) {
        return Err(format!("IRQ index {index} is out of range"));
    }
    Ok(index as u8)
}

pub(crate) fn irq_list(value: &str, count: u32) -> Result<Vec<u8>, String> {
    let value = value.trim();
    let Some(list) = value
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
    else {
        return irq_index(value, count).map(|index| vec![index]);
    };
    let mut indices = Vec::new();
    for item in list.split(',') {
        if let Some((start, end)) = item.split_once('-') {
            let start = irq_index(start, count)?;
            let end = irq_index(end, count)?;
            if start > end {
                return Err("descending IRQ range".into());
            }
            indices.extend(start..=end);
        } else {
            indices.push(irq_index(item, count)?);
        }
    }
    Ok(indices)
}

fn direct_irq(entry: &Entry, cpu: &str, line: u32) -> Result<bool, String> {
    let expected = format!("IRQ->{cpu}@{line}");
    match entry.body.as_slice() {
        [] => Ok(false),
        [(_, wiring)]
            if wiring
                .chars()
                .filter(|c| !c.is_whitespace())
                .eq(expected.chars()) =>
        {
            Ok(true)
        }
        _ => Err(entry.error(&format!("expected direct interrupt wiring {expected}"))),
    }
}

fn memory_init(command: &str, base: u32) -> Result<(u32, usize, u32), String> {
    const RANDOM_WORD: &str = r#"`py "print emulationManager.Instance.CurrentEmulation.RandomGenerator.NextUlong() & 0xffffffff"`"#;
    let (operation, arguments) = command
        .split_once(char::is_whitespace)
        .ok_or("expected a memory init write")?;
    let width = match operation {
        "WriteWord" => 2,
        "WriteDoubleWord" => 4,
        _ => return Err(format!("unsupported memory init command {operation:?}")),
    };
    let (offset, value) = arguments
        .trim_start()
        .split_once(char::is_whitespace)
        .ok_or("expected a memory init offset and value")?;
    let offset = number(offset)?;
    let value = match value.trim() {
        RANDOM_WORD if width == 4 => {
            // Mix the destination into a stable simulated ID without a Python runtime.
            let mut word = (u64::from(base) << 32) | u64::from(offset);
            word = word.wrapping_add(0x9e37_79b9_7f4a_7c15);
            word = (word ^ (word >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            word = (word ^ (word >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            (word ^ (word >> 31)) as u32
        }
        value => number(value)?,
    };
    if width == 2 && value > u32::from(u16::MAX) {
        return Err("memory init value exceeds 16 bits".into());
    }
    Ok((offset, width, value))
}

fn keys<'a>(entry: &'a Entry, allowed: &[&str]) -> Result<Vec<(&'a str, &'a str)>, String> {
    let mut seen = HashSet::new();
    entry
        .body
        .iter()
        .map(|(line, text)| {
            let pair = text
                .split_once(':')
                .map(|(key, value)| (key.trim(), value.trim()));
            pair.filter(|(key, _)| allowed.contains(key) && seen.insert(*key))
                .ok_or_else(|| format!("{line}: unknown or duplicate property {text:?}"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_overlays_merge_includes_and_keep_duplicate_diagnostics() {
        let directory = std::env::temp_dir().join(format!("penv-overlay-{}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        let base = directory.join("base.repl");
        let overlay = directory.join("overlay.repl");
        fs::write(
            &base,
            r#"cpu: CPU.CortexM @ sysbus
    cpuType: "cortex-m7"
nvic: IRQControllers.NVIC @ sysbus 0xe000e000
    -> cpu@0
exti: IRQControllers.STM32H7_EXTI @ sysbus 0x58000000
    0 -> nvic@6
ram: Memory.MappedMemory @ sysbus 0x20000000
    size: 4
"#,
        )
        .unwrap();
        let valid = r#"using "base.repl"
exti:
    1 -> nvic@7
ram: { size: 16 }
ram:
    size: 32
"#;
        fs::write(&overlay, valid).unwrap();
        let mut machine = load(&overlay, None).unwrap();
        assert_eq!(machine.bus.read_ram::<4>(0x2000_001c).unwrap(), 0);
        assert!(machine.bus.read_ram::<4>(0x2000_0020).is_err());
        machine
            .bus
            .write(0x5800_0080, 4, 3, &mut std::io::sink())
            .unwrap();
        machine
            .bus
            .write(0x5800_0008, 4, 3, &mut std::io::sink())
            .unwrap();
        machine
            .bus
            .sample_nvic(&crate::nvic::ActiveExceptions::default(), false);
        assert_eq!(
            machine.bus.read::<4>(0xe000_e200).unwrap(),
            (1 << 6) | (1 << 7)
        );

        for (addition, expected) in [
            (
                "ram: Memory.MappedMemory @ sysbus 0x20001000\n    size: 16\n",
                "duplicate declarations and overrides are unsupported",
            ),
            (
                "missing:\n    size: 4\n",
                "REPL overlay target has no earlier declaration",
            ),
            (
                "nvic:\n    systickFrequency: 1\n    systickFrequency: 2\n",
                "duplicate REPL property \"systickFrequency\"",
            ),
        ] {
            fs::write(&overlay, format!("{valid}{addition}")).unwrap();
            let error = load(&overlay, None).err().unwrap();
            assert!(error.contains(expected), "{error}");
        }
        fs::remove_file(&base).unwrap();
        fs::remove_file(&overlay).unwrap();
        fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn memory_init_loads_system_words_and_rejects_cross_region_writes() {
        let path =
            std::env::temp_dir().join(format!("penv-memory-init-{}.repl", std::process::id()));
        let repl = r#"cpu: CPU.CortexM @ sysbus
    cpuType: "cortex-m0"
system_flash_ro: Memory.MappedMemory @ sysbus 0x1ff00000
    size: 0x20000
    init:
        WriteDoubleWord 0x1e800 `py "print emulationManager.Instance.CurrentEmulation.RandomGenerator.NextUlong() & 0xffffffff"`
        WriteDoubleWord 0x1e804 `py "print emulationManager.Instance.CurrentEmulation.RandomGenerator.NextUlong() & 0xffffffff"`
        WriteWord 0x1e880 0x800
"#;
        fs::write(&path, repl).unwrap();
        let first = load(&path, None).unwrap();
        let second = load(&path, None).unwrap();
        let uid = |machine: &Machine, offset: u32| {
            machine.bus.read_ram::<4>(0x1ff0_0000 + offset).unwrap()
        };
        assert_eq!(uid(&first, 0x1e800), uid(&second, 0x1e800));
        assert_ne!(uid(&first, 0x1e800), uid(&first, 0x1e804));
        assert_eq!(first.bus.read_ram::<2>(0x1ff1_e880).unwrap(), 0x800);
        assert_eq!(first.bus.read_ram::<2>(0x1ff1_e882).unwrap(), 0);

        fs::write(&path, repl.replace("0x1e880 0x800", "0x1ffff 0x800")).unwrap();
        let result = load(&path, None);
        fs::remove_file(&path).unwrap();
        assert!(
            result
                .err()
                .unwrap()
                .contains("memory init write exceeds the declared region")
        );
        assert!(memory_init("WriteWord 0 0x10000", 0).is_err());
        assert!(memory_init("WriteDoubleWord 0 `py \"print 1\"`", 0).is_err());
    }
}
