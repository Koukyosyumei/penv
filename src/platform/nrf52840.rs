use super::{Entry, number};
use crate::{bus::Bus, nrf52840::Peripheral};

pub(super) fn map_aliases(entry: &Entry, bus: &mut Bus, size: usize) -> Result<(), String> {
    let registrations = entry
        .registration
        .as_deref()
        .and_then(|text| text.strip_prefix('{'))
        .and_then(|text| text.strip_suffix('}'))
        .ok_or_else(|| entry.error("expected a list of sysbus registrations"))?;
    let mut bases = registrations.split(';').map(|text| {
        let address = text
            .trim()
            .strip_prefix("sysbus ")
            .ok_or_else(|| entry.error("expected a sysbus address"))?;
        number(address.trim()).map_err(|e| entry.error(&e))
    });
    let base = bases.next().unwrap()?;
    bus.map_ram(base, size).map_err(|e| entry.error(&e))?;
    for alias in bases {
        bus.map_ram_alias(base, alias?, size)
            .map_err(|e| entry.error(&e))?;
    }
    Ok(())
}

pub(super) fn map_peripheral(entry: &Entry, nvic: &Entry, bus: &mut Bus) -> Result<(), String> {
    let uart = entry.model == "UART.NRF52840_UART";
    let mut irq = None;
    let mut easy_dma = false;
    let mut channels = None;
    for (_, item) in &entry.body {
        if let Some(destination) = item.strip_prefix("->") {
            let (target, line) = destination
                .split_once('@')
                .ok_or_else(|| entry.error("expected an NVIC IRQ connection"))?;
            if target.trim() != nvic.name || irq.is_some() {
                return Err(entry.error("expected one IRQ connection to NVIC"));
            }
            irq = Some(number(line.trim()).map_err(|e| entry.error(&e))?);
        } else if uart
            && !easy_dma
            && item
                .split_once(':')
                .is_some_and(|(name, value)| name.trim() == "easyDMA" && value.trim() == "true")
        {
            easy_dma = true;
        } else if entry.model == "Timers.NRF52840_RTC"
            && channels.is_none()
            && let Some(("numberOfEvents", count)) = item.split_once(':')
        {
            let count = number(count.trim()).map_err(|e| entry.error(&e))?;
            if !(1..=4).contains(&count) {
                return Err(entry.error("expected 1–4 RTC compare channels"));
            }
            channels = Some(count as usize);
        } else {
            return Err(entry.error("unsupported nRF52840 peripheral property or wiring"));
        }
    }
    if uart && !easy_dma {
        return Err(entry.error("only the EasyDMA UART mode is supported"));
    }
    let irq = irq
        .filter(|line| *line < 48)
        .ok_or_else(|| entry.error("expected an nRF52840 NVIC IRQ in 0–47"))?;
    let base = entry.address()?;
    let name = entry.name.clone();
    let device = if uart {
        Peripheral::uarte(base, irq as u8, name)
    } else if entry.model == "Timers.NRF52840_RTC" {
        Peripheral::rtc(base, irq as u8, name, channels.unwrap_or(4))
    } else {
        Peripheral::clock(base, irq as u8, name)
    };
    bus.map_nrf_peripheral(device).map_err(|e| entry.error(&e))
}

pub(super) fn map_tags(entry: &Entry, bus: &mut Bus) -> Result<(), String> {
    if entry.name != "sysbus" || entry.registration.is_some() {
        return Err(entry.error("expected a sysbus init block"));
    }
    let mut body = entry.body.iter();
    if !body.next().is_some_and(|(_, line)| line == "init:") {
        return Err(entry.error("expected a sysbus init block"));
    }
    for (line, text) in body {
        let error = || {
            format!(
                "{}:{line}: unsupported nRF52840 sysbus init command {text:?}",
                entry.source.display()
            )
        };
        // This SVD supplies register names to Renode's diagnostics. It does not create devices.
        if text == "ApplySVD @https://dl.antmicro.com/projects/renode/svd/NRF52840.svd.gz" {
            continue;
        }
        let text = text
            .split_once('#')
            .map_or(text.as_str(), |(command, _)| command)
            .trim();
        let (range, tag) = text
            .strip_prefix("Tag <")
            .and_then(|text| text.split_once('>'))
            .ok_or_else(error)?;
        let (start, end) = range.split_once(',').ok_or_else(error)?;
        let start = number(start.trim()).map_err(|_| error())?;
        let end = number(end.trim()).map_err(|_| error())?;
        let (name, value) = tag
            .trim()
            .strip_prefix('"')
            .and_then(|text| text.split_once('"'))
            .ok_or_else(error)?;
        if start > end || name.is_empty() {
            return Err(error());
        }
        let value = match value.trim() {
            "" => 0,
            value => number(value).map_err(|_| error())?,
        };
        bus.add_nrf_tag(start, end, value);
        if (start, end, name) == (0x1000_1200, 0x1000_1203, "PSELRESET") {
            // The paired UICR PSELRESET register also reads as zero on the upstream bus.
            bus.add_nrf_tag(0x1000_1204, 0x1000_1207, 0);
        }
    }
    Ok(())
}
