use std::collections::HashMap;

use crate::{
    machine::{MAX_HARTS, Machine},
    platform::Entry,
    rv64::{Isa, Rv64},
    rv64_devices::{FallbackRange, Plic, Uart},
    rv64_ethernet::{Ethernet, Phy},
    rv64_priv::Pmp,
    rv64_starfive::ResetGenerator,
};

pub fn load(entries: Vec<Entry>) -> Result<Machine, String> {
    let entries = merge(entries)?;
    let mut machine = Machine::without_default_ram();
    let mut cpus = Vec::new();
    let mut clint = None;
    let mut plic = None;
    let mut uarts = Vec::new();
    let mut ram = false;

    for entry in &entries {
        if entry.registration.as_deref() == Some("none") {
            continue;
        }
        if entry.name == "sysbus" && entry.model.is_empty() {
            for (_, command) in &entry.body {
                if let Some(range) = fallback_range(entry, command)? {
                    machine.bus.rv64_devices.fallback_ranges.push(range);
                }
            }
            continue;
        }
        match entry.model.as_str() {
            "CPU.RiscV64" => {
                let hart = property(entry, "hartId").map_or(Ok(0), |value| number(entry, value))?;
                let hart = usize::try_from(hart)
                    .map_err(|_| entry.error("hart ID exceeds host limits"))?;
                cpus.push((hart, entry));
            }
            model if model.starts_with("CPU.") => {
                return Err(entry.error("mixed CPU architectures"));
            }
            "Memory.MappedMemory" | "Memory.ArrayMemory"
                if entry
                    .registration
                    .as_deref()
                    .is_some_and(|value| value.starts_with("sysbus") || value.starts_with('{')) =>
            {
                map_memory(&mut machine, entry)?;
                ram = true;
            }
            "IRQControllers.CoreLevelInterruptor" => {
                if clint.replace(entry).is_some() {
                    return Err(entry.error("only one CLINT is supported"));
                }
            }
            "IRQControllers.PlatformLevelInterruptController" => {
                if plic.replace(entry).is_some() {
                    return Err(entry.error("only one PLIC is supported"));
                }
            }
            "UART.NS16550" | "UART.SiFive_UART" => uarts.push(entry),
            _ => {}
        }
    }

    if !ram {
        return Err("RV64 platform has no mapped RAM".into());
    }
    cpus.sort_unstable_by_key(|&(hart, _)| hart);
    if cpus.is_empty()
        || cpus.len() > MAX_HARTS
        || cpus
            .iter()
            .enumerate()
            .any(|(index, &(hart, _))| index != hart)
    {
        return Err(format!(
            "RV64 platform requires contiguous hart IDs from zero (up to {MAX_HARTS})"
        ));
    }
    machine.configure_harts(cpus.len());
    for &(hart, entry) in &cpus {
        let cpu = create_cpu(entry, clint)?;
        let target = machine.hart_mut(hart);
        target.name = Some(entry.name.clone());
        target.rv64 = Some(Box::new(cpu));
    }

    if let Some(entry) = clint {
        let targets = property(entry, "numberOfTargets")
            .map_or(Ok(cpus.len() as u64), |value| number(entry, value))?;
        let targets = usize::try_from(targets)
            .map_err(|_| entry.error("CLINT target count exceeds host limits"))?;
        if targets != cpus.len() {
            return Err(entry.error("CLINT target count must match RV64 hart count"));
        }
        for (_, line) in &entry.body {
            let Some((outputs, input)) = line.split_once("->") else {
                continue;
            };
            let (name, pins) = endpoint(entry, input)?;
            let hart = hart_by_name(&cpus, entry, name)?;
            if list(entry, outputs)? != [hart as u64 * 2, hart as u64 * 2 + 1]
                || list(entry, pins)? != [3, 7]
            {
                return Err(entry.error("unsupported CLINT wiring"));
            }
        }
        machine
            .bus
            .map_clint(entry.address()?, targets)
            .map_err(|error| entry.error(&error))?;
        if let Some(frequency) = property(entry, "frequency") {
            let frequency = number(entry, frequency)?;
            let divider =
                property(entry, "divider").map_or(Ok(1), |divider| number(entry, divider))?;
            // Renode's default rate is 100 million instructions per second per
            // hart. A scheduler round represents one instruction on each hart.
            const DEFAULT_INSTRUCTIONS_PER_SECOND: u64 = 100_000_000;
            let steps = (DEFAULT_INSTRUCTIONS_PER_SECOND * targets as u64)
                .checked_mul(divider)
                .filter(|steps| *steps != 0 && frequency != 0)
                .ok_or_else(|| entry.error("invalid CLINT frequency or divider"))?;
            machine
                .bus
                .clint
                .as_mut()
                .unwrap()
                .set_step_rate(frequency, steps);
        }
    }

    let mut sources = 0;
    if let Some(entry) = plic {
        let count = required_number(entry, "numberOfContexts")?;
        if count == 0 || count > 2 * MAX_HARTS as u64 {
            return Err(entry.error("unsupported PLIC context count"));
        }
        let mut routes = vec![None; count as usize];
        for (_, line) in &entry.body {
            let Some((outputs, input)) = line.split_once("->") else {
                continue;
            };
            let (name, pins) = endpoint(entry, input)?;
            let hart = hart_by_name(&cpus, entry, name)?;
            let outputs = list(entry, outputs)?;
            let pins = list(entry, pins)?;
            if outputs.len() != pins.len() {
                return Err(entry.error("mismatched PLIC output and CPU input counts"));
            }
            for (output, pin) in outputs.into_iter().zip(pins) {
                let slot = usize::try_from(output)
                    .ok()
                    .and_then(|index| routes.get_mut(index))
                    .ok_or_else(|| entry.error("invalid PLIC context index"))?;
                if slot.is_some() || !matches!(pin, 9 | 11) {
                    return Err(entry.error("invalid or duplicate PLIC context route"));
                }
                *slot = Some((hart, pin as u32));
            }
        }
        let priorities = flag(entry, "prioritiesEnabled", true)?;
        let base = entry.address()?;
        sources = required_number(entry, "numberOfSources")?;
        if !(1..=1023).contains(&sources) {
            return Err(entry.error("PLIC requires 1–1023 sources"));
        }
        check_mapping(&machine, entry, base, Plic::SIZE)?;
        machine.bus.rv64_devices.plic = Some(Plic::new(base, sources as usize, routes, priorities));
    }

    for entry in uarts {
        let base = entry.address()?;
        let sifive = entry.model == "UART.SiFive_UART";
        let wide = flag(entry, "wideRegisters", false)?;
        let mut source = 0;
        let mut cpu_lines = Vec::new();
        for (_, line) in &entry.body {
            if let Some((output, input)) = line.split_once("->") {
                if !matches!(output.trim(), "" | "IRQ") {
                    return Err(entry.error("unsupported UART output"));
                }
                for input in input.split('|') {
                    let (name, pin) = endpoint(entry, input)?;
                    let pin = number(entry, pin)?;
                    if plic.is_some_and(|plic| plic.name == name) {
                        if !(1..=sources).contains(&pin) || source != 0 {
                            return Err(entry.error("invalid or duplicate UART PLIC source"));
                        }
                        source = pin;
                    } else {
                        let hart = hart_by_name(&cpus, entry, name)?;
                        if !(16..64).contains(&pin) {
                            return Err(entry.error("unsupported direct UART interrupt"));
                        }
                        machine
                            .hart_mut(hart)
                            .rv64
                            .as_mut()
                            .unwrap()
                            .privilege
                            .add_interrupt(pin as u32);
                        cpu_lines.push((hart, pin as u32));
                    }
                }
            }
        }
        let uart = Uart::new(
            entry.name.clone(),
            base,
            source as usize,
            cpu_lines,
            sifive,
            wide,
        );
        check_mapping(&machine, entry, base, uart.size())?;
        machine.bus.rv64_devices.uarts.push(uart);
    }
    for entry in &entries {
        if entry.model != "Python.PythonPeripheral"
            || !matches!(
                entry.name.as_str(),
                "rstgen" | "audiorstgen" | "voutsysrstgen"
            )
        {
            continue;
        }
        let Ok(base) = entry.address() else { continue };
        let size = required_number(entry, "size")?;
        if let Some(device) = ResetGenerator::from_repl(&entry.name, base, size) {
            check_mapping(&machine, entry, base, device.size())?;
            machine.bus.rv64_devices.reset_generators.push(device);
        }
    }

    map_ethernet(&mut machine, &entries)?;
    Ok(machine)
}

fn map_ethernet(machine: &mut Machine, entries: &[Entry]) -> Result<(), String> {
    let mut macs = HashMap::new();
    for entry in entries {
        if entry.model != "Network.SynopsysEthernetMAC"
            || property(entry, "version") != Some("SynopsysEthernetVersion.BeagleV")
            || entry.registration.as_deref() == Some("none")
        {
            continue;
        }
        let base = entry.address()?;
        if base & 3 != 0 {
            return Err(entry.error("unaligned Ethernet address"));
        }
        check_mapping(machine, entry, base, Ethernet::SIZE)?;
        macs.insert(entry.name.as_str(), machine.bus.rv64_devices.ethernet.len());
        machine.bus.rv64_devices.ethernet.push(Ethernet::new(base));
    }

    for entry in entries
        .iter()
        .filter(|entry| entry.model == "Network.EthernetPhysicalLayer")
    {
        let Some(registration) = &entry.registration else {
            continue;
        };
        let registration = registration
            .strip_prefix('{')
            .and_then(|value| value.strip_suffix('}'))
            .unwrap_or(registration);
        for target in registration
            .split(';')
            .map(str::trim)
            .filter(|target| !target.is_empty())
        {
            let Some((name, address)) = target.split_once(char::is_whitespace) else {
                continue;
            };
            let Some(&mac) = macs.get(name) else { continue };
            let address = number(entry, address)?;
            if address >= 32
                || !machine.bus.rv64_devices.ethernet[mac]
                    .attach(address as usize, ethernet_phy(entry)?)
            {
                return Err(entry.error("invalid or duplicate MDIO PHY address"));
            }
        }
    }
    Ok(())
}

fn ethernet_phy(entry: &Entry) -> Result<Phy, String> {
    let mut phy = Phy::default();
    let names = [
        "BasicControl",
        "BasicStatus",
        "Id1",
        "Id2",
        "AutoNegotiationAdvertisement",
        "AutoNegotiationLinkPartnerBasePageAbility",
        "AutoNegotiationExpansion",
        "AutoNegotiationNextPageTransmit",
        "AutoNegotiationLinkPartnerReceivedNextPage",
        "MasterSlaveControl",
        "MasterSlaveStatus",
        "PowerSourcingEquipmentControl",
        "PowerSourcingEquipmentStatus",
        "MDIOManageableDeviceAccessControl",
        "MDIOManageableDeviceAcceddAddressData",
        "ExtendedStatus",
    ];
    for (index, slot) in phy.registers.iter_mut().enumerate() {
        let vendor;
        let name: &str = if let Some(name) = names.get(index) {
            name
        } else {
            vendor = format!("VendorSpecific{}", index - names.len());
            &vendor
        };
        if let Some(value) = property(entry, name) {
            *slot = u16::try_from(number(entry, value)?)
                .map_err(|_| entry.error("PHY register value exceeds 16 bits"))?;
        }
    }
    Ok(phy)
}

fn fallback_range(entry: &Entry, command: &str) -> Result<Option<FallbackRange>, String> {
    let Some((command, arguments)) = command.split_once(char::is_whitespace) else {
        return Ok(None);
    };
    if !matches!(command, "Tag" | "SilenceRange") {
        return Ok(None);
    }
    let (bounds, rest) = arguments
        .trim()
        .strip_prefix('<')
        .and_then(|text| text.split_once('>'))
        .ok_or_else(|| entry.error("invalid tagged address range"))?;
    let (start, end) = if let Some((start, end)) = bounds.split_once(',') {
        let start = number(entry, start)?;
        let end = if let Some(size) = end.trim().strip_prefix('+') {
            range_end(entry, start, number(entry, size)?)?
        } else {
            number(entry, end)?
        };
        (start, end)
    } else {
        let (start, size) = bounds
            .trim()
            .split_once(char::is_whitespace)
            .ok_or_else(|| entry.error("tagged range requires a start and size"))?;
        let start = number(entry, start)?;
        (start, range_end(entry, start, number(entry, size)?)?)
    };
    if end < start {
        return Err(entry.error("reversed tagged address range"));
    }
    let mut value = 0;
    if command == "Tag" {
        let (_, tail) = rest
            .trim()
            .strip_prefix('"')
            .and_then(|label| label.split_once('"'))
            .ok_or_else(|| entry.error("Tag requires a quoted name"))?;
        if let Some(default) = tail
            .split_whitespace()
            .next()
            .filter(|token| !token.starts_with('#'))
        {
            value = number(entry, default)?;
        }
    }
    Ok(Some(FallbackRange { start, end, value }))
}

fn range_end(entry: &Entry, start: u64, size: u64) -> Result<u64, String> {
    size.checked_sub(1)
        .and_then(|last| start.checked_add(last))
        .ok_or_else(|| entry.error("invalid tagged address range size"))
}

fn create_cpu(entry: &Entry, clint: Option<&Entry>) -> Result<Rv64, String> {
    if entry.registration.as_deref() != Some("sysbus") {
        return Err(entry.error("expected RV64 CPU registered on sysbus"));
    }
    let cpu_type = property(entry, "cpuType").ok_or_else(|| entry.error("missing cpuType"))?;
    let isa = Isa::parse(cpu_type.trim_matches('"')).map_err(|error| entry.error(&error))?;
    if let Some(time) = property(entry, "timeProvider")
        && time != "empty"
        && !clint.is_some_and(|entry| entry.name == time)
    {
        return Err(entry.error("unsupported RV64 time provider"));
    }
    if let Some(architecture) = property(entry, "privilegedArchitecture")
        .or_else(|| property(entry, "privilegeArchitecture"))
        && !matches!(
            architecture,
            "PrivilegedArchitecture.Priv1_10"
                | "PrivilegedArchitecture.Priv1_11"
                | "PrivilegedArchitecture.Priv1_12"
        )
    {
        return Err(entry.error("unsupported RV64 privileged architecture"));
    }
    let mut cpu = Rv64::new(isa);
    cpu.allow_unaligned = flag(entry, "allowUnalignedAccesses", false)?;
    let pmp_count =
        property(entry, "pmpEntryCount").map_or(Ok(64), |value| number(entry, value))?;
    let pmp_bits =
        property(entry, "pmpNumberOfAddrBits").map_or(Ok(54), |value| number(entry, value))?;
    if pmp_count > 64 || !(1..=54).contains(&pmp_bits) {
        return Err(entry.error("unsupported RV64 PMP configuration"));
    }
    if pmp_count != 0 {
        cpu.privilege.pmp = Some(Pmp::new(pmp_count as usize, pmp_bits as u32));
    }
    if entry
        .body
        .iter()
        .any(|(_, value)| value == "RegisterCustomCSR \"BPM\" 0x7C0 Machine")
    {
        cpu.privilege.bpm = Some(0);
    }
    Ok(cpu)
}

fn check_mapping(machine: &Machine, entry: &Entry, base: u32, size: u64) -> Result<(), String> {
    let start = u64::from(base);
    let end = start + size;
    let devices = &machine.bus.rv64_devices;
    let overlaps = devices.plic.as_ref().is_some_and(|plic| {
        start < u64::from(plic.base) + Plic::SIZE && u64::from(plic.base) < end
    }) || devices
        .uarts
        .iter()
        .any(|uart| start < u64::from(uart.base) + uart.size() && u64::from(uart.base) < end);
    let overlaps = overlaps
        || devices.reset_generators.iter().any(|device| {
            start < u64::from(device.base) + device.size() && u64::from(device.base) < end
        });
    let overlaps = overlaps
        || devices.ethernet.iter().any(|device| {
            start < u64::from(device.base) + Ethernet::SIZE && u64::from(device.base) < end
        });
    if end > 1 << 32 || overlaps || machine.bus.overlaps(start, end) {
        return Err(entry.error("invalid or overlapping device registration"));
    }
    Ok(())
}

fn merge(entries: Vec<Entry>) -> Result<Vec<Entry>, String> {
    let mut names: HashMap<String, usize> = HashMap::new();
    let mut merged: Vec<Entry> = Vec::new();
    for entry in entries {
        if let Some(&index) = names.get(&entry.name) {
            let original = &mut merged[index];
            if !entry.model.is_empty() && entry.model != original.model {
                return Err(entry.error("cannot replace a peripheral model"));
            }
            if entry.registration.is_some() {
                original.registration = entry.registration;
            }
            for (line, value) in entry.body {
                if let Some((key, _)) = value.split_once(':') {
                    original.body.retain(|(_, old)| {
                        !old.split_once(':')
                            .is_some_and(|(old, _)| old.trim() == key.trim())
                    });
                }
                original.body.push((line, value));
            }
        } else {
            names.insert(entry.name.clone(), merged.len());
            merged.push(entry);
        }
    }
    Ok(merged)
}

fn map_memory(machine: &mut Machine, entry: &Entry) -> Result<(), String> {
    let size = required_number(entry, "size")?;
    let mut registrations = entry.registration.as_deref().unwrap().trim();
    if let Some(multiple) = registrations
        .strip_prefix('{')
        .and_then(|value| value.strip_suffix('}'))
    {
        registrations = multiple.trim();
    }
    let mut windows = Vec::new();
    for registration in registrations
        .split(';')
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let value = registration
            .strip_prefix("sysbus")
            .ok_or_else(|| entry.error("unsupported RAM registration"))?
            .trim();
        let (address, length) = if let Some(range) = value
            .strip_prefix('<')
            .and_then(|value| value.strip_suffix('>'))
        {
            let (base, limit) = range
                .split_once(',')
                .ok_or_else(|| entry.error("invalid RAM range"))?;
            let base = number(entry, base)?;
            let length = if let Some(length) = limit.trim().strip_prefix('+') {
                number(entry, length)?
            } else {
                number(entry, limit)?
                    .checked_sub(base)
                    .and_then(|value| value.checked_add(1))
                    .ok_or_else(|| entry.error("invalid RAM range"))?
            };
            (base, length)
        } else {
            (number(entry, value)?, size)
        };
        windows.push((address, length));
    }
    let Some(&(primary, _)) = windows.first() else {
        return Err(entry.error("missing RAM registration"));
    };
    let len = usize::try_from(size).map_err(|_| entry.error("RAM exceeds host limits"))?;
    if windows
        .iter()
        .any(|&(_, length)| length == 0 || length > size)
    {
        return Err(entry.error("invalid RAM window size"));
    }
    let backing = machine
        .bus
        .map_ram_backing64(primary, len, windows[0].1 as usize)
        .map_err(|error| entry.error(&error))?;
    for &(address, length) in &windows[1..] {
        machine
            .bus
            .map_alias64(backing, address, length as usize)
            .map_err(|error| entry.error(&error))?;
    }
    Ok(())
}

fn hart_by_name(cpus: &[(usize, &Entry)], entry: &Entry, name: &str) -> Result<usize, String> {
    cpus.iter()
        .find_map(|&(hart, cpu)| (cpu.name == name).then_some(hart))
        .ok_or_else(|| entry.error("unknown RV64 hart in interrupt route"))
}

fn endpoint<'a>(entry: &Entry, value: &'a str) -> Result<(&'a str, &'a str), String> {
    value
        .split_once('@')
        .map(|(name, pin)| (name.trim(), pin.trim()))
        .ok_or_else(|| entry.error("invalid interrupt connection"))
}

fn list(entry: &Entry, value: &str) -> Result<Vec<u64>, String> {
    let value = value.trim();
    let value = value
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(value);
    value.split(',').map(|value| number(entry, value)).collect()
}

fn property<'a>(entry: &'a Entry, name: &str) -> Option<&'a str> {
    entry.body.iter().rev().find_map(|(_, line)| {
        let (key, value) = line.split_once(':')?;
        (key.trim() == name).then_some(value.trim())
    })
}

fn required_number(entry: &Entry, name: &str) -> Result<u64, String> {
    number(
        entry,
        property(entry, name).ok_or_else(|| entry.error(&format!("missing {name}")))?,
    )
}

fn flag(entry: &Entry, name: &str, default: bool) -> Result<bool, String> {
    property(entry, name).map_or(Ok(default), |value| {
        value
            .parse()
            .map_err(|_| entry.error(&format!("invalid {name} flag")))
    })
}

fn number(entry: &Entry, value: &str) -> Result<u64, String> {
    let digits = value.trim().replace('_', "");
    match digits.strip_prefix("0x") {
        Some(hex) => u64::from_str_radix(hex, 16),
        None => digits.parse(),
    }
    .map_err(|_| entry.error(&format!("invalid number {value:?}")))
}
