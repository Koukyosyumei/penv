use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

use crate::{
    machine::{MAX_HARTS, Machine},
    stm32_uart::Stm32Irq,
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

    fn sysbus(&self) -> Result<Option<u32>, String> {
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

    pub(crate) fn address(&self) -> Result<u32, String> {
        self.sysbus()?
            .ok_or_else(|| self.error("expected a sysbus address"))
    }
}

fn number(value: &str) -> Result<u32, String> {
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
    let mut multiline_script = false;
    let mut registration_depth: i32 = 0;
    for (index, raw) in content.lines().enumerate() {
        if multiline_script {
            if raw.contains("'''") {
                multiline_script = false;
            }
            continue;
        }
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
                return Err(error("invalid registration braces"));
            }
            continue;
        }
        if text
            .strip_prefix("script:")
            .is_some_and(|value| value.trim() == "'''")
        {
            multiline_script = true;
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
            Some((model, registration)) => (model.trim(), Some(registration.trim().into())),
            None => (declaration.trim(), None),
        };
        registration_depth = registration.as_deref().map_or(0, brace_depth);
        if registration_depth < 0 {
            return Err(error("invalid registration braces"));
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
    if multiline_script || registration_depth != 0 {
        return Err(format!(
            "{}: unterminated script or registration",
            path.display()
        ));
    }
    entries.extend(current);
    visiting.remove(&canonical);
    Ok(())
}

fn brace_depth(value: &str) -> i32 {
    unquoted_delimiters(value, &['{', '}'])
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
    if entries.iter().any(|entry| entry.model == "CPU.RiscV64") {
        return crate::rv64_platform::load(
            entries,
            path.file_name().and_then(|name| name.to_str()),
        );
    }
    let mut machine = Machine::without_default_ram();
    let mut names = HashSet::new();
    let mut cpus = Vec::new();
    let mut clint = None;
    let mut nvic = None;
    let mut exti = None;
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
        match entry.model.as_str() {
            "CPU.RiscV32" | "CPU.VexRiscv" | "CPU.CortexM" => {
                if entry.sysbus()?.is_some() {
                    return Err(entry.error("expected CPU registered on sysbus without an address"));
                }
                cpus.push(entry);
            }
            "Memory.MappedMemory" | "Memory.ArrayMemory" => {
                let (_, size) = keys(entry, &["size"])?
                    .into_iter()
                    .next()
                    .ok_or_else(|| entry.error("missing memory size"))?;
                machine
                    .bus
                    .map_ram(
                        entry.address()?,
                        number(size).map_err(|e| entry.error(&e))? as usize,
                    )
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
            "UART.STM32F7_USART" => stm32_uarts.push(entry),
            "UART.Murax_UART" | "UART.NS16550" | "UART.LiteX_UART" => {
                if uart.replace(entry).is_some() {
                    return Err(entry.error("only one UART is currently supported"));
                }
            }
            "IRQControllers.STM32F4_EXTI" => {
                if exti.replace(entry).is_some() {
                    return Err(entry.error("only one STM32 EXTI is supported"));
                }
            }
            "Miscellaneous.CombinedInput" => combiners.push(entry),
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
        let mut vectors = None;
        for (key, value) in keys(
            entry,
            &[
                "cpuType",
                "vectorTableOffset",
                "InitVectorTableOffset",
                "nvic",
            ],
        )? {
            match key {
                "cpuType" => cpu_type = Some(value),
                "nvic" if nvic.is_some_and(|controller| controller.name == value) => {}
                "nvic" => return Err(entry.error("unknown NVIC reference")),
                "vectorTableOffset" | "InitVectorTableOffset" if vectors.is_none() => {
                    vectors = Some(number(value).map_err(|e| entry.error(&e))?);
                }
                "vectorTableOffset" | "InitVectorTableOffset" => {
                    return Err(entry.error("duplicate vector table offset"));
                }
                _ => unreachable!(),
            }
        }
        if !matches!(cpu_type, Some(r#""cortex-m0""# | r#""cortex-m0+""#)) {
            return Err(entry.error("expected Cortex-M0 or Cortex-M0+ cpuType"));
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
                if irq >= 32 {
                    return Err(uart.error("NVIC IRQ must be in 0–31"));
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
                    output = Some(irq_index(index, 32).map_err(|e| combiner.error(&e))?);
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
                    32
                } else if let Some((count, _)) = combined {
                    *count
                } else {
                    return Err(controller.error("unknown EXTI output target"));
                };
                let from = irq_list(from, count).map_err(|e| controller.error(&e))?;
                let to = irq_list(pins, pin_count).map_err(|e| controller.error(&e))?;
                if from.len() != to.len() {
                    return Err(controller.error("mismatched EXTI wiring widths"));
                }
                for (line, pin) in from.into_iter().zip(to) {
                    let output = if direct_nvic {
                        pin
                    } else {
                        combined.unwrap().1
                    };
                    if outputs[usize::from(line)].replace(output).is_some() {
                        return Err(controller.error("duplicate EXTI output"));
                    }
                }
            }
            machine
                .bus
                .map_exti(controller.address()?, outputs, direct)
                .map_err(|e| controller.error(&e))?;
            count
        } else {
            0
        };
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
                        Stm32Irq::Nvic(irq_index(line, 32).map_err(|e| uart.error(&e))?)
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
        machine.primary.configure_cortex_m(vectors);
        return Ok(machine);
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
    Ok(machine)
}

fn irq_index(value: &str, count: u32) -> Result<u8, String> {
    let index = number(value.trim())?;
    if index >= count || index > u32::from(u8::MAX) {
        return Err(format!("IRQ index {index} is out of range"));
    }
    Ok(index as u8)
}

fn irq_list(value: &str, count: u32) -> Result<Vec<u8>, String> {
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
