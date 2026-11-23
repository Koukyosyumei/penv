mod nrf52840;
mod stm32f0;

use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

use crate::{
    machine::{MAX_HARTS, Machine},
    stm32_adc::Stm32F0Adc,
    stm32_can::Stm32Can,
    stm32_dma::Stm32Dma,
    stm32_gpio::Stm32Gpio,
    stm32_gpio_board::{GpioButton, GpioLed},
    stm32_i2c::{Eeprom, Stm32I2c},
    stm32_rtc::Stm32Rtc,
    stm32_spi::{Stm32F0Spi, Stp08},
    stm32_timer::{Stm32Timer, TimerPin},
    stm32_uart::{Stm32Irq, Stm32Usart},
};

type Properties = Vec<(usize, String)>;

#[derive(Debug)]
struct Entry {
    name: String,
    model: String,
    registration: Option<String>,
    body: Properties,
    source: PathBuf,
    line: usize,
}

impl Entry {
    fn error(&self, detail: &str) -> String {
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

    fn address(&self) -> Result<u32, String> {
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
        .find(|(index, _)| line[*index..].starts_with("//") && !line[..*index].ends_with(':'))
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
    for (index, raw) in content.lines().enumerate() {
        let line = without_comment(raw);
        let text = line.trim();
        let lineno = index + 1;
        let error = |detail: &str| format!("{}:{lineno}: {detail}", path.display());
        if text.is_empty() {
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
            Some((model, registration)) => (model.trim(), Some(registration.trim().into())),
            None => (declaration.trim(), None),
        };
        current = Some(Entry {
            name: name.into(),
            model: model.into(),
            registration,
            body,
            source: path.into(),
            line: lineno,
        });
    }
    entries.extend(current);
    visiting.remove(&canonical);
    Ok(())
}

pub fn load(path: &Path, root: Option<&Path>, stm32f0_console: bool) -> Result<Machine, String> {
    let mut entries = Vec::new();
    parse(
        path,
        root.unwrap_or(Path::new(".")),
        &mut HashSet::new(),
        &mut entries,
    )?;
    nrf52840::apply_overrides(&mut entries)?;
    let mut machine = Machine::without_default_ram();
    if stm32f0::is_stm32f0(&entries) {
        machine
            .bus
            .set_stm32f0_tags(stm32f0::prepare(&mut entries)?);
    } else if stm32f0_console {
        return Err("--stm32f0-console requires a platform with an STM32F0 RCC".into());
    }
    let cortex_m4 = entries.iter().any(|entry| {
        entry.model == "CPU.CortexM"
            && entry.body.iter().any(|(_, text)| {
                text.split_once(':').is_some_and(|(key, value)| {
                    key.trim() == "cpuType"
                        && matches!(value.trim(), r#""cortex-m4""# | r#""cortex-m4f""#)
                })
            })
    });
    let mut semihosting_handler = None;
    let mut semihosting_uarts = Vec::new();
    let mut nrf_peripherals = Vec::new();
    let mut names = HashSet::new();
    let mut cpus = Vec::new();
    let mut clint = None;
    let mut nvic = None;
    let mut exti = None;
    let mut combiners = Vec::new();
    let mut ram = false;
    let mut uart = None;
    let mut stm32_uarts = Vec::new();
    let mut stm32_timers = Vec::new();
    let mut stm32_spis = Vec::new();
    let mut spi_shifts = Vec::new();
    let mut stm32_gpios = Vec::new();
    let mut gpio_leds = Vec::new();
    let mut gpio_buttons = Vec::new();
    let mut nrf_gpio_attachments = Vec::new();
    let mut stm32_i2cs = Vec::new();
    let mut stm32_cans = Vec::new();
    let mut i2c_eeproms = Vec::new();
    let mut stm32_adc = None;
    let mut stm32_rtc = None;
    let mut stm32_dma = None;
    let mut plic = None;
    let mut timer = None;
    for entry in &entries {
        if !names.insert(&entry.name) {
            return Err(entry.error("duplicate declarations and overrides are unsupported"));
        }
        match entry.model.as_str() {
            "" if cortex_m4 && entry.name == "sysbus" => {
                nrf52840::map_tags(entry, &mut machine.bus)?
            }
            "CPU.RiscV32" | "CPU.VexRiscv" | "CPU.CortexM" => {
                if entry.sysbus()?.is_some() {
                    return Err(entry.error("expected CPU registered on sysbus without an address"));
                }
                cpus.push(entry);
            }
            "CPU.SemihostingHandler" if cortex_m4 => {
                keys(entry, &[])?;
                if semihosting_handler.replace(entry).is_some() {
                    return Err(entry.error("multiple semihosting handlers are unsupported"));
                }
            }
            "UART.SemihostingUart" if cortex_m4 => {
                keys(entry, &[])?;
                semihosting_uarts.push(entry);
            }
            "Mocks.BusFaultingPeripheral" if cortex_m4 => {
                keys(entry, &[])?;
                machine
                    .bus
                    .map_bus_faulting_peripheral(entry.address()?)
                    .map_err(|e| entry.error(&e))?;
            }
            "Memory.MappedMemory" | "Memory.ArrayMemory" => {
                let (_, size) = keys(entry, &["size"])?
                    .into_iter()
                    .next()
                    .ok_or_else(|| entry.error("missing memory size"))?;
                let size = number(size).map_err(|e| entry.error(&e))? as usize;
                if cortex_m4
                    && entry
                        .registration
                        .as_ref()
                        .is_some_and(|text| text.starts_with('{'))
                {
                    nrf52840::map_aliases(entry, &mut machine.bus, size)?;
                } else {
                    machine
                        .bus
                        .map_ram(entry.address()?, size)
                        .map_err(|e| entry.error(&e))?;
                }
                ram = true;
            }
            "UART.TrivialUart" => {
                keys(entry, &[])?;
                machine
                    .bus
                    .map_trivial_uart(entry.address()?)
                    .map_err(|e| entry.error(&e))?;
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
            "Miscellaneous.NRF52840_PPI" if cortex_m4 => {
                keys(entry, &[])?;
                machine
                    .bus
                    .map_nrf_peripheral(crate::nrf52840::Peripheral::ppi(
                        entry.address()?,
                        entry.name.clone(),
                    ))
                    .map_err(|e| entry.error(&e))?;
            }
            "GPIOPort.NRF52840_GPIO" if cortex_m4 => {
                nrf52840::map_gpio(entry, &entries, &mut machine.bus)?;
            }
            "GPIOPort.STM32_GPIOPort" => stm32_gpios.push(entry),
            "Miscellaneous.LED" | "Miscellaneous.Button"
                if cortex_m4
                    && entry
                        .registration
                        .as_deref()
                        .and_then(|s| s.split_whitespace().next())
                        .is_some_and(|name| {
                            entries.iter().any(|gpio| {
                                gpio.name == name && gpio.model == "GPIOPort.NRF52840_GPIO"
                            })
                        }) =>
            {
                nrf_gpio_attachments.push(entry)
            }
            "Miscellaneous.LED" => gpio_leds.push(entry),
            "Miscellaneous.Button" => gpio_buttons.push(entry),
            "CRC.STM32_CRC" => {
                let mut series = None;
                let mut configurable_poly = false;
                for (key, value) in keys(entry, &["series", "configurablePoly"])? {
                    match key {
                        "series" => series = Some(value),
                        _ => {
                            configurable_poly = value
                                .parse()
                                .map_err(|_| entry.error("invalid configurablePoly"))?
                        }
                    }
                }
                if series != Some("STM32Series.F0") {
                    return Err(entry.error("expected STM32Series.F0 CRC"));
                }
                machine
                    .bus
                    .map_stm32f0_crc(entry.address()?, configurable_poly)
                    .map_err(|e| entry.error(&e))?;
            }
            "Miscellaneous.STM32F0_RCC" => {
                keys(entry, &[])?;
                machine
                    .bus
                    .map_stm32f0_rcc(entry.address()?)
                    .map_err(|e| entry.error(&e))?;
            }
            "Timers.STM32_Timer" => stm32_timers.push(entry),
            "Sensors.ArduCAMMini2MPPlus" if cortex_m4 => {
                nrf52840::camera_routes(entry, &entries)?;
            }
            "Sensors.LSM9DS1_IMU" | "Sensors.LSM9DS1_Magnetic" if cortex_m4 => {
                nrf52840::validate_lsm9ds1(entry, &entries)?;
            }
            "Sensors.BMP180" if cortex_m4 => {
                nrf52840::bmp180_config(entry, &entries)?;
            }
            "Sensors.LSM303DLHC_Accelerometer" | "Sensors.LSM303DLHC_Gyroscope" if cortex_m4 => {
                nrf52840::lsm303_config(entry, &entries)?;
            }
            "Sensors.LSM330_Accelerometer" | "Sensors.LSM330_Gyroscope" if cortex_m4 => {
                nrf52840::lsm330_config(entry, &entries)?;
            }
            "Sensors.LIS2DS12" if cortex_m4 => {
                nrf52840::lis2ds12_config(entry, &entries)?;
            }
            "I2C.BME280" if cortex_m4 => nrf52840::validate_bme280(entry, &entries)?,
            "Sensors.ADXL372" if cortex_m4 => nrf52840::validate_adxl(entry, &entries)?,
            "SPI.SPIMultiplexer" if cortex_m4 => nrf52840::validate_mux(entry, &entries)?,
            "Mocks.DummySPISlave" if cortex_m4 => nrf52840::validate_mux_slave(entry, &entries)?,
            "SPI.SPILoopback" if cortex_m4 => nrf52840::validate_spi_loopback(entry, &entries)?,
            "SPI.STM32SPI" => stm32_spis.push(entry),
            "SPI.STP08ShiftRegister" => spi_shifts.push(entry),
            "CAN.STMCAN" => stm32_cans.push(entry),
            "I2C.STM32F7_I2C" => stm32_i2cs.push(entry),
            "Memory.GenericI2cEeprom" => i2c_eeproms.push(entry),
            "Timers.STM32F4_RTC" => {
                if stm32_rtc.replace(entry).is_some() {
                    return Err(entry.error("only one STM32 RTC is supported"));
                }
            }
            "DMA.STM32G0DMA" => {
                if stm32_dma.replace(entry).is_some() {
                    return Err(entry.error("only one STM32 DMA controller is supported"));
                }
            }
            "Analog.STM32F0_ADC" => {
                if stm32_adc.replace(entry).is_some() {
                    return Err(entry.error("only one STM32F0 ADC is supported"));
                }
            }
            "UART.NRF52840_UART"
            | "Miscellaneous.NRF_CLOCK"
            | "I2C.NRF52840_I2C"
            | "SPI.NRF52840_SPI"
            | "Sound.NRF52840_I2S"
            | "Sound.NRF52840_PDM"
            | "Wireless.NRF52840_Radio"
            | "Miscellaneous.NRF52840_RNG"
            | "Miscellaneous.NRF52840_ECB"
            | "Miscellaneous.NRF52840_EGU"
            | "Timers.NRF52840_RTC"
            | "Timers.NRF52840_Timer"
            | "Timers.NRF52840_Watchdog"
            | "GPIOPort.NRF52840_GPIOTasksEvents"
                if cortex_m4 =>
            {
                nrf_peripherals.push(entry)
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
        if !matches!(
            cpu_type,
            Some(r#""cortex-m0""# | r#""cortex-m0+""# | r#""cortex-m4""# | r#""cortex-m4f""#)
        ) {
            return Err(
                entry.error("expected Cortex-M0, Cortex-M0+, Cortex-M4 or Cortex-M4F cpuType")
            );
        }
        if !cortex_m4 {
            machine
                .bus
                .map_stm32f072_flash()
                .map_err(|e| entry.error(&e))?;
        }
        for peripheral in nrf_peripherals {
            let controller =
                nvic.ok_or_else(|| peripheral.error("nRF52840 peripherals require NVIC"))?;
            nrf52840::map_peripheral(peripheral, controller, &entries, &mut machine.bus)?;
        }
        for attachment in nrf_gpio_attachments {
            nrf52840::map_gpio_attachment(attachment, &entries, &mut machine.bus)?;
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
        let mut uart_irq = None;
        if let Some(uart) = uart.filter(|uart| uart.model == "UART.NS16550") {
            keys(uart, &[])?;
            machine
                .bus
                .map_ns16550(uart.address()?, 0, false)
                .map_err(|e| uart.error(&e))?;
        }
        if let Some(uart) = uart.filter(|uart| uart.model != "UART.NS16550") {
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
        let mut gpio_ports = HashMap::new();
        let mut led_inputs = HashMap::new();
        for entry in stm32_gpios {
            let mut port = make_stm32_gpio(entry)?;
            for (_, text) in &entry.body {
                let Some((outputs, destinations)) = text.split_once("->") else {
                    continue;
                };
                let outputs = irq_list(outputs, 16).map_err(|e| entry.error(&e))?;
                for destination in destinations.split('|') {
                    let (target, inputs) = destination
                        .split_once('@')
                        .ok_or_else(|| entry.error("invalid STM32 GPIO destination"))?;
                    let target = target.trim();
                    if exti.is_some_and(|exti| exti.name == target) {
                        let inputs = irq_list(inputs, exti_lines).map_err(|e| entry.error(&e))?;
                        if inputs.len() != outputs.len() {
                            return Err(entry.error("mismatched GPIO and EXTI wiring widths"));
                        }
                        for (&pin, line) in outputs.iter().zip(inputs) {
                            port.exti_routes[usize::from(pin)] |= 1 << line;
                        }
                    } else if let Some(timer) =
                        stm32_timers.iter().find(|timer| timer.name == target)
                    {
                        let pins = irq_list(inputs, 4).map_err(|e| entry.error(&e))?;
                        if pins.len() != outputs.len() {
                            return Err(entry.error("mismatched GPIO and timer wiring widths"));
                        }
                        let base = timer.address()?;
                        for (&pin, channel) in outputs.iter().zip(pins) {
                            machine
                                .bus
                                .route_gpio_timer_input(port.base, pin, base, channel);
                        }
                    } else if gpio_leds.iter().any(|led| led.name == target) {
                        if inputs.trim() != "0"
                            || outputs.len() != 1
                            || led_inputs.insert(target, (port.base, outputs[0])).is_some()
                        {
                            return Err(entry.error("invalid or duplicate GPIO LED wiring"));
                        }
                    } else {
                        return Err(entry.error("unsupported STM32 GPIO destination"));
                    }
                }
            }
            gpio_ports.insert(entry.name.as_str(), (port.base, port.alternate_count()));
            machine
                .bus
                .map_stm32_gpio(port)
                .map_err(|e| entry.error(&e))?;
        }
        let mut shift_targets = HashMap::new();
        for shift in spi_shifts {
            keys(shift, &[])?;
            let parent = shift
                .registration
                .as_deref()
                .filter(|name| stm32_spis.iter().any(|spi| spi.name == *name))
                .ok_or_else(|| {
                    shift.error("expected shift register attached to an STM32 SPI controller")
                })?;
            if shift_targets
                .insert(parent, (shift.name.as_str(), Stp08 { leds: Vec::new() }))
                .is_some()
            {
                return Err(shift.error("only one target per STM32 SPI controller is supported"));
            }
        }
        for led in gpio_leds {
            let invert = keys(led, &["invert"])?
                .first()
                .map_or(Ok(false), |(_, value)| value.parse())
                .map_err(|_| led.error("invalid LED invert property"))?;
            if let Some((port, pin)) = led_inputs.remove(led.name.as_str()) {
                machine
                    .bus
                    .add_gpio_led(port, GpioLed::new(led.name.clone(), pin, invert));
                continue;
            }
            let (target, pin) = led
                .registration
                .as_deref()
                .and_then(|registration| registration.split_once(char::is_whitespace))
                .ok_or_else(|| {
                    led.error("expected LED wired to a GPIO output or attached to a shift register")
                })?;
            let (_, shift) = shift_targets
                .values_mut()
                .find(|(name, _)| target == *name)
                .ok_or_else(|| {
                    led.error("expected LED wired to a GPIO output or attached to a shift register")
                })?;
            let pin = irq_index(pin, 8).map_err(|e| led.error(&e))?;
            shift.leds.push(GpioLed::new(led.name.clone(), pin, invert));
        }
        for button in gpio_buttons {
            let mut invert = None;
            let mut target = None;
            for (_, text) in &button.body {
                if let Some((source, destination)) = text.split_once("->")
                    && matches!(source.trim(), "" | "0" | "IRQ")
                    && let Some((port, pin)) = destination.split_once('@')
                    && target.is_none()
                {
                    let (base, _) = gpio_ports
                        .get(port.trim())
                        .ok_or_else(|| button.error("unknown button GPIO port"))?;
                    let pin = irq_index(pin, 16).map_err(|e| button.error(&e))?;
                    target = Some((*base, pin));
                } else if let Some(value) = text.strip_prefix("invert:")
                    && invert.is_none()
                {
                    invert = Some(
                        value
                            .trim()
                            .parse()
                            .map_err(|_| button.error("invalid button invert property"))?,
                    );
                } else {
                    return Err(button.error("unsupported or duplicate button property or wiring"));
                }
            }
            let (port, pin) =
                target.ok_or_else(|| button.error("expected button wired to a GPIO input"))?;
            machine.bus.add_gpio_button(GpioButton::new(
                button.name.clone(),
                port,
                pin,
                invert.unwrap_or(false),
            ));
        }
        if let Some(rtc) = stm32_rtc {
            let mut frequency = None;
            let mut errata = None;
            let mut routes = [None; 2];
            for (_, text) in &rtc.body {
                if let Some((source, destination)) = text.split_once("->") {
                    let slot = match source.trim() {
                        "AlarmIRQ" => &mut routes[0],
                        "WakeupIRQ" => &mut routes[1],
                        _ => return Err(rtc.error("unsupported STM32 RTC output")),
                    };
                    let (target, pin) = destination
                        .split_once('@')
                        .ok_or_else(|| rtc.error("invalid STM32 RTC output target"))?;
                    if slot.is_some() {
                        return Err(rtc.error("duplicate STM32 RTC output"));
                    }
                    *slot = Some(if nvic.is_some_and(|nvic| nvic.name == target.trim()) {
                        Stm32Irq::Nvic(irq_index(pin, 32).map_err(|e| rtc.error(&e))?)
                    } else if exti.is_some_and(|exti| exti.name == target.trim()) {
                        Stm32Irq::Exti(irq_index(pin, exti_lines).map_err(|e| rtc.error(&e))?)
                    } else if let Some(&(count, irq)) = combined_routes.get(target.trim()) {
                        irq_index(pin, count).map_err(|e| rtc.error(&e))?;
                        Stm32Irq::Nvic(irq)
                    } else {
                        return Err(rtc.error("unsupported STM32 RTC interrupt target"));
                    });
                } else if let Some((key, value)) = text.split_once(':') {
                    match key.trim() {
                        "wakeupTimerFrequency" if frequency.is_none() => {
                            let value = number(value.trim()).map_err(|e| rtc.error(&e))?;
                            if value == 0 {
                                return Err(rtc.error("RTC wakeupTimerFrequency must be positive"));
                            }
                            frequency = Some(value);
                        }
                        "WakeupTimerRegisterErrata" if errata.is_none() => {
                            errata = Some(
                                value
                                    .trim()
                                    .parse()
                                    .map_err(|_| rtc.error("invalid WakeupTimerRegisterErrata"))?,
                            );
                        }
                        _ => return Err(rtc.error("unsupported or duplicate STM32 RTC property")),
                    }
                } else {
                    return Err(rtc.error("expected STM32 RTC property or wiring"));
                }
            }
            let mut device = Stm32Rtc::new(
                rtc.address()?,
                frequency.unwrap_or(32768),
                errata.unwrap_or(false),
            );
            device.routes = routes;
            machine
                .bus
                .map_stm32_rtc(device)
                .map_err(|e| rtc.error(&e))?;
        }
        let dma_channels = if let Some(dma) = stm32_dma {
            let count = dma
                .body
                .iter()
                .find_map(|(_, text)| text.strip_prefix("numberOfChannels:"))
                .ok_or_else(|| dma.error("missing DMA numberOfChannels"))?;
            let count = number(count.trim()).map_err(|e| dma.error(&e))?;
            if !(1..=7).contains(&count) {
                return Err(dma.error("STM32 DMA supports 1–7 channels"));
            }
            let mut outputs = vec![None; count as usize];
            let mut count_seen = false;
            for (_, text) in &dma.body {
                if text.starts_with("numberOfChannels:") && !count_seen {
                    count_seen = true;
                    continue;
                }
                let Some((channels, destination)) = text.split_once("->") else {
                    return Err(dma.error("expected DMA interrupt wiring"));
                };
                let Some((target, pins)) = destination.split_once('@') else {
                    return Err(dma.error("invalid DMA interrupt target"));
                };
                let routes = if nvic.is_some_and(|nvic| nvic.name == target.trim()) {
                    irq_list(pins, 32).map_err(|e| dma.error(&e))?
                } else if let Some(&(size, irq)) = combined_routes.get(target.trim()) {
                    irq_list(pins, size)
                        .map_err(|e| dma.error(&e))?
                        .into_iter()
                        .map(|_| irq)
                        .collect()
                } else {
                    return Err(dma.error("unsupported DMA interrupt target"));
                };
                let channels = irq_list(channels, count).map_err(|e| dma.error(&e))?;
                if channels.len() != routes.len() {
                    return Err(dma.error("mismatched DMA wiring widths"));
                }
                for (channel, irq) in channels.into_iter().zip(routes) {
                    if outputs[usize::from(channel)].replace(irq).is_some() {
                        return Err(dma.error("duplicate DMA output"));
                    }
                }
            }
            machine
                .bus
                .map_stm32_dma(Stm32Dma::new(dma.address()?, outputs))
                .map_err(|e| dma.error(&e))?;
            count
        } else {
            0
        };
        if let Some(adc) = stm32_adc {
            let mut volts: Option<f64> = None;
            let mut dma_ref = None;
            let mut dma_channel = None;
            let mut frequency = None;
            let mut irq = None;
            for (_, text) in &adc.body {
                if let Some((source, destination)) = text.split_once("->")
                    && matches!(source.trim(), "IRQ" | "")
                    && let Some((target, pin)) = destination.split_once('@')
                    && irq.is_none()
                {
                    irq = Some(if nvic.is_some_and(|nvic| nvic.name == target.trim()) {
                        irq_index(pin, 32).map_err(|e| adc.error(&e))?
                    } else if let Some(&(count, irq)) = combined_routes.get(target.trim()) {
                        irq_index(pin, count).map_err(|e| adc.error(&e))?;
                        irq
                    } else {
                        return Err(adc.error("unsupported STM32F0 ADC interrupt target"));
                    });
                } else if let Some((key, value)) = text.split_once(':') {
                    match key.trim() {
                        "dmaPeripheral" if dma_ref.is_none() => dma_ref = Some(value.trim()),
                        "dmaChannel" if dma_channel.is_none() => dma_channel = Some(value.trim()),
                        "referenceVoltage" if volts.is_none() => {
                            volts = Some(
                                value
                                    .trim()
                                    .parse()
                                    .map_err(|_| adc.error("invalid ADC referenceVoltage"))?,
                            );
                        }
                        "externalEventFrequency" if frequency.is_none() => {
                            frequency = Some(number(value.trim()).map_err(|e| adc.error(&e))?);
                        }
                        _ => return Err(adc.error("unsupported or duplicate STM32F0 ADC property")),
                    }
                } else {
                    return Err(adc.error("expected STM32F0 ADC property or interrupt wiring"));
                }
            }
            let volts = volts
                .filter(|v| v.is_finite() && *v > 0.0)
                .ok_or_else(|| adc.error("referenceVoltage must be positive and finite"))?;
            let frequency = frequency
                .filter(|f| *f > 0)
                .ok_or_else(|| adc.error("externalEventFrequency must be positive"))?;
            let mut device = Stm32F0Adc::new(adc.address()?, irq, volts, frequency);
            match (dma_ref, dma_channel) {
                (Some(name), Some(channel)) if stm32_dma.is_some_and(|dma| dma.name == name) => {
                    device.dma_request =
                        dma_input(channel, dma_channels).map_err(|e| adc.error(&e))?;
                }
                (None, None) => {}
                _ => return Err(adc.error("invalid STM32 ADC DMA configuration")),
            }
            machine
                .bus
                .map_stm32f0_adc(device)
                .map_err(|e| adc.error(&e))?;
        }
        for can in stm32_cans {
            let (base, size) = sysbus_range(can, Stm32Can::SIZE)?;
            let mut routes = [0; 4];
            for (_, text) in &can.body {
                let (source, outputs) = text
                    .split_once("->")
                    .ok_or_else(|| can.error("expected STM32 CAN output wiring"))?;
                let source = irq_list(source, 4).map_err(|e| can.error(&e))?;
                for output in outputs.split('|') {
                    let (target, pins) = output
                        .split_once('@')
                        .ok_or_else(|| can.error("invalid STM32 CAN output target"))?;
                    let pins = if nvic.is_some_and(|nvic| nvic.name == target.trim()) {
                        irq_list(pins, 32).map_err(|e| can.error(&e))?
                    } else if let Some(&(count, irq)) = combined_routes.get(target.trim()) {
                        irq_list(pins, count)
                            .map_err(|e| can.error(&e))?
                            .into_iter()
                            .map(|_| irq)
                            .collect()
                    } else {
                        return Err(can.error("unsupported STM32 CAN output target"));
                    };
                    if source.len() != pins.len() {
                        return Err(can.error("mismatched STM32 CAN wiring widths"));
                    }
                    for (&signal, line) in source.iter().zip(pins) {
                        routes[usize::from(signal)] |= 1 << line;
                    }
                }
            }
            machine
                .bus
                .map_stm32_can(Stm32Can::new(base, size, routes))
                .map_err(|e| can.error(&e))?;
        }
        let mut eeproms = HashMap::<_, Vec<_>>::new();
        for eeprom in i2c_eeproms {
            let (target, address) = eeprom
                .registration
                .as_deref()
                .and_then(|s| s.split_once(char::is_whitespace))
                .ok_or_else(|| {
                    eeprom.error("expected EEPROM registered on an I²C controller with an address")
                })?;
            if cortex_m4
                && entries
                    .iter()
                    .any(|i2c| i2c.model == "I2C.NRF52840_I2C" && i2c.name == target)
            {
                // The nRF platform handler attaches targets when it maps the controller.
                continue;
            }
            if !stm32_i2cs.iter().any(|i2c| i2c.name == target) {
                return Err(eeprom.error("unknown EEPROM I²C controller"));
            }
            let address = number(address.trim()).map_err(|e| eeprom.error(&e))?;
            eeproms.entry(target).or_default().push((eeprom, address));
        }
        for i2c in stm32_i2cs {
            let mut device = Stm32I2c::new(i2c.address()?);
            for (_, text) in &i2c.body {
                let (source, destinations) = text
                    .split_once("->")
                    .ok_or_else(|| i2c.error("expected STM32 I²C wiring"))?;
                for destination in destinations.split('|') {
                    let (target, pin) = destination
                        .split_once('@')
                        .ok_or_else(|| i2c.error("invalid STM32 I²C output destination"))?;
                    let target = target.trim();
                    match source.trim() {
                        "DmaReceive" | "DmaSend"
                            if stm32_dma.is_some_and(|dma| dma.name == target) =>
                        {
                            let mask = dma_input(pin, dma_channels).map_err(|e| i2c.error(&e))?;
                            if source.trim() == "DmaReceive" {
                                device.dma_rx |= mask;
                            } else {
                                device.dma_tx |= mask;
                            }
                        }
                        "" | "EventInterrupt" | "EventIRQ" if device.irq.is_none() => {
                            device.irq = Some(if nvic.is_some_and(|nvic| nvic.name == target) {
                                irq_index(pin, 32).map_err(|e| i2c.error(&e))?
                            } else if let Some(&(count, output)) = combined_routes.get(target) {
                                irq_index(pin, count).map_err(|e| i2c.error(&e))?;
                                output
                            } else {
                                return Err(i2c.error("unsupported STM32 I²C interrupt target"));
                            });
                        }
                        _ => return Err(i2c.error("unsupported or duplicate STM32 I²C wiring")),
                    }
                }
            }
            for (entry, address) in eeproms.remove(i2c.name.as_str()).unwrap_or_default() {
                device
                    .attach(address, i2c_eeprom(entry)?)
                    .map_err(|e| entry.error(&e))?;
            }
            machine
                .bus
                .map_stm32_i2c(device)
                .map_err(|e| i2c.error(&e))?;
        }
        for spi in stm32_spis {
            let mut series = None;
            let mut capacity = None;
            let mut irq = None;
            let mut dma_rx = 0;
            let mut dma_tx = 0;
            for (_, text) in &spi.body {
                if let Some((key, value)) = text.split_once(':') {
                    match key.trim() {
                        "series" if series.is_none() => series = Some(value.trim()),
                        "bufferCapacity" if capacity.is_none() => {
                            capacity = Some(number(value.trim()).map_err(|e| spi.error(&e))?);
                        }
                        _ => return Err(spi.error("unsupported or duplicate SPI property")),
                    }
                    continue;
                }
                let (source, destinations) = text
                    .split_once("->")
                    .ok_or_else(|| spi.error("expected SPI property or wiring"))?;
                for destination in destinations.split('|') {
                    let (target, pin) = destination
                        .split_once('@')
                        .ok_or_else(|| spi.error("invalid SPI output destination"))?;
                    let target = target.trim();
                    match source.trim() {
                        "DMAReceive" | "DMASend"
                            if stm32_dma.is_some_and(|dma| dma.name == target) =>
                        {
                            let mask = dma_input(pin, dma_channels).map_err(|e| spi.error(&e))?;
                            if source.trim() == "DMAReceive" {
                                dma_rx |= mask;
                            } else {
                                dma_tx |= mask;
                            }
                        }
                        "IRQ" | "" if irq.is_none() => {
                            irq = Some(if nvic.is_some_and(|nvic| nvic.name == target) {
                                irq_index(pin, 32).map_err(|e| spi.error(&e))?
                            } else if let Some(&(count, output)) = combined_routes.get(target) {
                                irq_index(pin, count).map_err(|e| spi.error(&e))?;
                                output
                            } else {
                                return Err(spi.error("unsupported SPI interrupt target"));
                            });
                        }
                        _ => return Err(spi.error("unsupported or duplicate SPI wiring")),
                    }
                }
            }
            if series != Some("STM32Series.F0") || capacity == Some(0) {
                return Err(spi.error("expected STM32Series.F0 SPI with positive bufferCapacity"));
            }
            let mut device = Stm32F0Spi::new(spi.address()?, capacity.unwrap_or(4));
            device.irq = irq;
            device.shift_register = shift_targets
                .remove(spi.name.as_str())
                .map(|(_, shift)| shift);
            device.dma_rx = dma_rx;
            device.dma_tx = dma_tx;
            machine
                .bus
                .map_stm32f0_spi(device)
                .map_err(|e| spi.error(&e))?;
        }
        for timer in stm32_timers {
            let mut frequency = None;
            let mut limit = None;
            let mut routes = [0; 8];
            let mut dma_routes = [0; 8];
            let mut gpio_outputs = Vec::new();
            for (_, text) in &timer.body {
                if let Some((source, destinations)) = text.split_once("->") {
                    for destination in destinations.split('|') {
                        let (target, pin) = destination
                            .split_once('@')
                            .ok_or_else(|| timer.error("invalid STM32 timer wiring"))?;
                        if let Some((port, gpio_pin)) = target.trim().split_once('#') {
                            let &(port, functions) = gpio_ports
                                .get(port)
                                .ok_or_else(|| timer.error("unknown timer GPIO output port"))?;
                            let gpio_pin = irq_index(gpio_pin, 16).map_err(|e| timer.error(&e))?;
                            let function =
                                irq_index(pin, functions).map_err(|e| timer.error(&e))?;
                            let channel = irq_index(source, 4).map_err(|e| timer.error(&e))?;
                            gpio_outputs.push((
                                channel,
                                TimerPin {
                                    port,
                                    pin: gpio_pin,
                                    function,
                                },
                            ));
                            continue;
                        }
                        if stm32_dma.is_some_and(|dma| dma.name == target.trim()) {
                            let source = source.trim();
                            let sources = if let Some(list) =
                                source.strip_prefix('[').and_then(|s| s.strip_suffix(']'))
                            {
                                list.split(',').map(str::trim).collect::<Vec<_>>()
                            } else {
                                vec![source]
                            };
                            let pins =
                                irq_list(pin, dma_channels + 1).map_err(|e| timer.error(&e))?;
                            if pins.len() != sources.len() || pins.contains(&0) {
                                return Err(
                                    timer.error("invalid STM32 timer DMA wiring width or channel")
                                );
                            }
                            for (source, pin) in sources.into_iter().zip(pins) {
                                let flags =
                                    stm32_timer_output(source).map_err(|e| timer.error(e))?;
                                for (index, route) in dma_routes.iter_mut().enumerate() {
                                    if flags & (1 << index) != 0 {
                                        *route |= 1 << (pin - 1);
                                    }
                                }
                            }
                            continue;
                        }
                        let flags =
                            stm32_timer_output(source.trim()).map_err(|e| timer.error(e))?;
                        let irq = if nvic.is_some_and(|nvic| nvic.name == target.trim()) {
                            irq_index(pin, 32).map_err(|e| timer.error(&e))?
                        } else if let Some(&(count, irq)) = combined_routes.get(target.trim()) {
                            irq_index(pin, count).map_err(|e| timer.error(&e))?;
                            irq
                        } else {
                            return Err(timer.error("unsupported STM32 timer interrupt target"));
                        };
                        for (index, route) in routes.iter_mut().enumerate() {
                            if flags & (1 << index) != 0 {
                                *route |= 1 << irq;
                            }
                        }
                    }
                } else if let Some((key, value)) = text.split_once(':') {
                    let value = number(value.trim()).map_err(|e| timer.error(&e))?;
                    match key.trim() {
                        "frequency" if frequency.is_none() && value != 0 => frequency = Some(value),
                        "initialLimit" if limit.is_none() => limit = Some(value),
                        _ => {
                            return Err(timer
                                .error("unsupported, duplicate, or invalid STM32 timer property"));
                        }
                    }
                } else {
                    return Err(timer.error("expected STM32 timer property or interrupt wiring"));
                }
            }
            let mut device = Stm32Timer::new(
                timer.address()?,
                frequency.ok_or_else(|| timer.error("missing STM32 timer frequency"))?,
                limit.ok_or_else(|| timer.error("missing STM32 timer initialLimit"))?,
                routes,
            );
            device.dma_routes = dma_routes;
            device.gpio_outputs = gpio_outputs;
            machine
                .bus
                .map_stm32_timer(device)
                .map_err(|e| timer.error(&e))?;
        }
        for uart in stm32_uarts {
            let mut irq = None;
            let mut low_power = None;
            let mut frequency = None;
            let mut dma_rx = 0;
            let mut dma_tx = 0;
            for (_, text) in &uart.body {
                if let Some((source, destinations)) = text.split_once("->") {
                    let source = source.trim();
                    for destination in destinations.split('|') {
                        let (name, line) = destination
                            .split_once('@')
                            .ok_or_else(|| uart.error("invalid USART output destination"))?;
                        let name = name.trim();
                        if stm32_dma.is_some_and(|dma| dma.name == name) {
                            let mask =
                                dma_input(line.trim(), dma_channels).map_err(|e| uart.error(&e))?;
                            match source {
                                "ReceiveDmaRequest" => dma_rx |= mask,
                                "IRQ" | "TransmitDmaRequest" | "" => dma_tx |= mask,
                                _ => return Err(uart.error("unsupported USART DMA output")),
                            }
                            continue;
                        }
                        if !matches!(source, "IRQ" | "") || irq.is_some() {
                            return Err(
                                uart.error("unsupported or duplicate USART interrupt wiring")
                            );
                        }
                        irq = Some(if nvic.is_some_and(|controller| controller.name == name) {
                            Stm32Irq::Nvic(irq_index(line, 32).map_err(|e| uart.error(&e))?)
                        } else if exti.is_some_and(|controller| controller.name == name) {
                            Stm32Irq::Exti(irq_index(line, exti_lines).map_err(|e| uart.error(&e))?)
                        } else if let Some(&(pins, output)) = combined_routes.get(name) {
                            irq_index(line, pins).map_err(|e| uart.error(&e))?;
                            Stm32Irq::Nvic(output)
                        } else {
                            return Err(uart.error("unsupported USART IRQ target"));
                        });
                    }
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
            let mut device = Stm32Usart::new(uart.address()?, irq, low_power.unwrap_or(false));
            device.dma_rx = dma_rx;
            device.dma_tx = dma_tx;
            machine
                .bus
                .map_stm32_usart(device)
                .map_err(|e| uart.error(&e))?;
        }
        machine
            .bus
            .map_nvic(
                priority.unwrap_or(if cortex_m4 { 0xe0 } else { 0xc0 }),
                uart_irq,
                cortex_m4,
            )
            .map_err(|e| entry.error(&e))?;
        machine.primary.name = Some(entry.name.clone());
        if let Some(handler) = semihosting_handler {
            if handler.registration.as_deref() != Some(entry.name.as_str()) {
                return Err(handler.error("expected the Cortex-M CPU as parent"));
            }
            let mut model = crate::semihosting::Semihosting::default();
            let mut combined = false;
            for uart in semihosting_uarts {
                let registration: Vec<_> = uart
                    .registration
                    .as_deref()
                    .unwrap_or_default()
                    .split_whitespace()
                    .collect();
                match registration.as_slice() {
                    [parent, ".InputOutputError"]
                        if *parent == handler.name && !model.console && !model.split_error =>
                    {
                        model.console = true;
                        combined = true;
                    }
                    [parent, ".InputOutput"] if *parent == handler.name && !model.console => {
                        model.console = true
                    }
                    [parent, ".Error"]
                        if *parent == handler.name && !combined && !model.split_error =>
                    {
                        model.split_error = true
                    }
                    _ => {
                        return Err(
                            uart.error("invalid or duplicate semihosting UART registration")
                        );
                    }
                }
            }
            if model.split_error && !model.console {
                return Err(handler.error("a separate error UART requires an input/output UART"));
            }
            machine.bus.semihosting = Some(model);
        } else if let Some(uart) = semihosting_uarts.first() {
            return Err(uart.error("a semihosting UART requires a handler"));
        }
        machine.primary.configure_cortex_m(vectors, cortex_m4);
        return Ok(machine);
    }
    if let Some(nvic) = nvic {
        return Err(nvic.error("NVIC requires a Cortex-M CPU"));
    }
    if let Some(rtc) = stm32_rtc {
        return Err(rtc.error("STM32 RTC requires a Cortex-M platform"));
    }
    if let Some(dma) = stm32_dma {
        return Err(dma.error("STM32 DMA requires a Cortex-M platform"));
    }
    if let Some(adc) = stm32_adc {
        return Err(adc.error("STM32F0 ADC requires a Cortex-M platform"));
    }
    if let Some(device) = gpio_leds.first().or(gpio_buttons.first()) {
        return Err(device.error("STM32 board wiring requires a Cortex-M platform"));
    }
    for entry in stm32_gpios {
        if entry.body.iter().any(|(_, text)| text.contains("->")) {
            return Err(entry.error("STM32 GPIO interrupt wiring requires a Cortex-M platform"));
        }
        machine
            .bus
            .map_stm32_gpio(make_stm32_gpio(entry)?)
            .map_err(|e| entry.error(&e))?;
    }
    if let Some(can) = stm32_cans.first() {
        return Err(can.error("STM32 CAN requires a Cortex-M platform"));
    }
    if let Some(i2c) = stm32_i2cs.first().or(i2c_eeproms.first()) {
        return Err(i2c.error("STM32 I²C requires a Cortex-M platform"));
    }
    if let Some(spi) = stm32_spis.first().or(spi_shifts.first()) {
        return Err(spi.error("STM32F0 SPI requires a Cortex-M platform"));
    }
    if let Some(timer) = stm32_timers.first() {
        return Err(timer.error("STM32 timer requires a Cortex-M platform"));
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

fn sysbus_range(entry: &Entry, default_size: u32) -> Result<(u32, u32), String> {
    let registration = entry
        .registration
        .as_deref()
        .and_then(|s| s.strip_prefix("sysbus"))
        .filter(|s| s.starts_with(char::is_whitespace))
        .ok_or_else(|| entry.error("expected peripheral registered on sysbus"))?
        .trim();
    let (base, size) = if let Some(range) = registration
        .strip_prefix('<')
        .and_then(|s| s.strip_suffix('>'))
    {
        let (base, size) = range
            .split_once(',')
            .and_then(|(base, size)| Some((base.trim(), size.trim().strip_prefix('+')?.trim())))
            .ok_or_else(|| entry.error("expected registration range <base, +size>"))?;
        (number(base), number(size))
    } else {
        (number(registration), Ok(default_size))
    };
    Ok((
        base.map_err(|e| entry.error(&e))?,
        size.map_err(|e| entry.error(&e))?,
    ))
}

fn make_stm32_gpio(entry: &Entry) -> Result<Stm32Gpio, String> {
    let (base, size) = sysbus_range(entry, 0x400)?;
    let mut reset = [0; 3];
    let mut alternate_count = 16;
    let mut inverted_pins = None;
    let mut seen = HashSet::new();
    for (_, text) in &entry.body {
        if text.contains("->") {
            continue;
        }
        let (key, value) = text
            .split_once(':')
            .ok_or_else(|| entry.error("expected STM32 GPIO property"))?;
        let key = key.trim();
        if !seen.insert(key) {
            return Err(entry.error("duplicate STM32 GPIO property"));
        }
        if key == "invertedAFPins" {
            inverted_pins = Some(value.trim());
            continue;
        }
        let value = number(value.trim()).map_err(|e| entry.error(&e))?;
        match key {
            "numberOfAFs" if (1..=16).contains(&value) => alternate_count = value,
            "numberOfAFs" => return Err(entry.error("numberOfAFs must be in 1–16")),
            "modeResetValue" => reset[0] = value,
            "outputSpeedResetValue" => reset[1] = value,
            "pullUpPullDownResetValue" => reset[2] = value,
            _ => return Err(entry.error("unsupported STM32 GPIO property")),
        }
    }
    let mut port = Stm32Gpio::new(base, size, reset, alternate_count);
    if let Some(pins) = inverted_pins {
        let mut remaining = pins
            .strip_prefix('[')
            .and_then(|s| s.strip_suffix(']'))
            .ok_or_else(|| entry.error("invalid invertedAFPins list"))?
            .trim();
        while !remaining.is_empty() {
            let (item, tail) = remaining
                .strip_prefix('[')
                .and_then(|s| s.split_once(']'))
                .ok_or_else(|| entry.error("invalid invertedAFPins entry"))?;
            let (pin, functions) = item
                .split_once(',')
                .ok_or_else(|| entry.error("expected pin and functions in invertedAFPins"))?;
            let pin = irq_index(pin, 16).map_err(|e| entry.error(&e))?;
            for function in functions.split(',') {
                let function = irq_index(function, alternate_count).map_err(|e| entry.error(&e))?;
                port.invert_alternate(pin, function);
            }
            remaining = tail.trim_start();
            if !remaining.is_empty() {
                remaining = remaining
                    .strip_prefix(',')
                    .ok_or_else(|| entry.error("expected comma in invertedAFPins"))?
                    .trim_start();
            }
        }
    }
    Ok(port)
}

fn i2c_eeprom(entry: &Entry) -> Result<Eeprom, String> {
    let mut size = 0;
    let mut address_bits = 8;
    let mut writable = true;
    let mut page_size = 64;
    for (key, value) in keys(entry, &["size", "addressBitSize", "writable", "pageSize"])? {
        if key == "writable" {
            writable = value
                .parse()
                .map_err(|_| entry.error("invalid EEPROM writable value"))?;
            continue;
        }
        let value = number(value).map_err(|e| entry.error(&e))? as usize;
        match key {
            "size" => size = value,
            "addressBitSize" => address_bits = value,
            _ => page_size = value,
        }
    }
    if !(1..=65536).contains(&size)
        || !matches!(address_bits, 8 | 16)
        || !page_size.is_power_of_two()
        || page_size > 65536
    {
        return Err(entry.error("invalid I²C EEPROM size, addressBitSize or pageSize"));
    }
    Ok(Eeprom::new(size, address_bits, writable, page_size))
}

fn stm32_timer_output(source: &str) -> Result<u32, &'static str> {
    Ok(match source {
        "" | "IRQ" => 0xff,
        "UpdateInterrupt" => 1,
        "0" => 1 << 1,
        "1" => 1 << 2,
        "2" => 1 << 3,
        "3" => 1 << 4,
        "CaptureCompareInterrupt" => 0x1e,
        "CommutationInterrupt" => 0x20,
        "TriggerInterrupt" => 0x40,
        "BreakInterrupt" => 0x80,
        _ => return Err("unsupported STM32 timer output"),
    })
}

fn dma_input(line: &str, count: u32) -> Result<u8, String> {
    let channel = number(line.trim())?;
    if !(1..=count).contains(&channel) {
        return Err(format!("DMA input channel {channel} outside 1–{count}"));
    }
    Ok(1 << (channel - 1))
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
