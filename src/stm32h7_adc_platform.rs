use crate::{
    machine::Machine,
    nvic::IRQ_LINES,
    platform::{Entry, irq_index, irq_list, number},
    stm32_uart::Stm32Irq,
    stm32h7_adc::{Adc, AdcClock, AdcRequest, CHANNELS},
    stm32h7_adc_dma::{AdcDma, DmaMux, StreamTarget},
};

const ADC: &str = "Analog.STM32H7_ADC";
const DMA: &str = "DMA.STM32DMA";
const MUX: &str = "Miscellaneous.STM32_DMAMUX";
const POTENTIOMETER: &str = "Analog.Potentiometer";

pub fn has_regions(model: &str) -> bool {
    matches!(model, ADC | DMA)
}

pub fn is_device(model: &str) -> bool {
    matches!(model, ADC | DMA | MUX | POTENTIOMETER)
}

pub fn configure(
    entries: &[Entry],
    nvic: Option<&Entry>,
    exti: Option<&Entry>,
    exti_lines: u32,
    machine: &mut Machine,
) -> Result<(), String> {
    let mut muxes = Vec::new();
    for entry in entries.iter().filter(|entry| entry.model == MUX) {
        let mux = parse_mux(entry, entries).map_err(|e| entry.error(&e))?;
        muxes.push((entry.name.as_str(), mux.0.base, mux.1));
        machine
            .bus
            .map_h7_dma_mux(mux.0)
            .map_err(|e| entry.error(&e))?;
    }
    for entry in entries.iter().filter(|entry| entry.model == DMA) {
        let (base, alias) = registration(entry, entries).map_err(|e| entry.error(&e))?;
        let mask = match alias {
            Some("DMA1") => 1,
            Some("DMA2") => 2,
            None => 0,
            _ => return Err(entry.error("invalid STM32 DMA RCC alias")),
        };
        let mut outputs = [None; AdcDma::STREAMS];
        for (_, text) in &entry.body {
            let (from, target, to) = wire(text).map_err(|e| entry.error(e))?;
            if !nvic.is_some_and(|nvic| nvic.name == target) {
                return Err(entry.error("expected DMA IRQ connected to NVIC"));
            }
            fill_routes(&mut outputs, from, to, IRQ_LINES as u32, |pin| pin)
                .map_err(|e| entry.error(&e))?;
        }
        machine
            .bus
            .map_h7_adc_dma(AdcDma::new(base, mask, outputs))
            .map_err(|e| entry.error(&e))?;
    }

    let mut adcs = Vec::new();
    for entry in entries.iter().filter(|entry| entry.model == ADC) {
        let adc = parse_adc(entry, entries, &muxes, nvic, exti, exti_lines)
            .map_err(|e| entry.error(&e))?;
        adcs.push((entry.name.as_str(), adc, [false; CHANNELS]));
    }
    for entry in entries.iter().filter(|entry| entry.model == POTENTIOMETER) {
        let (parent, channel) = entry
            .registration
            .as_deref()
            .and_then(|value| value.split_once(char::is_whitespace))
            .ok_or_else(|| entry.error("expected potentiometer on an ADC channel"))?;
        let channel =
            usize::from(irq_index(channel, CHANNELS as u32).map_err(|e| entry.error(&e))?);
        let (_, adc, used) = adcs
            .iter_mut()
            .find(|(name, _, _)| *name == parent)
            .ok_or_else(|| entry.error("unknown STM32H7 ADC parent"))?;
        if std::mem::replace(&mut used[channel], true) {
            return Err(entry.error("ADC channel already has a voltage source"));
        }
        let input = potentiometer(entry).map_err(|e| entry.error(&e))?;
        adc.set_input(channel, input);
    }
    for (_, adc, _) in adcs {
        machine.bus.map_h7_adc(adc)?;
    }
    Ok(())
}

fn parse_adc(
    entry: &Entry,
    entries: &[Entry],
    muxes: &[(&str, u32, u32)],
    nvic: Option<&Entry>,
    exti: Option<&Entry>,
    exti_lines: u32,
) -> Result<Adc, String> {
    let (base, alias) = registration(entry, entries)?;
    let clock = match alias {
        Some("ADC12") => Some(AdcClock::Adc12),
        Some("ADC3") => Some(AdcClock::Adc3),
        None => None,
        _ => return Err("invalid STM32H7 ADC RCC alias".into()),
    };
    let mut reference = None;
    let mut frequency = None;
    let mut mux_name = None;
    let mut request = None;
    let mut irq = None;
    for (_, text) in &entry.body {
        if let Some((key, value)) = text.split_once(':') {
            let value = value.trim();
            match key.trim() {
                "referenceVoltage" if reference.is_none() => reference = Some(voltage(value)?),
                "externalEventFrequency" if frequency.is_none() => frequency = Some(number(value)?),
                "dmaPeripheral" if mux_name.is_none() => mux_name = Some(value),
                "dmaChannel" if request.is_none() => request = Some(number(value)?),
                _ => return Err("unsupported or duplicate STM32H7 ADC property".into()),
            }
        } else {
            let (from, target, input) = wire(text)?;
            if !matches!(from, "" | "IRQ") || irq.is_some() {
                return Err("expected one STM32H7 ADC IRQ output".into());
            }
            irq = Some(if nvic.is_some_and(|nvic| nvic.name == target) {
                Stm32Irq::Nvic(irq_index(input, IRQ_LINES as u32)?)
            } else if exti.is_some_and(|exti| exti.name == target) {
                Stm32Irq::Exti(irq_index(input, exti_lines)?)
            } else {
                return Err("invalid STM32H7 ADC IRQ target".into());
            });
        }
    }
    let mut adc = Adc::new(
        base,
        reference
            .filter(|&reference| reference > 0)
            .ok_or("ADC reference voltage must be positive")?,
        frequency
            .filter(|&frequency| frequency > 0)
            .ok_or("ADC trigger frequency must be positive")?,
    );
    adc.clock = clock;
    adc.irq = irq;
    adc.dma = match (mux_name, request) {
        (None, None | Some(0)) => None,
        (Some(name), Some(request)) => {
            let &(_, mux, count) = muxes
                .iter()
                .find(|&&(candidate, _, _)| candidate == name)
                .ok_or("expected ADC DMA peripheral to reference an STM32 DMAMUX")?;
            if !(1..=count).contains(&request) {
                return Err(format!("ADC DMA request must be in 1–{count}"));
            }
            Some(AdcRequest {
                mux,
                id: request as u8,
            })
        }
        _ => return Err("ADC DMA requires both a DMAMUX and a request number".into()),
    };
    Ok(adc)
}

fn parse_mux(entry: &Entry, entries: &[Entry]) -> Result<(DmaMux, u32), String> {
    let mut inputs = None;
    let mut outputs = None;
    let mut generators = None;
    for (_, text) in &entry.body {
        if let Some((key, value)) = text.split_once(':') {
            let value = number(value.trim())?;
            match key.trim() {
                "numberOfInputRequestChannels"
                    if inputs.is_none() && (1..=127).contains(&value) =>
                {
                    inputs = Some(value)
                }
                "numberOfOutputRequestChannels"
                    if outputs.is_none() && (1..=32).contains(&value) =>
                {
                    outputs = Some(value)
                }
                "numberOfRequestGeneratorChannels" if generators.is_none() && value <= 16 => {
                    generators = Some(value)
                }
                _ => return Err("unsupported, duplicate or out-of-range DMAMUX property".into()),
            }
        }
    }
    let inputs = inputs.ok_or("missing DMAMUX input count")?;
    let mut targets = vec![None; outputs.ok_or("missing DMAMUX output count")? as usize];
    for (_, text) in &entry.body {
        if text.contains(':') {
            continue;
        }
        let (from, name, to) = wire(text)?;
        let target = entries
            .iter()
            .find(|entry| entry.model == DMA && entry.name == name)
            .ok_or("expected DMAMUX output connected to an STM32 DMA stream")?;
        let base = registration(target, entries)?.0;
        fill_routes(&mut targets, from, to, AdcDma::STREAMS as u32, |pin| {
            StreamTarget {
                base,
                stream: usize::from(pin),
            }
        })?;
    }
    let base = entry.sysbus()?.ok_or("missing DMAMUX address")?;
    Ok((
        DmaMux::new(base, targets, generators.unwrap_or(0) as usize),
        inputs,
    ))
}

fn fill_routes<T>(
    outputs: &mut [Option<T>],
    from: &str,
    to: &str,
    destination_count: u32,
    map: impl Fn(u8) -> T,
) -> Result<(), String> {
    let from = irq_list(from, outputs.len() as u32)?;
    let to = irq_list(to, destination_count)?;
    if from.len() != to.len() {
        return Err("mismatched routing widths".into());
    }
    for (output, pin) in from.into_iter().zip(to) {
        if outputs[usize::from(output)].replace(map(pin)).is_some() {
            return Err("duplicate output route".into());
        }
    }
    Ok(())
}

fn registration<'a>(entry: &'a Entry, entries: &[Entry]) -> Result<(u32, Option<&'a str>), String> {
    let text = entry
        .registration
        .as_deref()
        .ok_or("missing sysbus registration")?;
    let Some(list) = text
        .strip_prefix('{')
        .and_then(|text| text.strip_suffix('}'))
    else {
        return Ok((entry.sysbus()?.ok_or("missing sysbus address")?, None));
    };
    let mut base = None;
    let mut alias = None;
    for registration in list
        .split(';')
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let mut words = registration.split_whitespace();
        match (words.next(), words.next(), words.next()) {
            (Some("sysbus"), Some(address), None) if base.is_none() => {
                base = Some(number(address)?)
            }
            (Some(parent), Some(value), None)
                if alias.is_none()
                    && entries.iter().any(|entry| {
                        entry.name == parent && entry.model == "Miscellaneous.STM32H7_RCC"
                    }) =>
            {
                alias = Some(
                    value
                        .strip_prefix("RegistrationPeripheralAlias.")
                        .ok_or("expected RCC registration alias")?,
                );
            }
            _ => return Err("invalid ADC or DMA registration".into()),
        }
    }
    Ok((base.ok_or("missing sysbus address")?, alias))
}

fn wire(text: &str) -> Result<(&str, &str, &str), &'static str> {
    let (from, to) = text.split_once("->").ok_or("expected peripheral wiring")?;
    let (target, pins) = to
        .split_once('@')
        .ok_or("missing wiring destination index")?;
    Ok((from.trim(), target.trim(), pins.trim()))
}

fn potentiometer(entry: &Entry) -> Result<u32, String> {
    let mut min = None;
    let mut max = None;
    let mut percentage = None;
    for (_, text) in &entry.body {
        let (key, value) = text
            .split_once(':')
            .ok_or("expected potentiometer property")?;
        let value = value.trim();
        match key.trim() {
            "minVolt" if min.is_none() => min = Some(voltage(value)?),
            "maxVolt" if max.is_none() => max = Some(voltage(value)?),
            "minMicroVolt" if min.is_none() => min = Some(number(value)?),
            "maxMicroVolt" if max.is_none() => max = Some(number(value)?),
            "Percentage" if percentage.is_none() => {
                percentage = Some(
                    value
                        .parse::<f64>()
                        .ok()
                        .filter(|value| (0.0..=100.0).contains(value))
                        .ok_or("invalid potentiometer percentage")?,
                );
            }
            _ => return Err("unsupported or duplicate potentiometer property".into()),
        }
    }
    let min = min.ok_or("missing potentiometer minimum voltage")?;
    let max = max
        .filter(|&max| max >= min)
        .ok_or("missing or invalid potentiometer maximum voltage")?;
    Ok(min + (f64::from(max - min) * (percentage.unwrap_or(0.0) / 100.0)) as u32)
}

fn voltage(value: &str) -> Result<u32, String> {
    let volts = value
        .parse::<f64>()
        .map_err(|_| format!("invalid voltage {value:?}"))?;
    let microvolts = volts * 1_000_000.0;
    if !(0.0..=f64::from(u32::MAX)).contains(&microvolts) {
        return Err(format!("invalid voltage {value:?}"));
    }
    Ok(microvolts as u32)
}
