use crate::{
    machine::Machine,
    platform::{Entry, irq_index, irq_list, number},
    stm32h7_gpio::Stm32Gpio,
};

pub const GPIO: &str = "GPIOPort.STM32_GPIOPort";
const SYSCFG: &str = "Miscellaneous.STM32_SYSCFG";
const BUTTON: &str = "Miscellaneous.Button";

pub fn is_device(model: &str) -> bool {
    matches!(model, GPIO | BUTTON)
}

pub fn configure(
    entries: &[Entry],
    exti: Option<&Entry>,
    exti_lines: u32,
    machine: &mut Machine,
) -> Result<(), String> {
    let syscfg = entries.iter().find(|entry| entry.model == SYSCFG);
    let mut outputs = [None; 16];
    if let Some(syscfg) = syscfg {
        for (_, text) in &syscfg.body {
            let (pins, target) = text
                .split_once("->")
                .ok_or_else(|| syscfg.error("expected SYSCFG output wiring"))?;
            let (name, lines) = target
                .split_once('@')
                .ok_or_else(|| syscfg.error("missing SYSCFG output index"))?;
            if !exti.is_some_and(|exti| exti.name == name.trim()) {
                return Err(syscfg.error("expected SYSCFG output routed to EXTI"));
            }
            let from = irq_list(pins, 16).map_err(|e| syscfg.error(&e))?;
            let to = irq_list(lines, exti_lines).map_err(|e| syscfg.error(&e))?;
            if from.len() != to.len() {
                return Err(syscfg.error("mismatched SYSCFG wiring widths"));
            }
            for (from, to) in from.into_iter().zip(to) {
                if outputs[usize::from(from)].replace(to).is_some() {
                    return Err(syscfg.error("duplicate SYSCFG output"));
                }
            }
        }
    }
    machine.bus.set_gpio_exti_outputs(outputs);

    let mut ports = Vec::new();
    for entry in entries.iter().filter(|entry| entry.model == GPIO) {
        ports.push((
            entry.name.as_str(),
            port(entry, entries, syscfg).map_err(|e| entry.error(&e))?,
        ));
    }
    for entry in entries.iter().filter(|entry| entry.model == BUTTON) {
        let (name, pin) = entry
            .registration
            .as_deref()
            .and_then(|text| text.split_once(char::is_whitespace))
            .ok_or_else(|| entry.error("expected button registered on a GPIO pin"))?;
        let pin = irq_index(pin, 16).map_err(|e| entry.error(&e))?;
        let (_, port) = ports
            .iter_mut()
            .find(|(parent, _)| *parent == name)
            .ok_or_else(|| entry.error("unknown button GPIO port"))?;
        let mut invert = false;
        for (_, text) in &entry.body {
            if let Some(value) = text.strip_prefix("invert:") {
                invert = value
                    .trim()
                    .parse()
                    .map_err(|_| entry.error("invalid button inversion"))?;
            } else if text
                .strip_prefix("->")
                .and_then(|wire| wire.split_once('@'))
                .is_none_or(|(target, line)| {
                    target.trim() != name || irq_index(line, 16) != Ok(pin)
                })
            {
                return Err(entry.error("expected button wired to its GPIO pin"));
            }
        }
        port.set_external(pin, invert);
    }

    for (_, port) in ports {
        machine.bus.map_h7_gpio(port)?;
    }
    Ok(())
}

fn port(entry: &Entry, entries: &[Entry], syscfg: Option<&Entry>) -> Result<Stm32Gpio, String> {
    let (base, size, rcc_pin) = registration(entry, entries)?;
    let mut resets = [0; 3];
    let mut properties = [false; 4];
    let mut alternate_count = 16;
    let mut inverted = None;
    for (_, text) in &entry.body {
        if let Some((key, value)) = text.split_once(':') {
            let value = value.trim();
            let index = match key.trim() {
                "modeResetValue" => 0,
                "outputSpeedResetValue" => 1,
                "pullUpPullDownResetValue" => 2,
                "numberOfAFs" => 3,
                "invertedAFPins" if inverted.is_none() => {
                    inverted = Some(value);
                    continue;
                }
                _ => return Err("unsupported or duplicate GPIO property".into()),
            };
            if std::mem::replace(&mut properties[index], true) {
                return Err("duplicate GPIO property".into());
            }
            let value = number(value)?;
            if index < resets.len() {
                resets[index] = value;
            } else {
                alternate_count = value;
            }
        }
    }
    if !(1..=16).contains(&alternate_count) {
        return Err("GPIO alternate-function count must be in 1–16".into());
    }
    if let Some(inverted) = inverted {
        let groups = inverted
            .strip_prefix("[[")
            .and_then(|text| text.strip_suffix("]]"))
            .ok_or("expected GPIO inverted AF pin lists")?;
        for group in groups.split("],") {
            let mut values = group.trim().trim_start_matches('[').split(',');
            irq_index(values.next().ok_or("missing inverted AF pin")?, 16)?;
            for value in values {
                irq_index(value, alternate_count)?;
            }
        }
    }
    let mut port = Stm32Gpio::new(base, size, resets, alternate_count);
    port.rcc_pin = rcc_pin;
    for (_, text) in &entry.body {
        if text.contains(':') {
            continue;
        }
        let (pins, target) = text.split_once("->").ok_or("expected GPIO output wiring")?;
        let (target, inputs) = target
            .split_once('@')
            .ok_or("missing GPIO destination pins")?;
        let (name, selector) = target
            .trim()
            .split_once('#')
            .ok_or("missing SYSCFG port selector")?;
        if !syscfg.is_some_and(|syscfg| syscfg.name == name) {
            return Err("expected GPIO routed through SYSCFG".into());
        }
        let selector = irq_index(selector, 16)?;
        if port.syscfg_port.is_some_and(|port| port != selector) {
            return Err("GPIO must use one SYSCFG port selector".into());
        }
        port.syscfg_port = Some(selector);
        let pins = irq_list(pins, 16)?;
        let inputs = irq_list(inputs, 16)?;
        if pins.len() != inputs.len() {
            return Err("mismatched GPIO wiring widths".into());
        }
        for (pin, input) in pins.into_iter().zip(inputs) {
            if port.syscfg_inputs[usize::from(pin)]
                .replace(input)
                .is_some()
            {
                return Err("duplicate GPIO output".into());
            }
        }
    }
    Ok(port)
}

fn registration(entry: &Entry, entries: &[Entry]) -> Result<(u32, u32, Option<u8>), String> {
    let text = entry
        .registration
        .as_deref()
        .ok_or("missing GPIO registration")?;
    let list = text
        .strip_prefix('{')
        .and_then(|text| text.strip_suffix('}'))
        .unwrap_or(text);
    let mut mapping = None;
    let mut alias = None;
    for registration in list
        .split(';')
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        if let Some(bus) = registration.strip_prefix("sysbus ")
            && mapping.is_none()
        {
            let (base, size) = if let Some(range) = bus
                .trim()
                .strip_prefix('<')
                .and_then(|text| text.strip_suffix('>'))
            {
                range
                    .split_once(',')
                    .ok_or("expected GPIO address and range size")?
            } else {
                (bus.trim(), "0x400")
            };
            let size = number(size.trim().trim_start_matches('+'))?;
            if size < 0x2c {
                return Err("GPIO range is too small for its registers".into());
            }
            mapping = Some((number(base.trim())?, size));
        } else if let Some((parent, target)) = registration.split_once(char::is_whitespace)
            && alias.is_none()
            && entries
                .iter()
                .any(|entry| entry.name == parent && entry.model == "Miscellaneous.STM32H7_RCC")
        {
            let index = target
                .trim()
                .strip_prefix("RegistrationPeripheralAlias.GPIO")
                .and_then(|pin| match pin.as_bytes() {
                    [pin @ b'A'..=b'K'] => Some(pin - b'A'),
                    _ => None,
                })
                .ok_or("invalid GPIO RCC alias")?;
            alias = Some(index);
        } else {
            return Err("invalid GPIO registration".into());
        }
    }
    let (base, size) = mapping.ok_or("missing GPIO bus range")?;
    Ok((base, size, alias))
}
