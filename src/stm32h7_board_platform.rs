use crate::ft5336::Ft5336;
use crate::ina228::Ina228;
use crate::{
    designware_usb::DesignWareUsb,
    iis2mdc::Iis2mdc,
    lps25hb::Lps25hb,
    lsm6dso::Lsm6dso,
    machine::Machine,
    nvic::IRQ_LINES,
    platform::{Entry, irq_index, irq_list, number},
    stm32_crc::Stm32Crc,
    stm32_iwdg::Iwdg,
    stm32_uart::Stm32Irq,
    stm32h7_crypto::Crypto,
    stm32h7_dma2d::Dma2d,
    stm32h7_dsi::Dsi,
    stm32h7_fdcan::Fdcan,
    stm32h7_hsem::Hsem,
    stm32h7_i2c::{I2C, I2c, Target},
    stm32h7_lptim::LpTimer,
    stm32h7_ltdc::Ltdc,
    stm32h7_qspi::Qspi,
    stm32h7_rng::Rng,
    stm32h7_sdmmc::Sdmmc,
    stm32h7_timer::{Timer, TimerClock, TimerOutput},
    tmp108::Tmp108,
    veml7700::Veml7700,
};

pub fn has_regions(model: &str) -> bool {
    matches!(
        model,
        I2C | Rng::MODEL
            | Qspi::MODEL
            | Timer::MODEL
            | Stm32Crc::MODEL
            | Dma2d::MODEL
            | Ltdc::MODEL
            | LpTimer::MODEL
            | Hsem::MODEL
    )
}

pub fn is_device(model: &str) -> bool {
    has_regions(model)
        || matches!(
            model,
            Qspi::FLASH
                | Fdcan::MODEL
                | Ina228::MODEL
                | Veml7700::MODEL
                | Iis2mdc::MODEL
                | Ft5336::MODEL
                | Tmp108::MODEL
                | Lps25hb::MODEL
                | Lsm6dso::MODEL
                | Iwdg::MODEL
                | Sdmmc::MODEL
                | DesignWareUsb::MODEL
                | Crypto::MODEL
                | Dsi::MODEL
        )
}

pub fn has_rcc_binding(entry: &Entry, rcc: &Entry) -> bool {
    if entry.model == Timer::MODEL {
        return TimerClock::ALIASES.iter().any(|(alias, _)| {
            crate::network_platform::rcc_alias(
                entry,
                &format!("RegistrationPeripheralAlias.{alias}"),
            ) == Some(&rcc.name)
        });
    }
    let accepted: &[&str] = match entry.model.as_str() {
        I2C => &["I2C1", "I2C2", "I2C3", "I2C4"],
        Dma2d::MODEL => &["DMA2D"],
        Ltdc::MODEL => &["LTDC"],
        Hsem::MODEL => &["HSEM"],
        LpTimer::MODEL => &LpTimer::RCC_ALIASES,
        Rng::MODEL => &["RNG"],
        Qspi::MODEL => &["QSPI"],
        Stm32Crc::MODEL => &["CRC"],
        _ => return false,
    };
    accepted.iter().any(|alias| {
        crate::network_platform::rcc_alias(entry, &format!("RegistrationPeripheralAlias.{alias}"))
            == Some(&rcc.name)
    })
}

pub fn configure(
    entries: &[Entry],
    nvic: Option<&Entry>,
    exti: Option<&Entry>,
    exti_lines: u32,
    machine: &mut Machine,
) -> Result<(), String> {
    for entry in entries
        .iter()
        .filter(|entry| matches!(entry.model.as_str(), Sdmmc::MODEL | DesignWareUsb::MODEL))
    {
        let base = entry
            .sysbus()?
            .ok_or_else(|| entry.error("missing peripheral address"))?;
        let irq = match entry.body.as_slice() {
            [] => None,
            [(_, text)] => {
                let (pin, target, line) = wire(text).map_err(|e| entry.error(e))?;
                if !matches!(pin, "" | "IRQ") || !nvic.is_some_and(|nvic| nvic.name == target) {
                    return Err(entry.error("expected IRQ connected to NVIC"));
                }
                Some(irq_index(line, IRQ_LINES as u32).map_err(|e| entry.error(&e))?)
            }
            _ => return Err(entry.error("expected at most one IRQ")),
        };
        if entry.model == Sdmmc::MODEL {
            machine.bus.map_h7_sdmmc(Sdmmc::new(base, irq))
        } else {
            machine.bus.map_designware_usb(DesignWareUsb::new(base))
        }
        .map_err(|e| entry.error(&e))?;
    }

    for entry in entries.iter().filter(|entry| entry.model == Dsi::MODEL) {
        let base = entry
            .sysbus()?
            .ok_or_else(|| entry.error("missing DSI address"))?;
        let panel = match entry.body.as_slice() {
            [] => false,
            [(_, property)] if property == "panel: \"NT35510\"" => true,
            _ => return Err(entry.error("expected optional DSI panel: \"NT35510\"")),
        };
        machine
            .bus
            .map_h7_dsi(Dsi::new(base, panel))
            .map_err(|e| entry.error(&e))?;
    }
    for entry in entries.iter().filter(|entry| entry.model == Crypto::MODEL) {
        let base = entry
            .sysbus()?
            .ok_or_else(|| entry.error("missing CRYP address"))?;
        let mut irq = None;
        let mut out_dma = false;
        for (_, text) in &entry.body {
            let (pin, target, line) = wire(text).map_err(|e| entry.error(e))?;
            match pin {
                "" | "IRQ" if irq.is_none() && nvic.is_some_and(|nvic| nvic.name == target) => {
                    irq = Some(irq_index(line, IRQ_LINES as u32).map_err(|e| entry.error(&e))?);
                }
                "OutDMA"
                    if !out_dma
                        && entries.iter().any(|e| {
                            e.name == target && e.model == "Miscellaneous.STM32_DMAMUX"
                        }) =>
                {
                    irq_index(line, 128).map_err(|e| entry.error(&e))?;
                    out_dma = true;
                }
                _ => return Err(entry.error("invalid CRYP connection")),
            }
        }
        machine
            .bus
            .map_h7_crypto(Crypto::new(base, irq))
            .map_err(|e| entry.error(&e))?;
    }

    for entry in entries.iter().filter(|entry| entry.model == Hsem::MODEL) {
        let (base, alias) = registration(entry, entries).map_err(|e| entry.error(&e))?;
        if !matches!(alias, None | Some("HSEM")) {
            return Err(entry.error("invalid HSEM RCC alias"));
        }
        let irq = match entry.body.as_slice() {
            [] => None,
            [(_, text)] => {
                let (pin, target, line) = wire(text).map_err(|e| entry.error(e))?;
                if !matches!(pin, "" | "IRQ") || !nvic.is_some_and(|nvic| nvic.name == target) {
                    return Err(entry.error("expected HSEM IRQ connected to NVIC"));
                }
                Some(irq_index(line, IRQ_LINES as u32).map_err(|e| entry.error(&e))?)
            }
            _ => return Err(entry.error("expected at most one HSEM IRQ")),
        };
        machine
            .bus
            .map_h7_hsem(Hsem::new(base, alias.is_some(), irq))
            .map_err(|e| entry.error(&e))?;
    }

    for entry in entries.iter().filter(|entry| entry.model == Iwdg::MODEL) {
        let (base, alias) = registration(entry, entries).map_err(|e| entry.error(&e))?;
        if alias.is_some() {
            return Err(entry.error("IWDG uses an independent clock"));
        }
        let mut frequency = None;
        let mut window = None;
        let mut prescaler = None;
        for (_, text) in &entry.body {
            let (key, value) = text
                .split_once(':')
                .ok_or_else(|| entry.error("expected IWDG property"))?;
            match key.trim() {
                "frequency" if frequency.is_none() => {
                    frequency = Some(number(value.trim()).map_err(|e| entry.error(&e))?);
                }
                "windowOption" if window.is_none() => {
                    window = Some(
                        value
                            .trim()
                            .parse()
                            .map_err(|_| entry.error("invalid IWDG windowOption"))?,
                    );
                }
                "defaultPrescaler" if prescaler.is_none() => {
                    let value = number(value.trim()).map_err(|e| entry.error(&e))?;
                    if value > 7 {
                        return Err(entry.error("invalid IWDG defaultPrescaler"));
                    }
                    prescaler = Some(value as u8);
                }
                _ => return Err(entry.error("unsupported or duplicate IWDG property")),
            }
        }
        let frequency = frequency
            .filter(|&value| value != 0)
            .ok_or_else(|| entry.error("expected nonzero IWDG frequency"))?;
        machine
            .bus
            .map_iwdg(Iwdg::new(
                base,
                frequency,
                window.unwrap_or(true),
                prescaler.unwrap_or(0),
            ))
            .map_err(|e| entry.error(&e))?;
    }
    for entry in entries
        .iter()
        .filter(|entry| matches!(entry.model.as_str(), Dma2d::MODEL | Ltdc::MODEL))
    {
        let dma2d = entry.model == Dma2d::MODEL;
        let expected_alias = if dma2d { "DMA2D" } else { "LTDC" };
        let (base, alias) = registration(entry, entries).map_err(|e| entry.error(&e))?;
        if alias.is_some_and(|alias| alias != expected_alias) {
            return Err(entry.error("invalid STM32 graphics RCC alias"));
        }
        let irq = match entry.body.as_slice() {
            [] => None,
            [(_, text)] => {
                let (pin, target, line) = wire(text).map_err(|e| entry.error(e))?;
                if !matches!(pin, "" | "IRQ") || !nvic.is_some_and(|nvic| nvic.name == target) {
                    return Err(entry.error("expected graphics IRQ connected to NVIC"));
                }
                Some(irq_index(line, IRQ_LINES as u32).map_err(|e| entry.error(&e))?)
            }
            _ => return Err(entry.error("expected at most one graphics IRQ")),
        };
        if dma2d {
            machine
                .bus
                .map_h7_dma2d(Dma2d::new(base, alias.is_some(), irq))
        } else {
            machine
                .bus
                .map_h7_ltdc(Ltdc::new(base, alias.is_some(), irq))
        }
        .map_err(|e| entry.error(&e))?;
    }

    for entry in entries.iter().filter(|entry| entry.model == LpTimer::MODEL) {
        let (base, alias) = registration(entry, entries).map_err(|e| entry.error(&e))?;
        let clock = alias
            .map(|alias| {
                LpTimer::RCC_ALIASES
                    .iter()
                    .position(|candidate| *candidate == alias)
                    .map(|index| index as u8 + 1)
                    .ok_or_else(|| entry.error("invalid LPTIM RCC alias"))
            })
            .transpose()?;
        let mut frequency = None;
        let mut irq = None;
        for (_, text) in &entry.body {
            if let Some(value) = text
                .strip_prefix("frequency:")
                .filter(|_| frequency.is_none())
            {
                frequency = Some(number(value.trim()).map_err(|e| entry.error(&e))?);
            } else {
                let (pin, target, line) = wire(text).map_err(|e| entry.error(e))?;
                if irq.is_some() || !matches!(pin, "" | "IRQ") {
                    return Err(entry.error("expected at most one LPTIM IRQ"));
                }
                let (limit, route): (_, fn(u8) -> Stm32Irq) =
                    if nvic.is_some_and(|nvic| nvic.name == target) {
                        (IRQ_LINES as u32, Stm32Irq::Nvic)
                    } else if exti.is_some_and(|exti| exti.name == target) {
                        (exti_lines, Stm32Irq::Exti)
                    } else {
                        return Err(entry.error("expected LPTIM IRQ connected to EXTI or NVIC"));
                    };
                irq = Some(route(irq_index(line, limit).map_err(|e| entry.error(&e))?));
            }
        }
        let frequency = frequency
            .filter(|value| *value > 0)
            .ok_or_else(|| entry.error("expected nonzero LPTIM frequency"))?;
        machine
            .bus
            .map_h7_lptim(LpTimer::new(base, clock, frequency, irq))
            .map_err(|e| entry.error(&e))?;
    }

    let mut buses = Vec::new();
    for entry in entries.iter().filter(|entry| entry.model == I2C) {
        let i2c = parse_i2c(entry, entries, nvic, exti, exti_lines).map_err(|e| entry.error(&e))?;
        buses.push((entry, i2c));
    }
    for entry in entries.iter().filter(|entry| {
        matches!(
            entry.model.as_str(),
            Ina228::MODEL
                | Veml7700::MODEL
                | Iis2mdc::MODEL
                | Tmp108::MODEL
                | Lps25hb::MODEL
                | Lsm6dso::MODEL
                | Ft5336::MODEL
        )
    }) {
        let (parent, address) = entry
            .registration
            .as_deref()
            .and_then(|text| text.split_once(char::is_whitespace))
            .ok_or_else(|| entry.error("expected sensor on an I2C address"))?;
        let address = irq_index(address, 128).map_err(|e| entry.error(&e))?;
        let (_, bus) = buses
            .iter_mut()
            .find(|(target, _)| target.name == parent)
            .ok_or_else(|| entry.error("unknown sensor I2C parent"))?;
        if entry.model == Ina228::MODEL {
            if !entry.body.is_empty() {
                return Err(entry.error("unsupported INA228 properties"));
            }
            bus.attach(
                u16::from(address),
                Target::PowerMonitor(
                    format!("sysbus.{parent}.{}", entry.name),
                    Box::new(Ina228::new()),
                ),
            )
            .map_err(|e| entry.error(e))?;
            continue;
        }
        if entry.model == Ft5336::MODEL {
            let device = parse_touch(entry, entries).map_err(|e| entry.error(&e))?;
            bus.attach(
                u16::from(address),
                Target::Touch(format!("sysbus.{parent}.{}", entry.name), device),
            )
            .map_err(|e| entry.error(e))?;
            continue;
        }
        if entry.model == Lsm6dso::MODEL {
            let sensor = parse_imu(entry, entries).map_err(|e| entry.error(&e))?;
            bus.attach(
                u16::from(address),
                Target::Imu(format!("sysbus.{parent}.{}", entry.name), Box::new(sensor)),
            )
            .map_err(|e| entry.error(e))?;
            continue;
        }
        if entry.model == Lps25hb::MODEL {
            let mut sensor = Lps25hb::new();
            let mut set = [false; 2];
            for (_, text) in &entry.body {
                let (key, value) = text
                    .split_once(':')
                    .ok_or_else(|| entry.error("invalid LPS25HB property"))?;
                let value = value
                    .trim()
                    .parse::<f64>()
                    .map_err(|_| entry.error("invalid LPS25HB input"))?;
                let accepted = match key.trim() {
                    "Temperature" if !std::mem::replace(&mut set[0], true) => {
                        sensor.set_temperature(value)
                    }
                    "Pressure" if !std::mem::replace(&mut set[1], true) => {
                        sensor.set_pressure(value)
                    }
                    _ => return Err(entry.error("invalid or duplicate LPS25HB property")),
                };
                if !accepted {
                    return Err(entry.error("LPS25HB input is outside operating range"));
                }
            }
            bus.attach(
                u16::from(address),
                Target::Pressure(format!("sysbus.{parent}.{}", entry.name), sensor),
            )
            .map_err(|e| entry.error(e))?;
            continue;
        }
        if entry.model == Tmp108::MODEL {
            let celsius = match entry.body.as_slice() {
                [] => 0.0,
                [(_, text)] => text
                    .strip_prefix("Temperature:")
                    .and_then(|value| value.trim().parse::<f64>().ok())
                    .filter(|value| (-128.0..=127.9375).contains(value))
                    .ok_or_else(|| entry.error("expected TMP108 Temperature in sensor range"))?,
                _ => return Err(entry.error("unsupported TMP108 properties")),
            };
            bus.attach(
                u16::from(address),
                Target::Temperature(Tmp108::new(celsius)),
            )
            .map_err(|e| entry.error(e))?;
            continue;
        }
        if entry.model == Iis2mdc::MODEL {
            let mut values = [None; 3];
            let mut sample_csv = None;
            let mut route = None;
            for (_, property) in &entry.body {
                if property.starts_with("->") {
                    let (_, name, line) = wire(property).map_err(|e| entry.error(e))?;
                    let port = entries
                        .iter()
                        .find(|port| {
                            port.name == name && port.model == crate::stm32h7_gpio_platform::GPIO
                        })
                        .ok_or_else(|| entry.error("expected IIS2MDC DRDY connected to GPIO"))?;
                    let (base, _, _) = crate::stm32h7_gpio_platform::registration(port, entries)
                        .map_err(|e| entry.error(&e))?;
                    let pin = irq_index(line, 16).map_err(|e| entry.error(&e))?;
                    if route.replace((base, pin)).is_some() {
                        return Err(entry.error("duplicate IIS2MDC DRDY connection"));
                    }
                    continue;
                }
                let (key, value) = property
                    .split_once(':')
                    .ok_or_else(|| entry.error("expected IIS2MDC magnetic field property"))?;
                let axis = match key.trim() {
                    "SampleCsv" if sample_csv.is_none() => {
                        let value = value.trim();
                        sample_csv = Some(if value.starts_with('"') {
                            crate::platform::quoted_value(value).map_err(|e| entry.error(e))?
                        } else {
                            value.to_owned()
                        });
                        continue;
                    }
                    "MagneticFluxDensityX" => 0,
                    "MagneticFluxDensityY" => 1,
                    "MagneticFluxDensityZ" => 2,
                    _ => return Err(entry.error("unsupported IIS2MDC property")),
                };
                let value = value
                    .trim()
                    .parse::<i32>()
                    .map_err(|_| entry.error("expected magnetic field in integer nanoteslas"))?;
                if values[axis].replace(value).is_some() {
                    return Err(entry.error("duplicate IIS2MDC field axis"));
                }
            }
            let field = values.map(|value| value.unwrap_or(0));
            let mut sensor = Iis2mdc::new(field);
            if let Some(path) = sample_csv {
                sensor.load_samples(&path).map_err(|e| entry.error(&e))?;
            }
            if let Some((base, pin)) = route {
                sensor.set_route(base, pin);
            }
            bus.attach(
                u16::from(address),
                Target::Magnetometer(format!("sysbus.{parent}.{}", entry.name), sensor),
            )
            .map_err(|e| entry.error(e))?;
            continue;
        }
        let illuminance = match entry.body.as_slice() {
            [] => 0.0,
            [(_, text)] => text
                .split_once(':')
                .filter(|(key, _)| key.trim() == "Illuminance")
                .and_then(|(_, value)| value.trim().parse::<f64>().ok())
                .filter(|value| value.is_finite() && *value >= 0.0)
                .ok_or_else(|| entry.error("expected a nonnegative VEML7700 Illuminance value"))?,
            _ => return Err(entry.error("unsupported VEML7700 properties")),
        };
        bus.attach(
            u16::from(address),
            Target::Light(
                format!("sysbus.{parent}.{}", entry.name),
                Veml7700::new(illuminance),
            ),
        )
        .map_err(|e| entry.error(e))?;
    }
    for (entry, bus) in buses {
        machine.bus.map_h7_i2c(bus).map_err(|e| entry.error(&e))?;
    }

    for entry in entries.iter().filter(|entry| entry.model == Rng::MODEL) {
        let (base, alias) = registration(entry, entries).map_err(|e| entry.error(&e))?;
        if !matches!(alias, None | Some("RNG")) {
            return Err(entry.error("invalid STM32 RNG RCC alias"));
        }
        let mut irq = None;
        for (_, text) in &entry.body {
            if text == "series: STM32Series.H7" {
                continue;
            }
            let (output, target, input) = wire(text).map_err(|e| entry.error(e))?;
            if !matches!(output, "" | "IRQ")
                || irq.is_some()
                || !nvic.is_some_and(|nvic| nvic.name == target)
            {
                return Err(entry.error("expected one STM32 RNG IRQ connected to NVIC"));
            }
            irq = Some(irq_index(input, IRQ_LINES as u32).map_err(|e| entry.error(&e))?);
        }
        machine
            .bus
            .map_h7_rng(Rng::new(base, alias.is_some(), irq))
            .map_err(|e| entry.error(&e))?;
    }

    configure_qspi(entries, nvic, machine)?;
    for entry in entries.iter().filter(|entry| entry.model == Timer::MODEL) {
        let timer =
            parse_timer(entry, entries, nvic, exti, exti_lines).map_err(|e| entry.error(&e))?;
        machine
            .bus
            .map_h7_timer(timer)
            .map_err(|e| entry.error(&e))?;
    }

    for entry in entries
        .iter()
        .filter(|entry| entry.model == Stm32Crc::MODEL)
    {
        let (base, alias) = registration(entry, entries).map_err(|e| entry.error(&e))?;
        if !matches!(alias, None | Some("CRC")) {
            return Err(entry.error("invalid STM32 CRC RCC alias"));
        }
        let mut configurable = None;
        let mut series = None;
        for (_, text) in &entry.body {
            let (key, value) = text
                .split_once(':')
                .ok_or_else(|| entry.error("expected STM32 CRC property"))?;
            match (key.trim(), value.trim()) {
                ("series", "STM32Series.F0") if series.is_none() => series = Some(()),
                ("configurablePoly", value) if configurable.is_none() => {
                    configurable = Some(
                        value
                            .parse()
                            .map_err(|_| entry.error("invalid configurablePoly"))?,
                    );
                }
                _ => return Err(entry.error("unsupported or duplicate STM32 CRC property")),
            }
        }
        let mut crc = Stm32Crc::new(base, configurable.unwrap_or(false));
        crc.rcc_bound = alias.is_some();
        machine.bus.map_h7_crc(crc).map_err(|e| entry.error(&e))?;
    }

    for entry in entries.iter().filter(|entry| entry.model == Fdcan::MODEL) {
        let (base, alias) = registration(entry, entries).map_err(|e| entry.error(&e))?;
        if alias.is_some() {
            return Err(entry.error("unsupported FDCAN RCC alias"));
        }
        let mut lines = [None; 3];
        let mut message_ram = None;
        for (_, text) in &entry.body {
            if let Some((key, value)) = text.split_once(':') {
                if key.trim() != "messageRAM" || message_ram.is_some() {
                    return Err(entry.error("expected one FDCAN messageRAM property"));
                }
                message_ram = entries.iter().find(|memory| {
                    memory.name == value.trim()
                        && matches!(
                            memory.model.as_str(),
                            "Memory.ArrayMemory" | "Memory.MappedMemory"
                        )
                });
                if message_ram.is_none() {
                    return Err(entry.error("unknown FDCAN messageRAM"));
                }
                continue;
            }
            let (output, target, input) = wire(text).map_err(|e| entry.error(e))?;
            let index = match output {
                "Line0" => 0,
                "Line1" => 1,
                "Calibration" => 2,
                _ => return Err(entry.error("unknown FDCAN output")),
            };
            if lines[index].is_some() || !nvic.is_some_and(|nvic| nvic.name == target) {
                return Err(entry.error("expected one FDCAN output connected to NVIC"));
            }
            lines[index] = Some(irq_index(input, IRQ_LINES as u32).map_err(|e| entry.error(&e))?);
        }
        if message_ram.is_none() {
            return Err(entry.error("missing FDCAN messageRAM"));
        }
        machine
            .bus
            .map_h7_fdcan(Fdcan::new(base, [lines[0], lines[1]]))
            .map_err(|e| entry.error(&e))?;
    }

    Ok(())
}

fn parse_touch(entry: &Entry, entries: &[Entry]) -> Result<Ft5336, String> {
    let mut device = Ft5336::default();
    let mut seen = std::collections::HashSet::new();
    for (_, property) in &entry.body {
        if property.contains("->") {
            let (pin, name, target) = wire(property)?;
            if !matches!(pin, "" | "IRQ") || device.irq.is_some() {
                return Err("expected a single FT5336 GPIO route".into());
            }
            let port = entries
                .iter()
                .find(|port| port.name == name && port.model == crate::stm32h7_gpio_platform::GPIO)
                .ok_or("unknown FT5336 GPIO route")?;
            let (base, _, _) = crate::stm32h7_gpio_platform::registration(port, entries)?;
            device.irq = Some((base, irq_index(target, 16)?));
            continue;
        }
        let (key, value) = property.split_once(':').ok_or("expected FT5336 property")?;
        let (key, value) = (key.trim(), value.trim());
        if !seen.insert(key) {
            return Err(format!("duplicate FT5336 property {key}"));
        }
        match key {
            "isRotated" => device.rotated = value.parse().map_err(|_| "invalid isRotated")?,
            "isInvertedX" => {
                device.inverted_x = value.parse().map_err(|_| "invalid isInvertedX")?
            }
            "isInvertedY" => {
                device.inverted_y = value.parse().map_err(|_| "invalid isInvertedY")?
            }
            "MaxX" => device.max_x = value.parse().map_err(|_| "invalid MaxX")?,
            "MaxY" => device.max_y = value.parse().map_err(|_| "invalid MaxY")?,
            _ => return Err(format!("unsupported FT5336 property {key}")),
        }
    }
    Ok(device)
}

fn parse_imu(entry: &Entry, entries: &[Entry]) -> Result<Lsm6dso, String> {
    let mut input = [None; 6];
    let mut route = None;
    for (_, property) in &entry.body {
        if property.contains("->") {
            let (pin, name, target) = wire(property)?;
            if pin != "Interrupt1" || route.is_some() {
                return Err("expected a single LSM6DSO Interrupt1 GPIO route".into());
            }
            let port = entries
                .iter()
                .find(|port| port.name == name && port.model == crate::stm32h7_gpio_platform::GPIO)
                .ok_or("unknown LSM6DSO GPIO route")?;
            let (base, _, _) = crate::stm32h7_gpio_platform::registration(port, entries)?;
            route = Some((base, irq_index(target, 16)?));
            continue;
        }
        let (key, value) = property
            .split_once(':')
            .ok_or("expected LSM6DSO property")?;
        let value = value.trim();
        match key.trim() {
            axis => {
                let index = crate::lsm6dso::input_axis(axis).ok_or("invalid LSM6DSO property")?;
                let value = value
                    .parse::<f64>()
                    .ok()
                    .filter(|value| value.is_finite())
                    .ok_or("invalid LSM6DSO input")?;
                if input[index].replace(value).is_some() {
                    return Err("duplicate LSM6DSO input axis".into());
                }
            }
        }
    }
    Ok(Lsm6dso::new(input.map(|value| value.unwrap_or(0.0)), route))
}

fn parse_timer(
    entry: &Entry,
    entries: &[Entry],
    nvic: Option<&Entry>,
    exti: Option<&Entry>,
    exti_lines: u32,
) -> Result<Timer, String> {
    let (base, alias) = registration(entry, entries)?;
    let clock = alias.map(TimerClock::from_alias).transpose()?;
    let mut frequency = None;
    let mut limit = None;
    let mut routes = Vec::new();
    for (_, text) in &entry.body {
        if let Some((key, value)) = text.split_once(':') {
            let value = number(value.trim())?;
            match key.trim() {
                "frequency" if frequency.is_none() && value > 0 => frequency = Some(value),
                "initialLimit" if limit.is_none() && matches!(value, 0xffff | 0xffff_ffff) => {
                    limit = Some(value)
                }
                _ => return Err("unsupported, invalid or duplicate STM32 timer property".into()),
            }
        } else {
            if let Some((channel, targets)) = text.split_once("->")
                && irq_index(channel, 4).is_ok()
            {
                for target in targets.split('|') {
                    let (port, selector) = target
                        .trim()
                        .split_once('#')
                        .ok_or("missing timer GPIO pin")?;
                    let (pin, alternate) = selector
                        .split_once('@')
                        .ok_or("missing timer GPIO alternate function")?;
                    if !entries.iter().any(|entry| {
                        entry.name == port && entry.model == crate::stm32h7_gpio_platform::GPIO
                    }) {
                        return Err(
                            "expected timer output routed to a GPIO alternate function".into()
                        );
                    }
                    irq_index(pin, 16)?;
                    irq_index(alternate, 16)?;
                }
                continue;
            }
            let (output, target, input) = wire(text)?;
            let outputs = output
                .strip_prefix('[')
                .and_then(|list| list.strip_suffix(']'))
                .unwrap_or(output);
            let outputs = outputs
                .split(',')
                .map(TimerOutput::parse)
                .collect::<Result<Vec<_>, _>>()?;
            let (count, route): (_, fn(u8) -> Stm32Irq) =
                if nvic.is_some_and(|nvic| nvic.name == target) {
                    (IRQ_LINES as u32, Stm32Irq::Nvic)
                } else if exti.is_some_and(|exti| exti.name == target) {
                    (exti_lines, Stm32Irq::Exti)
                } else {
                    return Err("expected STM32 timer IRQ connected to NVIC or EXTI".into());
                };
            let inputs = irq_list(input, count)?;
            if outputs.len() != inputs.len() {
                return Err("STM32 timer output and destination list lengths differ".into());
            }
            routes.extend(outputs.into_iter().zip(inputs.into_iter().map(route)));
        }
    }
    let mut timer = Timer::new(
        entry.name.clone(),
        base,
        frequency.ok_or("missing STM32 timer frequency")?,
        limit.unwrap_or(0xffff),
        clock,
    );
    for (output, irq) in routes {
        timer.route(output, irq)?;
    }
    Ok(timer)
}

fn configure_qspi(
    entries: &[Entry],
    nvic: Option<&Entry>,
    machine: &mut Machine,
) -> Result<(), String> {
    let mut controllers = Vec::new();
    for entry in entries.iter().filter(|entry| entry.model == Qspi::MODEL) {
        let (base, alias) = registration(entry, entries).map_err(|e| entry.error(&e))?;
        if !matches!(alias, None | Some("QSPI")) {
            return Err(entry.error("invalid STM32H7 QuadSPI RCC alias"));
        }
        let mut irq = None;
        for (_, text) in &entry.body {
            let (output, target, input) = wire(text).map_err(|e| entry.error(e))?;
            if !matches!(output, "" | "IRQ")
                || irq.is_some()
                || !nvic.is_some_and(|nvic| nvic.name == target)
            {
                return Err(entry.error("expected one QuadSPI IRQ connected to NVIC"));
            }
            irq = Some(irq_index(input, IRQ_LINES as u32).map_err(|e| entry.error(&e))?);
        }
        controllers.push(Qspi::new(entry.name.clone(), base, alias.is_some(), irq));
    }
    for entry in entries.iter().filter(|entry| entry.model == Qspi::FLASH) {
        let qspi = controllers
            .iter_mut()
            .find(|device| Some(device.name.as_str()) == entry.registration.as_deref())
            .ok_or_else(|| entry.error("expected an STM32H7 QuadSPI parent"))?;
        let [(_, property)] = entry.body.as_slice() else {
            return Err(entry.error("expected underlyingMemory for ISSI flash"));
        };
        let memory = property
            .split_once(':')
            .filter(|(key, _)| key.trim() == "underlyingMemory")
            .and_then(|(_, name)| {
                entries.iter().find(|memory| {
                    memory.name == name.trim() && memory.model == "Memory.MappedMemory"
                })
            })
            .ok_or_else(|| entry.error("unknown underlyingMemory for ISSI flash"))?;
        let capacity = memory
            .body
            .iter()
            .find_map(|(_, property)| {
                let (key, value) = property.split_once(':')?;
                (key.trim() == "size").then_some(value.trim())
            })
            .ok_or_else(|| memory.error("missing ISSI flash memory size"))?;
        qspi.attach(number(capacity).map_err(|e| memory.error(&e))?)
            .map_err(|e| entry.error(e))?;
    }
    for controller in controllers {
        machine.bus.map_h7_qspi(controller)?;
    }
    Ok(())
}

fn parse_i2c(
    entry: &Entry,
    entries: &[Entry],
    nvic: Option<&Entry>,
    exti: Option<&Entry>,
    exti_lines: u32,
) -> Result<I2c, String> {
    let (base, alias) = registration(entry, entries)?;
    let clock = match alias {
        None => None,
        Some("I2C1") => Some(1),
        Some("I2C2") => Some(2),
        Some("I2C3") => Some(3),
        Some("I2C4") => Some(4),
        _ => return Err("invalid STM32H7 I2C RCC alias".into()),
    };
    let mut i2c = I2c::new(base, clock);
    for (_, text) in &entry.body {
        let (output, target, input) = wire(text)?;
        let route = match output {
            "EventInterrupt" if i2c.event.is_none() => &mut i2c.event,
            "ErrorInterrupt" if i2c.error.is_none() => &mut i2c.error,
            _ => return Err("unsupported or duplicate I2C output".into()),
        };
        *route = Some(
            if nvic.is_some_and(|controller| controller.name == target) {
                Stm32Irq::Nvic(irq_index(input, IRQ_LINES as u32)?)
            } else if exti.is_some_and(|controller| controller.name == target) {
                Stm32Irq::Exti(irq_index(input, exti_lines)?)
            } else {
                return Err("expected I2C IRQ connected to NVIC or EXTI".into());
            },
        );
    }
    Ok(i2c)
}

pub fn registration<'a>(
    entry: &'a Entry,
    entries: &[Entry],
) -> Result<(u32, Option<&'a str>), String> {
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
            _ => return Err("invalid peripheral bus or RCC registration".into()),
        }
    }
    Ok((base.ok_or("missing sysbus address")?, alias))
}

pub fn wire(text: &str) -> Result<(&str, &str, &str), &'static str> {
    let (from, to) = text.split_once("->").ok_or("expected peripheral wiring")?;
    let (target, pins) = to
        .split_once('@')
        .ok_or("missing wiring destination index")?;
    Ok((from.trim(), target.trim(), pins.trim()))
}
