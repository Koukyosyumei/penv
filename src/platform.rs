use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

use crate::{
    constant_mmio::ConstantMmio,
    machine::{MAX_HARTS, Machine},
    nvic::IRQ_LINES,
    stm32_can::StmCan,
    stm32_uart::{Stm32Irq, UsartLayout},
    stm32f4_flash::F4Flash,
    stm32f4_power::F4Power,
    stm32f4_spi::F4Spi,
    stm32h7_adc_platform,
    stm32h7_exti::H7Exti,
    stm32h7_flash::FlashController,
    stm32h7_gpio_platform, stm32h7_rtc_platform,
};

type Properties = Vec<(usize, String)>;

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

fn inline_body(declaration: &str, lineno: usize) -> Result<(&str, Properties), &'static str> {
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
            body.push((lineno, item.into()));
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
    let mut current: Option<Entry> = None;
    let mut registration_depth = 0;
    let mut lines = content.lines().enumerate();
    while let Some((index, raw)) = lines.next() {
        let line = without_comment(raw);
        let text = line.trim();
        let lineno = index + 1;
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
            entry.body.push((lineno, property));
            continue;
        }
        if line.starts_with(char::is_whitespace) {
            current
                .as_mut()
                .ok_or_else(|| error("property has no declaration"))?
                .body
                .push((lineno, text.into()));
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
        let (declaration, body) = inline_body(declaration, lineno).map_err(error)?;
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

pub fn load(path: &Path, root: Option<&Path>) -> Result<Machine, String> {
    let mut entries = Vec::new();
    parse(
        path,
        root.unwrap_or(Path::new(".")),
        &mut HashSet::new(),
        &mut entries,
    )?;
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
    let mut stm32_can = Vec::new();
    let mut plic = None;
    let mut timer = None;
    for entry in &entries {
        if !names.insert(&entry.name) {
            return Err(entry.error("duplicate declarations and overrides are unsupported"));
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
                let (_, size) = keys(entry, &["size"])?
                    .into_iter()
                    .next()
                    .ok_or_else(|| entry.error("missing memory size"))?;
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
        let (thumb2, default_mpu_regions, has_cache) = match cpu_type {
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
            let base = uart.address_with_optional_range(layout.size())?;
            machine
                .bus
                .map_stm32_usart(base, irq, layout)
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
                .configure_mpu(mpu_regions.unwrap_or(default_mpu_regions), has_cache);
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
    for entry in entries.iter().filter(|entry| entry.model == F4Spi::MODEL) {
        let mut spi = F4Spi::new(entry.address()?);
        spi.loopback = loopbacks.contains(entry.name.as_str());
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
            .map_stm32_can(StmCan::new(base, master, irq))
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
    let (base, option_base, size) = parse().ok_or_else(|| {
        controller.error("expected register and optionBytes sysbus registrations")
    })?;
    machine
        .bus
        .map_f4_flash(F4Flash::new(base, option_base, size, start..end))
        .map_err(|error| controller.error(&error))
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
                .ok_or_else(|| {
                    format!(
                        "{}:{line}: unknown or duplicate property {text:?}",
                        entry.source.display()
                    )
                })
        })
        .collect()
}
