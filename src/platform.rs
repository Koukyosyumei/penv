use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    rc::Rc,
};

use crate::{
    constant_mmio::ConstantMmio,
    machine::{MAX_HARTS, Machine},
    nvic::IRQ_LINES,
    stm32_uart::Stm32Irq,
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

    fn address(&self) -> Result<u32, String> {
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

fn without_comment(line: &str) -> &str {
    unquoted_delimiters(line, &['/'])
        .find(|(index, _)| line[*index..].starts_with("//"))
        .map_or(line, |(index, _)| &line[..index])
}

fn unquoted_delimiters<'a>(
    text: &'a str,
    delimiters: &'static [char],
) -> impl Iterator<Item = (usize, char)> + 'a {
    let mut quoted = false;
    let mut escaped = false;
    text.char_indices().filter(move |&(_, character)| {
        match character {
            _ if escaped => escaped = false,
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            _ if !quoted && delimiters.contains(&character) => return true,
            _ => {}
        }
        false
    })
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
            let filename = include
                .trim()
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .filter(|s| !s.is_empty() && !s.contains('"'))
                .ok_or_else(|| error("expected using \"path.repl\""))?;
            let local = canonical.parent().unwrap().join(filename);
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
        return Err(current.unwrap().error("unterminated Ethernet registration"));
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

fn property_key(text: &str) -> Option<&str> {
    let (key, value) = text.split_once(':')?;
    let key = key.trim();
    (!value.trim().is_empty()
        && !key.is_empty()
        && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
    .then_some(key)
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
            // Replace a property from an earlier entry. Preserve duplicates within an
            // entry for the individual peripheral's existing validation.
            let mut replaced = HashSet::new();
            for (_, text) in &entry.body {
                if let Some(key) = property_key(text)
                    && replaced.insert(key)
                {
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
    let mut entries = Vec::new();
    parse(
        path,
        root.unwrap_or(Path::new(".")),
        &mut HashSet::new(),
        &mut entries,
    )?;
    let entries = apply_overlays(entries);
    let mut machine = Machine::without_default_ram();
    let mut names = HashSet::new();
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
    let mut plic = None;
    let mut timer = None;
    for entry in &entries {
        if !names.insert(&entry.name) {
            return Err(entry.error("duplicate declarations and overrides are unsupported"));
        }
        if entry.model.is_empty() {
            let detail = if entry.name == "sysbus" {
                "built-in sysbus overrides are unsupported"
            } else if entry.registration.is_some() {
                "REPL registration overrides are unsupported"
            } else {
                "REPL overlay target has no earlier declaration"
            };
            return Err(entry.error(detail));
        }
        match entry.model.as_str() {
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
                machine
                    .bus
                    .map_ram(address, size as usize)
                    .map_err(|e| entry.error(&e))?;
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
            "UART.STM32F7_USART" => stm32_uarts.push(entry),
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
            "MTD.STM32H7_FlashController" => {
                if flash_controller.replace(entry).is_some() {
                    return Err(entry.error("only one STM32H7 flash controller is supported"));
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
        let thumb2 = match cpu_type {
            Some(r#""cortex-m0""# | r#""cortex-m0+""#) => false,
            Some(r#""cortex-m7""#) => true,
            _ => return Err(entry.error("expected Cortex-M0, Cortex-M0+ or Cortex-M7 cpuType")),
        };
        if !thumb2 && mpu_regions.is_some() {
            return Err(entry.error("MPU regions require Cortex-M7"));
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
            configure_h7_flash(flash, &entries, nvic, &mut machine)?;
        }
        let mut uart_irq = None;
        if let Some(uart) = uart {
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
                .map_litex_uart(uart.address()?)
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
                if outputs.iter().flatten().any(|&line| line >= 32) {
                    return Err(controller.error("STM32F4 EXTI outputs require NVIC lines 0–31"));
                }
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
                        "lowPowerMode" if low_power.is_none() => {
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
            machine
                .bus
                .map_stm32_usart(uart.address()?, irq, low_power.unwrap_or(false))
                .map_err(|e| uart.error(&e))?;
        }
        machine
            .bus
            .map_nvic(priority.unwrap_or(0xc0), uart_irq)
            .map_err(|e| entry.error(&e))?;
        machine.primary.name = Some(entry.name.clone());
        machine.primary.configure_cortex_m(vectors, thumb2);
        if thumb2 {
            machine
                .bus
                .nvic
                .as_mut()
                .unwrap()
                .configure_mpu(mpu_regions.unwrap_or(16));
        }
        crate::network_platform::configure(
            &entries,
            &mut machine,
            None,
            nvic.map(|entry| entry.name.as_str()),
        )?;
        return Ok(machine);
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
        return Err(flash.error("STM32H7 flash controller requires a Cortex-M CPU"));
    }
    if let Some(dwt) = dwt {
        return Err(dwt.error("DWT requires a Cortex-M CPU"));
    }
    if let Some(nvic) = nvic {
        return Err(nvic.error("NVIC requires a Cortex-M CPU"));
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
            hart.enable_vex_irq_controller();
        }
        hart.enable_user_mode();
        hart.enable_supervisor_mode();
        if pmp {
            hart.enable_pmp();
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
                .map_plic(entry.address()?, sources, &outputs[..count])
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
                .map_litex_uart(entry.address()?)
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
                .map_ns16550(entry.address()?, source, wide.unwrap_or(false))
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
            .sample_nvic(&crate::nvic::ActiveExceptions::default());
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
                "unsupported or duplicate NVIC property",
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
