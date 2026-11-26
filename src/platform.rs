mod nrf52840;
use crate::repl_syntax::{quoted_string, unquoted_delimiters, without_comment};

mod stm32f0;
mod scoped_nvic;

use std::{
    cell::RefCell,
    rc::Rc,
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
    stm32_adc::Stm32F0Adc,
    stm32_can::Stm32Can,
    stm32_dma::Stm32Dma,
    stm32_gpio::Stm32Gpio,
    stm32_gpio_board::{GpioButton, GpioEndpoints, GpioLed},
    stm32_i2c::{Eeprom, Stm32I2c},
    stm32_rtc::Stm32Rtc,
    stm32_spi::{Stm32F0Spi, Stp08},
    stm32_timer::{Stm32Timer, TimerPin},
    stm32_uart::{Stm32Irq, Stm32Usart},
    uart::LiteXTxTiming,
};

type Properties = Vec<(usize, String)>;

#[derive(Debug)]
pub(crate) struct Entry {
    pub(crate) name: String,
    pub(crate) model: String,
    pub(crate) registration: Option<String>,
    pub(crate) body: Properties,
    source: PathBuf,
    line: usize,
}

impl Entry {
    pub(crate) fn error(&self, detail: &str) -> String {
        format!(
            "{}:{}: {}: {detail}",
            self.source.display(),
            self.line,
            self.name
        )
    }

    fn sysbus(&self) -> Result<Option<u32>, String> {
        self.sysbus_address()?
            .map(number)
            .transpose()
            .map_err(|e| self.error(&e))
    }

    fn sysbus_address(&self) -> Result<Option<&str>, String> {
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
        Ok(Some(address.trim()))
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

    pub(crate) fn address(&self) -> Result<u32, String> {
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

struct BusPoint<'a> {
    address: u32,
    condition: &'a str,
}

fn conditional_bus_point(entry: &Entry) -> Result<Option<BusPoint<'_>>, String> {
    let Some(registration) = entry
        .registration
        .as_deref()
        .and_then(|value| value.strip_prefix("sysbus new Bus.BusPointRegistration"))
    else {
        return Ok(None);
    };
    let fields = registration
        .trim()
        .strip_prefix('{')
        .and_then(|value| value.strip_suffix('}'))
        .ok_or_else(|| entry.error("expected a bus point registration body"))?;
    let mut address = None;
    let mut condition = None;
    for field in fields
        .split(';')
        .map(str::trim)
        .filter(|field| !field.is_empty())
    {
        match field
            .split_once(':')
            .map(|(key, value)| (key.trim(), value.trim()))
        {
            Some(("address", value)) if address.is_none() => {
                address = Some(number(value).map_err(|e| entry.error(&e))?);
            }
            Some(("condition", value)) if condition.is_none() => {
                condition = Some(
                    value
                        .strip_prefix('"')
                        .and_then(|value| value.strip_suffix('"'))
                        .filter(|value| !value.trim().is_empty() && !value.contains('"'))
                        .ok_or_else(|| entry.error("expected a nonempty quoted bus condition"))?,
                );
            }
            _ => return Err(entry.error("unsupported or duplicate bus point property")),
        }
    }
    let address = address.ok_or_else(|| entry.error("expected a bus point address"))?;
    let condition = condition.ok_or_else(|| entry.error("expected a bus point condition"))?;
    Ok(Some(BusPoint { address, condition }))
}

fn validate_bus_point(
    entry: &Entry,
    point: &BusPoint<'_>,
    entries: &[Entry],
) -> Result<(), String> {
    for term in point.condition.split("&&") {
        if let Some((field, name)) = term.split_once("==")
            && field.trim() == "initiator"
        {
            let name = name.trim();
            if !entries.iter().any(|peer| peer.name == name) {
                return Err(entry.error(&format!("Invalid initiator: {name}")));
            }
        }
    }
    // In an otherwise empty platform the newly registered peripheral is
    // a bus target. There is no initiator that could supply state bits.
    if entries.len() == 1 && !entry.model.starts_with("CPU.") {
        return Err(entry.error(&format!(
            "Conditions provided ({}) but there are no peripherals implementing IPeripheralWithTransactionState or they have no common state bits",
            point.condition
        )));
    }
    Ok(())
}

pub(crate) fn number(value: &str) -> Result<u32, String> {
    let digits = value.replace('_', "");
    match digits.strip_prefix("0x") {
        Some(hex) => u32::from_str_radix(hex, 16),
        None => digits.parse(),
    }
    .map_err(|_| format!("invalid 32-bit number {value:?}"))
}

fn ends_with_typed_registration(prefix: &str) -> bool {
    matches!(
        prefix.split_whitespace().last(),
        Some("Bus.BusPointRegistration" | "Bus.BusRangeRegistration")
    )
}

fn inline_body(declaration: &str, lineno: usize) -> Result<(&str, Properties), &'static str> {
    let mut braces = unquoted_delimiters(declaration, &['{', '}']);
    let Some((mut begin, '{')) = braces.next() else {
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
    if ends_with_typed_registration(&declaration[..begin]) {
        match braces.next() {
            Some((_, '}')) => {}
            None => return Ok((declaration, Vec::new())),
            _ => return Err("invalid typed registration braces"),
        }
        match braces.next() {
            Some((next, '{')) => begin = next,
            None => return Ok((declaration, Vec::new())),
            _ => return Err("unexpected closing brace"),
        }
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
    let mut multiline_script = false;
    let mut registration_depth: i32 = 0;
    let mut property_block = false;
    let mut lines = content.lines().enumerate();
    let mut remainder = None;
    while let Some((index, raw)) = remainder.take().or_else(|| lines.next()) {
        if multiline_script {
            if raw.contains("'''") {
                multiline_script = false;
            }
            continue;
        }
        let mut line = without_comment(raw);
        if !property_block && registration_depth == 0 && !line.starts_with(char::is_whitespace)
            && let Some(separator) = crate::repl_parser::declaration_separator(line)
        {
            remainder = Some((index, line[separator + 1..].trim_start()));
            line = &line[..separator];
        }
        let text = line.trim();
        let lineno = index + 1;
        let error = |detail: &str| format!("{}:{lineno}: {detail}", path.display());
        if text.is_empty() {
            continue;
        }
        if property_block {
            let properties = text.strip_suffix('}').unwrap_or(text);
            property_block = properties == text;
            if brace_depth(properties) != 0 {
                return Err(error("invalid multi-line property block"));
            }
            let entry = current.as_mut().unwrap();
            let mut start = 0;
            for end in unquoted_delimiters(properties, &[';'])
                .map(|(index, _)| index)
                .chain([properties.len()])
            {
                let item = properties[start..end].trim();
                if !item.is_empty() {
                    entry.body.push((lineno, item.into()));
                }
                start = end + 1;
            }
            continue;
        }
        if registration_depth != 0 {
            let registration = current.as_mut().unwrap().registration.as_mut().unwrap();
            registration.push(' ');
            registration.push_str(text);
            registration_depth += brace_depth(text);
            if registration_depth < 0 {
                return Err(error("invalid registration braces"));
            }
            continue;
        }
        if text
            .strip_prefix("script:")
            .is_some_and(|value| value.trim() == "'''")
        {
            multiline_script = true;
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
            let filename = quoted_string(include)
                .filter(|filename| !filename.is_empty())
                .ok_or_else(|| error("expected using \"path.repl\""))?;
            let local = canonical.parent().unwrap().join(&filename);
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
        let declaration = declaration.trim_end();
        let (declaration, body) = if let Some(prefix) = declaration.strip_suffix('{')
            && !prefix.trim_end().ends_with('@')
            && !ends_with_typed_registration(prefix)
            && brace_depth(prefix) == 0
        {
            property_block = true;
            (prefix, Vec::new())
        } else {
            inline_body(declaration, lineno).map_err(error)?
        };
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
        registration_depth = registration.as_deref().map_or(0, brace_depth);
        if registration_depth < 0 {
            return Err(error("invalid registration braces"));
        }
        current = Some(Entry {
            name: name.into(),
            model: model.into(),
            registration,
            body,
            source: path.into(),
            line: lineno,
        });
    }
    if multiline_script || registration_depth != 0 || property_block {
        return Err(format!(
            "{}: unterminated script or registration",
            path.display()
        ));
    }
    entries.extend(current);
    visiting.remove(&canonical);
    Ok(())
}

fn brace_depth(value: &str) -> i32 {
    unquoted_delimiters(value, &['{', '}'])
        .map(|(_, brace)| if brace == '{' { 1 } else { -1 })
        .sum()
}

fn apply_repl_overrides(entries: &mut Vec<Entry>) -> Result<(), String> {
    let mut declarations = HashMap::<String, usize>::new();
    let mut merged: Vec<Entry> = Vec::with_capacity(entries.len());
    for entry in std::mem::take(entries) {
        if entry.model.is_empty()
            && entry.registration.is_none()
            && entry.name != "sysbus"
            && !entry.body.is_empty()
            && entry.body.iter().all(|(_, text)| property_key(text).is_some())
            && let Some(&index) = declarations.get(&entry.name)
            && !merged[index].model.is_empty()
        {
            let properties: HashSet<_> = entry.body.iter()
                .filter_map(|(_, text)| property_key(text)).collect();
            merged[index].body.retain(|(_, text)| {
                property_key(text).is_none_or(|key| !properties.contains(key))
            });
            merged[index].body.extend(entry.body);
        } else if entry.model.is_empty()
            && entry.registration.is_none()
            && !entry.body.is_empty()
            && let Some(&index) = declarations.get(&entry.name)
            && matches!(merged[index].model.as_str(), "UART.STM32F7_USART" | "UART.STM32_UART")
            && entry.body.iter().all(|(_, text)| {
                text.split_once("->").is_some_and(|(source, _)| {
                    matches!(source.trim(), "ReceiveDmaRequest" | "TransmitDmaRequest")
                })
            })
        {
            merged[index].body.extend(entry.body);
        } else if entry.model.is_empty()
            && entry.registration.is_none()
            && !entry.body.is_empty()
            && let Some(&index) = declarations.get(&entry.name)
            && matches!(
                merged[index].model.as_str(),
                "Timers.STM32_Timer" | "GPIOPort.STM32_GPIOPort"
            )
            && entry.body.iter().all(|(_, text)| {
                text.split_once("->").is_some_and(|(source, _)| {
                    source.trim().parse::<u8>().is_ok()
                })
            })
        {
            merged[index].body.extend(entry.body);
        } else if entry.model.is_empty()
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

pub(crate) fn property_key(text: &str) -> Option<&str> {
    let (key, value) = text.split_once(':')?;
    let key = key.trim();
    (!value.trim().is_empty()
        && !key.is_empty()
        && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
    .then_some(key)
}

pub(crate) fn validate_entry_properties<'a, S>(
    body: &'a [(S, String)],
    error: impl Fn(&S, &str) -> String,
) -> Result<(), String> {
    let mut properties = HashSet::new();
    for (source, text) in body {
        if crate::repl_parser::init_header(text).is_some() {
            break;
        }
        if let Some(key) = property_key(text)
            && !properties.insert(key)
        {
            return Err(error(source, key));
        }
    }
    Ok(())
}

pub fn load(path: &Path, root: Option<&Path>, stm32f0_console: bool) -> Result<Machine, String> {
    load_with_assets(path, root, stm32f0_console, None)
}

pub fn load_with_assets(path: &Path, root: Option<&Path>, stm32f0_console: bool, assets: Option<&Path>) -> Result<Machine, String> {
    load_platform(path, root, true, stm32f0_console, assets)
}

pub fn check(path: &Path) -> Result<(), String> {
    load_platform(path, None, false, false, None).map(|_| ())
}

fn load_platform(
    path: &Path,
    root: Option<&Path>,
    require_ram: bool,
    stm32f0_console: bool,
    assets: Option<&Path>,
) -> Result<Machine, String> {
    let mut entries = Vec::new();
    parse(
        path,
        root.unwrap_or(Path::new(".")),
        &mut HashSet::new(),
        &mut entries,
    )?;
    for entry in &entries {
        validate_entry_properties(&entry.body, |line, key| {
            format!(
                "{}:{line}: {}: duplicate REPL property {key:?}",
                entry.source.display(),
                entry.name
            )
        })?;
    }
    if scoped_nvic::matches(&entries) && (!require_ram || entries.iter().any(|entry| {
        matches!(entry.model.as_str(), "Memory.MappedMemory" | "Memory.ArrayMemory")
    })) {
        return scoped_nvic::construct(&entries);
    }
    apply_repl_overrides(&mut entries)?;
    if entries.iter().any(|entry| entry.model == "CPU.RiscV64") {
        return crate::rv64_platform::load(
            entries,
            path.file_name().and_then(|name| name.to_str()),
        );
    }
    let mut machine = Machine::without_default_ram();
    if stm32f0::is_stm32f0(&entries) {
        machine
            .bus
            .set_stm32f0_tags(stm32f0::prepare(&mut entries)?);
    } else if stm32f0_console {
        return Err("--stm32f0-console requires a platform with an STM32F0 RCC".into());
    }
    let cortex_m = entries.iter().any(|entry| entry.model == "CPU.CortexM");
    let mut names = HashSet::new();
    let mut cpus = Vec::new();
    let mut external_pmps = HashMap::new();
    let mut clint = None;
    let mut riscv_machine_timer = None;
    let mut andes_pits = Vec::new();
    let mut andes_gpios = Vec::new();
    let mut andes_watchdogs = Vec::new();
    let mut andes_spis = Vec::new();
    let mut andes_rtcs = Vec::new();
    let mut egis_syscon = None;
    let mut puya_flashes = Vec::new();
    let mut clic = None;
    let mut nvic = None;
    let mut exti = None;
    let mut combiners = Vec::new();
    let mut ram = false;
    let mut memory_bases = HashMap::new();
    let mut stm32l0_flash = None;
    let mut stm32l0_rcc = None;
    let mut stm32l0_power = None;
    let mut low_power_timers = Vec::new();
    let mut stm32_rngs = Vec::new();
    let mut flash_controller = None;
    let mut flash_chip = None;
    let mut stm32_spi_flashes = Vec::new();
    let mut uart = None;
    let mut stm32_uarts = Vec::new();
    let mut sifive_uarts = Vec::new();
    let mut sifive_gpios = Vec::new();
    let mut stm32_timers = Vec::new();
    let mut stm32_spis = Vec::new();
    let mut spi_shifts = Vec::new();
    let mut stm32_gpios = Vec::new();
    let mut gpio_leds = Vec::new();
    let mut gpio_pwm_testers = Vec::new();
    let mut gpio_buttons = Vec::new();
    let mut stm32_i2cs = Vec::new();
    let mut stm32_cans = Vec::new();
    let mut i2c_eeproms = Vec::new();
    let mut i2c_frams = HashMap::new();
    let mut i2c_fram_relays = Vec::new();
    let mut i2c_sensors = Vec::new();
    let mut stm32_adc = None;
    let mut stm32_regular_adcs = Vec::new();
    let mut stm32_rtc = None;
    let mut stm32f4_rcc = None;
    let mut stm32_dma = None;
    let mut plic = Vec::new();
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
            map_sysbus_ranges(entry, &mut machine.bus, root.unwrap_or(Path::new(".")), assets)?;
            continue;
        }
        if entry.model.is_empty() && entry.registration.is_none()
            && andes_gpios.iter().any(|gpio: &&Entry| gpio.name == entry.name)
        {
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
            "Miscellaneous.RiscVExternalPMP" => {
                let cpu = entry
                    .registration
                    .as_deref()
                    .ok_or_else(|| entry.error("missing CPU attachment"))?;
                let mut count = 64;
                for (_, value) in keys(entry, &["numberOfPMPEntries"])? {
                    count = number(value).map_err(|error| entry.error(&error))? as usize;
                }
                if !matches!(count, 16 | 64) || external_pmps.insert(cpu, (entry, count)).is_some()
                {
                    return Err(
                        entry.error("expected one external PMP per CPU, with 16 or 64 entries")
                    );
                }
            }
            "Memory.MappedMemory" | "Memory.ArrayMemory" => {
                let (_, size) = keys(entry, &["size"])?
                    .into_iter()
                    .next()
                    .ok_or_else(|| entry.error("missing memory size"))?;
                if entry.registration.is_none() && entry.model == "Memory.MappedMemory"
                    && entries.iter().any(|flash| (flash.model == "SPI.Puya_P25Q" || (cortex_m && flash.model == "SPI.Micron_MT25Q"))
                        && flash.body.iter().any(|(_, line)| line.strip_prefix("underlyingMemory:")
                            .is_some_and(|name| name.trim() == entry.name)))
                {
                    continue;
                }
                if entry.registration_list().is_none()
                    && let Some(address) = entry.sysbus_address()?
                    && let Ok(base) = crate::parse_bus_address(address)
                    && base > u64::from(u32::MAX)
                {
                    machine
                        .bus
                        .map_global_ram(base, number(size).map_err(|e| entry.error(&e))? as usize)
                        .map_err(|e| entry.error(&e))?;
                    ram = true;
                    continue;
                }
                let length = number(size).map_err(|e| entry.error(&e))?;
                let registrations = memory_registrations(entry, length)?;
                let (first_address, first_size, _) = registrations[0];
                let addresses: Vec<_> = registrations.iter().map(|&(address, _, _)| address).collect();
                if registrations.iter().any(|&(_, size, offset)| size != length || offset != 0) {
                    let backing = machine.bus.allocate_ram_backing(length as usize)
                        .map_err(|error| entry.error(&error))?;
                    for (address, size, offset) in registrations {
                        if u64::from(address) + u64::from(size) > 1 << 32 {
                            return Err(entry.error("memory window exceeds RV32 address space"));
                        }
                        machine.bus.map_alias64(backing, u64::from(address), size as usize, offset as usize, 0, false)
                            .map_err(|error| entry.error(&error))?;
                        machine.bus.register_traceable_memory(&entry.name, &[address], size);
                    }
                } else {
                    if entry.model == "Memory.ArrayMemory" {
                        machine.bus.map_array_memory_views(&addresses, length as usize)
                    } else {
                        machine.bus.map_ram_views(&addresses, length as usize)
                    }.map_err(|error| entry.error(&error))?;
                    machine.bus.register_traceable_memory(&entry.name, &addresses, length);
                }
                if entry.model == "Memory.MappedMemory" {
                    memory_bases.insert(entry.name.as_str(), (first_address, first_size));
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
            "SPI.Micron_MT25Q" if cortex_m => stm32_spi_flashes.push(entry),
            "SPI.Micron_MT25Q" => {
                if flash_chip.replace(entry).is_some() {
                    return Err(entry.error("only one LiteX SPI flash chip is supported"));
                }
            }
            "SPI.LiteX_SPI" => litex_spis.push(entry),
            "SPI.AndesATCSPI200" => andes_spis.push(entry),
            "SPI.Puya_P25Q" => puya_flashes.push(entry),
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
            "Miscellaneous.LED" | "Miscellaneous.Button" if !cortex_m => panel_parts.push(entry),
            "GPIOPort.LiteX_GPIO" => litex_gpios.push(entry),
            "Timers.AndesATCPIT100" => andes_pits.push(entry),
            "Timers.AndesATCRTC100" => andes_rtcs.push(entry),
            "Miscellaneous.EgisET171_SMU2" | "Miscellaneous.EgisET171_Crypto" => {
                keys(entry, &[])?;
                let base = entry.address()?;
                let device = if entry.model == "Miscellaneous.EgisET171_SMU2" {
                    crate::egis_syscon::EgisExtra::smu2(base)
                } else {
                    crate::egis_syscon::EgisExtra::entropy(base)
                };
                machine.bus.map_egis_extra(device).map_err(|error| entry.error(&error))?;
            }
            "Miscellaneous.EgisET171_AOSMU" => {
                if egis_syscon.replace(entry).is_some() {
                    return Err(entry.error("only one Egis AOSMU is supported"));
                }
            }
            "Timers.AndesATCWDT200_Watchdog" => andes_watchdogs.push(entry),
            "GPIOPort.AndesATCGPIO100" => andes_gpios.push(entry),
            "Timers.RiscVMachineTimer" => {
                if riscv_machine_timer.replace(entry).is_some() {
                    return Err(entry.error("only one RISC-V machine timer is supported"));
                }
            }
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
            "IRQControllers.PlatformLevelInterruptController" | "IRQControllers.AndesNCEPLIC100" => plic.push(entry),
            "GPIOPort.STM32_GPIOPort" => stm32_gpios.push(entry),
            "Miscellaneous.LED" => gpio_leds.push(entry),
            "UnimplementedRegistersPeripheral" => {
                keys(entry, &[])?;
                machine.bus.map_unimplemented_register_peripheral(entry.address()?, &entry.name)
                    .map_err(|error| entry.error(&error))?;
            }
            "Mocks.BusFaultingPeripheral" if cortex_m => map_bus_faulting_model(entry, &mut machine.bus)?,
            "PWMTester" => gpio_pwm_testers.push(entry),
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
            "Miscellaneous.STM32F4_RCC" => {
                if stm32f4_rcc.replace(entry).is_some() {
                    return Err(entry.error("only one STM32F4 RCC is supported"));
                }
            }
            crate::stm32_iwdg::Iwdg::MODEL if cortex_m => {
                let watchdog = crate::stm32_iwdg::Iwdg::from_repl(entry.address()?, entry.body.iter().map(|(_, text)| text.as_str()), number)
                    .map_err(|error| entry.error(&error))?;
                machine.bus.map_iwdg(watchdog).map_err(|error| entry.error(&error))?;
            }
            "Timers.STM32_Timer" => stm32_timers.push(entry),
            "SPI.STM32SPI" => stm32_spis.push(entry),
            "SPI.STP08ShiftRegister" => spi_shifts.push(entry),
            "CAN.STMCAN" => stm32_cans.push(entry),
            "I2C.STM32F7_I2C" => stm32_i2cs.push(entry),
            "I2C.MB85RC1MT" => {
                keys(entry, &[])?;
                if entry.registration.is_some() {
                    return Err(entry.error("MB85RC1MT backing device has no bus registration"));
                }
                i2c_frams.insert(entry.name.as_str(), Rc::new(RefCell::new(crate::stm32_i2c::Fram::new())));
            }
            "I2C.MB85RC1MTLo" | "I2C.MB85RC1MTHi" => i2c_fram_relays.push(entry),
            "Memory.GenericI2cEeprom" => i2c_eeproms.push(entry),
            "I2C.SHT45" | "I2C.BME280" | "Sensors.LIS2DW12" => i2c_sensors.push(entry),
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
            "Miscellaneous.STM32L0_PWR" => {
                if stm32l0_power.replace(entry).is_some() {
                    return Err(entry.error("only one STM32L0 power controller is supported"));
                }
            }
            "Miscellaneous.STM32L0_RCC" => {
                if stm32l0_rcc.replace(entry).is_some() {
                    return Err(entry.error("only one STM32L0 RCC is supported"));
                }
            }
            "MTD.STM32L0_FlashController" => {
                if stm32l0_flash.replace(entry).is_some() {
                    return Err(entry.error("only one STM32L0 flash controller is supported"));
                }
            }
            crate::stm32h7_lptim::LpTimer::MODEL => low_power_timers.push(entry),
            crate::stm32h7_rng::Rng::MODEL => stm32_rngs.push(entry),
            "Analog.STM32_ADC" => stm32_regular_adcs.push(entry),
            "Analog.STM32F0_ADC" => {
                if stm32_adc.replace(entry).is_some() {
                    return Err(entry.error("only one STM32F0 ADC is supported"));
                }
            }
            "UART.STM32F7_USART" | "UART.STM32_UART" => stm32_uarts.push(entry),
            "UART.SiFive_UART" => sifive_uarts.push(entry),
            "GPIOPort.SiFive_GPIO" => sifive_gpios.push(entry),
            "UART.Murax_UART" | "UART.NS16550" | "UART.LiteX_UART" | "UART.TrivialUart" => {
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
            "IRQControllers.CoreLocalInterruptController" => {
                if clic.replace(entry).is_some() {
                    return Err(entry.error("only one CLIC is supported"));
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
    let extended_nvic = stm32_uarts
        .iter()
        .any(|uart| uart.model == "UART.STM32_UART");
    if let Some(entry) = stm32l0_power {
        let line = match entry.body.as_slice() {
            [] => None,
            [(_, wire)] => {
                let (pin, destination) = wire.split_once("->").ok_or_else(|| entry.error("expected power IRQ wiring"))?;
                let (name, line) = destination.split_once('@').ok_or_else(|| entry.error("expected power EXTI destination"))?;
                if !matches!(pin.trim(), "" | "IRQ") || !exti.is_some_and(|exti| exti.name == name.trim()) {
                    return Err(entry.error("invalid power EXTI destination"));
                }
                Some(irq_index(line, 32).map_err(|error| entry.error(&error))?)
            }
            _ => return Err(entry.error("unsupported power controller property")),
        };
        let mut power = crate::stm32l0_power::Stm32l0Power::new(entry.address()?, line);
        power.name = Some(entry.name.clone());
        machine.bus.map_stm32l0_power(power)
            .map_err(|error| entry.error(&error))?;
    }
    for entry in stm32_rngs {
        let mut irq = None;
        let mut series = false;
        for (_, text) in &entry.body {
            if !series && text.split_once(':').is_some_and(|(key, value)| {
                key.trim() == "series" && matches!(value.trim(), "STM32Series.L0" | "STM32Series.F4" | "STM32Series.H7")
            }) {
                series = true;
                continue;
            }
            let (pin, target, line) = crate::stm32h7_board_platform::wire(text)
                .map_err(|error| entry.error(error))?;
            if irq.is_some() || !matches!(pin, "" | "IRQ")
                || !nvic.is_some_and(|controller| controller.name == target)
            {
                return Err(entry.error("expected at most one STM32 RNG IRQ connected to NVIC"));
            }
            irq = Some(irq_index(line, 240).map_err(|error| entry.error(&error))?);
        }
        if !series {
            return Err(entry.error("expected STM32Series.L0, F4 or H7 RNG"));
        }
        machine.bus.map_h7_rng(crate::stm32h7_rng::Rng::new(entry.address()?, false, irq))
            .map_err(|error| entry.error(&error))?;
    }
    for entry in &low_power_timers {
        let mut frequency = None;
        let mut irq = None;
        for (_, text) in &entry.body {
            if let Some(value) = text.strip_prefix("frequency:").filter(|_| frequency.is_none()) {
                frequency = Some(number(value.trim()).map_err(|error| entry.error(&error))?);
                continue;
            }
            let (pin, target, line) = crate::stm32h7_board_platform::wire(text)
                .map_err(|error| entry.error(error))?;
            if irq.is_some() || !matches!(pin, "" | "IRQ") {
                return Err(entry.error("expected at most one LPTIM IRQ"));
            }
            irq = Some(if exti.is_some_and(|exti| exti.name == target) {
                crate::stm32_uart::Stm32Irq::Exti(irq_index(line, 32).map_err(|error| entry.error(&error))?)
            } else if nvic.is_some_and(|nvic| nvic.name == target) {
                crate::stm32_uart::Stm32Irq::Nvic(irq_index(line, 240).map_err(|error| entry.error(&error))?)
            } else {
                return Err(entry.error("expected LPTIM IRQ connected to EXTI or NVIC"));
            });
        }
        let frequency = frequency.filter(|value| *value > 0)
            .ok_or_else(|| entry.error("expected nonzero LPTIM frequency"))?;
        machine.bus.map_h7_lptim(crate::stm32h7_lptim::LpTimer::new(entry.address()?, None, frequency, irq))
            .map_err(|error| entry.error(&error))?;
    }
    if let Some(entry) = stm32l0_rcc {
        let mut systick = false;
        let mut lptimer = None;
        let mut hse_frequency = 16_000_000;
        for (key, value) in keys(entry, &["systick", "rtc", "lptimer", "hseFrequency"])? {
            match key {
                "systick" => {
                    if !nvic.is_some_and(|controller| controller.name == value) {
                        return Err(entry.error("unknown RCC SysTick target"));
                    }
                    systick = true;
                }
                "lptimer" => {
                    let timer = low_power_timers.iter().find(|timer| timer.name == value)
                        .ok_or_else(|| entry.error("unknown RCC LPTIM target"))?;
                    lptimer = Some(timer.address()?);
                }
                "hseFrequency" => {
                    hse_frequency = number(value).map_err(|error| entry.error(&error))?;
                    if hse_frequency == 0 {
                        return Err(entry.error("invalid HSE frequency"));
                    }
                }
                _ if !entries.iter().any(|other| other.name == value) => return Err(entry.error("unknown RCC clock target")),
                _ => {}
            }
        }
        machine.bus.map_stm32l0_rcc(crate::stm32l0_rcc::Stm32l0Rcc::new(
            entry.address()?, systick, hse_frequency, lptimer,
        )).map_err(|error| entry.error(&error))?;
    }
    if let Some(entry) = stm32l0_flash {
        let registrations = entry.registration_list().ok_or_else(|| entry.error("missing flash controller registrations"))?;
        let mut registers = None;
        let mut signature = None;
        for registration in registrations {
            if let Some((base, size, region)) = multi_registration(entry, registration)? {
                if region != "signature" || signature.replace((base, size)).is_some() {
                    return Err(entry.error("invalid flash signature registration"));
                }
            } else if registers.replace(entry.registration_address(registration)?).is_some() {
                return Err(entry.error("duplicate flash control registers"));
            }
        }
        let properties = keys(entry, &["flash", "eeprom"])?;
        let memory = |key| properties.iter().find_map(|&(name, value)| (name == key).then_some(value))
            .and_then(|name| memory_bases.get(name)).copied()
            .ok_or_else(|| entry.error("expected mapped flash and EEPROM backing"));
        machine.bus.map_stm32l0_flash(crate::stm32l0_flash::Stm32l0Flash::new(
            registers.ok_or_else(|| entry.error("missing flash registers"))?,
            signature.ok_or_else(|| entry.error("missing flash signature"))?,
            memory("flash")?, memory("eeprom")?,
        )).map_err(|error| entry.error(&error))?;
    }
    let irq_lines = if extended_nvic { 48 } else { 32 };
    for entry in stm32_regular_adcs {
        let irq = match entry.body.as_slice() {
            [] => None,
            [(_, wire)] => {
                let (pin, destination) = wire.split_once("->").ok_or_else(|| entry.error("expected ADC IRQ wiring"))?;
                let (name, line) = destination.split_once('@').ok_or_else(|| entry.error("expected ADC NVIC destination"))?;
                if !matches!(pin.trim(), "" | "IRQ") || !nvic.is_some_and(|nvic| nvic.name == name.trim()) {
                    return Err(entry.error("invalid ADC NVIC destination"));
                }
                Some(irq_index(line, irq_lines).map_err(|error| entry.error(&error))?)
            }
            _ => return Err(entry.error("unsupported ADC property or wiring")),
        };
        machine.bus.map_stm32_adc(crate::stm32_adc::Stm32Adc::new(entry.address()?, entry.name.clone(), irq))
            .map_err(|error| entry.error(&error))?;
    }
    if let Some(rcc) = stm32f4_rcc {
        let options = keys(rcc, &["rtcPeripheral"])?;
        if !options
            .iter()
            .any(|(_, target)| stm32_rtc.is_some_and(|rtc| rtc.name == *target))
        {
            return Err(rcc.error("STM32F4 RCC requires an attached STM32 RTC"));
        }
        machine
            .bus
            .map_stm32f4_rcc(rcc.address()?)
            .map_err(|e| rcc.error(&e))?;
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
    if let [entry] = entries.as_slice()
        && entry.model == "UART.TrivialUart"
    {
        keys(entry, &[])?;
        machine.bus.map_trivial_uart(entry.address()?, &entry.name).map_err(|e| entry.error(&e))?;
        machine.cpu_less = true;
        // The master clock's unit is 10 ns even with no CPU attached.
        machine.bus.set_step_hz(crate::bus::CORTEX_M_STEP_HZ as u32);
        return Ok(machine);
    }
    if cpus.is_empty() && !stm32_uarts.is_empty() && entries.len() == stm32_uarts.len() {
        for uart in stm32_uarts {
            let mut frequency = None;
            let mut low_power = false;
            for (key, value) in keys(uart, &["frequency", "lowPowerMode"])? {
                if key == "frequency" {
                    frequency = Some(number(value).map_err(|error| uart.error(&error))?);
                } else {
                    low_power = value.parse().map_err(|_| uart.error("invalid USART lowPowerMode"))?;
                }
            }
            if frequency == Some(0) {
                return Err(uart.error("USART frequency must be positive"));
            }
            let layout = crate::stm32_uart::UsartLayout::F7 { low_power };
            let (base, size) = sysbus_range(uart, layout.size())?;
            let legacy = uart.model == "UART.STM32_UART";
            if size < if legacy { 0x1c } else { 0x30 } {
                return Err(uart.error("registered range is too small for STM32 USART registers"));
            }
            let mut device = Stm32Usart::with_layout(uart.name.clone(), base, None, layout);
            device.legacy_layout = legacy;
            device.mapped_size = Some(size);
            device.frequency = frequency;
            machine.bus.map_stm32_usart(device).map_err(|error| uart.error(&error))?;
        }
        machine.cpu_less = true;
        machine.bus.set_step_hz(crate::bus::CORTEX_M_STEP_HZ as u32);
        return Ok(machine);
    }
    if cpus.is_empty() && !stm32_i2cs.is_empty()
        && entries.len() == stm32_i2cs.len() + i2c_sensors.len()
    {
        let mut sensors = group_stm32_i2c_sensors(i2c_sensors, &stm32_i2cs)?;
        for i2c in stm32_i2cs {
            keys(i2c, &[])?;
            let mut device = Stm32I2c::new(i2c.address()?);
            device.name = Some(i2c.name.clone());
            for (sensor, address) in sensors.remove(i2c.name.as_str()).unwrap_or_default() {
                device.attach_sensor(address, sensor.name.clone(), &sensor.model)
                    .map_err(|error| sensor.error(&error))?;
            }
            machine.bus.map_stm32_i2c(device).map_err(|error| i2c.error(&error))?;
        }
        machine.cpu_less = true;
        machine.bus.set_step_hz(crate::bus::CORTEX_M_STEP_HZ as u32);
        return Ok(machine);
    }
    if cpus.is_empty() || (require_ram && !ram) {
        return Err(format!(
            "{}: platform requires at least one CPU{}",
            path.display(),
            if require_ram { " and mapped RAM" } else { "" }
        ));
    }
    for (name, (entry, _)) in &external_pmps {
        if !cpus
            .iter()
            .any(|cpu| cpu.name == *name && cpu.model == "CPU.RiscV32")
        {
            return Err(entry.error("external PMP requires an attached CPU.RiscV32"));
        }
    }
    if let Some(entry) = cpus.iter().find(|entry| entry.model == "CPU.CortexM") {
        if cpus.len() != 1
            || clint.is_some()
            || !plic.is_empty()
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
        let mut cpu_wait = false;
        let mut trustzone = false;
        for (key, value) in keys(
            entry,
            &[
                "cpuType",
                "vectorTableOffset",
                "InitVectorTableOffset",
                "isCpuWaitSignalSet",
                "nvic",
                "enableTrustZone",
            ],
        )? {
            match key {
                "cpuType" => cpu_type = Some(value),
                "enableTrustZone" => {
                    trustzone = value.parse().map_err(|_| entry.error("invalid enableTrustZone"))?;
                }
                "isCpuWaitSignalSet" => {
                    cpu_wait = value
                        .parse()
                        .map_err(|_| entry.error("invalid isCpuWaitSignalSet"))?;
                }
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
        let cortex_m55 = cpu_type == Some(r#""cortex-m55""#);
        let cortex_m4f = matches!(cpu_type, Some(r#""cortex-m4""# | r#""cortex-m4f""#)) || cortex_m55;
        if !matches!(cpu_type, Some(r#""cortex-m0""# | r#""cortex-m0+""#)) && !cortex_m4f {
            return Err(entry.error("expected Cortex-M0, Cortex-M0+, Cortex-M4F or Cortex-M55 cpuType"));
        }
        if trustzone && !cortex_m55 {
            return Err(entry.error("TrustZone requires a Cortex-M55 CPU"));
        }
        if !cortex_m4f {
            machine.bus.map_stm32f072_flash().map_err(|error| entry.error(&error))?;
        }
        machine.bus.map_systick().map_err(|e| entry.error(&e))?;
        let mut priority = None;
        let mut frequency = None;
        if let Some(controller) = nvic {
            let base = if trustzone && let Some(registrations) = controller.registration_list() {
                let [secure, nonsecure] = registrations.as_slice() else {
                    return Err(controller.error("TrustZone NVIC expects Secure and Non-secure registrations"));
                };
                let alias = nonsecure.strip_prefix("sysbus new Bus.BusMultiRegistration")
                    .and_then(|fields| fields.trim().strip_prefix('{'))
                    .and_then(|fields| fields.strip_suffix('}'))
                    .ok_or_else(|| controller.error("expected a Non-secure NVIC registration"))?;
                let mut fields = alias.split(';').map(str::trim);
                let valid = fields.next().and_then(|f| f.strip_prefix("address:"))
                    .and_then(|address| number(address.trim()).ok()) == Some(0xe002_e000)
                    && fields.next().and_then(|f| f.strip_prefix("size:"))
                        .and_then(|size| number(size.trim()).ok()) == Some(0x1000)
                    && fields.next().and_then(|f| f.strip_prefix("region:"))
                        .is_some_and(|region| region.trim() == r#""NonSecure""#)
                    && fields.next().is_none();
                if !valid {
                    return Err(controller.error("invalid Non-secure NVIC alias"));
                }
                controller.registration_address(secure)?
            } else {
                controller.address()?
            };
            if base != crate::nvic::BASE {
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
        machine.bus.refresh_stm32l0_clock();
        let mut uart_irq = None;
        if let Some(uart) = uart.filter(|uart| uart.model == "UART.TrivialUart") {
            keys(uart, &[])?;
            machine.bus.map_trivial_uart(uart.address()?, &uart.name)
                .map_err(|error| uart.error(&error))?;
        } else if let Some(uart) = uart {
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
        let mut gpio_ports = HashMap::new();
        let mut led_inputs = HashMap::new();
        let mut pwm_inputs = HashMap::new();
        let mut flash_selects = HashMap::new();
        for &entry in &stm32_gpios {
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
                    } else if let Some(receiver) =
                        stm32_gpios.iter().find(|gpio| gpio.name == target)
                    {
                        let (base, _) = sysbus_range(receiver, 0x400)?;
                        let inputs = irq_list(inputs, 16).map_err(|e| entry.error(&e))?;
                        if inputs.len() != outputs.len() {
                            return Err(entry.error("mismatched GPIO link widths"));
                        }
                        for (&output, input) in outputs.iter().zip(inputs) {
                            machine.bus.route_gpio_pin(port.base, output, base, input);
                        }
                    } else if let Some(fram) = i2c_frams.get(target) {
                        let [pin] = outputs.as_slice() else {
                            return Err(entry.error("expected one FRAM write-protect source"));
                        };
                        if irq_index(inputs, 1).is_err() {
                            return Err(entry.error("FRAM write protection requires GPIO input zero"));
                        }
                        machine.bus.wire_fram_write_protect(port.base, *pin, Rc::clone(fram));
                    } else if stm32_spi_flashes.iter().any(|flash| flash.name == target) {
                        let [pin] = outputs.as_slice() else {
                            return Err(entry.error("expected one SPI flash chip-select source"));
                        };
                        if irq_index(inputs, 1).is_err() || flash_selects.insert(target, (port.base, *pin)).is_some() {
                            return Err(entry.error("invalid or duplicate SPI flash chip-select wiring"));
                        }
                    } else if gpio_pwm_testers.iter().any(|tester| tester.name == target) {
                        let [pin] = outputs.as_slice() else {
                            return Err(entry.error("expected one PWM tester output source"));
                        };
                        if irq_index(inputs, 1).is_err()
                            || pwm_inputs.insert(target, (entry.name.as_str(), port.base, *pin)).is_some()
                        {
                            return Err(entry.error("invalid or duplicate PWM tester wiring"));
                        }
                    } else if gpio_leds.iter().any(|led| led.name == target) {
                        if inputs.trim() != "0"
                            || outputs.len() != 1
                            || led_inputs
                                .insert(target, (entry.name.as_str(), port.base, outputs[0]))
                                .is_some()
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
        for tester in gpio_pwm_testers {
            keys(tester, &[])?;
            let (parent, _) = tester.registration.as_deref()
                .and_then(|registration| registration.split_once(char::is_whitespace))
                .ok_or_else(|| tester.error("expected PWM tester attached to a GPIO port"))?;
            let (source, port, pin) = pwm_inputs.remove(tester.name.as_str())
                .ok_or_else(|| tester.error("missing PWM tester input wiring"))?;
            if parent != source {
                return Err(tester.error("PWM tester input source differs from its parent"));
            }
            machine.bus.map_pwm_tester(format!("{parent}.{}", tester.name), port, pin);
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
            if let Some((parent, port, pin)) = led_inputs.remove(led.name.as_str()) {
                let mut device = GpioLed::new(led.name.clone(), pin, invert);
                device.parent = Some(parent.to_owned());
                machine.bus.add_gpio_led(port, device);
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
        machine.bus.refresh_gpio_wiring();
        for button in gpio_buttons {
            let mut invert = None;
            let mut endpoints = GpioEndpoints::default();
            for (_, text) in &button.body {
                if let Some((source, destination)) = text.split_once("->")
                    && matches!(source.trim(), "" | "0" | "IRQ")
                {
                    for target in destination.split('|') {
                        let (port, pin) = target
                            .split_once('@')
                            .ok_or_else(|| button.error("expected button target port@pin"))?;
                        let port = port.trim();
                        let (base, _) = gpio_ports
                            .get(port)
                            .ok_or_else(|| button.error("unknown button GPIO port"))?;
                        let pin = irq_index(pin, 256).map_err(|e| button.error(&e))?;
                        endpoints
                            .connect(*base, port, pin, 16)
                            .map_err(|e| button.error(&e))?;
                    }
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
                    return Err(button.error(
                        "unsupported button wiring or invalid or duplicate button property",
                    ));
                }
            }
            machine.bus.add_gpio_button(GpioButton::new(
                button.name.clone(),
                button
                    .registration
                    .as_deref()
                    .and_then(|registration| registration.split_whitespace().next())
                    .map(str::to_owned),
                endpoints,
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
                        Stm32Irq::Nvic(irq_index(pin, irq_lines).map_err(|e| rtc.error(&e))?)
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
            let mut device = Stm32Dma::new(dma.address()?, outputs);
            if stm32l0_rcc.is_some() {
                device.start_tx_on_enable();
            }
            machine
                .bus
                .map_stm32_dma(device)
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
                .map_stm32f0_adc(adc.name.clone(), device)
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
            if !stm32_i2cs.iter().any(|i2c| i2c.name == target) {
                return Err(eeprom.error("unknown EEPROM I²C controller"));
            }
            let address = number(address.trim()).map_err(|e| eeprom.error(&e))?;
            eeproms.entry(target).or_default().push((eeprom, address));
        }
        let mut fram_relays = HashMap::<_, Vec<_>>::new();
        for relay in i2c_fram_relays {
            let (controller, address) = relay.registration.as_deref()
                .and_then(|registration| registration.split_once(char::is_whitespace))
                .filter(|(name, _)| stm32_i2cs.iter().any(|i2c| i2c.name == *name))
                .ok_or_else(|| relay.error("expected STM32 I²C controller and address"))?;
            let address = number(address.trim()).map_err(|error| relay.error(&error))?;
            let options = keys(relay, &["mb85rc1mt"])?;
            let [(_, parent)] = options.as_slice() else {
                return Err(relay.error("expected MB85RC1MT backing device"));
            };
            let fram = i2c_frams.get(parent).ok_or_else(|| relay.error("unknown MB85RC1MT backing device"))?;
            fram_relays.entry(controller).or_default().push((relay, address, Rc::clone(fram)));
        }
        let mut sensors = group_stm32_i2c_sensors(i2c_sensors, &stm32_i2cs)?;
        for i2c in stm32_i2cs {
            let mut device = Stm32I2c::new(i2c.address()?);
            device.name = Some(i2c.name.clone());
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
                                crate::stm32_uart::Stm32Irq::Nvic(irq_index(pin, 32).map_err(|e| i2c.error(&e))?)
                            } else if exti.is_some_and(|exti| exti.name == target) {
                                crate::stm32_uart::Stm32Irq::Exti(irq_index(pin, exti_lines).map_err(|e| i2c.error(&e))?)
                            } else if let Some(&(count, output)) = combined_routes.get(target) {
                                irq_index(pin, count).map_err(|e| i2c.error(&e))?;
                                crate::stm32_uart::Stm32Irq::Nvic(output)
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
            for (relay, address, fram) in fram_relays.remove(i2c.name.as_str()).unwrap_or_default() {
                device.attach_fram(address, fram, relay.model == "I2C.MB85RC1MTHi")
                    .map_err(|error| relay.error(&error))?;
            }
            for (sensor, address) in sensors.remove(i2c.name.as_str()).unwrap_or_default() {
                device.attach_sensor(address, sensor.name.clone(), &sensor.model).map_err(|error| sensor.error(&error))?;
            }
            machine
                .bus
                .map_stm32_i2c(device)
                .map_err(|e| i2c.error(&e))?;
        }
        let mut flash_targets = HashMap::new();
        for flash in stm32_spi_flashes {
            let controller = flash.registration.as_deref().filter(|name| stm32_spis.iter().any(|spi| spi.name == *name))
                .ok_or_else(|| flash.error("expected STM32 SPI controller"))?;
            let select = flash_selects.remove(flash.name.as_str())
                .ok_or_else(|| flash.error("missing SPI flash chip-select wiring"))?;
            let options = keys(flash, &["underlyingMemory"])?;
            let [(_, backing)] = options.as_slice() else {
                return Err(flash.error("expected SPI flash backing memory"));
            };
            let memory = entries.iter().find(|entry| entry.name == *backing
                && entry.model == "Memory.MappedMemory" && entry.registration.is_none())
                .ok_or_else(|| flash.error("expected unregistered MappedMemory for SPI flash"))?;
            let options = keys(memory, &["size"])?;
            let [(_, size)] = options.as_slice() else {
                return Err(memory.error("missing SPI flash memory size"));
            };
            let size = number(size).map_err(|error| memory.error(&error))? as usize;
            if flash_targets.insert(controller, crate::stm32_spi::SpiFlash::new(select, size)
                .map_err(|error| flash.error(&error))?).is_some()
            {
                return Err(flash.error("only one flash per STM32 SPI controller is supported"));
            }
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
            if !matches!(series, Some("STM32Series.F0" | "STM32Series.L0")) || capacity == Some(0) {
                return Err(spi.error("expected STM32Series.F0 or L0 SPI with positive bufferCapacity"));
            }
            let mut device = Stm32F0Spi::new(spi.address()?, capacity.unwrap_or(4));
            device.irq = irq;
            device.shift_register = shift_targets
                .remove(spi.name.as_str())
                .map(|(_, shift)| shift);
            if let Some(flash) = flash_targets.remove(spi.name.as_str()) {
                if device.shift_register.is_some() {
                    return Err(spi.error("only one target per STM32 SPI controller is supported"));
                }
                device.flash = Some(flash);
            }
            device.dma_rx = dma_rx;
            device.dma_tx = dma_tx;
            machine
                .bus
                .map_stm32f0_spi(device)
                .map_err(|e| spi.error(&e))?;
        }
        machine.bus.refresh_gpio_wiring();
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
                        "initialLimit" if limit.is_none() && value != 0 => limit = Some(value),
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
            device.name = Some(timer.name.clone());
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
                            Stm32Irq::Nvic(irq_index(line, irq_lines).map_err(|e| uart.error(&e))?)
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
            let mut device = Stm32Usart::new(
                uart.name.clone(),
                sysbus_range(
                    uart,
                    crate::stm32_uart::UsartLayout::F7 { low_power: false }.size(),
                )?
                .0,
                irq,
                low_power.unwrap_or(false),
            );
            device.legacy_layout = uart.model == "UART.STM32_UART";
            device.frequency = frequency;
            device.dma_rx = dma_rx;
            device.dma_tx = dma_tx;
            machine
                .bus
                .map_stm32_usart(device)
                .map_err(|e| uart.error(&e))?;
        }
        machine
            .bus
            .map_nrf_nvic(priority.unwrap_or(if cortex_m4f { 0xe0 } else { 0xc0 }), uart_irq, cortex_m4f)
            .map_err(|e| entry.error(&e))?;
        machine
            .bus
            .nvic
            .as_mut()
            .unwrap()
            .set_external_lines(irq_lines as usize);
        if let Some(controller) = nvic {
            machine.bus.set_nvic_name(controller.name.clone());
        }
        machine.primary.name = Some(entry.name.clone());
        machine.primary.configure_cortex_m(vectors, cortex_m4f);
        if cortex_m55 {
            machine.primary.mark_cortex_m55(trustzone)?;
            if trustzone {
                machine.bus.nvic.as_mut().unwrap().enable_trustzone();
            }
        }
        machine.set_cortex_m_cpu_wait(&entry.name, cpu_wait)?;
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
        let ibex = entry.model == "CPU.IbexRiscV32";
        let mut isa = match entry.model.as_str() {
            "CPU.IbexRiscV32" => Some((true, false, true, false)),
            "CPU.Minerva" => Some((false, false, false, false)),
            "CPU.VexRiscv" => Some((true, false, false, false)),
            _ => None,
        };
        let mut compressed_float = (true, true);
        let mut floating = (false, false, false, false, false);
        let mut zfa = false;
        let mut zfa_fli = false;
        let mut zfa_min_max = false;
        let mut zfa_quiet_compare = false;
        let mut zfa_round = false;
        let mut zicond = false;
        let mut zacas = false;
        let mut zmmul = false;
        let mut zabha = false;
        let mut zaamo = false;
        let mut zalrsc = false;
        let mut smepmp = false;
        let mut sscofpmf = false;
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
        let (properties, _) = split_rv32_init(entry);
        let cpu_properties = properties.iter().position(|(_, line)| line == "reset:")
            .map_or(properties, |index| &properties[..index]);
        if cpu_properties.len() != properties.len() {
            let reset = egis_syscon.ok_or_else(|| entry.error("unsupported CPU reset hook"))?;
            let expected = format!("PC `{} ResetVector`", reset.name);
            if !matches!(&properties[cpu_properties.len() + 1..], [(_, hook)] if hook == &expected) {
                return Err(entry.error("unsupported CPU reset hook"));
            }
        }
        for (key, value) in keys_in(
            entry,
            cpu_properties,
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
                "zfaFli",
                "zfaMinMax",
                "zfaQuietCompare",
                "zfaRound",
                "interruptMode",
            ],
        )? {
            match (key, value) {
                ("cpuType", value) => {
                    let architecture = value.trim_matches('"').to_ascii_lowercase();
                    let mut base = architecture.as_str();
                    zfa = false;
                    zicond = false;
                    zacas = false;
                    zmmul = false;
                    zabha = false;
                    zaamo = false;
                    zalrsc = false;
                    smepmp = false;
                    sscofpmf = false;
                    let mut zca = false;
                    let mut zcf = false;
                    let mut zcd = false;
                    let mut zfh = false;
                    let mut zfhmin = false;
                    let mut zfbfmin = false;
                    bitmanip = BitManip::default();
                    while let Some((prefix, extension)) = base.rsplit_once('_') {
                        match extension {
                            "zicsr" | "zifencei" if !ibex => {}
                            "zihintpause" => {}
                            "zicond" if !zicond => zicond = true,
                            "zmmul" if !zmmul => zmmul = true,
                            "zaamo" if !zaamo => zaamo = true,
                            "zalrsc" if !zalrsc => zalrsc = true,
                            "zabha" if !zabha => zabha = true,
                            "zacas" if !zacas => zacas = true,
                            "sscofpmf" if !sscofpmf => sscofpmf = true,
                            "smepmp" if !smepmp => smepmp = true,
                            "zfbfmin" if !zfbfmin => zfbfmin = true,
                            "zfa" if !zfa => zfa = true,
                            "zfhmin" if !zfhmin => zfhmin = true,
                            "zfh" if !zfh => zfh = true,
                            "zba" if !bitmanip.zba => bitmanip.zba = true,
                            "zbb" if !bitmanip.zbb => bitmanip.zbb = true,
                            "zksed" if !bitmanip.zksed => bitmanip.zksed = true,
                            "zksh" if !bitmanip.zksh => bitmanip.zksh = true,
                            "zkne" if !bitmanip.zkne => bitmanip.zkne = true,
                            "zknd" if !bitmanip.zknd => bitmanip.zknd = true,
                            "zknh" if !bitmanip.zknh => bitmanip.zknh = true,
                            "zbkc" if !bitmanip.zbkc => bitmanip.zbkc = true,
                            "zbc" if !bitmanip.zbc => bitmanip.zbc = true,
                            "xandesvsintload" if !bitmanip.xandesvsintload => bitmanip.xandesvsintload = true,
                            "xandesvbfhcvt" if !bitmanip.xandesvbfhcvt => bitmanip.xandesvbfhcvt = true,
                            "xandesvqmac" if !bitmanip.xandesvqmac => bitmanip.xandesvqmac = true,
                            "xandesvdot" if !bitmanip.xandesvdot => bitmanip.xandesvdot = true,
                            "xandesvpackfph" if !bitmanip.xandesvpackfph => bitmanip.xandesvpackfph = true,
                            "xandesvsinth" if !bitmanip.xandesvsinth => bitmanip.xandesvsinth = true,
                            "v" if !bitmanip.vector => bitmanip.vector = true,
                            "xandesbfhcvt" if !bitmanip.xandesbfhcvt => {
                                bitmanip.xandesbfhcvt = true
                            }
                            "xandes" | "xandesperf" if !bitmanip.xandesperf => {
                                bitmanip.xandesperf = true
                            }
                            "zca" if !zca => zca = true,
                            "zcf" if !zcf => zcf = true,
                            "zcd" if !zcd => zcd = true,
                            "zcb" if !bitmanip.zcb => bitmanip.zcb = true,
                            "zbkx" if !bitmanip.zbkx => bitmanip.zbkx = true,
                            "zbkb" if !bitmanip.zbkb => bitmanip.zbkb = true,
                            "zbs" if !bitmanip.zbs => bitmanip.zbs = true,
                            _ => break,
                        }
                        base = prefix;
                    }
                    let (letters, suffix) = base.split_at(base.find('z').unwrap_or(base.len()));
                    let letters = letters.strip_prefix("rv32").unwrap_or("");
                    let letters = letters.replace('_', "");
                    bitmanip.vector |= letters.contains('v');
                    let general = letters.contains('g');
                    let embedded = letters.contains('e');
                    let single = general || letters.contains('f') || bitmanip.xandesbfhcvt;
                    let double = general || letters.contains('d');
                    // Renode accepts separated C even after a named extension.
                    let compressed =
                        letters.contains('c') || suffix.split('_').any(|part| part == "c");
                    let valid_suffix = suffix.split('_').all(|part| {
                        matches!(part, "" | "c" | "zicsr" | "zifencei" | "zihintpause")
                    });
                    let ibex_legacy = ibex && base == "rv32imcu_zicsr_zifencei";
                    if !ibex_legacy
                        && (!(general || embedded || letters.contains('i'))
                            || !letters.chars().all(|letter| "iemafdcgbv".contains(letter))
                            || (embedded && !letters.chars().all(|letter| "emac".contains(letter)))
                            || !valid_suffix
                            || (double && !single))
                    {
                        return Err(entry.error(&unsupported_rv32_isa(value, &architecture)));
                    }
                    let legacy_compressed = compressed || bitmanip.zcb;
                    if zcf && (!(legacy_compressed || zca) || !single) {
                        return Err(entry.error("Zcf requires RV32 Zca and F"));
                    }
                    if zcd && (!(legacy_compressed || zca) || !double) {
                        return Err(entry.error("Zcd requires RV32 Zca and D"));
                    }
                    compressed_float = (legacy_compressed || zcf, legacy_compressed || zcd);
                    floating = (single, double, zfh, zfbfmin, zfhmin);
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
                        legacy_compressed || zca,
                        embedded,
                    ));
                }
                ("hartId", value) => id = number(value).map_err(|e| entry.error(&e))? as usize,
                ("privilegedArchitecture", "PrivilegedArchitecture.Priv1_10") => {}
                ("privilegedArchitecture", "PrivilegedArchitecture.PrivUnratified")
                | ("timeProvider", "empty") if clic.is_some() => {}
                ("privilegedArchitecture", "PrivilegedArchitecture.Priv1_11")
                | ("timeProvider", "empty")
                    if ibex => {}
                ("timeProvider", name)
                    if clint
                        .or(litex_cpu_timer)
                        .is_some_and(|timer| timer.name == name)
                        || riscv_machine_timer.is_some_and(|timer| timer.name == name) => {}
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
                ("timeProvider", "empty")
                    if matches!(entry.model.as_str(), "CPU.RiscV32" | "CPU.VexRiscv") => {}
                ("zfaFli", "true" | "false") if entry.model == "CPU.RiscV32" => {
                    zfa_fli = value == "true";
                }
                ("zfaMinMax", "true" | "false") if entry.model == "CPU.RiscV32" => {
                    zfa_min_max = value == "true";
                }
                ("zfaQuietCompare", "true" | "false") if entry.model == "CPU.RiscV32" => {
                    zfa_quiet_compare = value == "true";
                }
                ("zfaRound", "true" | "false") if entry.model == "CPU.RiscV32" => {
                    zfa_round = value == "true";
                }
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
                if zfa && !floating.0 {
                    return Err(entry.error("Zfa requires the RV32 F extension"));
                }
                if zfa_fli && !floating.0 {
                    return Err(entry.error("zfaFli requires the RV32 F extension"));
                }
                if zfa_min_max && !floating.0 {
                    return Err(entry.error("zfaMinMax requires the RV32 F extension"));
                }
                if zfa_quiet_compare && !floating.0 {
                    return Err(entry.error("zfaQuietCompare requires the RV32 F extension"));
                }
                if zfa_round && !floating.0 {
                    return Err(entry.error("zfaRound requires the RV32 F extension"));
                }
                if floating.2 && !floating.0 {
                    return Err(entry.error("Zfh requires the RV32 F extension"));
                }
                if floating.4 && !floating.0 {
                    return Err(entry.error("Zfhmin requires the RV32 F extension"));
                }
                if floating.3 && !floating.0 {
                    return Err(entry.error("Zfbfmin requires the RV32 F extension"));
                }
                if smepmp && pmp == 0 {
                    return Err(entry.error("Smepmp requires internal PMP entries"));
                }
                if zacas && !(atomic || zaamo) {
                    return Err(entry.error("Zacas requires RV32 Zaamo or A"));
                }
                if zabha && !(atomic || zaamo) {
                    return Err(entry.error("Zabha requires RV32 Zaamo or A"));
                }
                (
                    (multiply, zmmul),
                    atomic,
                    (compressed, compressed_float),
                    embedded,
                    zicond,
                    (zacas, zabha, zaamo, zalrsc),
                    (smepmp, sscofpmf),
                    bitmanip,
                    (
                        (floating, zfa),
                        (zfa_fli, zfa_min_max, zfa_quiet_compare, zfa_round),
                    ),
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
            (
                (multiply, zmmul),
                atomic,
                (compressed, compressed_float),
                embedded,
                zicond,
                (zacas, zabha, zaamo, zalrsc),
                (smepmp, sscofpmf),
                bitmanip,
                ((floating, zfa), (zfa_fli, zfa_min_max, zfa_quiet_compare, zfa_round)),
            ),
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
        hart.set_integer_extensions(multiply, atomic || (zaamo && zalrsc), compressed);
        hart.set_compressed_float(compressed_float.0, compressed_float.1);
        hart.set_atomic_subextensions(zaamo, zalrsc);
        hart.set_zmmul(zmmul);
        hart.set_embedded_base(embedded);
        hart.set_zicond(zicond);
        hart.set_zacas(zacas);
        hart.set_zabha(zabha);
        hart.set_sscofpmf(sscofpmf);
        hart.set_bitmanip(bitmanip);
        if floating.0 {
            hart.enable_floating(floating.1, floating.2);
            hart.set_zfbfmin(floating.3);
            hart.set_zfhmin(floating.4);
            hart.set_zfa(zfa);
            hart.set_zfa_diagnostics(zfa_fli, zfa_min_max, zfa_quiet_compare, zfa_round);
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
            let reset = if entry.body.iter().any(|(_, line)| line == "reset:") { 0x7000_0000 } else { 0x1000 };
            hart.configure_reset_vector(reset);
            if default_vector_mapped {
                hart.set_mtvec(DEFAULT_MTVEC);
            }
        }
        if let Some((external, count)) = external_pmps.get(entry.name.as_str()) {
            if smepmp {
                return Err(external.error("Smepmp requires internal PMP"));
            }
            hart.attach_external_pmp(format!("{}.{}", entry.name, external.name), *count);
            let message = format!("{}: Enabling External PMP", entry.name);
            crate::monitor_log::record(1, &message);
            eprintln!("penv: {message}");
        } else if pmp != 0 {
            hart.enable_pmp(pmp);
            if smepmp {
                hart.enable_smepmp();
            }
        }
        if bitmanip.has_vector_execution() || split_rv32_init(entry).0.iter().any(|(_, text)| {
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
        machine.bus.guest_cpu_names = configs.iter().map(|config| config.1.name.clone()).collect();
    }
    if let Some(entry) = clic {
        let properties = keys(entry, &["cpu", "machineLevelBits"])?;
        if configs.len() != 1 || !properties.contains(&("cpu", cpu.name.as_str())) {
            return Err(entry.error("CLIC requires one named RV32 CPU"));
        }
        let registrations = entry
            .registration_list()
            .ok_or_else(|| entry.error("missing CLIC registrations"))?;
        let range = |register: &str| {
            let (name, range) = register.split_once(char::is_whitespace)?;
            let (base, size) = range.trim().strip_prefix('<')?.strip_suffix('>')?.split_once(',')?;
            Some((name.to_owned(), number(base.trim()).ok()?,
                  number(size.trim().strip_prefix('+')?).ok()?))
        };
        let regions: Vec<_> = registrations.iter().filter_map(|reg| range(reg)).collect();
        let base = regions.iter().find_map(|(name, base, size)| {
            (name == "sysbus" && *size == crate::clic::Clic::SIZE).then_some(*base)
        }).ok_or_else(|| entry.error("unsupported CLIC system bus range"))?;
        if registrations.len() != 2 || !regions.contains(&(cpu.name.clone(), 0x1000, 0x4a1)) {
            return Err(entry.error("unsupported CLIC indirect CSR range"));
        }
        let machine_levels = properties.iter()
            .find_map(|(key, value)| (*key == "machineLevelBits").then_some(*value))
            .map(number).transpose().map_err(|e| entry.error(&e))?.unwrap_or(8);
        if machine_levels > 8 {
            return Err(entry.error("CLIC machineLevelBits must be between 0 and 8"));
        }
        machine.bus.map_clic(base, &entry.name, machine_levels as u8)
            .map_err(|e| entry.error(&e))?;
        machine.primary.enable_clic();
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
                clic.unwrap_or(target).name
            );
            if !wires.contains(&expected) {
                return Err(entry.error(&format!("expected CLINT connection {expected}")));
            }
        }
        let address = if let Some(registration) = entry.registration_list()
            .and_then(|items| match items.as_slice() {
                [item] => item.strip_prefix("sysbus new Bus.BusPointRegistration"),
                _ => None,
            })
        {
            let body = registration.trim().strip_prefix('{').and_then(|text| text.strip_suffix('}'))
                .ok_or_else(|| entry.error("expected a CLINT bus point registration body"))?;
            let fields = object_fields(body).map_err(|error| entry.error(error))?;
            let mut address = None;
            let mut registration_cpu = None;
            for (key, value) in fields {
                match key {
                    "address" if address.is_none() => address = Some(number(value).map_err(|error| entry.error(&error))?),
                    "cpu" if registration_cpu.is_none() => registration_cpu = Some(value),
                    _ => return Err(entry.error("unsupported or duplicate CLINT bus point field")),
                }
            }
            if configs.len() != 1 || registration_cpu != Some(configs[0].1.name.as_str()) {
                return Err(entry.error("expected a CLINT bus point scoped to its single attached CPU"));
            }
            address.ok_or_else(|| entry.error("missing CLINT bus point address"))?
        } else {
            entry.address()?
        };
        // VexRiscv's custom CLINT wiring uses the established one-tick-per-step
        // time base used by the LiteX Linux demos.
        if let Some(frequency) = frequency.filter(|_| !uses_vex_irq(cpu)) {
            machine
                .bus
                .map_clint_with_frequency(address, targets, frequency)
        } else {
            machine.bus.map_clint(address, targets)
        }
        .map_err(|e| entry.error(&e))?;
        if wires.is_empty() {
            machine.bus.clint.as_mut().unwrap().disconnect_interrupts();
        }
    }
    if let Some(entry) = riscv_machine_timer {
        if configs.len() != 1 || cortex_m {
            return Err(entry.error("RISC-V machine timer requires a single RISC-V CPU"));
        }
        let mut frequency = None;
        let mut connected = false;
        let expected = format!("->{}@7", cpu.name);
        for (_, text) in &entry.body {
            if let Some(value) = text.strip_prefix("frequency:") && frequency.is_none() {
                frequency = Some(number(value.trim()).map_err(|error| entry.error(&error))?);
                continue;
            }
            let wire: String = text.chars().filter(|character| !character.is_whitespace()).collect();
            if connected || wire.strip_prefix("IRQ").unwrap_or(&wire) != expected {
                return Err(entry.error(&format!("expected machine timer wiring {expected}")));
            }
            connected = true;
        }
        let frequency = frequency.filter(|frequency| *frequency != 0)
            .ok_or_else(|| entry.error("machine timer requires a positive frequency"))?;
        if !connected {
            return Err(entry.error(&format!("expected machine timer wiring {expected}")));
        }
        let time_provider = cpu.body.iter().any(|(_, row)| {
            row.strip_prefix("timeProvider:").is_some_and(|name| name.trim() == entry.name)
        });
        machine.bus.map_riscv_machine_timer(&entry.name, crate::riscv_machine_timer::RiscvMachineTimer::new(
            entry.address()?, 0, time_provider, frequency,
        )).map_err(|error| entry.error(&error))?;
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
    let mut plic_config = None;
    for entry in plic {
        let andes = entry.model == "IRQControllers.AndesNCEPLIC100";
        let mut outputs = vec![0; if andes { 32 } else { 2 }];
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
                    if !(matches!(line, 9 | 11) || andes && line == 3)
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
            .filter(|count| (1..=outputs.len() as u32).contains(count))
            .ok_or_else(|| entry.error("invalid PLIC context count"))?
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
        if andes {
            let (base, size) = sysbus_range(entry, crate::plic::Plic::SIZE as u32)?;
            machine.bus.map_andes_plic(&entry.name, base, size, sources, &outputs[..count], priorities.unwrap_or(true))
        } else {
            machine.bus.map_plic(entry.address()?, sources, &outputs[..count], priorities.unwrap_or(true))
        }.map_err(|e| entry.error(&e))?;
        plic_config.get_or_insert((entry, sources));
    }
    let mut flash_sizes = HashMap::new();
    for flash in puya_flashes {
        let parent = flash.registration.as_deref().ok_or_else(|| flash.error("expected SPI parent"))?;
        if !andes_spis.iter().any(|spi| spi.name == parent) {
            return Err(flash.error("Puya flash requires an Andes SPI controller"));
        }
        let properties = keys(flash, &["underlyingMemory"])?;
        let [(_, memory_name)] = properties.as_slice() else {
            return Err(flash.error("expected Puya flash backing memory"));
        };
        let memory = entries.iter().find(|entry| entry.name == *memory_name
            && entry.model == "Memory.MappedMemory" && entry.registration.is_none())
            .ok_or_else(|| flash.error("expected unregistered mapped memory for Puya flash"))?;
        let properties = keys(memory, &["size"])?;
        let [(_, size)] = properties.as_slice() else {
            return Err(memory.error("missing Puya backing size"));
        };
        let size = number(size).map_err(|error| memory.error(&error))?;
        if !(1..=0x100_0000).contains(&size) || flash_sizes.insert(parent, size).is_some() {
            return Err(flash.error("invalid or duplicate Puya flash"));
        }
    }
    for entry in andes_rtcs {
        let mut irq = None;
        for (_, wire) in &entry.body {
            let (pin, destination) = wire.split_once("->").ok_or_else(|| entry.error("expected RTC wiring"))?;
            let (target, source) = destination.split_once('@').ok_or_else(|| entry.error("expected RTC PLIC destination"))?;
            let source = number(source.trim()).map_err(|error| entry.error(&error))?;
            let (_, sources) = plic_config.filter(|(plic, _)| plic.name == target.trim())
                .ok_or_else(|| entry.error("unknown RTC PLIC destination"))?;
            if irq.is_some() || !matches!(pin.trim(), "" | "IRQ") || !(1..=sources).contains(&source) {
                return Err(entry.error("invalid RTC PLIC source"));
            }
            irq = machine.bus.plic_index(target.trim()).map(|index| (index, source));
        }
        machine.bus.map_andes_rtc(crate::andes_rtc::AndesRtc::new(
            entry.address()?, irq,
        )).map_err(|error| entry.error(&error))?;
    }
    for entry in andes_spis {
        let mut irq = None;
        for (_, wire) in &entry.body {
            let (pin, destination) = wire.split_once("->").ok_or_else(|| entry.error("expected SPI wiring"))?;
            let (target, source) = destination.split_once('@').ok_or_else(|| entry.error("expected SPI PLIC destination"))?;
            let source = number(source.trim()).map_err(|error| entry.error(&error))?;
            let (_, sources) = plic_config.filter(|(plic, _)| plic.name == target.trim())
                .ok_or_else(|| entry.error("unknown SPI PLIC destination"))?;
            if irq.is_some() || !matches!(pin.trim(), "" | "IRQ") || !(1..=sources).contains(&source) {
                return Err(entry.error("invalid SPI PLIC source"));
            }
            irq = machine.bus.plic_index(target.trim()).map(|index| (index, source));
        }
        let size = flash_sizes.get(entry.name.as_str()).map(|&size| size as usize);
        let mut attached = false;
        spi_loopbacks.retain(|child| {
            if !attached && child.registration.as_deref() == Some(entry.name.as_str()) {
                attached = true;
                false
            } else {
                true
            }
        });
        if attached && size.is_some() {
            return Err(entry.error("Andes SPI supports one attached peripheral"));
        }
        let spi = crate::andes_spi::AndesSpi::new(entry.address()?, entry.name.clone(), irq, size);
        let spi = if attached { spi.with_loopback() } else { spi };
        machine.bus.map_andes_spi(spi).map_err(|error| entry.error(&error))?;
    }
    if let Some(entry) = spi_loopbacks.first() {
        return Err(entry.error("expected attachment to a compatible SPI controller"));
    }
    for entry in andes_watchdogs {
        let mut frequency = None;
        let mut irq = None;
        for (_, row) in &entry.body {
            if let Some(value) = row.strip_prefix("clockFrequency:") && frequency.is_none() {
                frequency = Some(number(value.trim()).map_err(|error| entry.error(&error))?);
                continue;
            }
            let (pin, destination) = row.split_once("->").ok_or_else(|| entry.error("expected watchdog wiring"))?;
            let (target, source) = destination.split_once('@').ok_or_else(|| entry.error("expected watchdog PLIC destination"))?;
            let source = number(source.trim()).map_err(|error| entry.error(&error))?;
            let (_, sources) = plic_config.filter(|(plic, _)| plic.name == target.trim())
                .ok_or_else(|| entry.error("unknown watchdog PLIC destination"))?;
            if irq.is_some() || !matches!(pin.trim(), "" | "IRQ") || !(1..=sources).contains(&source) {
                return Err(entry.error("invalid watchdog PLIC source"));
            }
            irq = machine.bus.plic_index(target.trim()).map(|index| (index, source));
        }
        let frequency = frequency.filter(|frequency| *frequency != 0)
            .ok_or_else(|| entry.error("watchdog requires a positive clock frequency"))?;
        machine.bus.map_andes_watchdog(crate::andes_watchdog::AndesWatchdog::new(
            entry.address()?, entry.name.clone(), frequency, irq,
        )).map_err(|error| entry.error(&error))?;
    }
    let mut gpio_indices = HashMap::new();
    let mut gpio_routes = Vec::new();
    for entry in andes_gpios {
        let mut irq = None;
        let overlays = entries.iter().filter(|other| other.name == entry.name && other.model.is_empty());
        for declaration in std::iter::once(entry).chain(overlays) {
            for (_, wire) in &declaration.body {
                let (pin, destination) = wire.split_once("->").ok_or_else(|| declaration.error("expected GPIO wiring"))?;
                let (target, index) = destination.split_once('@').ok_or_else(|| declaration.error("expected GPIO wire destination"))?;
                let pin = pin.trim();
                let target = target.trim();
                if matches!(pin, "" | "IRQ") {
                    let source = number(index.trim()).map_err(|error| declaration.error(&error))?;
                    let (_, sources) = plic_config.filter(|(plic, _)| plic.name == target)
                        .ok_or_else(|| declaration.error("unknown GPIO PLIC destination"))?;
                    if irq.is_some() || !(1..=sources).contains(&source) {
                        return Err(declaration.error("invalid GPIO PLIC source"));
                    }
                    irq = machine.bus.plic_index(target).map(|index| (index, source));
                    continue;
                }
                let pins = irq_list(pin, 32).map_err(|error| declaration.error(&error))?;
                let inputs = irq_list(index, 32).map_err(|error| declaration.error(&error))?;
                if pins.len() != inputs.len() {
                    return Err(declaration.error("mismatched GPIO wire ranges"));
                }
                gpio_routes.extend(pins.into_iter().zip(inputs).map(|(pin, input)| (declaration, pin, target, input)));
            }
        }
        let index = machine.bus.map_andes_gpio(crate::andes_gpio::AndesGpio::new(
            entry.address()?, irq,
        )).map_err(|error| entry.error(&error))?;
        gpio_indices.insert(entry.name.as_str(), index);
    }
    for (entry, pin, target, input) in gpio_routes {
        let source = gpio_indices[entry.name.as_str()];
        let index = gpio_indices.get(target).copied().ok_or_else(|| entry.error("unknown Andes GPIO wire destination"))?;
        machine.bus.connect_andes_gpio(source, pin, index, input);
    }
    for entry in andes_pits {
        let mut frequency = None;
        let mut channels = None;
        let mut irq = None;
        for (_, line) in &entry.body {
            if let Some((pin, target)) = line.split_once("->") {
                let (target, source) = target.split_once('@').ok_or_else(|| entry.error("expected PIT PLIC wiring"))?;
                let source = number(source.trim()).map_err(|error| entry.error(&error))?;
                let Some((controller, sources)) = plic_config.filter(|(plic, _)| plic.name == target.trim()) else {
                    return Err(entry.error("unknown PIT interrupt destination"));
                };
                if irq.is_some() || !matches!(pin.trim(), "" | "IRQ") || !(1..=sources).contains(&source) {
                    return Err(entry.error("invalid PIT interrupt connection"));
                }
                irq = machine.bus.plic_index(&controller.name).map(|index| (index, source));
            } else if let Some((name, value)) = line.split_once(':') {
                let value = number(value.trim()).map_err(|error| entry.error(&error))?;
                match name.trim() {
                    "clockFrequency" if frequency.is_none() => frequency = Some(value),
                    "numberOfChannels" if channels.is_none() => channels = Some(value),
                    _ => return Err(entry.error("unsupported PIT property")),
                }
            } else {
                return Err(entry.error("unsupported PIT declaration"));
            }
        }
        let frequency = frequency.filter(|frequency| *frequency != 0)
            .ok_or_else(|| entry.error("PIT requires a positive clock frequency"))?;
        let channels = channels.unwrap_or(4);
        if !(1..=4).contains(&channels) {
            return Err(entry.error("PIT requires between one and four channels"));
        }
        machine.bus.map_andes_pit(crate::andes_pit::AndesPit::new(
            entry.address()?, entry.name.clone(), frequency, channels as usize, irq,
        )).map_err(|error| entry.error(&error))?;
    }
    if let Some(entry) = egis_syscon {
        let properties = keys(entry, &["mtimer", "pit", "wdt"])?;
        let mut devices = [0; 3];
        for (index, (key, model)) in [
            ("mtimer", "Timers.RiscVMachineTimer"),
            ("pit", "Timers.AndesATCPIT100"),
            ("wdt", "Timers.AndesATCWDT200_Watchdog"),
        ].into_iter().enumerate() {
            let name = properties.iter().find_map(|&(name, value)| (name == key).then_some(value))
                .ok_or_else(|| entry.error("missing AOSMU clock target"))?;
            devices[index] = entries.iter().find(|device| device.name == name && device.model == model)
                .ok_or_else(|| entry.error("invalid AOSMU clock target"))?.address()?;
        }
        machine.bus.map_egis_syscon(crate::egis_syscon::EgisSyscon::new(
            entry.address()?, entry.name.clone(), devices,
        )).map_err(|error| entry.error(&error))?;
        machine.configure_egis_reset();
    }
    for entry in sifive_uarts {
        let mut irq = 0;
        for (_, wire) in &entry.body {
            let (pin, destination) = wire.split_once("->").ok_or_else(|| entry.error("expected SiFive UART IRQ wiring"))?;
            let (target, index) = destination.split_once('@').ok_or_else(|| entry.error("expected PLIC destination"))?;
            let index = number(index.trim()).map_err(|error| entry.error(&error))?;
            if irq != 0 || !matches!(pin.trim(), "" | "IRQ")
                || !plic_config.is_some_and(|(controller, sources)| controller.name == target.trim() && (1..=sources).contains(&index))
            {
                return Err(entry.error("invalid SiFive UART PLIC source"));
            }
            irq = index as usize;
        }
        machine.bus.map_sifive_uart(&entry.name, entry.address()?, irq).map_err(|error| entry.error(&error))?;
    }
    for entry in sifive_gpios {
        let mut routes = [None; 32];
        for (_, wire) in &entry.body {
            let (pins, destination) = wire.split_once("->")
                .ok_or_else(|| entry.error("expected SiFive GPIO IRQ wiring"))?;
            let (target, sources) = destination.split_once('@')
                .ok_or_else(|| entry.error("expected PLIC destination"))?;
            let (_, count) = plic_config.filter(|(controller, _)| controller.name == target.trim())
                .ok_or_else(|| entry.error("unknown SiFive GPIO PLIC destination"))?;
            let pins = irq_list(pins, 32).map_err(|error| entry.error(&error))?;
            let sources = irq_list(sources, count + 1).map_err(|error| entry.error(&error))?;
            if pins.len() != sources.len() {
                return Err(entry.error("SiFive GPIO and PLIC IRQ ranges have different lengths"));
            }
            for (pin, source) in pins.into_iter().zip(sources) {
                if source == 0 || routes[usize::from(pin)].replace(source).is_some() {
                    return Err(entry.error("invalid SiFive GPIO PLIC source"));
                }
            }
        }
        machine.bus.map_sifive_gpio(entry.address()?, routes).map_err(|error| entry.error(&error))?;
    }
    for entry in litex_i2s {
        map_litex_i2s(entry, &configs, &mut machine.bus)?;
    }
    if let Some(entry) = uart {
        if entry.model == "UART.TrivialUart" {
            keys(entry, &[])?;
            machine.bus.map_trivial_uart(entry.address()?, &entry.name).map_err(|e| entry.error(&e))?;
        } else if entry.model == "UART.Murax_UART" {
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
                    .filter(|(pin, _)| matches!(pin.trim(), "" | "IRQ"))
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
                .map_named_ns16550(&entry.name, entry.address()?, source, wide.unwrap_or(false))
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

fn memory_registrations(entry: &Entry, length: u32) -> Result<Vec<(u32, u32, u32)>, String> {
    let Some(registrations) = entry.registration_list() else {
        return Ok(vec![(entry.address()?, length, 0)]);
    };
    if registrations.is_empty() {
        return Err(entry.error("expected sysbus memory registrations"));
    }
    let parse = |value: &str| number(value.trim()).map_err(|error| entry.error(&error));
    registrations.into_iter().map(|registration| {
        let address = registration.strip_prefix("sysbus").filter(|text| text.starts_with(char::is_whitespace))
            .ok_or_else(|| entry.error("expected sysbus memory registration"))?.trim();
        if let Some(range) = address.strip_prefix('<').and_then(|text| text.strip_suffix('>')) {
            let (base, size) = range.split_once(',').and_then(|(base, size)| Some((base, size.trim().strip_prefix('+')?)))
                .ok_or_else(|| entry.error("expected memory registration <base, +size>"))?;
            return Ok((parse(base)?, parse(size)?, 0));
        }
        if let Some(body) = address.strip_prefix("new Bus.BusRangeRegistration") {
            let body = body.trim().strip_prefix('{').and_then(|text| text.strip_suffix('}'))
                .ok_or_else(|| entry.error("expected memory registration properties"))?;
            let mut address = None;
            let mut size = None;
            let mut offset = None;
            for (key, value) in object_fields(body).map_err(|error| entry.error(error))? {
                match key {
                    "address" if address.is_none() => address = Some(parse(value)?),
                    "size" if size.is_none() => size = Some(parse(value)?),
                    "offset" if offset.is_none() => offset = Some(parse(value)?),
                    _ => return Err(entry.error("unsupported or duplicate memory registration property")),
                }
            }
            return match (address, size) {
                (Some(address), Some(size)) => Ok((address, size, offset.unwrap_or(0))),
                _ => Err(entry.error("memory range registration requires address and size")),
            };
        }
        Ok((parse(address)?, length, 0))
    }).collect()
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
    port.name = Some(entry.name.clone());
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

pub(crate) fn map_bus_faulting_model(entry: &Entry, bus: &mut Bus) -> Result<(), String> {
    let mut offset = None;
    let mut writes = true;
    for (key, value) in keys(entry, &["FaultOffset", "FaultOnWrites"])? {
        if key == "FaultOffset" {
            offset = Some(number(value).map_err(|error| entry.error(&error))? as usize);
        } else {
            writes = value.parse().map_err(|_| entry.error("invalid FaultOnWrites boolean"))?;
        }
    }
    let address = entry.address()?;
    bus.map_bus_faulting_peripheral(address, offset).map_err(|error| entry.error(&error))?;
    bus.name_faulting_peripheral(address, entry.name.clone());
    bus.set_mock_fault_on_writes(address, writes);
    Ok(())
}

fn group_stm32_i2c_sensors<'a>(
    entries: Vec<&'a Entry>,
    controllers: &[&Entry],
) -> Result<HashMap<&'a str, Vec<(&'a Entry, u32)>>, String> {
    let mut sensors = HashMap::<_, Vec<_>>::new();
    for sensor in entries {
        keys(sensor, &[])?;
        let (controller, address) = sensor.registration.as_deref()
            .and_then(|registration| registration.split_once(char::is_whitespace))
            .filter(|(name, _)| controllers.iter().any(|i2c| i2c.name == *name))
            .ok_or_else(|| sensor.error("expected STM32 I²C controller and address"))?;
        let address = number(address.trim()).map_err(|error| sensor.error(&error))?;
        sensors.entry(controller).or_default().push((sensor, address));
    }
    Ok(sensors)
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

fn map_sysbus_ranges(entry: &Entry, bus: &mut Bus, root: &Path, assets: Option<&Path>) -> Result<(), String> {
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
        if command == "ApplySVD" {
            let file = range.trim().strip_prefix('@').unwrap_or(range.trim());
            let path = crate::monitor::Asset::new(file, &entry.source).input_path(root, assets)?;
            bus.svd.push(crate::svd::Svd::open(&path).map_err(|e| error(&e))?);
            continue;
        }
        let (range, tail) = range
            .trim_start()
            .strip_prefix('<')
            .and_then(|text| text.split_once('>'))
            .ok_or_else(|| error("expected <start size>"))?;
        let (start, size) = if let Some((start, end)) = range.split_once(',') {
            let start = number(start.trim()).map_err(|e| error(&e))?;
            let end = end.trim();
            let size = if let Some(size) = end.strip_prefix('+') {
                number(size.trim()).map_err(|e| error(&e))?
            } else {
                number(end).map_err(|e| error(&e))?
                    .checked_sub(start)
                    .and_then(|length| length.checked_add(1))
                    .ok_or_else(|| error("invalid inclusive address range"))?
            };
            (start, size)
        } else {
            let mut values = range.split_whitespace();
            let (Some(start), Some(size), None) = (values.next(), values.next(), values.next()) else {
                return Err(error("expected <start size> or <start, end>"));
            };
            (number(start).map_err(|e| error(&e))?, number(size).map_err(|e| error(&e))?)
        };
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
                "c" | "zicsr"
                    | "zifencei"
                    | "zihintpause"
                    | "zicond"
                    | "zba"
                    | "zbb"
                    | "zbc"
                    | "zbkc"
                    | "zkne"
                    | "zknd"
                    | "zknh"
                    | "zksed"
                    | "zksh"
                    | "zbs"
                    | "zbkb"
                    | "zbkx"
                    | "zca"
                    | "zcf"
                    | "zcd"
                    | "zcb"
                    | "xandes"
                    | "xandesperf"
                    | "v"
                    | "xandesbfhcvt"
                    | "zacas"
                    | "zaamo"
                    | "zalrsc"
                    | "zabha"
                    | "zmmul"
                    | "zfa"
                    | "zfhmin"
                    | "zfh"
                    | "zfbfmin"
                    | "smepmp"
                    | "sscofpmf"
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
        (bool, bool),
        bool,
        (bool, (bool, bool)),
        bool,
        bool,
        (bool, bool, bool, bool),
        (bool, bool),
        BitManip,
        (
            ((bool, bool, bool, bool, bool), bool),
            (bool, bool, bool, bool),
        ),
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
    use crate::repl_parser::{IrqDestination, IrqEnd, irq_connection};

    if let Ok(connection) = irq_connection(wire)
        && matches!(connection.sources.as_deref(), None | Some([IrqEnd::Property("IRQ")]))
        && let [IrqDestination { name: cpu, local: None, groups }] = connection.destinations.as_slice()
        && let [IrqEnd::Number(line)] = groups.as_slice()
        && let Some((hart, &(_, target, _, _, _))) =
            configs
                .iter()
                .enumerate()
                .find(|(_, (_, target, _, _, _))| {
                    target.name == cpu.trim()
                        && (uses_vex_irq(target) || target.model == "CPU.Minerva")
                })
    {
        return match *line {
            0..=31 => Ok((hart, *line as u8)),
            1000..=1031 if uses_vex_irq(target) => Ok((hart, (line - 1000 + 32) as u8)),
            _ => Err(entry.error("VexRiscv external IRQ must be in 0–31 or 1000–1031")),
        };
    }
    Err(entry.error("expected IRQ wired to a VexRiscv external interrupt"))
}

pub(crate) fn irq_index(value: &str, count: u32) -> Result<u8, String> {
    checked_irq_index(number(value.trim())?, count)
}

fn checked_irq_index(index: u32, count: u32) -> Result<u8, String> {
    if index >= count || index > u32::from(u8::MAX) {
        return Err(format!("IRQ index {index} is out of range"));
    }
    Ok(index as u8)
}

fn irq_list(value: &str, count: u32) -> Result<Vec<u8>, String> {
    use crate::repl_parser::{IrqEnd, irq_ends, irq_range};

    let value = value.trim();
    if !(value.starts_with('[') && value.ends_with(']')) {
        return irq_index(value, count).map(|index| vec![index]);
    }

    let mut indices = Vec::new();
    for end in irq_ends(value, false)? {
        match end {
            IrqEnd::Number(index) => indices.push(checked_irq_index(index, count)?),
            IrqEnd::Range(start, end) => {
                let start = checked_irq_index(start, count)?;
                let end = checked_irq_index(end, count)?;
                if start > end {
                    return Err("descending IRQ range".into());
                }
                indices.extend(irq_range(start.into(), end.into()).map(|index| index as u8));
            }
            IrqEnd::Property(name) => return Err(format!("unexpected IRQ property {name}")),
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
    fn andes_gpio_overlay_routes_output_to_rising_edge_interrupt() {
        let directory = std::env::temp_dir().join(format!("penv-andes-gpio-{}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        let path = directory.join("platform.repl");
        fs::write(&path, r#"cpu: CPU.RiscV32 @ sysbus
    cpuType: "rv32i"
    timeProvider: empty
ram: Memory.MappedMemory @ sysbus 0x10000000
    size: 0x1000
plic: IRQControllers.AndesNCEPLIC100 @ sysbus <0x20000000, +0x400000>
    numberOfSources: 1023
    numberOfContexts: 1
    0 -> cpu@11
gpio: GPIOPort.AndesATCGPIO100 @ sysbus 0x30000000
    IRQ -> plic@21
gpio:
    4 -> gpio@5
"#).unwrap();
        let machine = load(&path, None, false);
        fs::remove_dir_all(directory).unwrap();
        let mut bus = machine.unwrap().bus;
        let mut sink = std::io::sink();
        bus.write(0x2000_0054, 4, 1, &mut sink).unwrap();
        bus.write(0x2000_2000, 4, 1 << 21, &mut sink).unwrap();
        bus.write(0x3000_0028, 4, 1 << 4, &mut sink).unwrap();
        bus.write(0x3000_0054, 4, 6 << 20, &mut sink).unwrap();
        assert_eq!(bus.tick_plic(), 0);
        bus.write(0x3000_0030, 4, 1 << 4, &mut sink).unwrap();
        assert_eq!(bus.read::<4>(0x3000_0020).unwrap(), 1 << 5);
        assert_eq!(bus.read::<4>(0x3000_0064).unwrap(), 1 << 5);
        assert_eq!(bus.tick_plic(), 1 << 11);
        assert_eq!(bus.read::<4>(0x2020_0004).unwrap(), 21);
        bus.write(0x3000_0064, 4, 1 << 5, &mut sink).unwrap();
        assert_eq!(bus.tick_plic(), 0);
        bus.write(0x2020_0004, 4, 21, &mut sink).unwrap();
        assert_eq!(bus.tick_plic(), 0);
    }

    #[test]
    fn rv32_memory_windows_share_backing_and_apply_bounds_and_offsets() {
        let directory = std::env::temp_dir().join(format!("penv-rv32-window-{}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        let path = directory.join("platform.repl");
        fs::write(&path, r#"cpu: CPU.RiscV32 @ sysbus
    cpuType: "rv32i"
    timeProvider: empty
ram: Memory.MappedMemory @ {
    sysbus 0x10000000;
    sysbus <0x20000000, +0x200>;
    sysbus new Bus.BusRangeRegistration { address: 0x30000000; size: 0x200; offset: 0x100 };
}
    size: 0x1000
"#).unwrap();
        let machine = load(&path, None, false);
        fs::remove_dir_all(directory).unwrap();
        let mut bus = machine.unwrap().bus;
        bus.write(0x1000_0180, 4, 0x1234_5678, &mut std::io::sink()).unwrap();
        assert_eq!(bus.read::<4>(0x2000_0180).unwrap(), 0x1234_5678);
        assert_eq!(bus.read::<4>(0x3000_0080).unwrap(), 0x1234_5678);
        assert!(bus.read_ram::<4>(0x2000_0200).is_err());
        assert!(bus.read_ram::<4>(0x3000_0200).is_err());
    }

    #[test]
    fn parses_multiple_inline_declarations_on_one_line() {
        let directory = std::env::temp_dir().join(format!("penv-inline-declarations-{}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        let path = directory.join("platform.repl");
        fs::write(&path, concat!(
            "first: Memory.MappedMemory @ sysbus 0x1000 { size: 0x100; }; ",
            "second: Memory.MappedMemory @ sysbus 0x2000 { size: 0x200 }\n",
        )).unwrap();

        let mut entries = Vec::new();
        let result = parse(&path, &directory, &mut HashSet::new(), &mut entries);
        fs::remove_dir_all(directory).unwrap();
        result.unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "first");
        assert_eq!(entries[0].body, [(1, "size: 0x100".into())]);
        assert_eq!(entries[1].name, "second");
        assert_eq!(entries[1].body, [(1, "size: 0x200".into())]);
    }

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
        let machine = load(&path, None, false);
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

pub(crate) fn irq_receiver(target: &str) -> Result<(&str, Option<u32>), String> {
    let Some((name, index)) = target.split_once('#') else {
        return Ok((target.trim(), None));
    };
    let name = name.trim();
    if name.is_empty() {
        return Err("missing local IRQ receiver name".into());
    }
    Ok((name, Some(number(index.trim())?)))
}

pub(crate) use crate::repl_parser::{ListItem, boolean_value, list_value, object_fields};

pub(crate) fn multiline_value(value: &str) -> Option<String> {
    decode_multiline_value(value, false)
}

pub(crate) fn decode_multiline_value(value: &str, keep_lone_line_escape: bool) -> Option<String> {
    let body = value.trim().strip_prefix("'''")?.strip_suffix("'''")?;
    let mut decoded = String::new();
    let mut start = 0;
    for (index, _) in body.match_indices("\\'''") {
        if index != 0 && body.as_bytes()[index - 1] == b'\\' {
            continue;
        }
        if keep_lone_line_escape && body[index + 4..].starts_with('\n') {
            let line_start = body[..index].rfind('\n').map_or(0, |end| end + 1);
            if !body[line_start..index].contains("'''") {
                continue;
            }
        }
        decoded.push_str(&body[start..index]);
        start = index + 1;
    }
    decoded.push_str(&body[start..]);
    Some(decoded)
}
