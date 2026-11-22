use crate::{
    machine::Machine,
    nvic::IRQ_LINES,
    platform::{Entry, irq_index, number},
    stm32_uart::Stm32Irq,
    stm32h7_rtc::Stm32Rtc,
};

pub const RTC: &str = "Timers.STM32F4_RTC";

pub fn configure(
    entries: &[Entry],
    nvic: Option<&Entry>,
    exti: Option<&Entry>,
    exti_lines: u32,
    machine: &mut Machine,
) -> Result<(), String> {
    for entry in entries.iter().filter(|entry| entry.model == RTC) {
        let rtc = parse(entry, entries, nvic, exti, exti_lines).map_err(|e| entry.error(&e))?;
        machine.bus.map_h7_rtc(rtc).map_err(|e| entry.error(&e))?;
    }
    Ok(())
}

fn parse(
    entry: &Entry,
    entries: &[Entry],
    nvic: Option<&Entry>,
    exti: Option<&Entry>,
    exti_lines: u32,
) -> Result<Stm32Rtc, String> {
    let (base, rcc_bound) = registration(entry, entries)?;
    let mut routes = [None; 2];
    let mut frequency = None;
    let mut errata = None;
    for (_, text) in &entry.body {
        if let Some((output, target)) = text.split_once("->") {
            let index = match output.trim() {
                "AlarmIRQ" => 0,
                "WakeupIRQ" => 1,
                _ => return Err("unsupported RTC output".into()),
            };
            let (name, line) = target
                .split_once('@')
                .ok_or("missing RTC interrupt destination index")?;
            let name = name.trim();
            let route = if nvic.is_some_and(|nvic| nvic.name == name) {
                Stm32Irq::Nvic(irq_index(line, IRQ_LINES as u32)?)
            } else if exti.is_some_and(|exti| exti.name == name) {
                Stm32Irq::Exti(irq_index(line, exti_lines)?)
            } else {
                return Err("expected RTC interrupt routed to NVIC or EXTI".into());
            };
            if routes[index].replace(route).is_some() {
                return Err("duplicate RTC output route".into());
            }
        } else if let Some((key, value)) = text.split_once(':') {
            match key.trim() {
                "wakeupTimerFrequency" if frequency.is_none() => {
                    let frequency_value = number(value.trim())?;
                    if frequency_value == 0 {
                        return Err("RTC frequency must be positive".into());
                    }
                    frequency = Some(frequency_value);
                }
                "WakeupTimerRegisterErrata" if errata.is_none() => {
                    errata = Some(
                        value
                            .trim()
                            .parse()
                            .map_err(|_| "invalid RTC errata property")?,
                    );
                }
                _ => return Err("unsupported or duplicate RTC property".into()),
            }
        } else {
            return Err("expected RTC property or interrupt wiring".into());
        }
    }
    let mut rtc = Stm32Rtc::new(base, frequency.unwrap_or(32768), errata.unwrap_or(false));
    rtc.routes = routes;
    rtc.rcc_bound = rcc_bound;
    Ok(rtc)
}

fn registration(entry: &Entry, entries: &[Entry]) -> Result<(u32, bool), String> {
    let text = entry
        .registration
        .as_deref()
        .ok_or("missing RTC registration")?;
    let Some(list) = text
        .strip_prefix('{')
        .and_then(|text| text.strip_suffix('}'))
    else {
        return Ok((entry.sysbus()?.ok_or("missing RTC bus address")?, false));
    };
    let mut base = None;
    let mut clock = false;
    for registration in list
        .split(';')
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let (target, value) = registration
            .split_once(char::is_whitespace)
            .ok_or("invalid RTC registration")?;
        if target == "sysbus" && base.is_none() {
            base = Some(number(value.trim())?);
        } else if !clock
            && value.trim() == "RegistrationPeripheralAlias.RTC"
            && entries
                .iter()
                .any(|entry| entry.name == target && entry.model == "Miscellaneous.STM32H7_RCC")
        {
            clock = true;
        } else {
            return Err("invalid RTC bus or RCC registration".into());
        }
    }
    Ok((base.ok_or("missing RTC bus address")?, clock))
}
