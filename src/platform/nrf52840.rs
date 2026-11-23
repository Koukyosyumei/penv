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

pub(super) fn map_peripheral(
    entry: &Entry,
    nvic: &Entry,
    entries: &[Entry],
    bus: &mut Bus,
) -> Result<(), String> {
    let uart = entry.model == "UART.NRF52840_UART";
    let mut irq = None;
    let mut easy_dma = None;
    let mut loopback = None;
    let mut channels = None;
    let mut ports = [None; 2];
    for (_, item) in &entry.body {
        if let Some(destination) = item.strip_prefix("->") {
            let (target, line) = destination
                .split_once('@')
                .ok_or_else(|| entry.error("expected an NVIC IRQ connection"))?;
            if target.trim() != nvic.name || irq.is_some() {
                return Err(entry.error("expected one IRQ connection to NVIC"));
            }
            irq = Some(number(line.trim()).map_err(|e| entry.error(&e))?);
        } else if matches!(
            entry.model.as_str(),
            "Sound.NRF52840_I2S" | "Wireless.NRF52840_Radio"
        ) && loopback.is_none()
            && let Some(("loopback", value)) = item.split_once(':')
        {
            loopback = Some(
                value
                    .trim()
                    .parse::<bool>()
                    .map_err(|_| entry.error("invalid loopback property"))?,
            );
        } else if (uart || entry.model == "SPI.NRF52840_SPI")
            && easy_dma.is_none()
            && let Some(("easyDMA", value)) = item.split_once(':')
        {
            easy_dma = Some(
                value
                    .trim()
                    .parse::<bool>()
                    .map_err(|_| entry.error("invalid easyDMA property"))?,
            );
        } else if matches!(
            entry.model.as_str(),
            "Timers.NRF52840_RTC" | "Timers.NRF52840_Timer"
        ) && channels.is_none()
            && let Some(("numberOfEvents", count)) = item.split_once(':')
        {
            let count = number(count.trim()).map_err(|e| entry.error(&e))?;
            let max = if entry.model == "Timers.NRF52840_Timer" {
                6
            } else {
                4
            };
            if !(1..=max).contains(&count) {
                return Err(entry.error(&format!("expected 1–{max} compare channels")));
            }
            channels = Some(count as usize);
        } else if entry.model == "GPIOPort.NRF52840_GPIOTasksEvents"
            && let Some((key @ ("port0" | "port1"), name)) = item.split_once(':')
        {
            let index = usize::from(key == "port1");
            if ports[index].is_some() {
                return Err(entry.error("duplicate GPIO port reference"));
            }
            let port = entries
                .iter()
                .find(|port| port.name == name.trim() && port.model == "GPIOPort.NRF52840_GPIO")
                .ok_or_else(|| entry.error("unknown nRF52840 GPIO port"))?;
            ports[index] = Some(port.address()?);
        } else {
            return Err(entry.error("unsupported nRF52840 peripheral property or wiring"));
        }
    }
    if uart && easy_dma != Some(true) {
        return Err(entry.error("only the EasyDMA UART mode is supported"));
    }
    let irq = irq
        .filter(|line| *line < 48)
        .ok_or_else(|| entry.error("expected an nRF52840 NVIC IRQ in 0–47"))?;
    let base = entry.address()?;
    let name = entry.name.clone();
    if entry.model == "Miscellaneous.NRF_CLOCK" && base == 0x4000_0000 {
        bus.map_nrf_peripheral(Peripheral::nvmc())
            .map_err(|e| entry.error(&e))?;
    }
    let device = if uart {
        Peripheral::uarte(base, irq as u8, name)
    } else if entry.model == "Miscellaneous.NRF52840_EGU" {
        Peripheral::egu(base, irq as u8, name)
    } else if entry.model == "Miscellaneous.NRF52840_ECB" {
        Peripheral::ecb(base, irq as u8, name)
    } else if entry.model == "Miscellaneous.NRF52840_RNG" {
        Peripheral::rng(base, irq as u8, name)
    } else if entry.model == "Wireless.NRF52840_Radio" {
        if base == 0x4000_1000 {
            bus.map_nrf_peripheral(Peripheral::aar())
                .map_err(|e| entry.error(&e))?;
        }
        Peripheral::radio(base, irq as u8, name, loopback.unwrap_or(false))
    } else if entry.model == "Sound.NRF52840_PDM" {
        Peripheral::pdm(base, irq as u8, name)
    } else if entry.model == "Sound.NRF52840_I2S" {
        Peripheral::i2s(base, irq as u8, name, loopback.unwrap_or(false))
    } else if entry.model == "Timers.NRF52840_Timer" {
        let channels = channels.ok_or_else(|| entry.error("missing numberOfEvents"))?;
        Peripheral::timer(base, irq as u8, name, channels)
    } else if entry.model == "Timers.NRF52840_RTC" {
        Peripheral::rtc(base, irq as u8, name, channels.unwrap_or(4))
    } else if entry.model == "GPIOPort.NRF52840_GPIOTasksEvents" {
        let [Some(port0), Some(port1)] = ports else {
            return Err(entry.error("GPIOTE requires both GPIO ports"));
        };
        if port0 == port1 {
            return Err(entry.error("GPIOTE requires distinct GPIO ports"));
        }
        Peripheral::gpiote(base, irq as u8, name, [port0, port1])
    } else if entry.model == "SPI.NRF52840_SPI" {
        if easy_dma == Some(true) {
            return Err(entry.error("nRF52840 SPIM EasyDMA is unsupported"));
        }
        let mut children = entries.iter().filter(|child| {
            child.model == "SPI.SPILoopback"
                && child.registration.as_deref() == Some(entry.name.as_str())
        });
        let loopback = children.next().is_some();
        if let Some(extra) = children.next() {
            return Err(extra.error("nRF52840 SPI accepts only one attached device"));
        }
        Peripheral::spi(base, irq as u8, name, loopback)
    } else if entry.model == "I2C.NRF52840_I2C" {
        let mut twi = Peripheral::twi(base, irq as u8, name);
        for child in entries
            .iter()
            .filter(|child| child.model == "Memory.GenericI2cEeprom")
        {
            let Some((parent, address)) = child
                .registration
                .as_deref()
                .and_then(|text| text.split_once(char::is_whitespace))
            else {
                continue;
            };
            if parent == entry.name {
                let address = number(address.trim()).map_err(|e| child.error(&e))?;
                twi.attach_eeprom(address, super::i2c_eeprom(child)?)
                    .map_err(|e| child.error(&e))?;
            }
        }
        twi
    } else if entry.model == "Timers.NRF52840_Watchdog" {
        Peripheral::watchdog(base, irq as u8, name)
    } else {
        Peripheral::clock(base, irq as u8, name)
    };
    bus.map_nrf_peripheral(device).map_err(|e| entry.error(&e))
}

pub(super) fn validate_spi_loopback(entry: &Entry, entries: &[Entry]) -> Result<(), String> {
    super::keys(entry, &[])?;
    if !entries.iter().any(|parent| {
        parent.model == "SPI.NRF52840_SPI"
            && entry.registration.as_deref() == Some(parent.name.as_str())
    }) {
        return Err(entry.error("expected SPILoopback registered on an nRF52840 SPI controller"));
    }
    Ok(())
}

pub(super) fn map_gpio(entry: &Entry, bus: &mut Bus) -> Result<(), String> {
    super::keys(entry, &[])?;
    let address = entry.address()?;
    let pins = match address {
        0x5000_0500 => 32,
        0x5000_0800 => 16,
        _ => return Err(entry.error("expected an nRF52840 GPIO port base")),
    };
    bus.map_nrf_peripheral(Peripheral::gpio(address, entry.name.clone(), pins))
        .map_err(|e| entry.error(&e))
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
