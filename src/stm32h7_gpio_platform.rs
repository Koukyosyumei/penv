use crate::{
    machine::Machine,
    platform::{Entry, irq_index, irq_list, number},
    stm32h7_gpio::Stm32Gpio,
};

pub const GPIO: &str = "GPIOPort.STM32_GPIOPort";
const SYSCFG: &str = "Miscellaneous.STM32_SYSCFG";
const PWM_LED: &str = "Miscellaneous.PWMLED";
const LED: &str = "Miscellaneous.LED";
const BUTTON: &str = "Miscellaneous.Button";

pub fn is_device(model: &str) -> bool {
    matches!(model, GPIO | BUTTON | LED | PWM_LED)
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
        let registration = entry
            .registration
            .as_deref()
            .ok_or_else(|| entry.error("expected button registered on a GPIO port"))?;
        let (name, pin) = if let Some((name, pin)) = registration.split_once(char::is_whitespace) {
            (name, irq_index(pin, 16).map_err(|e| entry.error(&e))?)
        } else {
            let mut wires = entry
                .body
                .iter()
                .filter_map(|(_, text)| text.strip_prefix("->"));
            let pin = wires
                .next()
                .and_then(|wire| wire.split_once('@'))
                .filter(|(target, _)| target.trim() == registration)
                .ok_or_else(|| entry.error("expected button wired to a pin on its GPIO port"))?;
            if wires.next().is_some() {
                return Err(entry.error("expected a single GPIO button wire"));
            }
            (
                registration,
                irq_index(pin.1, 16).map_err(|e| entry.error(&e))?,
            )
        };
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
        machine
            .bus
            .add_gpio_button(format!("{name}.{}", entry.name), port.base, pin, invert);
    }

    for led in entries
        .iter()
        .filter(|entry| matches!(entry.model.as_str(), LED | PWM_LED))
    {
        let (parent, pin) = led
            .registration
            .as_deref()
            .and_then(|registration| registration.split_once(char::is_whitespace))
            .ok_or_else(|| led.error("expected LED registered on a GPIO pin"))?;
        let pin = irq_index(pin, 16).map_err(|error| led.error(&error))?;
        let (_, port) = ports
            .iter()
            .find(|(name, _)| *name == parent)
            .ok_or_else(|| led.error("unknown LED GPIO port"))?;
        let inverted = match led.body.as_slice() {
            [] => false,
            [(_, property)] => property
                .strip_prefix("invert:")
                .and_then(|value| value.trim().parse().ok())
                .ok_or_else(|| led.error("expected LED invert property"))?,
            _ => return Err(led.error("unsupported LED properties")),
        };
        let wired = entries
            .iter()
            .find(|entry| entry.name == parent)
            .is_some_and(|entry| {
                entry.body.iter().any(|(_, wire)| {
                    wire.split_once("->").is_some_and(|(output, target)| {
                        irq_index(output, 16) == Ok(pin)
                            && target.trim().strip_suffix("@0") == Some(&led.name)
                    })
                })
            });
        if !wired {
            return Err(led.error("expected GPIO pin wired to LED input 0"));
        }
        if led.model == PWM_LED {
            let mut routes = [None; 16];
            for timer in entries
                .iter()
                .filter(|timer| timer.model == crate::stm32h7_timer::Timer::MODEL)
            {
                for (_, text) in &timer.body {
                    let Some((channel, targets)) = text.split_once("->") else {
                        continue;
                    };
                    let Ok(channel) = irq_index(channel, 4) else {
                        continue;
                    };
                    for target in targets.split('|') {
                        let Some((port_name, function)) = target.trim().split_once('#') else {
                            continue;
                        };
                        let Some((output_pin, alternate)) = function.split_once('@') else {
                            continue;
                        };
                        if port_name != parent || irq_index(output_pin, 16) != Ok(pin) {
                            continue;
                        }
                        let alternate = irq_index(alternate, 16).map_err(|e| timer.error(&e))?;
                        let (base, _) = crate::stm32h7_board_platform::registration(timer, entries)
                            .map_err(|e| timer.error(&e))?;
                        if routes[usize::from(alternate)]
                            .replace((base, channel))
                            .is_some()
                        {
                            return Err(led.error(
                                "multiple timers drive the same LED pin and alternate function",
                            ));
                        }
                    }
                }
            }
            machine
                .bus
                .add_gpio_pwm_led(led.name.clone(), port.base, pin, inverted, routes);
        } else {
            machine
                .bus
                .add_gpio_led(led.name.clone(), port.base, pin, inverted);
        }
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
        // These are ordinary GPIO outputs. The upstream alternate-output selector
        // is only used for the reverse timer-to-GPIO direction.
        if let Some(pin) = irq_index(pins, 16).ok().filter(|_| {
            target.split('|').all(|endpoint| {
                endpoint.split_once('@').is_some_and(|(name, _)| {
                    entries.iter().any(|timer| {
                        timer.name == name.trim()
                            && timer.model == crate::stm32h7_timer::Timer::MODEL
                    })
                })
            })
        }) {
            for endpoint in target.split('|') {
                let (name, channel) = endpoint
                    .split_once('@')
                    .ok_or("missing timer input channel")?;
                let timer = entries
                    .iter()
                    .find(|timer| timer.name == name.trim())
                    .ok_or("unknown GPIO timer target")?;
                let (base, _) = crate::stm32h7_board_platform::registration(timer, entries)?;
                port.connect_timer_input(pin, base, irq_index(channel, 4)?)?;
            }
            continue;
        }
        let (target, inputs) = target
            .split_once('@')
            .ok_or("missing GPIO destination pins")?;
        if let Some(led) = entries
            .iter()
            .find(|led| matches!(led.model.as_str(), LED | PWM_LED) && led.name == target.trim())
        {
            let expected = led
                .registration
                .as_deref()
                .and_then(|registration| registration.split_once(char::is_whitespace));
            if !matches!(expected, Some((parent, pin)) if parent == entry.name && irq_index(pin, 16) == irq_index(pins, 16) && irq_index(pins, 16).is_ok())
                || inputs.trim() != "0"
            {
                return Err("invalid GPIO LED connection".into());
            }
            continue;
        }
        let (name, selector) = crate::platform::irq_receiver(target)?;
        if !syscfg.is_some_and(|syscfg| syscfg.name == name) {
            return Err("expected GPIO routed through SYSCFG".into());
        }
        let selector = selector
            .filter(|&index| index < 16)
            .ok_or("expected SYSCFG port selector from 0 to 15")? as u8;
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

pub(crate) fn registration(
    entry: &Entry,
    entries: &[Entry],
) -> Result<(u32, u32, Option<u8>), String> {
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
