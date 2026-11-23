use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

use crate::{
    bitmanip::BitManip,
    bus::Bus,
    csr::{DEFAULT_MTVEC, InterruptMode},
    litex::{Gpio, GpioMode, IoDevice, Mmcm, Spi},
    litex_eth::{Ethernet, Phy, Region as EthernetRegion, Registration},
    litex_i2c::{I2c, Sensor},
    litex_i2s::I2s,
    litex_panel::{Button, Panel},
    litex_sd::{self, Controller as SdController},
    litex_video::Framebuffer,
    machine::{MAX_HARTS, Machine},
    stm32_uart::Stm32Irq,
    uart::LiteXTxTiming,
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

    fn registration_list(&self) -> Option<Vec<&str>> {
        let list = self
            .registration
            .as_deref()?
            .strip_prefix('{')?
            .strip_suffix('}')?;
        let mut depth = 0;
        let mut start = 0;
        let mut registrations = Vec::new();
        for (index, delimiter) in unquoted_delimiters(list, &['{', '}', ';']) {
            match delimiter {
                '{' => depth += 1,
                '}' => depth -= 1,
                ';' if depth == 0 => {
                    registrations.push(list[start..index].trim());
                    start = index + 1;
                }
                _ => {}
            }
        }
        if !list[start..].trim().is_empty() {
            registrations.push(list[start..].trim());
        }
        Some(registrations)
    }

    fn addresses(&self) -> Result<Vec<u32>, String> {
        if let Some(registrations) = self.registration_list() {
            if registrations.is_empty() {
                return Err(self.error("expected sysbus registrations"));
            }
            return registrations
                .into_iter()
                .map(|registration| self.registration_address(registration))
                .collect();
        }
        Ok(vec![self.address()?])
    }

    fn registration_address(&self, registration: &str) -> Result<u32, String> {
        let address = registration
            .strip_prefix("sysbus")
            .filter(|address| address.starts_with(char::is_whitespace))
            .ok_or_else(|| self.error("expected a sysbus address in registration list"))?;
        number(address.trim()).map_err(|e| self.error(&e))
    }

    fn address(&self) -> Result<u32, String> {
        if let Some(registrations) = self.registration_list() {
            let [registration] = registrations.as_slice() else {
                return Err(self.error("expected one sysbus address"));
            };
            return self.registration_address(registration);
        }
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
    let mut lines = content.lines().enumerate();
    while let Some((index, raw)) = lines.next() {
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
            Some((model, registration)) => {
                let mut registration = registration.trim().to_string();
                if registration.starts_with('{') {
                    let mut depth = 0_i32;
                    for (_, brace) in unquoted_delimiters(&registration, &['{', '}']) {
                        depth += if brace == '{' { 1 } else { -1 };
                    }
                    while depth > 0 {
                        let (_, line) = lines
                            .next()
                            .ok_or_else(|| error("unterminated registration list"))?;
                        let line = without_comment(line).trim();
                        for (_, brace) in unquoted_delimiters(line, &['{', '}']) {
                            depth += if brace == '{' { 1 } else { -1 };
                        }
                        registration.push(' ');
                        registration.push_str(line);
                    }
                    if depth != 0 || !registration.ends_with('}') {
                        return Err(error("invalid registration list"));
                    }
                }
                (model.trim(), Some(registration))
            }
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

fn apply_litex_irq_overrides(entries: &mut Vec<Entry>) -> Result<(), String> {
    let mut declarations = HashMap::<String, usize>::new();
    let mut merged: Vec<Entry> = Vec::with_capacity(entries.len());
    for entry in std::mem::take(entries) {
        if entry.model.is_empty()
            && entry.name != "sysbus"
            && entry.registration.is_none()
            && let Some(&index) = declarations.get(&entry.name)
            && matches!(
                merged[index].model.as_str(),
                "UART.LiteX_UART"
                    | "Timers.LiteX_Timer"
                    | "Timers.LiteX_Timer_CSR32"
                    | "Network.LiteX_Ethernet"
                    | "Network.LiteX_Ethernet_CSR32"
            )
        {
            let [(_, wire)] = entry.body.as_slice() else {
                return Err(entry.error("expected one LiteX IRQ override"));
            };
            let is_irq = |text: &str| {
                text.split_once("->")
                    .is_some_and(|(pin, _)| matches!(pin.trim(), "" | "IRQ"))
            };
            if !is_irq(wire) {
                return Err(entry.error("expected LiteX IRQ override"));
            }
            merged[index].body.retain(|(_, text)| !is_irq(text));
            merged[index].body.extend(entry.body);
        } else {
            declarations.insert(entry.name.clone(), merged.len());
            merged.push(entry);
        }
    }
    *entries = merged;
    Ok(())
}

pub fn load(path: &Path, root: Option<&Path>) -> Result<Machine, String> {
    let mut entries = Vec::new();
    parse(
        path,
        root.unwrap_or(Path::new(".")),
        &mut HashSet::new(),
        &mut entries,
    )?;
    apply_litex_irq_overrides(&mut entries)?;
    let mut machine = Machine::without_default_ram();
    let mut names = HashSet::new();
    let mut cpus = Vec::new();
    let mut clint = None;
    let mut nvic = None;
    let mut exti = None;
    let mut combiners = Vec::new();
    let mut ram = false;
    let mut memory_bases = HashMap::new();
    let mut flash_controller = None;
    let mut flash_chip = None;
    let mut uart = None;
    let mut stm32_uarts = Vec::new();
    let mut plic = None;
    let mut timer = None;
    let mut litex_timer = None;
    let mut litex_cpu_timer = None;
    let mut litex_soc = None;
    let mut litex_eth = None;
    let mut litex_phy = None;
    let mut litex_panel = None;
    let mut panel_parts = Vec::new();
    let mut litex_gpios = Vec::new();
    let mut litex_spis = Vec::new();
    let mut spi_loopbacks = Vec::new();
    let mut litex_i2c = None;
    let mut sensor = None;
    let mut litex_sd = None;
    let mut litex_video = None;
    let mut litex_i2s = Vec::new();
    for entry in &entries {
        if entry.name == "sysbus" && entry.model.is_empty() && entry.registration.is_none() {
            map_sysbus_ranges(entry, &mut machine.bus)?;
            continue;
        }
        if !names.insert(&entry.name) {
            return Err(entry.error("duplicate declarations and overrides are unsupported"));
        }
        match entry.model.as_str() {
            "CPU.RiscV32" | "CPU.VexRiscv" | "CPU.IbexRiscV32" | "CPU.Minerva" | "CPU.CortexM" => {
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
                let addresses = entry.addresses()?;
                machine
                    .bus
                    .map_ram_views(
                        &addresses,
                        number(size).map_err(|e| entry.error(&e))? as usize,
                    )
                    .map_err(|e| entry.error(&e))?;
                if entry.model == "Memory.MappedMemory" {
                    memory_bases.insert(
                        entry.name.as_str(),
                        (addresses[0], number(size).map_err(|e| entry.error(&e))?),
                    );
                }
                ram = true;
            }
            "GPIOPort.Murax_GPIO" => {
                keys(entry, &[])?;
                machine
                    .bus
                    .map_murax_gpio(entry.address()?)
                    .map_err(|e| entry.error(&e))?;
            }
            "SPI.LiteX_SPI_Flash" => {
                keys(entry, &[])?;
                if flash_controller.replace(entry).is_some() {
                    return Err(entry.error("only one LiteX SPI flash controller is supported"));
                }
            }
            "SPI.Micron_MT25Q" => {
                if flash_chip.replace(entry).is_some() {
                    return Err(entry.error("only one LiteX SPI flash chip is supported"));
                }
            }
            "SPI.LiteX_SPI" => litex_spis.push(entry),
            "SPI.SPILoopback" => {
                keys(entry, &[])?;
                spi_loopbacks.push(entry);
            }
            "I2C.LiteX_I2C" => {
                keys(entry, &[])?;
                if litex_i2c.replace(entry).is_some() {
                    return Err(entry.error("only one LiteX I2C controller is supported"));
                }
            }
            "Sensors.SI70xx" => {
                if sensor.replace(entry).is_some() {
                    return Err(entry.error("only one SI70xx sensor is supported"));
                }
            }
            "Miscellaneous.LiteX_MMCM" | "Miscellaneous.LiteX_MMCM_CSR32" => {
                keys(entry, &[])?;
                let mmcm = if entry.model == "Miscellaneous.LiteX_MMCM" {
                    Mmcm::narrow()
                } else {
                    Mmcm::default()
                };
                machine
                    .bus
                    .map_litex_io(entry.addresses()?, IoDevice::Mmcm(Box::new(mmcm)))
                    .map_err(|e| entry.error(&e))?;
            }
            "GPIOPort.LiteX_ControlAndStatus" => {
                if litex_panel.replace(entry).is_some() {
                    return Err(
                        entry.error("only one LiteX control and status peripheral is supported")
                    );
                }
            }
            "Miscellaneous.LED" | "Miscellaneous.Button" => panel_parts.push(entry),
            "GPIOPort.LiteX_GPIO" => litex_gpios.push(entry),
            "Timers.Murax_Timer" => {
                if timer.replace(entry).is_some() {
                    return Err(entry.error("only one Murax timer is currently supported"));
                }
            }
            "Timers.LiteX_CPUTimer" => {
                if litex_cpu_timer.replace(entry).is_some() {
                    return Err(entry.error("only one LiteX CPU timer is supported"));
                }
            }
            "Timers.LiteX_Timer" | "Timers.LiteX_Timer_CSR32" => {
                if litex_timer.replace(entry).is_some() {
                    return Err(entry.error("only one LiteX timer is supported"));
                }
            }
            "Sound.LiteX_I2S_Master" | "Sound.LiteX_I2S_Slave" => litex_i2s.push(entry),
            "Video.LiteX_Framebuffer_CSR32" => {
                if litex_video.replace(entry).is_some() {
                    return Err(entry.error("only one LiteX framebuffer is supported"));
                }
            }
            "SD.LiteSDCard" | "SD.LiteSDCard_CSR32" => {
                keys(entry, &[])?;
                if litex_sd.replace(entry).is_some() {
                    return Err(entry.error("only one LiteX SD controller is supported"));
                }
            }
            "Network.LiteX_Ethernet" | "Network.LiteX_Ethernet_CSR32" => {
                if litex_eth.replace(entry).is_some() {
                    return Err(entry.error("only one LiteX Ethernet controller is supported"));
                }
            }
            "Network.EthernetPhysicalLayer" => {
                if litex_phy.replace(entry).is_some() {
                    return Err(entry.error("only one LiteX Ethernet PHY is supported"));
                }
            }
            "Miscellaneous.LiteX_SoC_Controller" | "Miscellaneous.LiteX_SoC_Controller_CSR32" => {
                if litex_soc.replace(entry).is_some() {
                    return Err(entry.error("only one LiteX SoC controller is supported"));
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
    for spi in litex_spis {
        keys(spi, &[])?;
        let mut loopback = false;
        spi_loopbacks.retain(|entry| {
            if entry.registration.as_deref() == Some(spi.name.as_str()) && !loopback {
                loopback = true;
                false
            } else {
                true
            }
        });
        let mut controller = Spi::default().with_loopback_slave(loopback);
        controller.name = Some(spi.name.clone());
        machine
            .bus
            .map_litex_io(spi.addresses()?, IoDevice::Spi(controller))
            .map_err(|e| spi.error(&e))?;
    }
    if let Some(entry) = spi_loopbacks.first() {
        return Err(entry.error("expected attachment to a LiteX SPI controller"));
    }
    if let Some(controller) = litex_i2c {
        let sensor = sensor
            .map(|sensor| parse_si70xx(sensor, controller))
            .transpose()?;
        machine
            .bus
            .map_litex_io(
                controller.addresses()?,
                IoDevice::I2c(Box::new(I2c::new(sensor))),
            )
            .map_err(|e| controller.error(&e))?;
    } else if let Some(sensor) = sensor {
        return Err(sensor.error("expected a LiteX I2C controller attachment"));
    }
    if let Some(entry) = litex_video {
        map_litex_video(entry, &memory_bases, &mut machine.bus)?;
    }
    if let Some(entry) = litex_sd {
        let registrations = entry
            .registration_list()
            .ok_or_else(|| entry.error("expected LiteX SD PHY and core registrations"))?
            .into_iter()
            .map(|registration| sd_registration(entry, registration))
            .collect::<Result<Vec<_>, _>>()?;
        if ![litex_sd::Region::Phy, litex_sd::Region::Core]
            .iter()
            .all(|region| {
                registrations
                    .iter()
                    .any(|registration| registration.region == *region)
            })
        {
            return Err(entry.error("expected LiteX SD PHY and core registrations"));
        }
        machine
            .bus
            .map_litex_sd(SdController::new(
                entry.name.clone(),
                registrations,
                entry.model == "SD.LiteSDCard_CSR32",
            ))
            .map_err(|e| entry.error(&e))?;
    }
    if let Some(chip) = flash_chip {
        let controller = flash_controller
            .filter(|entry| chip.registration.as_deref() == Some(&entry.name))
            .ok_or_else(|| chip.error("expected a LiteX SPI flash controller attachment"))?;
        let (_, memory) = keys(chip, &["underlyingMemory"])?
            .into_iter()
            .next()
            .ok_or_else(|| chip.error("expected underlyingMemory"))?;
        let (backing, _) = *memory_bases
            .get(memory)
            .ok_or_else(|| chip.error("unknown flash backing memory"))?;
        machine
            .bus
            .map_litex_flash(controller.addresses()?, backing)
            .map_err(|e| controller.error(&e))?;
    } else if let Some(controller) = flash_controller {
        return Err(controller.error("expected an attached Micron MT25Q flash chip"));
    }
    if cpus.is_empty() || !ram {
        return Err(format!(
            "{}: platform requires at least one CPU and mapped RAM",
            path.display()
        ));
    }
    if let Some(entry) = cpus.iter().find(|entry| entry.model == "CPU.CortexM") {
        if cpus.len() != 1
            || clint.is_some()
            || plic.is_some()
            || timer.is_some()
            || litex_timer.is_some()
            || litex_cpu_timer.is_some()
            || litex_soc.is_some()
            || litex_eth.is_some()
            || litex_phy.is_some()
            || litex_panel.is_some()
            || !panel_parts.is_empty()
            || !litex_gpios.is_empty()
        {
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
                .map_litex_uart(vec![uart.address()?], None)
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
        let ibex = entry.model == "CPU.IbexRiscV32";
        let mut isa = match entry.model.as_str() {
            "CPU.IbexRiscV32" => Some((true, false, true, false)),
            "CPU.Minerva" => Some((false, false, false, false)),
            "CPU.VexRiscv" => Some((true, false, false, false)),
            _ => None,
        };
        let mut floating = (false, false, false);
        let mut zicond = false;
        let mut zacas = false;
        let mut smepmp = false;
        let mut bitmanip = BitManip::default();
        let mut id = 0;
        let mut privilege_levels = (
            entry.model != "CPU.Minerva",
            !matches!(entry.model.as_str(), "CPU.Minerva" | "CPU.IbexRiscV32"),
        );
        let mut pmp = if matches!(entry.model.as_str(), "CPU.RiscV32" | "CPU.IbexRiscV32") {
            16
        } else {
            0
        };
        for (key, value) in keys_in(
            entry,
            split_rv32_init(entry).0,
            &[
                "cpuType",
                "hartId",
                "timeProvider",
                "privilegedArchitecture",
                "privilegeLevels",
                "pmpEntryCount",
                "builtInIrqController",
                "allowUnalignedAccesses",
                "vectorBootstrap",
                "interruptMode",
            ],
        )? {
            match (key, value) {
                ("cpuType", value) => {
                    let architecture = value.trim_matches('"').to_ascii_lowercase();
                    let mut base = architecture.as_str();
                    zicond = false;
                    zacas = false;
                    smepmp = false;
                    let mut zfh = false;
                    bitmanip = BitManip::default();
                    while let Some((prefix, extension)) = base.rsplit_once('_') {
                        match extension {
                            "zicond" if !zicond => zicond = true,
                            "zacas" if !zacas => zacas = true,
                            "smepmp" if !smepmp => smepmp = true,
                            "zfh" if !zfh => zfh = true,
                            "zba" if !bitmanip.zba => bitmanip.zba = true,
                            "zbb" if !bitmanip.zbb => bitmanip.zbb = true,
                            "zbc" if !bitmanip.zbc => bitmanip.zbc = true,
                            "zbkb" if !bitmanip.zbkb => bitmanip.zbkb = true,
                            "zbs" if !bitmanip.zbs => bitmanip.zbs = true,
                            _ => break,
                        }
                        base = prefix;
                    }
                    let (letters, suffix) = base.split_at(base.find('z').unwrap_or(base.len()));
                    let letters = letters.strip_prefix("rv32").unwrap_or("");
                    let letters = letters.trim_matches('_');
                    let general = letters.contains('g');
                    let embedded = letters.contains('e');
                    let single = general || letters.contains('f');
                    let double = general || letters.contains('d');
                    let valid_csr_suffix = suffix
                        .split('_')
                        .all(|part| matches!(part, "" | "zicsr" | "zifencei"));
                    let ibex_legacy = ibex && base == "rv32imcu_zicsr_zifencei";
                    if !ibex_legacy
                        && (!(general || embedded || letters.contains('i'))
                            || !letters.chars().all(|letter| "iemafdcgb".contains(letter))
                            || (embedded && !letters.chars().all(|letter| "emac".contains(letter)))
                            || !valid_csr_suffix
                            || (double && !single))
                    {
                        return Err(entry.error(&unsupported_rv32_isa(value, &architecture)));
                    }
                    floating = (single, double, zfh);
                    if letters.contains('b') {
                        // Renode enables all four supported bit-manipulation subsets for B.
                        bitmanip = BitManip {
                            zba: true,
                            zbb: true,
                            zbc: true,
                            zbs: true,
                            ..bitmanip
                        };
                    }
                    isa = Some((
                        general || letters.contains('m'),
                        general || letters.contains('a'),
                        letters.contains('c'),
                        embedded,
                    ));
                }
                ("hartId", value) => id = number(value).map_err(|e| entry.error(&e))? as usize,
                ("privilegedArchitecture", "PrivilegedArchitecture.Priv1_10") => {}
                ("privilegedArchitecture", "PrivilegedArchitecture.Priv1_11")
                | ("timeProvider", "empty")
                    if ibex => {}
                ("timeProvider", name)
                    if clint
                        .or(litex_cpu_timer)
                        .is_some_and(|timer| timer.name == name) => {}
                ("builtInIrqController", "true" | "false" | "True" | "False")
                    if entry.model == "CPU.VexRiscv" => {}
                ("allowUnalignedAccesses", "true" | "false" | "True" | "False")
                    if matches!(entry.model.as_str(), "CPU.RiscV32" | "CPU.IbexRiscV32") => {}
                (
                    "interruptMode",
                    "InterruptMode.Auto"
                    | "InterruptMode.Direct"
                    | "InterruptMode.Vectored"
                    | "0"
                    | "1"
                    | "2",
                ) if entry.model == "CPU.RiscV32" => {}
                ("privilegeLevels", value) if entry.model == "CPU.RiscV32" => {
                    privilege_levels = match value {
                        "PrivilegeLevels.Machine" => (false, false),
                        "PrivilegeLevels.MachineUser" => (true, false),
                        "PrivilegeLevels.MachineSupervisorUser" => (true, true),
                        _ => return Err(entry.error("unsupported RV32 privilegeLevels")),
                    };
                }
                ("timeProvider", "empty") if entry.model == "CPU.RiscV32" => {}
                ("vectorBootstrap", "true") if entry.model == "CPU.RiscV32" => {}
                ("pmpEntryCount", "0") => pmp = 0,
                ("pmpEntryCount", "16") => pmp = 16,
                ("pmpEntryCount", "64") => pmp = 64,
                _ => return Err(entry.error(&format!("unsupported CPU property {key} = {value}"))),
            }
        }
        configs.push((
            id,
            entry,
            {
                let (multiply, atomic, compressed, embedded) =
                    isa.ok_or_else(|| entry.error("missing cpuType"))?;
                if floating.2 && !floating.0 {
                    return Err(entry.error("Zfh requires the RV32 F extension"));
                }
                if smepmp && pmp == 0 {
                    return Err(entry.error("Smepmp requires internal PMP entries"));
                }
                if zacas && !atomic {
                    return Err(entry.error("Zacas requires the RV32 A extension"));
                }
                (
                    multiply, atomic, compressed, embedded, zicond, zacas, smepmp, bitmanip,
                    floating,
                )
            },
            pmp,
            privilege_levels,
        ));
    }
    configs.sort_unstable_by_key(|config| config.0);
    if configs.len() > MAX_HARTS || configs.windows(2).any(|pair| pair[0].0 == pair[1].0) {
        return Err(format!(
            "{}: hart IDs must be unique, up to {MAX_HARTS} harts",
            path.display()
        ));
    }
    // A platform with RAM at the Renode reset trap vector can receive guest traps
    // before firmware initializes MTVEC. Boards without a mapped handler can
    // still use the CLI stop conditions until the guest or monitor sets a vector.
    let default_vector_mapped = machine.bus.read_ram::<4>(DEFAULT_MTVEC).is_ok();
    machine.configure_harts(configs.len());
    for (
        slot,
        &(
            id,
            entry,
            (multiply, atomic, compressed, embedded, zicond, zacas, smepmp, bitmanip, floating),
            pmp,
            (user, supervisor),
        ),
    ) in configs.iter().enumerate()
    {
        let hart = machine.hart_mut(slot);
        hart.set_hart_id(id);
        hart.name = Some(entry.name.clone());
        let unaligned = entry
            .body
            .iter()
            .find_map(|(_, text)| {
                let (key, value) = text.split_once(':')?;
                (key.trim() == "allowUnalignedAccesses")
                    .then(|| value.trim().eq_ignore_ascii_case("true"))
            })
            .unwrap_or(entry.model == "CPU.IbexRiscV32");
        hart.set_unaligned_accesses(unaligned);
        hart.set_integer_extensions(multiply, atomic, compressed);
        hart.set_embedded_base(embedded);
        hart.set_zicond(zicond);
        hart.set_zacas(zacas);
        hart.set_bitmanip(bitmanip);
        if floating.0 {
            hart.enable_floating(floating.1, floating.2);
        }
        match entry.model.as_str() {
            "CPU.VexRiscv" => hart.enable_vex_core(uses_vex_irq(entry)),
            "CPU.IbexRiscV32" => hart.enable_ibex(),
            "CPU.Minerva" => hart.enable_minerva(),
            _ => {}
        }
        if user {
            hart.enable_user_mode();
        }
        if supervisor {
            hart.enable_supervisor_mode();
        }
        if entry.model == "CPU.RiscV32" {
            hart.enable_address_triggers();
            hart.configure_reset_vector(0x1000);
            if default_vector_mapped {
                hart.set_mtvec(DEFAULT_MTVEC);
            }
        }
        if pmp != 0 {
            hart.enable_pmp(pmp);
            if smepmp {
                hart.enable_smepmp();
            }
        }
        if split_rv32_init(entry).0.iter().any(|(_, text)| {
            text.split_once(':').is_some_and(|(key, value)| {
                key.trim() == "vectorBootstrap" && value.trim() == "true"
            })
        }) {
            hart.enable_vector_bootstrap();
        }
        if let Some(mode) = split_rv32_init(entry).0.iter().find_map(|(_, text)| {
            let (key, value) = text.split_once(':')?;
            if key.trim() != "interruptMode" {
                return None;
            }
            match value.trim() {
                "InterruptMode.Auto" | "0" => Some(InterruptMode::Auto),
                "InterruptMode.Direct" | "1" => Some(InterruptMode::Direct),
                "InterruptMode.Vectored" | "2" => Some(InterruptMode::Vectored),
                _ => None,
            }
        }) {
            hart.configure_interrupt_mode(mode);
        }
        if let Some(commands) = split_rv32_init(entry).1 {
            if commands.is_empty() {
                return Err(entry.error("expected an RV32 init command"));
            }
            for (line, command) in commands {
                let error = |detail: &str| format!("{}:{line}: {detail}", entry.source.display());
                let (register, value) = command
                    .split_once(char::is_whitespace)
                    .ok_or_else(|| error("expected RV32 init command REGISTER VALUE"))?;
                let value = number(value.trim()).map_err(|detail| error(&detail))?;
                match register {
                    "MSTATUS" => hart.set_mstatus(value),
                    "MTVEC" => hart.set_mtvec(value),
                    _ => return Err(error("expected RV32 init register MSTATUS or MTVEC")),
                }
            }
        }
    }
    let cpu = configs[0].1;
    if configs.iter().all(|config| config.1.model == "CPU.RiscV32") {
        // Renode's system bus returns zero for an unmapped data read
        // and discards an unmapped data write.
        machine.bus.use_renode_unmapped_data();
    }
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
        if frequency == Some(0)
            || targets == 0
            || targets > configs.len()
            || (!wires.is_empty() && wires.len() != targets)
        {
            return Err(entry.error("invalid CLINT frequency or target wiring"));
        }
        for (slot, &(_, target, _, _, _)) in configs[..wires.len()].iter().enumerate() {
            let (software, timer) = if uses_vex_irq(target) {
                (101, 100)
            } else {
                (3, 7)
            };
            let expected = format!(
                "[{},{}]->{}@[{software},{timer}]",
                2 * slot,
                2 * slot + 1,
                target.name
            );
            if !wires.contains(&expected) {
                return Err(entry.error(&format!("expected CLINT connection {expected}")));
            }
        }
        // VexRiscv's custom CLINT wiring uses the established one-tick-per-step
        // time base used by the LiteX Linux demos.
        if let Some(frequency) = frequency.filter(|_| !uses_vex_irq(cpu)) {
            machine
                .bus
                .map_clint_with_frequency(entry.address()?, targets, frequency)
        } else {
            machine.bus.map_clint(entry.address()?, targets)
        }
        .map_err(|e| entry.error(&e))?;
        if wires.is_empty() {
            machine.bus.clint.as_mut().unwrap().disconnect_interrupts();
        }
    }
    if let Some(entry) = litex_cpu_timer {
        if configs.len() != 1 || clint.is_some() || !uses_vex_irq(cpu) {
            return Err(entry.error("LiteX CPU timer requires a single VexRiscv CPU without CLINT"));
        }
        let expected = format!("->{}@100", cpu.name);
        let mut connected = false;
        let mut frequency = None;
        for (_, text) in &entry.body {
            if let Some(value) = text.strip_prefix("frequency:")
                && frequency.is_none()
            {
                frequency = Some(number(value.trim()).map_err(|e| entry.error(&e))?);
            } else {
                let wire: String = text
                    .chars()
                    .filter(|character| !character.is_whitespace())
                    .collect();
                if connected || wire.strip_prefix("IRQ").unwrap_or(&wire) != expected {
                    return Err(entry.error(&format!("expected LiteX CPU timer wiring {expected}")));
                }
                connected = true;
            }
        }
        let frequency = frequency
            .filter(|&frequency| frequency != 0 && connected)
            .ok_or_else(|| {
                entry.error("expected positive LiteX CPU timer frequency and IRQ wiring")
            })?;
        machine
            .bus
            .map_litex_cpu_timer(entry.address()?, frequency)
            .map_err(|e| entry.error(&e))?;
    }
    let plic_config = plic
        .map(|entry| {
            let mut outputs = [0; 2];
            let mut sources = None;
            let mut contexts = None;
            let mut priorities = None;
            for (_, line) in &entry.body {
                if let Some((context, target)) = line.split_once("->") {
                    let (name, lines) = target
                        .split_once('@')
                        .ok_or_else(|| entry.error("invalid PLIC connection"))?;
                    let contexts =
                        irq_list(context, outputs.len() as u32).map_err(|e| entry.error(&e))?;
                    let lines = irq_list(lines, 32).map_err(|e| entry.error(&e))?;
                    if name.trim() != cpu.name || contexts.len() != lines.len() {
                        return Err(entry.error("invalid PLIC connection"));
                    }
                    for (context, line) in contexts.into_iter().zip(lines) {
                        let context = usize::from(context);
                        if !matches!(line, 9 | 11)
                            || outputs[context] != 0
                            || outputs.contains(&(1 << line))
                        {
                            return Err(entry.error("invalid or duplicate PLIC connection"));
                        }
                        outputs[context] = 1 << line;
                    }
                } else if let Some((key, value)) = line.split_once(':') {
                    match key.trim() {
                        "numberOfSources" if sources.is_none() => {
                            sources = Some(number(value.trim()).map_err(|e| entry.error(&e))?)
                        }
                        "numberOfContexts" if contexts.is_none() => {
                            contexts = Some(number(value.trim()).map_err(|e| entry.error(&e))?)
                        }
                        "prioritiesEnabled" if priorities.is_none() => {
                            priorities = Some(
                                value
                                    .trim()
                                    .parse::<bool>()
                                    .map_err(|_| entry.error("invalid prioritiesEnabled"))?,
                            );
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
                || !matches!(cpu.model.as_str(), "CPU.RiscV32" | "CPU.VexRiscv")
            {
                return Err(
                    entry.error("expected PLIC context outputs wired to standard RISC-V IRQs")
                );
            }
            let sources = sources.ok_or_else(|| entry.error("missing PLIC numberOfSources"))?;
            machine
                .bus
                .map_plic(
                    entry.address()?,
                    sources,
                    &outputs[..count],
                    priorities.unwrap_or(true),
                )
                .map_err(|e| entry.error(&e))?;
            Ok((entry, sources))
        })
        .transpose()?;
    for entry in litex_i2s {
        map_litex_i2s(entry, &configs, &mut machine.bus)?;
    }
    if let Some(entry) = uart {
        if entry.model == "UART.Murax_UART" {
            let connected = direct_irq(entry, &cpu.name, 11)?;
            machine
                .bus
                .map_uart(entry.address()?, connected)
                .map_err(|e| entry.error(&e))?;
        } else if entry.model == "UART.LiteX_UART" {
            let mut irq = None;
            let mut capacity = None;
            let mut flush = None;
            let mut timeout = None;
            for (_, text) in &entry.body {
                if text.contains("->") && irq.is_none() {
                    irq = Some(vex_irq(entry, text, &configs)?);
                    continue;
                }
                let property = text
                    .split_once(':')
                    .map(|(key, value)| (key.trim(), value.trim()));
                let (slot, value) = match property {
                    Some(("txFifoCapacity", value)) if capacity.is_none() => (&mut capacity, value),
                    Some(("flushDelayNs", value)) if flush.is_none() => (&mut flush, value),
                    Some(("timeoutNs", value)) if timeout.is_none() => (&mut timeout, value),
                    _ => return Err(entry.error("unsupported or duplicate LiteX UART property")),
                };
                *slot = Some(
                    value
                        .parse::<u64>()
                        .map_err(|_| entry.error("invalid LiteX UART value"))?,
                );
            }
            let mut timing = LiteXTxTiming::default();
            if let Some(capacity) = capacity {
                timing.capacity = usize::try_from(capacity)
                    .map_err(|_| entry.error("LiteX UART FIFO capacity exceeds host limit"))?;
            }
            if matches!(flush, Some(0))
                || matches!(timeout, Some(0))
                || timing.capacity == 0 && (flush.is_some() || timeout.is_some())
            {
                return Err(entry.error("LiteX UART delays require a FIFO and positive durations"));
            }
            if let Some(nanoseconds) = flush {
                timing.flush_steps = nanoseconds.div_ceil(10);
            }
            if let Some(nanoseconds) = timeout {
                timing.timeout_steps = nanoseconds.div_ceil(10);
            }
            let timing =
                (irq.is_some() || capacity.is_some() || flush.is_some() || timeout.is_some())
                    .then_some(timing);
            machine
                .bus
                .map_litex_uart_with_timing(entry.addresses()?, irq, timing)
                .map_err(|e| entry.error(&e))?;
        } else {
            let mut source = None;
            let mut wide = None;
            for (_, line) in &entry.body {
                if let Some(target) = line
                    .split_once("->")
                    .filter(|(pin, _)| pin.trim() == "IRQ")
                    .and_then(|(_, target)| target.trim().split_once('@'))
                    .filter(|(name, _)| {
                        plic_config.is_some_and(|(plic, _)| name.trim() == plic.name)
                    })
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
            if let Some(source) = source
                && !plic_config.is_some_and(|(_, sources)| (1..=sources).contains(&source))
            {
                return Err(entry.error("expected a valid NS16550 IRQ connection to PLIC"));
            }
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
    if let Some(entry) = litex_timer {
        let mut frequency = None;
        let mut irq = None;
        for (_, text) in &entry.body {
            if text.contains("->") && irq.is_none() {
                irq = Some(vex_irq(entry, text, &configs)?);
            } else if let Some(("frequency", value)) = text
                .split_once(':')
                .map(|(key, value)| (key.trim(), value.trim()))
                && frequency.is_none()
            {
                frequency = Some(number(value).map_err(|e| entry.error(&e))?);
            } else {
                return Err(entry.error("unsupported or duplicate LiteX timer property"));
            }
        }
        let frequency = frequency
            .filter(|&frequency| frequency != 0)
            .ok_or_else(|| entry.error("expected positive LiteX timer frequency"))?;
        let timer = crate::litex::Timer::new(frequency, entry.model == "Timers.LiteX_Timer_CSR32");
        machine
            .bus
            .map_litex_timer(entry.addresses()?, timer, irq)
            .map_err(|e| entry.error(&e))?;
    }
    if let Some(entry) = litex_soc {
        keys(entry, &[])?;
        machine
            .bus
            .map_litex_soc(
                entry.address()?,
                entry.model == "Miscellaneous.LiteX_SoC_Controller_CSR32",
            )
            .map_err(|e| entry.error(&e))?;
    }
    let phy = litex_phy
        .map(|phy| {
            let eth = litex_eth.ok_or_else(|| phy.error("expected a LiteX Ethernet controller"))?;
            let mut registration = phy.registration.as_deref().unwrap_or("").split_whitespace();
            let (Some(parent), Some(address), None) = (
                registration.next(),
                registration.next(),
                registration.next(),
            ) else {
                return Err(phy.error("expected an Ethernet controller name and PHY address"));
            };
            if parent != eth.name {
                return Err(phy.error("unknown LiteX Ethernet controller"));
            }
            let address =
                irq_index(address, 32).map_err(|_| phy.error("expected PHY address in 0–31"))?;
            let mut device = Phy::new(address);
            device.name = Some(format!("{parent}.{}", phy.name));
            for (key, value) in keys(phy, &["VendorSpecific1"])? {
                let value = number(value)
                    .ok()
                    .and_then(|value| u16::try_from(value).ok())
                    .ok_or_else(|| phy.error(&format!("invalid {key} register value")))?;
                device.set_vendor_specific1(value);
            }
            Ok(device)
        })
        .transpose()?;
    if let Some(entry) = litex_eth {
        let irq = match entry.body.as_slice() {
            [] => None,
            [(_, wire)] => Some(vex_irq(entry, wire, &configs)?),
            _ => return Err(entry.error("expected at most one LiteX Ethernet IRQ")),
        };
        let registrations = if let Some(registrations) = entry.registration_list() {
            registrations
                .into_iter()
                .map(|registration| ethernet_registration(entry, registration))
                .collect::<Result<Vec<_>, _>>()?
        } else {
            vec![Registration {
                base: entry.address()?,
                size: Ethernet::MAC_SIZE,
                region: EthernetRegion::Mac,
            }]
        };
        if !registrations
            .iter()
            .any(|registration| registration.region == EthernetRegion::Mac)
        {
            return Err(entry.error("expected a LiteX Ethernet MAC registration"));
        }
        machine
            .bus
            .map_litex_eth(
                entry.name.clone(),
                registrations,
                irq,
                phy,
                entry.model == "Network.LiteX_Ethernet_CSR32",
            )
            .map_err(|e| entry.error(&e))?;
    }
    for gpio in litex_gpios {
        let parts = take_gpio_parts(&mut panel_parts, &gpio.name);
        map_litex_gpio(gpio, &parts, &configs, &mut machine.bus)?;
    }
    if let Some(panel) = litex_panel {
        let parts = take_gpio_parts(&mut panel_parts, &panel.name);
        map_litex_panel(panel, &parts, &configs, &mut machine.bus)?;
    }
    if let Some(entry) = panel_parts.first() {
        return Err(entry.error("expected a supported GPIO attachment"));
    }
    Ok(machine)
}

fn parse_si70xx(entry: &Entry, controller: &Entry) -> Result<Sensor, String> {
    let mut registration = entry
        .registration
        .as_deref()
        .unwrap_or("")
        .split_whitespace();
    let (Some(parent), Some(address), None) = (
        registration.next(),
        registration.next(),
        registration.next(),
    ) else {
        return Err(entry.error("expected an I2C controller and address"));
    };
    if parent != controller.name {
        return Err(entry.error("unknown LiteX I2C controller"));
    }
    let address =
        irq_index(address, 128).map_err(|_| entry.error("expected a 7-bit I2C address"))?;
    let mut model = None;
    let mut temperature = 0;
    let mut humidity = 0;
    for (key, value) in keys(entry, &["model", "Temperature", "Humidity"])? {
        match (key, value) {
            ("model", "Model.SI7021") => model = Some(0x15),
            ("model", "Model.SI7006") => model = Some(0x06),
            ("Temperature", value) => {
                temperature = sensor_quantity(entry, key, value, -40.0, 85.0)?
            }
            ("Humidity", value) => {
                humidity = sensor_quantity(entry, key, value, 0.0, 100.0)? as u32
            }
            _ => return Err(entry.error("unsupported SI70xx model")),
        }
    }
    let model = model.ok_or_else(|| entry.error("expected SI70xx model"))?;
    Ok(Sensor {
        name: format!("{parent}.{}", entry.name),
        address,
        model,
        temperature,
        humidity,
    })
}

fn sensor_quantity(
    entry: &Entry,
    key: &str,
    value: &str,
    min: f64,
    max: f64,
) -> Result<i32, String> {
    value
        .parse::<f64>()
        .ok()
        .filter(|value| (min..=max).contains(value))
        .map(|value| (value * 100.0).round() as i32)
        .ok_or_else(|| entry.error(&format!("invalid {key} value")))
}

fn take_gpio_parts<'a>(parts: &mut Vec<&'a Entry>, parent: &str) -> Vec<&'a Entry> {
    let mut attached = Vec::new();
    parts.retain(|entry| {
        if entry
            .registration
            .as_deref()
            .and_then(|registration| registration.split_whitespace().next())
            == Some(parent)
        {
            attached.push(*entry);
            false
        } else {
            true
        }
    });
    attached
}

fn gpio_attachment(part: &Entry, parent: &Entry, count: u32) -> Result<u8, String> {
    let mut registration = part
        .registration
        .as_deref()
        .unwrap_or("")
        .split_whitespace();
    let (Some(name), Some(pin), None) = (
        registration.next(),
        registration.next(),
        registration.next(),
    ) else {
        return Err(part.error("expected a GPIO controller name and pin"));
    };
    if name != parent.name {
        return Err(part.error("unknown GPIO controller"));
    }
    irq_index(pin, count).map_err(|e| part.error(&e))
}

fn parse_gpio_button(part: &Entry, parent: &Entry, pin: u8) -> Result<Button, String> {
    let mut connected = false;
    let mut inverted = None;
    let expected = format!("->{}@{pin}", parent.name);
    for (_, line) in &part.body {
        let compact: String = line
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect();
        if !connected && compact.strip_prefix("IRQ").unwrap_or(&compact) == expected {
            connected = true;
        } else if let Some(("invert", value)) = line
            .split_once(':')
            .map(|(key, value)| (key.trim(), value.trim()))
            && inverted.is_none()
        {
            inverted = Some(
                value
                    .parse::<bool>()
                    .map_err(|_| part.error("expected a boolean invert property"))?,
            );
        } else {
            return Err(part.error("invalid button wiring or property"));
        }
    }
    if !connected {
        return Err(part.error("missing GPIO button wiring"));
    }
    Ok(Button::new(
        part.name.clone(),
        pin,
        inverted.unwrap_or(false),
    ))
}

fn map_litex_gpio(
    gpio: &Entry,
    parts: &[&Entry],
    configs: &[RiscvConfig<'_>],
    bus: &mut Bus,
) -> Result<(), String> {
    let mut mode = None;
    let mut irq_enabled = None;
    let mut irq = None;
    let mut led_wires = Vec::new();
    for (_, text) in &gpio.body {
        if let Some((output, target)) = text.split_once("->") {
            if let Ok(pin) = irq_index(output, 32)
                && let Some((target, "0")) = target.trim().split_once('@')
            {
                led_wires.push((pin, target.trim()));
            } else if irq.is_none() {
                irq = Some(vex_irq(gpio, text, configs)?);
            } else {
                return Err(gpio.error("duplicate GPIO IRQ wiring"));
            }
            continue;
        }
        match text
            .split_once(':')
            .map(|(key, value)| (key.trim(), value.trim()))
        {
            Some(("type", value)) if mode.is_none() => {
                mode = Some(match value {
                    "Type.In" => GpioMode::In,
                    "Type.Out" => GpioMode::Out,
                    "Type.InOut" => GpioMode::InOut,
                    _ => return Err(gpio.error("unsupported LiteX GPIO type")),
                });
            }
            Some(("enableIrq", value)) if irq_enabled.is_none() => {
                irq_enabled = Some(
                    value
                        .parse::<bool>()
                        .map_err(|_| gpio.error("expected boolean enableIrq"))?,
                );
            }
            _ => return Err(gpio.error("unsupported or duplicate LiteX GPIO property")),
        }
    }
    let mode = mode.ok_or_else(|| gpio.error("missing LiteX GPIO type"))?;
    let irq_enabled = irq_enabled.unwrap_or(false);
    if irq_enabled && matches!(mode, GpioMode::Out) || irq.is_some() && !irq_enabled {
        return Err(gpio.error("LiteX GPIO IRQ requires an input and enableIrq"));
    }
    let mut buttons = Vec::new();
    let mut leds = Vec::new();
    let mut input_pins = HashSet::new();
    for part in parts {
        let pin = gpio_attachment(part, gpio, 32)?;
        if part.model == "Miscellaneous.LED" {
            keys(part, &[])?;
            if matches!(mode, GpioMode::In) {
                return Err(part.error("LED requires GPIO output"));
            }
            let wires_before = led_wires.len();
            led_wires.retain(|&(output, name)| output != pin || name != part.name);
            if led_wires.len() != wires_before {
                leds.push((format!("{}.{}", gpio.name, part.name), pin));
            }
        } else {
            if matches!(mode, GpioMode::Out) || !input_pins.insert(pin) {
                return Err(part.error("button requires an available GPIO input"));
            }
            buttons.push(parse_gpio_button(part, gpio, pin)?);
        }
    }
    if !led_wires.is_empty() {
        return Err(gpio.error("GPIO LED wiring does not match an attachment"));
    }
    bus.map_litex_io(
        gpio.addresses()?,
        IoDevice::Gpio(Gpio::new(mode, irq_enabled, irq, buttons, leds)),
    )
    .map_err(|e| gpio.error(&e))
}

fn map_litex_panel(
    panel: &Entry,
    parts: &[&Entry],
    configs: &[RiscvConfig<'_>],
    bus: &mut Bus,
) -> Result<(), String> {
    let mut leds = HashMap::new();
    let mut buttons = Vec::new();
    let mut used_pins = HashSet::new();
    for part in parts {
        let pin = gpio_attachment(part, panel, 96)?;
        if !used_pins.insert(pin) {
            return Err(part.error("duplicate LiteX panel attachment"));
        }
        if part.model == "Miscellaneous.LED" {
            keys(part, &[])?;
            if pin >= 32 {
                return Err(part.error("LED requires an output pin in 0–31"));
            }
            leds.insert(part.name.as_str(), pin);
        } else {
            if pin < 32 {
                return Err(part.error("button requires an input pin in 32–95"));
            }
            buttons.push(parse_gpio_button(part, panel, pin)?);
        }
    }
    let mut irq = None;
    let mut led_outputs = HashSet::new();
    for (_, wire) in &panel.body {
        if let Some((output, target)) = wire.split_once("->")
            && let Ok(pin) = number(output.trim())
            && let Some((name, "0")) = target.trim().split_once('@')
        {
            if leds.get(name.trim()).map(|&pin| u32::from(pin)) != Some(pin)
                || !led_outputs.insert(pin)
            {
                return Err(panel.error("invalid or duplicate LiteX panel LED wiring"));
            }
        } else if irq.is_none() {
            irq = Some(vex_irq(panel, wire, configs)?);
        } else {
            return Err(panel.error("duplicate LiteX panel IRQ wiring"));
        }
    }
    let led_pins = leds
        .into_iter()
        .filter(|(_, pin)| led_outputs.contains(&u32::from(*pin)))
        .map(|(name, pin)| (format!("{}.{name}", panel.name), pin))
        .collect();
    bus.map_litex_panel(Panel::new(panel.addresses()?, irq, buttons, led_pins))
        .map_err(|e| panel.error(&e))
}

fn ethernet_registration(entry: &Entry, registration: &str) -> Result<Registration, String> {
    let Some((base, size, name)) = multi_registration(entry, registration)? else {
        return Ok(Registration {
            base: entry.registration_address(registration)?,
            size: Ethernet::MAC_SIZE,
            region: EthernetRegion::Mac,
        });
    };
    let region = match name {
        "buffer" => EthernetRegion::Buffer,
        "phy" => EthernetRegion::Phy,
        _ => return Err(entry.error("unsupported Ethernet region")),
    };
    Ok(Registration { base, size, region })
}

fn map_litex_i2s(entry: &Entry, configs: &[RiscvConfig<'_>], bus: &mut Bus) -> Result<(), String> {
    let mut csr = None;
    let mut buffer = None;
    for registration in entry
        .registration_list()
        .ok_or_else(|| entry.error("expected LiteX I2S regions"))?
    {
        match multi_registration(entry, registration)? {
            Some((address, size, "buffer")) if size != 0 && buffer.is_none() => {
                buffer = Some((address, size));
            }
            None if csr.is_none() => {
                let address = registration
                    .strip_prefix("sysbus ")
                    .ok_or_else(|| entry.error("expected I2S CSR on sysbus"))?;
                csr = Some(number(address.trim()).map_err(|e| entry.error(&e))?);
            }
            _ => return Err(entry.error("invalid or duplicate I2S registration")),
        }
    }
    let mut irq = None;
    let mut rate = None;
    let mut width = None;
    let mut channels = None;
    for (_, text) in &entry.body {
        if text.contains("->") && irq.is_none() {
            irq = Some(vex_irq(entry, text, configs)?);
            continue;
        }
        let (key, value) = text
            .split_once(':')
            .map(|(key, value)| (key.trim(), value.trim()))
            .ok_or_else(|| entry.error("expected I2S property or IRQ"))?;
        let target = match key {
            "format" if value == "DataFormat.Standard" => continue,
            "samplingRateHz" if rate.is_none() => &mut rate,
            "sampleWidthBits" if width.is_none() => &mut width,
            "numberOfChannels" if channels.is_none() => &mut channels,
            _ => return Err(entry.error("unsupported or duplicate I2S property")),
        };
        *target = Some(number(value).map_err(|e| entry.error(&e))?);
    }
    let mut device = I2s::new(
        entry.name.clone(),
        csr.ok_or_else(|| entry.error("missing I2S CSR region"))?,
        buffer.ok_or_else(|| entry.error("missing I2S buffer region"))?,
        irq.ok_or_else(|| entry.error("missing I2S IRQ"))?,
        entry.model == "Sound.LiteX_I2S_Master",
    );
    device.rate = rate
        .filter(|rate| (1_000..=192_000).contains(rate))
        .ok_or_else(|| entry.error("invalid I2S sample rate"))?;
    device.sample_bytes = width
        .filter(|width| matches!(width, 8 | 16 | 24 | 32))
        .ok_or_else(|| entry.error("invalid I2S sample width"))? as usize
        / 8;
    device.channels = channels
        .filter(|channels| (1..=2).contains(channels))
        .ok_or_else(|| entry.error("invalid I2S channel count"))?;
    bus.map_litex_i2s(device).map_err(|e| entry.error(&e))
}

fn map_litex_video(
    entry: &Entry,
    memories: &HashMap<&str, (u32, u32)>,
    bus: &mut Bus,
) -> Result<(), String> {
    let mut dma = None;
    let mut vtg = None;
    for registration in entry
        .registration_list()
        .ok_or_else(|| entry.error("expected framebuffer regions"))?
    {
        let (address, size, region) = multi_registration(entry, registration)?
            .ok_or_else(|| entry.error("expected named framebuffer region"))?;
        match region {
            "dma" if size == Framebuffer::DMA_SIZE && dma.is_none() => dma = Some(address),
            "vtg" if size == Framebuffer::VTG_SIZE && vtg.is_none() => vtg = Some(address),
            _ => return Err(entry.error("invalid or duplicate framebuffer region")),
        }
    }
    let mut memory = None;
    let mut properties = [0; 3];
    let mut swap_red_blue = false;
    for (key, value) in keys(entry, &["memory", "offset", "hres", "vres", "format"])? {
        match key {
            "memory" => memory = memories.get(value).copied(),
            "format" => {
                swap_red_blue = match value {
                    "PixelFormat.XBGR8888" => false,
                    "PixelFormat.XRGB8888" => true,
                    _ => return Err(entry.error("unsupported LiteX framebuffer format")),
                }
            }
            "offset" | "hres" | "vres" => {
                let index = match key {
                    "offset" => 0,
                    "hres" => 1,
                    _ => 2,
                };
                properties[index] = number(value).map_err(|e| entry.error(&e))?;
            }
            _ => unreachable!(),
        }
    }
    let (memory, memory_size) = memory.ok_or_else(|| entry.error("unknown framebuffer memory"))?;
    let [offset, width, height] = properties;
    let width = u16::try_from(width).map_err(|_| entry.error("invalid hres"))?;
    let height = u16::try_from(height).map_err(|_| entry.error("invalid vres"))?;
    bus.map_litex_video(Framebuffer {
        name: entry.name.clone(),
        dma: dma.ok_or_else(|| entry.error("missing framebuffer DMA range"))?,
        vtg: vtg.ok_or_else(|| entry.error("missing framebuffer VTG range"))?,
        memory,
        memory_size,
        offset,
        width,
        height,
        swap_red_blue,
        active: None,
    })
    .map_err(|e| entry.error(&e))
}

fn sd_registration(entry: &Entry, registration: &str) -> Result<litex_sd::Registration, String> {
    let Some((base, size, name)) = multi_registration(entry, registration)? else {
        return Ok(litex_sd::Registration {
            base: entry.registration_address(registration)?,
            size: SdController::PHY_SIZE,
            region: litex_sd::Region::Phy,
        });
    };
    let region = match name {
        "core" => litex_sd::Region::Core,
        "reader" => litex_sd::Region::Reader,
        "writer" => litex_sd::Region::Writer,
        _ => return Err(entry.error("unsupported LiteX SD region")),
    };
    Ok(litex_sd::Registration { base, size, region })
}

fn multi_registration<'a>(
    entry: &Entry,
    registration: &'a str,
) -> Result<Option<(u32, u32, &'a str)>, String> {
    let Some(properties) = registration.strip_prefix("sysbus new Bus.BusMultiRegistration") else {
        return Ok(None);
    };
    let properties = properties
        .trim()
        .strip_prefix('{')
        .and_then(|text| text.strip_suffix('}'))
        .ok_or_else(|| entry.error("expected multi-registration properties"))?;
    let mut base = None;
    let mut size = None;
    let mut region = None;
    for property in properties.split_terminator(';') {
        let (key, value) = property
            .split_once(':')
            .map(|(key, value)| (key.trim(), value.trim()))
            .ok_or_else(|| entry.error("invalid multi-registration property"))?;
        match key {
            "address" if base.is_none() => base = Some(number(value).map_err(|e| entry.error(&e))?),
            "size" if size.is_none() => size = Some(number(value).map_err(|e| entry.error(&e))?),
            "region" if region.is_none() => {
                region = Some(
                    value
                        .strip_prefix('"')
                        .and_then(|text| text.strip_suffix('"'))
                        .ok_or_else(|| entry.error("expected a quoted registration region"))?,
                );
            }
            _ => return Err(entry.error("unsupported or duplicate multi-registration property")),
        }
    }
    match (base, size, region) {
        (Some(base), Some(size), Some(region)) => Ok(Some((base, size, region))),
        _ => Err(entry.error("expected address, size and region")),
    }
}

fn map_sysbus_ranges(entry: &Entry, bus: &mut Bus) -> Result<(), String> {
    let [(_, init), ranges @ ..] = entry.body.as_slice() else {
        return Err(entry.error("expected a sysbus init block"));
    };
    if !matches!(init.as_str(), "init:" | "init add:") || ranges.is_empty() {
        return Err(entry.error("expected a sysbus init block"));
    }
    for (line, text) in ranges {
        let error = |detail: &str| format!("{}:{line}: {detail}", entry.source.display());
        let text = text.split('#').next().unwrap().trim();
        let (command, range) = text
            .split_once(char::is_whitespace)
            .ok_or_else(|| error("expected a sysbus init command"))?;
        let (range, tail) = range
            .trim_start()
            .strip_prefix('<')
            .and_then(|text| text.split_once('>'))
            .ok_or_else(|| error("expected <start size>"))?;
        let mut values = range.split_whitespace();
        let (Some(start), Some(size), None) = (values.next(), values.next(), values.next()) else {
            return Err(error("expected <start size>"));
        };
        let start = number(start).map_err(|e| error(&e))?;
        let size = number(size).map_err(|e| error(&e))?;
        let tail = tail.trim();
        match command {
            "SilenceRange" if tail.is_empty() => bus.map_silencer(start, size),
            "Tag"
                if tail
                    .strip_prefix('"')
                    .and_then(|value| value.strip_suffix('"'))
                    .is_some_and(|name| !name.is_empty() && !name.contains('"')) =>
            {
                bus.map_tag(start, size)
            }
            _ => return Err(error("expected SilenceRange or Tag")),
        }
        .map_err(|e| error(&e))?;
    }
    Ok(())
}

fn unsupported_rv32_isa(value: &str, architecture: &str) -> String {
    let mut parts = architecture.split('_');
    let base = parts.next().and_then(|part| part.strip_prefix("rv32"));
    let mut missing = Vec::new();
    if let Some(base) = base {
        if base.contains('e') && base.chars().all(|letter| "iemafdcvbg".contains(letter)) {
            if base.contains(['i', 'g']) {
                return "ISA string cannot contain both I and E base instruction sets at the same time.".into();
            }
            if base.chars().any(|letter| !"emac".contains(letter)) {
                return "RV32E can only have M, A and C standard extensions".into();
            }
        }
        for letter in base.chars() {
            match letter {
                'i' | 'm' | 'a' | 'c' => {}
                'g' | 'f' | 'd' => {}
                'v' => missing.push("V"),
                _ => return format!("unsupported CPU type {value}"),
            }
        }
        for extension in parts.clone() {
            let remainder = extension.trim_start_matches(|letter| "iemafdcvbgu".contains(letter));
            if remainder.starts_with(|letter: char| {
                letter.is_ascii_alphabetic() && !matches!(letter, 's' | 'x' | 'z')
            }) {
                return format!("Undefined instructions set extension: '{remainder}'");
            }
        }
        missing.extend(parts.filter(|extension| {
            !matches!(
                *extension,
                "zicsr"
                    | "zifencei"
                    | "zicond"
                    | "zba"
                    | "zbb"
                    | "zbc"
                    | "zbs"
                    | "zbkb"
                    | "zacas"
                    | "zfh"
                    | "smepmp"
            )
        }));
    }
    missing.dedup();
    if missing.is_empty() {
        format!("unsupported CPU type {value}")
    } else {
        format!(
            "unsupported CPU type {value}; unavailable extensions: {}",
            missing.join(", ")
        )
    }
}

type RiscvConfig<'a> = (
    usize,
    &'a Entry,
    (
        bool,
        bool,
        bool,
        bool,
        bool,
        bool,
        bool,
        BitManip,
        (bool, bool, bool),
    ),
    usize,
    (bool, bool),
);

fn uses_vex_irq(entry: &Entry) -> bool {
    entry.model == "CPU.VexRiscv"
        && !entry.body.iter().any(|(_, text)| {
            text.split_once(':').is_some_and(|(key, value)| {
                key.trim() == "builtInIrqController" && value.trim().eq_ignore_ascii_case("false")
            })
        })
}

fn vex_irq(entry: &Entry, wire: &str, configs: &[RiscvConfig<'_>]) -> Result<(usize, u8), String> {
    if let Some((pin, target)) = wire.split_once("->")
        && matches!(pin.trim(), "" | "IRQ")
        && let Some((cpu, line)) = target.split_once('@')
        && let Some((hart, &(_, target, _, _, _))) =
            configs
                .iter()
                .enumerate()
                .find(|(_, (_, target, _, _, _))| {
                    target.name == cpu.trim()
                        && (uses_vex_irq(target) || target.model == "CPU.Minerva")
                })
    {
        let line = number(line.trim()).map_err(|e| entry.error(&e))?;
        return match line {
            0..=31 => Ok((hart, line as u8)),
            1000..=1031 if uses_vex_irq(target) => Ok((hart, (line - 1000 + 32) as u8)),
            _ => Err(entry.error("VexRiscv external IRQ must be in 0–31 or 1000–1031")),
        };
    }
    Err(entry.error("expected IRQ wired to a VexRiscv external interrupt"))
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
    keys_in(entry, &entry.body, allowed)
}

fn split_rv32_init(entry: &Entry) -> (&[(usize, String)], Option<&[(usize, String)]>) {
    if entry.model == "CPU.RiscV32"
        && let Some(index) = entry.body.iter().position(|(_, text)| text == "init:")
    {
        return (&entry.body[..index], Some(&entry.body[index + 1..]));
    }
    (&entry.body, None)
}

fn keys_in<'a>(
    entry: &'a Entry,
    body: &'a [(usize, String)],
    allowed: &[&str],
) -> Result<Vec<(&'a str, &'a str)>, String> {
    let mut seen = HashSet::new();
    body.iter()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_litex_phy_and_clock_controller_from_repl() {
        let directory = std::env::temp_dir().join(format!("penv-litex-phy-{}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        let path = directory.join("platform.repl");
        fs::write(
            &path,
            r#"cpu: CPU.VexRiscv @ sysbus
ram: Memory.MappedMemory @ sysbus 0x40000000
    size: 0x1000
eth: Network.LiteX_Ethernet @ {
    sysbus 0xe0007800;
    sysbus new Bus.BusMultiRegistration { address: 0xe0007000; size: 0x800; region: "phy" }
}
phy: Network.EthernetPhysicalLayer @ eth 0
    VendorSpecific1: 0x4400
clock0: Miscellaneous.LiteX_MMCM_CSR32 @ sysbus 0xe0004800
"#,
        )
        .unwrap();
        let machine = load(&path, None);
        fs::remove_dir_all(directory).unwrap();

        fn read_phy_status(bus: &mut Bus) -> u32 {
            let mut sink = std::io::sink();
            let mut tick = |bit: u32| {
                bus.write(0xe000_7004, 4, bit << 2, &mut sink).unwrap();
                bus.write(0xe000_7004, 4, (bit << 2) | 1, &mut sink)
                    .unwrap();
                bus.read::<4>(0xe000_7008).unwrap()
            };
            for _ in 0..32 {
                tick(1);
            }
            for bit in 0..16 {
                tick((0x2206 >> bit) & 1);
            }
            (0..16).fold(0, |status, _| (status << 1) | tick(0))
        }

        let mut machine = machine.unwrap();
        machine.set_entry(0x4000_0000);
        assert_eq!(machine.bus.read::<4>(0xe000_4804).unwrap(), 1);
        machine
            .bus
            .memory(0x4000_0000, 4)
            .unwrap()
            .copy_from_slice(&0x0000_006fu32.to_le_bytes());
        assert!(
            machine
                .schedule_phy_vendor_specific1(1, "other.phy".into(), 0)
                .is_err()
        );
        machine
            .schedule_phy_vendor_specific1(2, "sysbus.eth.phy".into(), 0)
            .unwrap();
        let mut sink = std::io::sink();
        machine.run(1, &mut sink).unwrap();
        assert_eq!(read_phy_status(&mut machine.bus), 0x4400);
        machine.run(1, &mut sink).unwrap();
        assert_eq!(read_phy_status(&mut machine.bus), 0);
    }
}
