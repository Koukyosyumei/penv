use super::{Entry, number};
use crate::{
    bus::Bus,
    nrf52840::{Peripheral, SpiMuxPort},
    stm32_gpio_board::{GpioButton, GpioLed},
};

pub(super) fn apply_overrides(entries: &mut Vec<Entry>) -> Result<(), String> {
    let mut index = 0;
    while index < entries.len() {
        let entry = &entries[index];
        let Some(parent) = entries[..index].iter().position(|old| {
            old.name == entry.name
                && matches!(
                    old.model.as_str(),
                    "UART.NRF52840_UART" | "GPIOPort.NRF52840_GPIO"
                )
        }) else {
            index += 1;
            continue;
        };
        if !entry.model.is_empty() || entry.registration.is_some() {
            index += 1;
            continue;
        }
        if entries[parent].model == "UART.NRF52840_UART" {
            if !matches!(entry.body.as_slice(), [(_, property)] if property.starts_with("easyDMA:"))
            {
                return Err(entry.error("expected an easyDMA UART override"));
            }
            entries[parent]
                .body
                .retain(|(_, value)| !value.starts_with("easyDMA:"));
        }
        let mut entry = entries.remove(index);
        entries[parent].body.append(&mut entry.body);
    }
    Ok(())
}

fn acceleration(entry: &Entry) -> Result<[i16; 3], String> {
    let mut values = [0; 3];
    let axes = ["accelerationX", "accelerationY", "accelerationZ"];
    for (axis, value) in super::keys(entry, &axes)? {
        let gs = value
            .parse::<f64>()
            .map_err(|_| entry.error("invalid acceleration"))?;
        if !gs.is_finite() || !(-200.0..=200.0).contains(&gs) {
            return Err(entry.error("acceleration must be within ±200 g"));
        }
        let index = axes.iter().position(|&name| name == axis).unwrap();
        values[index] = (gs * 10.0).round() as i16;
    }
    Ok(values)
}

pub(super) fn lis2ds12_config(
    entry: &Entry,
    entries: &[Entry],
) -> Result<([f64; 3], u32, u8), String> {
    let (port, address) = entry
        .registration
        .as_deref()
        .and_then(|registration| registration.split_once(char::is_whitespace))
        .ok_or_else(|| entry.error("expected LIS2DS12 registered on TWI with an address"))?;
    number(address.trim()).map_err(|e| entry.error(&e))?;
    if !entries
        .iter()
        .any(|parent| parent.name == port && parent.model == "I2C.NRF52840_I2C")
    {
        return Err(entry.error("expected LIS2DS12 registered on nRF52840 TWI"));
    }
    let axes = ["accelerationX", "accelerationY", "accelerationZ"];
    let mut values = [0.0; 3];
    let mut irq = None;
    for (_, line) in &entry.body {
        if let Some((key, value)) = line.split_once(':')
            && let Some(index) = axes.iter().position(|&axis| axis == key.trim())
        {
            let value = value
                .trim()
                .parse()
                .map_err(|_| entry.error("invalid LIS2DS12 acceleration"))?;
            if !(-19.5..=19.5).contains(&value) {
                return Err(entry.error("LIS2DS12 acceleration is outside the model's input range"));
            }
            values[index] = value;
        } else if let Some((source, destination)) = line.split_once("->")
            && source.trim() == "IRQ"
            && irq.is_none()
            && let Some((name, pin)) = destination.trim().split_once('@')
        {
            let port = entries
                .iter()
                .find(|gpio| gpio.name == name && gpio.model == "GPIOPort.NRF52840_GPIO")
                .ok_or_else(|| entry.error("invalid LIS2DS12 IRQ GPIO port"))?;
            let base = port.address()?;
            let pin = number(pin).map_err(|e| entry.error(&e))?;
            if pin >= if base == 0x5000_0500 { 32 } else { 16 } {
                return Err(entry.error("invalid LIS2DS12 IRQ pin"));
            }
            irq = Some((base, pin as u8));
        } else {
            return Err(entry.error("unsupported LIS2DS12 option"));
        }
    }
    let (gpio, pin) = irq.ok_or_else(|| entry.error("missing LIS2DS12 IRQ wiring"))?;
    Ok((values, gpio, pin))
}

pub(super) fn validate_bme280(entry: &Entry, entries: &[Entry]) -> Result<(), String> {
    super::keys(entry, &[])?;
    validate_twi_target(entry, entries)
}

fn validate_twi_target(entry: &Entry, entries: &[Entry]) -> Result<(), String> {
    let (port, address) = entry
        .registration
        .as_deref()
        .and_then(|registration| registration.split_once(char::is_whitespace))
        .ok_or_else(|| entry.error("expected a TWI controller and address"))?;
    number(address.trim()).map_err(|e| entry.error(&e))?;
    if !entries
        .iter()
        .any(|parent| parent.name == port && parent.model == "I2C.NRF52840_I2C")
    {
        return Err(entry.error("expected target registered on nRF52840 TWI"));
    }
    Ok(())
}

pub(super) fn bmp180_config(entry: &Entry, entries: &[Entry]) -> Result<(f64, u32), String> {
    validate_twi_target(entry, entries)?;
    let mut temperature = 0.0;
    let mut pressure = 0;
    for (property, value) in super::keys(entry, &["temperature", "uncompensatedPressure"])? {
        if property == "temperature" {
            temperature = value
                .parse()
                .map_err(|_| entry.error("invalid BMP180 temperature"))?;
            if !(-40.0..=85.0).contains(&temperature) {
                return Err(entry.error("BMP180 temperature is outside the model's range"));
            }
        } else {
            pressure = number(value).map_err(|e| entry.error(&e))?;
            if pressure > 0x7ffff {
                return Err(entry.error("BMP180 uncompensated pressure exceeds 19 bits"));
            }
        }
    }
    Ok((temperature, pressure))
}

pub(super) fn lsm303_config(
    entry: &Entry,
    entries: &[Entry],
) -> Result<([f64; 3], Option<(u32, [u8; 2])>), String> {
    validate_twi_target(entry, entries)?;
    let accel = entry.model == "Sensors.LSM303DLHC_Accelerometer";
    let axes = if accel {
        ["accelerationX", "accelerationY", "accelerationZ"]
    } else {
        ["magneticFieldX", "magneticFieldY", "magneticFieldZ"]
    };
    let mut values = [0.0; 3];
    let mut irq = None;
    for (_, line) in &entry.body {
        if let Some((key, value)) = line.split_once(':')
            && let Some(index) = axes.iter().position(|&axis| axis == key.trim())
        {
            let value = value
                .trim()
                .parse()
                .map_err(|_| entry.error("invalid LSM303DLHC axis value"))?;
            let limit = if accel { 19.5 } else { 29.5 };
            if !(-limit..=limit).contains(&value) {
                return Err(entry.error("LSM303DLHC axis value out of range"));
            }
            values[index] = value;
        } else if let Some((source, destination)) = line.split_once("->")
            && source.trim() == "[IRQ0,IRQ1]"
            && accel
            && irq.is_none()
            && let Some((name, pins)) = destination.trim().split_once('@')
            && let Some(pins) = pins.strip_prefix('[').and_then(|s| s.strip_suffix(']'))
            && let Some((first, second)) = pins.split_once(',')
        {
            let port = entries
                .iter()
                .find(|gpio| gpio.name == name && gpio.model == "GPIOPort.NRF52840_GPIO")
                .ok_or_else(|| entry.error("invalid LSM303DLHC IRQ port"))?;
            let gpio = port.address()?;
            let first = number(first.trim()).map_err(|e| entry.error(&e))?;
            let second = number(second.trim()).map_err(|e| entry.error(&e))?;
            let limit = if gpio == 0x5000_0500 { 32 } else { 16 };
            if first >= limit || second >= limit || first == second {
                return Err(entry.error("invalid LSM303DLHC IRQ pins"));
            }
            irq = Some((gpio, [first as u8, second as u8]));
        } else {
            return Err(entry.error("unsupported LSM303DLHC property or wiring"));
        }
    }
    if accel && irq.is_none() {
        return Err(entry.error("missing LSM303DLHC IRQ wiring"));
    }
    Ok((values, irq))
}

pub(super) fn lsm330_config(entry: &Entry, entries: &[Entry]) -> Result<([f64; 3], bool), String> {
    validate_twi_target(entry, entries)?;
    let gyro = entry.model == "Sensors.LSM330_Gyroscope";
    let keys = if gyro {
        ["angularRateX", "angularRateY", "angularRateZ"]
    } else {
        ["accelerationX", "accelerationY", "accelerationZ"]
    };
    let mut values = [0.0; 3];
    for (property, value) in super::keys(entry, &keys)? {
        let axis = keys.iter().position(|&key| key == property).unwrap();
        values[axis] = value
            .parse::<f64>()
            .ok()
            .filter(|n| n.is_finite())
            .ok_or_else(|| entry.error("invalid LSM330 axis value"))?;
    }
    Ok((values, gyro))
}

pub(super) fn camera_routes<'a>(
    entry: &'a Entry,
    entries: &[Entry],
) -> Result<(&'a str, &'a str, u32), String> {
    let [(_, spi), (_, twi), (_, end)] = entry.body.as_slice() else {
        return Err(entry.error("expected ArduCAM SPI and TWI registrations"));
    };
    let spi = spi
        .strip_suffix(';')
        .ok_or_else(|| entry.error("invalid ArduCAM SPI registration"))?
        .trim();
    let (twi, address) = twi
        .split_once(char::is_whitespace)
        .ok_or_else(|| entry.error("invalid ArduCAM TWI registration"))?;
    if entry.registration.as_deref() != Some("{")
        || end != "}"
        || !entries
            .iter()
            .any(|port| port.name == spi && port.model == "SPI.NRF52840_SPI")
        || !entries
            .iter()
            .any(|port| port.name == twi && port.model == "I2C.NRF52840_I2C")
    {
        return Err(entry.error("expected ArduCAM on nRF52840 SPI and TWI"));
    }
    spi_cs(entry, entries)?;
    Ok((
        spi,
        twi,
        number(address.trim()).map_err(|e| entry.error(&e))?,
    ))
}

pub(super) fn validate_lsm9ds1(entry: &Entry, entries: &[Entry]) -> Result<(), String> {
    super::keys(entry, &[])?;
    validate_twi_target(entry, entries)
}

pub(super) fn validate_adxl(entry: &Entry, entries: &[Entry]) -> Result<(), String> {
    let registration = entry.registration.as_deref().unwrap_or("");
    let parent = registration.split_whitespace().next();
    let controller = entries
        .iter()
        .find(|other| Some(other.name.as_str()) == parent);
    match controller.map(|device| device.model.as_str()) {
        Some("SPI.NRF52840_SPI") if Some(registration) == parent => {
            spi_cs(entry, entries)?;
        }
        Some("I2C.NRF52840_I2C") if registration.split_whitespace().count() == 2 => {}
        _ => return Err(entry.error("expected an ADXL372 registered on SPI or TWI")),
    }
    acceleration(entry)?;
    Ok(())
}

fn spi_cs(entry: &Entry, entries: &[Entry]) -> Result<(u32, u32), String> {
    spi_cs_line(entry, entries, 0)
}

fn spi_cs_line(entry: &Entry, entries: &[Entry], line: u32) -> Result<(u32, u32), String> {
    let endpoint = format!("{}@{line}", entry.name);
    let endpoint = endpoint.as_str();
    let mut pins = entries
        .iter()
        .filter(|gpio| gpio.model == "GPIOPort.NRF52840_GPIO")
        .flat_map(|gpio| {
            gpio.body.iter().filter_map(move |(_, wire)| {
                let (pin, target) = wire.split_once("->")?;
                (target.trim() == endpoint).then_some((gpio, pin.trim()))
            })
        });
    let (gpio, pin) = pins
        .next()
        .ok_or_else(|| entry.error("missing ADXL372 chip-select wiring"))?;
    let base = gpio.address()?;
    let pin = number(pin).map_err(|e| gpio.error(&e))?;
    if pins.next().is_some() || pin >= if base == 0x5000_0500 { 32 } else { 16 } {
        return Err(gpio.error("invalid or duplicate ADXL372 chip-select pin"));
    }
    Ok((base, pin))
}

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
    let irq = irq
        .filter(|line| *line < 48)
        .ok_or_else(|| entry.error("expected an nRF52840 NVIC IRQ in 0–47"))?;
    let base = entry.address()?;
    let name = entry.name.clone();
    if entry.model == "Miscellaneous.NRF_CLOCK" && base == 0x4000_0000 {
        bus.map_nrf_peripheral(Peripheral::nvmc())
            .map_err(|e| entry.error(&e))?;
        bus.map_nrf_peripheral(Peripheral::temperature())
            .map_err(|e| entry.error(&e))?;
        bus.map_nrf_peripheral(Peripheral::spim3_config())
            .map_err(|e| entry.error(&e))?;
        bus.map_nrf_peripheral(Peripheral::usbd_config())
            .map_err(|e| entry.error(&e))?;
        // Nordic SDK checks for pre-production parts before applying errata.
        bus.add_nrf_tag(0xf000_0fe0, 0xf000_0fe3, 0);
        for (index, base) in [0x4001_c000, 0x4002_1000, 0x4002_2000, 0x4002_d000]
            .into_iter()
            .enumerate()
        {
            bus.map_nrf_peripheral(Peripheral::pwm_config(base, index))
                .map_err(|e| entry.error(&e))?;
        }
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
            matches!(
                child.model.as_str(),
                "SPI.SPILoopback" | "Sensors.ADXL372" | "SPI.SPIMultiplexer"
            ) && child.registration.as_deref() == Some(entry.name.as_str())
                || (child.model == "Sensors.ArduCAMMini2MPPlus"
                    && camera_routes(child, entries).is_ok_and(|(spi, _, _)| spi == entry.name))
        });
        let child = children.next();
        let loopback = child.is_some_and(|child| child.model == "SPI.SPILoopback");
        if let Some(extra) = children.next() {
            return Err(extra.error("nRF52840 SPI accepts only one attached device"));
        }
        let mut spi = Peripheral::spi(base, irq as u8, name, loopback);
        if let Some(child) = child {
            if child.model == "Sensors.ADXL372" {
                let (gpio, pin) = spi_cs(child, entries)?;
                spi.attach_adxl_spi(gpio, pin, child.name.clone(), acceleration(child)?);
            } else if child.model == "SPI.SPIMultiplexer" {
                spi.attach_spi_mux(spi_mux_ports(child, entries)?);
            } else if child.model == "Sensors.ArduCAMMini2MPPlus" {
                let (gpio, pin) = spi_cs(child, entries)?;
                spi.attach_camera(gpio, pin);
            }
        }
        spi
    } else if entry.model == "I2C.NRF52840_I2C" {
        let mut twi = Peripheral::twi(base, irq as u8, name);
        for child in entries.iter().filter(|child| {
            matches!(
                child.model.as_str(),
                "Memory.GenericI2cEeprom"
                    | "Sensors.ADXL372"
                    | "I2C.BME280"
                    | "Sensors.LIS2DS12"
                    | "Sensors.BMP180"
                    | "Sensors.LSM330_Accelerometer"
                    | "Sensors.LSM330_Gyroscope"
                    | "Sensors.LSM303DLHC_Accelerometer"
                    | "Sensors.LSM303DLHC_Gyroscope"
                    | "Sensors.LSM9DS1_IMU"
                    | "Sensors.LSM9DS1_Magnetic"
            )
        }) {
            let Some((parent, address)) = child
                .registration
                .as_deref()
                .and_then(|text| text.split_once(char::is_whitespace))
            else {
                continue;
            };
            if parent == entry.name {
                let address = number(address.trim()).map_err(|e| child.error(&e))?;
                if child.model == "Sensors.ADXL372" {
                    twi.attach_adxl_i2c(address, child.name.clone(), acceleration(child)?)
                } else if child.model == "I2C.BME280" {
                    twi.attach_bme280(address)
                } else if child.model == "Sensors.LIS2DS12" {
                    let (acceleration, gpio, pin) = lis2ds12_config(child, entries)?;
                    twi.attach_lis2ds12(address, acceleration, gpio, pin)
                } else if child.model == "Sensors.BMP180" {
                    let (temperature, pressure) = bmp180_config(child, entries)?;
                    twi.attach_bmp180(address, temperature, pressure)
                } else if matches!(
                    child.model.as_str(),
                    "Sensors.LSM330_Accelerometer" | "Sensors.LSM330_Gyroscope"
                ) {
                    let (values, gyro) = lsm330_config(child, entries)?;
                    twi.attach_lsm330(address, values, gyro)
                } else if matches!(
                    child.model.as_str(),
                    "Sensors.LSM303DLHC_Accelerometer" | "Sensors.LSM303DLHC_Gyroscope"
                ) {
                    let (values, irq) = lsm303_config(child, entries)?;
                    twi.attach_lsm303(address, values, irq)
                } else if matches!(
                    child.model.as_str(),
                    "Sensors.LSM9DS1_IMU" | "Sensors.LSM9DS1_Magnetic"
                ) {
                    twi.attach_lsm9ds1(address, child.model == "Sensors.LSM9DS1_Magnetic")
                } else {
                    twi.attach_eeprom(address, super::i2c_eeprom(child)?)
                }
                .map_err(|e| child.error(&e))?;
            }
        }
        for child in entries
            .iter()
            .filter(|child| child.model == "Sensors.ArduCAMMini2MPPlus")
        {
            let (_, port, address) = camera_routes(child, entries)?;
            if port == entry.name {
                twi.attach_ov2640(address).map_err(|e| child.error(&e))?;
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

fn init_arguments<'a>(entry: &'a Entry, command: &str) -> Result<Vec<u32>, String> {
    let mut body = entry.body.iter();
    if !body.next().is_some_and(|(_, line)| line == "init:") {
        return Err(entry.error("expected init block"));
    }
    body.map(|(_, line)| {
        let argument = line
            .strip_prefix(command)
            .and_then(|value| value.strip_prefix(' '))
            .ok_or_else(|| entry.error("unsupported SPI init command"))?;
        number(argument.trim()).map_err(|e| entry.error(&e))
    })
    .collect()
}

fn mux_slave_address<'a>(entry: &'a Entry, entries: &[Entry]) -> Result<(&'a str, u32), String> {
    let (parent, addr) = entry
        .registration
        .as_deref()
        .and_then(|s| s.split_once(char::is_whitespace))
        .ok_or_else(|| entry.error("expected DummySPISlave registered on a multiplexer"))?;
    if !entries
        .iter()
        .any(|mux| mux.name == parent && mux.model == "SPI.SPIMultiplexer")
    {
        return Err(entry.error("unknown SPI multiplexer"));
    }
    Ok((parent, number(addr.trim()).map_err(|e| entry.error(&e))?))
}

pub(super) fn validate_mux_slave(entry: &Entry, entries: &[Entry]) -> Result<(), String> {
    mux_slave_address(entry, entries)?;
    Ok(())
}

pub(super) fn validate_mux(entry: &Entry, entries: &[Entry]) -> Result<(), String> {
    if !entries.iter().any(|parent| {
        parent.model == "SPI.NRF52840_SPI"
            && entry.registration.as_deref() == Some(parent.name.as_str())
    }) {
        return Err(entry.error("expected SPIMultiplexer registered on nRF52840 SPI"));
    }
    Ok(())
}

fn spi_mux_ports(entry: &Entry, entries: &[Entry]) -> Result<Vec<SpiMuxPort>, String> {
    let active_low = init_arguments(entry, "SetActiveLow")?;
    let mut seen = std::collections::HashSet::new();
    let mut ports = Vec::new();
    for slave in entries
        .iter()
        .filter(|child| child.model == "Mocks.DummySPISlave")
    {
        let (parent, addr) = mux_slave_address(slave, entries)?;
        if parent != entry.name {
            continue;
        }
        if !seen.insert(addr) {
            return Err(slave.error("duplicate SPI multiplexer slave address"));
        }
        let (gpio, pin) = spi_cs_line(entry, entries, addr)?;
        let responses = init_arguments(slave, "EnqueueValue")?
            .into_iter()
            .map(|byte| u8::try_from(byte).map_err(|_| slave.error("SPI response must be a byte")))
            .collect::<Result<_, _>>()?;
        ports.push(SpiMuxPort {
            gpio,
            pin,
            active_low: active_low.contains(&addr),
            responses,
        });
    }
    if ports.is_empty() || active_low.iter().any(|addr| !seen.contains(addr)) {
        return Err(entry.error("missing SPI multiplexer slave"));
    }
    Ok(ports)
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

pub(super) fn map_gpio_attachment(
    entry: &Entry,
    entries: &[Entry],
    bus: &mut Bus,
) -> Result<(), String> {
    let (name, pin) = entry
        .registration
        .as_deref()
        .and_then(|registration| registration.split_once(char::is_whitespace))
        .ok_or_else(|| entry.error("expected an nRF52840 GPIO port and pin"))?;
    let gpio = entries
        .iter()
        .find(|gpio| gpio.name == name && gpio.model == "GPIOPort.NRF52840_GPIO")
        .ok_or_else(|| entry.error("unknown nRF52840 GPIO port"))?;
    let base = gpio.address()?;
    let pin = number(pin.trim()).map_err(|e| entry.error(&e))?;
    if pin >= if base == 0x5000_0500 { 32 } else { 16 } {
        return Err(entry.error("invalid nRF52840 GPIO pin"));
    }
    let mut invert = None;
    let mut wired = false;
    for (_, property) in &entry.body {
        if let Some(value) = property.strip_prefix("invert:")
            && invert.is_none()
        {
            invert = Some(
                value
                    .trim()
                    .parse()
                    .map_err(|_| entry.error("invalid invert property"))?,
            );
        } else if let Some(destination) = property.strip_prefix("->")
            && !wired
            && entry.model == "Miscellaneous.Button"
            && destination.trim() == format!("{name}@{pin}")
        {
            wired = true;
        } else {
            return Err(entry.error("unsupported nRF52840 GPIO attachment property"));
        }
    }
    let invert = invert.unwrap_or(false);
    if entry.model == "Miscellaneous.Button" && wired {
        bus.add_gpio_button(GpioButton::new(entry.name.clone(), base, pin as u8, invert));
    } else if entry.model == "Miscellaneous.LED" {
        let input = gpio
            .body
            .iter()
            .find_map(|(_, wire)| {
                let (from, to) = wire.split_once("->")?;
                let (receiver, input) = to.trim().split_once('@')?;
                (number(from.trim()) == Ok(pin) && receiver == entry.name).then_some(input)
            })
            .ok_or_else(|| entry.error("missing nRF52840 GPIO wiring"))?;
        let input = number(input).map_err(|e| entry.error(&e))?;
        bus.add_nrf_gpio_led(
            base,
            GpioLed::new(entry.name.clone(), pin as u8, invert),
            input,
        )
        .map_err(|e| entry.error(&e))?;
    } else {
        return Err(entry.error("missing nRF52840 GPIO wiring"));
    }
    Ok(())
}

pub(super) fn map_gpio(entry: &Entry, entries: &[Entry], bus: &mut Bus) -> Result<(), String> {
    for (_, wire) in &entry.body {
        let (_, target) = wire
            .split_once("->")
            .ok_or_else(|| entry.error("unsupported GPIO property"))?;
        let (target, line) = target
            .trim()
            .split_once('@')
            .ok_or_else(|| entry.error("unsupported GPIO connection"))?;
        number(line).map_err(|e| entry.error(&e))?;
        if !entries.iter().any(|child| {
            child.name == target
                && (matches!(
                    child.model.as_str(),
                    "SPI.SPIMultiplexer" | "Miscellaneous.LED"
                ) || (line == "0"
                    && matches!(
                        child.model.as_str(),
                        "Sensors.ADXL372" | "Sensors.ArduCAMMini2MPPlus"
                    )))
        }) {
            return Err(entry.error("unsupported GPIO destination"));
        }
    }
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
