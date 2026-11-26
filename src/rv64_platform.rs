use std::collections::HashMap;

use crate::{
    machine::{MAX_HARTS, Machine},
    platform::Entry,
    rv64::{Isa, Rv64},
    rv64_devices::{FallbackRange, MpfsCacheWayEnable, Plic, Uart},
    rv64_ethernet::{Ethernet, Phy},
    rv64_gem::Gem,
    rv64_mpfs::{DdrMock, Gpio, I2c, Qspi, Spi, Sysreg, Usb},
    rv64_mpfs_ihc::FireIhc,
    rv64_mpfs_sd,
    rv64_mpfs_wdog::{Output as WatchdogOutput, Watchdog},
    rv64_priv::{Levels, Pmp},
    rv64_starfive::{ResetGenerator, StarLightResetPin},
};

pub fn load(entries: Vec<Entry>, board_name: Option<&str>) -> Result<Machine, String> {
    let entries = merge(entries)?;
    let mut machine = Machine::without_default_ram();
    let mut cpus = Vec::new();
    let mut clint = None;
    let mut plic = None;
    let mut uarts = Vec::new();

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
            "CPU.RiscV64" | "CPU.RiscV32" => {
                if entry.registration.as_deref() != Some("sysbus")
                    && !entries.iter().any(|cluster| {
                        cluster.model == "CPU.Cluster"
                            && entry.registration.as_deref() == Some(cluster.name.as_str())
                    })
                {
                    return Err(
                        entry.error("expected RISC-V CPU registered on sysbus or a CPU cluster")
                    );
                }
                let hart = property(entry, "hartId").map_or(Ok(0), |value| number(entry, value))?;
                let hart = usize::try_from(hart)
                    .map_err(|_| entry.error("hart ID exceeds host limits"))?;
                cpus.push((hart, entry));
            }
            "CPU.Cluster" => {}
            model if model.starts_with("CPU.") => {
                return Err(entry.error("mixed CPU architectures"));
            }
            "Memory.MappedMemory" | "Memory.ArrayMemory"
                if entry
                    .registration
                    .as_deref()
                    .is_some_and(|value| value.starts_with("sysbus") || value.starts_with('{')) =>
            {
                map_memory(&mut machine, entry, &entries)?;
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

    cpus.sort_by_key(|&(hart, _)| hart);
    let independent_cpus = clint.is_none() && plic.is_none()
        && cpus.iter().all(|&(_, entry)| crate::platform::has_empty_time_provider(entry));
    if cpus.is_empty()
        || cpus.len() > MAX_HARTS
        || !independent_cpus && cpus.windows(2).any(|pair| pair[0].0 == pair[1].0)
    {
        return Err(format!(
            "RISC-V platform requires unique hart IDs (up to {MAX_HARTS} CPUs)"
        ));
    }
    let mut external_pmps = [None; MAX_HARTS];
    for entry in entries
        .iter()
        .filter(|entry| entry.model == "Miscellaneous.RiscVExternalPMP")
    {
        let Some(registration) = entry
            .registration
            .as_deref()
            .filter(|value| *value != "none")
        else {
            continue;
        };
        let target = registration.strip_prefix("sysbus.").unwrap_or(registration);
        let hart = hart_by_name(&cpus, entry, target)?;
        if external_pmps[hart].replace(entry).is_some() {
            return Err(entry.error("duplicate external PMP for RV64 hart"));
        }
    }
    machine.configure_harts(cpus.len());
    machine.bus.guest_cpu_names = cpus.iter().map(|(_, entry)| entry.name.clone()).collect();
    // Each scheduler slot runs one hart. Scale the slot rate so adding harts
    // does not take virtual execution time away from the existing CPUs.
    machine.bus.set_step_hz(crate::machine::VIRTUAL_STEPS_PER_SECOND * cpus.len() as u32);
    let default_vector_mapped = machine.bus.read_ram::<4>(crate::csr::DEFAULT_MTVEC).is_ok();
    for (slot, &(hart, entry)) in cpus.iter().enumerate() {
        let target = machine.hart_mut(slot);
        if entry.model == "CPU.RiscV32" {
            let config = crate::platform::parse_rv32_cpu(entry, [clint, None, None], None, None)?;
            let external = external_pmps[slot].map(|external| {
                let count = property(external, "numberOfPMPEntries").map_or(Ok(64), |value| number(external, value))?;
                if !matches!(count, 16 | 64) {
                    return Err(external.error("expected 16 or 64 RV32 external PMP entries"));
                }
                Ok((external, count as usize))
            }).transpose()?;
            crate::platform::configure_rv32_cpu(target, &config, default_vector_mapped, external)?;
            continue;
        }
        let mut cpu = create_cpu(entry, clint, external_pmps[slot])?;
        cpu.hart_id = slot;
        cpu.mhartid_override = (hart != slot).then_some(hart as u64);
        target.name = Some(entry.name.clone());
        target.rv64 = Some(Box::new(cpu));
        if external_pmps[slot].is_some() {
            let message = format!("{}: Enabling External PMP", entry.name);
            crate::monitor_log::record(1, &message);
            eprintln!("penv: {message}");
        }
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
        let (base, uart_harts) = uart_registration(entry, &entries)?;
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
                            .ok_or_else(|| entry.error("direct UART interrupts require an RV64 target"))?
                            .privilege
                            .add_interrupt(pin as u32);
                        cpu_lines.push((hart, pin as u32));
                    }
                }
            }
        }
        let mut uart = Uart::new(
            entry.name.clone(),
            base,
            source as usize,
            cpu_lines,
            sifive,
            wide,
        );
        uart.harts = uart_harts;
        check_mapping(&machine, entry, base, uart.size())?;
        machine.bus.rv64_devices.uarts.push(uart);
    }
    for entry in &entries {
        if entry.model == "SPI.MPFS_SPI" && entry.registration.as_deref() != Some("none") {
            let base = entry.address()?;
            check_mapping(&machine, entry, base, Spi::SIZE)?;
            machine.bus.rv64_devices.mpfs_spi.push(Spi::new(base));
            continue;
        }
        if entry.model == "Timers.MPFS_Watchdog" && entry.registration.as_deref() != Some("none") {
            let base = entry.address()?;
            check_mapping(&machine, entry, base, Watchdog::SIZE)?;
            let frequency = required_number(entry, "frequency")?;
            if frequency == 0 {
                return Err(entry.error("watchdog frequency must be nonzero"));
            }
            let mut outputs: [WatchdogOutput; 2] = Default::default();
            for (_, line) in &entry.body {
                let Some((name, targets)) = line.split_once("->") else {
                    continue;
                };
                let output = match name.trim() {
                    "RefreshEnable" => &mut outputs[0],
                    "Trigger" => &mut outputs[1],
                    _ => return Err(entry.error("unsupported watchdog output")),
                };
                for target in targets.split('|') {
                    let (controller, pin) = endpoint(entry, target)?;
                    let pin = number(entry, pin)?;
                    if plic.is_some_and(|plic| plic.name == controller) {
                        if output.plic != 0 || !(1..=sources).contains(&pin) {
                            return Err(entry.error("invalid watchdog PLIC route"));
                        }
                        output.plic = pin as usize;
                    } else {
                        let hart = hart_by_name(&cpus, entry, controller)?;
                        if !(16..64).contains(&pin) {
                            return Err(entry.error("invalid watchdog hart interrupt"));
                        }
                        machine
                            .hart_mut(hart)
                            .rv64
                            .as_mut()
                            .ok_or_else(|| entry.error("direct watchdog interrupts require an RV64 target"))?
                            .privilege
                            .add_interrupt(pin as u32);
                        output.harts.push((hart, pin as u32));
                    }
                }
            }
            let steps_per_second = 100_000_000 * cpus.len() as u64;
            machine.bus.rv64_devices.mpfs_watchdogs.push(Watchdog::new(
                base,
                frequency,
                steps_per_second,
                outputs,
            ));
            continue;
        }
        if entry.model == "SD.MPFS_SDController" && entry.registration.as_deref() != Some("none") {
            let base = entry.address()?;
            check_mapping(&machine, entry, base, rv64_mpfs_sd::Controller::SIZE)?;
            let mut source = 0;
            for (_, line) in &entry.body {
                if let Some((output, target)) = line.split_once("->")
                    && output.trim() == "IRQ"
                {
                    let (controller, pin) = endpoint(entry, target)?;
                    let pin = number(entry, pin)?;
                    if source != 0
                        || !plic.is_some_and(|plic| plic.name == controller)
                        || !(1..=sources).contains(&pin)
                    {
                        return Err(entry.error("invalid MPFS SD interrupt route"));
                    }
                    source = pin as usize;
                }
            }
            machine
                .bus
                .rv64_devices
                .mpfs_sd
                .push(rv64_mpfs_sd::Controller::new(
                    entry.name.clone(),
                    base,
                    source,
                ));
            continue;
        }
        if entry.model == "Network.CadenceGEM" && entry.registration.as_deref() != Some("none") {
            map_gem(&mut machine, entry)?;
            continue;
        }
        if entry.model == "I2C.MPFS_I2C" && entry.registration.as_deref() != Some("none") {
            let base = entry.address()?;
            check_mapping(&machine, entry, base, I2c::SIZE)?;
            let mut source = 0;
            for (_, line) in &entry.body {
                if let Some((output, target)) = line.split_once("->")
                    && output.trim() == "IRQ"
                {
                    let (controller, pin) = endpoint(entry, target)?;
                    let pin = number(entry, pin)?;
                    if source != 0
                        || !plic.is_some_and(|plic| plic.name == controller)
                        || !(1..=sources).contains(&pin)
                    {
                        return Err(entry.error("invalid MPFS I2C interrupt route"));
                    }
                    source = pin as usize;
                }
            }
            machine
                .bus
                .rv64_devices
                .mpfs_i2c
                .push(I2c::new(base, source));
            continue;
        }
        if entry.model == "SPI.MPFS_QSPI" && entry.registration.as_deref() != Some("none") {
            let base = entry.address()?;
            let size = required_number(entry, "size")?;
            if size < 0x54 || size > (1 << 32) {
                return Err(entry.error("invalid MPFS QSPI size"));
            }
            check_mapping(&machine, entry, base, size)?;
            machine
                .bus
                .rv64_devices
                .mpfs_qspi
                .push(Qspi::new(base, size));
            continue;
        }
        if entry.model == "GPIOPort.MPFS_GPIO" && entry.registration.as_deref() != Some("none") {
            let base = entry.address()?;
            check_mapping(&machine, entry, base, Gpio::SIZE)?;
            let mut gpio = Gpio::new(base, entry.name.clone());
            for button in entries.iter().filter(|button| {
                button.model == "Miscellaneous.Button"
                    && button.registration.as_deref() == Some(entry.name.as_str())
            }) {
                let wiring = button
                    .body
                    .iter()
                    .find_map(|(_, line)| line.strip_prefix("->"))
                    .ok_or_else(|| button.error("missing MPFS GPIO button wiring"))?;
                let (target, pin) = endpoint(button, wiring)?;
                let pin = number(button, pin)?;
                if target != entry.name || pin >= 32 || gpio.button.is_some() {
                    return Err(button.error("invalid MPFS GPIO button wiring"));
                }
                gpio.button = Some((
                    button.name.clone(),
                    pin as u32,
                    flag(button, "invert", false)?,
                ));
                gpio.set_button(false);
            }
            machine.bus.rv64_devices.mpfs_gpio.push(gpio);
            continue;
        }
        if entry.model == "USB.MPFS_USB" && entry.registration.as_deref() != Some("none") {
            let base = entry.address()?;
            check_mapping(&machine, entry, base, Usb::SIZE)?;
            machine.bus.rv64_devices.mpfs_usb.push(Usb::new(base));
            continue;
        }
        if entry.model == "Miscellaneous.MPFS_DDRMock"
            && entry.registration.as_deref() != Some("none")
        {
            let base = entry.address()?;
            check_mapping(&machine, entry, base, DdrMock::SIZE)?;
            machine.bus.rv64_devices.mpfs_ddr.push(DdrMock::new(base));
            continue;
        }
        if entry.model == "Miscellaneous.MPFS_Sysreg"
            && entry.registration.as_deref() != Some("none")
        {
            let base = entry.address()?;
            check_mapping(&machine, entry, base, Sysreg::SIZE)?;
            if machine
                .bus
                .rv64_devices
                .mpfs_sysreg
                .replace(Sysreg::new(base))
                .is_some()
            {
                return Err(entry.error("only one MPFS SYSREG is supported"));
            }
            continue;
        }
        if entry.model != "Python.PythonPeripheral" || entry.registration.as_deref() == Some("none")
        {
            continue;
        }
        if entry.name == "CacheConfig_WayEnable" {
            let base = entry.address()?;
            if required_number(entry, "size")? != MpfsCacheWayEnable::SIZE {
                return Err(entry.error("invalid PolarFire cache way-enable register size"));
            }
            check_mapping(&machine, entry, base, MpfsCacheWayEnable::SIZE)?;
            let way_masks = base
                .checked_add(MpfsCacheWayEnable::WAY_MASK_OFFSET)
                .filter(|&start| {
                    machine
                        .bus
                        .rv64_devices
                        .fallback_ranges
                        .iter()
                        .any(|range| {
                            range.silent
                                && range.start <= u64::from(start)
                                && u64::from(start) + MpfsCacheWayEnable::WAY_MASK_SIZE - 1
                                    <= range.end
                        })
                });
            if let Some(start) = way_masks {
                check_mapping(&machine, entry, start, MpfsCacheWayEnable::WAY_MASK_SIZE)?;
            }
            machine.bus.rv64_devices.mpfs_cache_way_enable =
                Some(MpfsCacheWayEnable::new(base, way_masks));
            continue;
        }
        if !matches!(
            entry.name.as_str(),
            "rstgen" | "audiorstgen" | "voutsysrstgen"
        ) {
            continue;
        }
        let Ok(base) = entry.address() else { continue };
        let size = required_number(entry, "size")?;
        if let Some(device) = ResetGenerator::from_repl(&entry.name, base, size) {
            check_mapping(&machine, entry, base, device.size())?;
            machine.bus.rv64_devices.reset_generators.push(device);
        }
    }

    map_ethernet(&mut machine, &entries, plic, sources)?;
    let starlight = board_name == Some("beaglev_starlight.repl")
        || entries.iter().any(|entry| {
            entry.model == "Network.SynopsysEthernetMAC"
                && entry.address().is_ok_and(|address| address == 0x1002_0000)
                && property(entry, "version") == Some("SynopsysEthernetVersion.BeagleV")
        });
    if starlight
        && let Some(gpio) = entries.iter().find(|entry| {
            entry.name == "gpio"
                && entry.model == "GPIOPort.SiFive_GPIO"
                && entry
                    .address()
                    .is_ok_and(|address| matches!(address, 0x0119_1000 | 0x1191_0000))
        })
    {
        // The upstream placeholder is a SiFive model; use the address in the guest DTB.
        check_mapping(&machine, gpio, StarLightResetPin::DOUT, 8)?;
        machine.bus.rv64_devices.starlight_reset_pin = Some(StarLightResetPin::default());
    }
    if board_name == Some("beaglev-fire.repl")
        && let Some((index, _)) = machine
            .bus
            .rv64_devices
            .mpfs_sd
            .iter()
            .enumerate()
            .find(|(_, controller)| controller.name == "mmc")
    {
        let entry = entries.iter().find(|entry| entry.name == "mmc").unwrap();
        check_mapping(&machine, entry, rv64_mpfs_sd::FIRE_MEDIA_SELECT, 4)?;
        machine.bus.rv64_devices.mpfs_sd[index].select_sd(false);
        machine.bus.rv64_devices.fire_media_controller = Some(index);
    }
    if board_name == Some("beaglev-fire.repl")
        && cpus.len() == 5
        && sources >= FireIhc::FIRST_IRQ as u64
    {
        check_mapping(&machine, cpus[0].1, FireIhc::BASE, FireIhc::SIZE)?;
        machine.bus.rv64_devices.fire_ihc = Some(Box::default());
    }
    Ok(machine)
}

fn map_gem(machine: &mut Machine, entry: &Entry) -> Result<(), String> {
    let registration = entry.registration.as_deref().unwrap_or("").trim();
    let registration = registration
        .strip_prefix('{')
        .and_then(|value| value.strip_suffix('}'))
        .unwrap_or(registration);
    let mut windows = Vec::<u32>::new();
    for window in registration
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let value = window
            .strip_prefix("sysbus")
            .filter(|rest| rest.starts_with(char::is_whitespace))
            .ok_or_else(|| entry.error("expected Cadence GEM sysbus registration"))?;
        let address = u32::try_from(number(entry, value.trim())?)
            .ok()
            .filter(|base| base % 4 == 0)
            .ok_or_else(|| entry.error("invalid Cadence GEM address"))?;
        if windows
            .iter()
            .any(|&base| u64::from(address.abs_diff(base)) < Gem::SIZE)
        {
            return Err(entry.error("overlapping Cadence GEM aliases"));
        }
        check_mapping(machine, entry, address, Gem::SIZE)?;
        windows.push(address);
    }
    let id = property(entry, "moduleId").map_or(Ok(2), |value| number(entry, value))?;
    let revision =
        property(entry, "moduleRevision").map_or(Ok(0x118), |value| number(entry, value))?;
    if windows.is_empty() || id > u64::from(u16::MAX) || revision > u64::from(u16::MAX) {
        return Err(entry.error("invalid Cadence GEM registration or module ID"));
    }
    let gems = &mut machine.bus.rv64_devices.gems;
    let index =
        u8::try_from(gems.len()).map_err(|_| entry.error("too many Cadence GEM instances"))?;
    gems.push(Gem::new(
        entry.name.clone(),
        windows,
        ((id & 0xfff) << 16 | revision) as u32,
        index,
    ));
    Ok(())
}

fn map_ethernet(
    machine: &mut Machine,
    entries: &[Entry],
    plic: Option<&Entry>,
    sources: u64,
) -> Result<(), String> {
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
        let mut source = 0;
        for (_, line) in &entry.body {
            if let Some((output, input)) = line.split_once("->")
                && output.trim() == "IRQ"
            {
                let (controller, pin) = endpoint(entry, input)?;
                let pin = number(entry, pin)?;
                if !plic.is_some_and(|plic| plic.name == controller)
                    || !(1..=sources).contains(&pin)
                    || source != 0
                {
                    return Err(entry.error("invalid or duplicate Ethernet interrupt route"));
                }
                source = pin as usize;
            }
        }
        macs.insert(entry.name.as_str(), machine.bus.rv64_devices.ethernet.len());
        machine
            .bus
            .rv64_devices
            .ethernet
            .push(Ethernet::new(base, source));
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
            if let Some(gem) = machine
                .bus
                .rv64_devices
                .gems
                .iter_mut()
                .find(|gem| gem.name == name)
            {
                let address = number(entry, address)?;
                if address >= 32 || !gem.attach(address as usize, ethernet_phy(entry)?) {
                    return Err(entry.error("invalid or duplicate Cadence GEM PHY address"));
                }
                continue;
            }
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
    Ok(Some(FallbackRange {
        start,
        end,
        value,
        silent: command == "SilenceRange",
    }))
}

fn range_end(entry: &Entry, start: u64, size: u64) -> Result<u64, String> {
    size.checked_sub(1)
        .and_then(|last| start.checked_add(last))
        .ok_or_else(|| entry.error("invalid tagged address range size"))
}

fn create_cpu(
    entry: &Entry,
    clint: Option<&Entry>,
    external_pmp: Option<&Entry>,
) -> Result<Rv64, String> {
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
    cpu.renode_cas_word_result = true;
    if let Some(value) = property(entry, "CyclesPerInstruction") {
        let cycles = u32::try_from(number(entry, value)?).ok().filter(|cycles| *cycles != 0)
            .ok_or_else(|| entry.error("CyclesPerInstruction requires a positive integer"))?;
        cpu.privilege.cycles_per_instruction = Some(cycles);
    }
    if let Some(address) = property(entry, "resetVector").or_else(|| property(entry, "ResetVector"))
    {
        cpu.set_reset_vector(number(entry, address)?);
    }
    if let Some(length) =
        property(entry, "vectorRegisterLength").or_else(|| property(entry, "VectorRegisterLength"))
    {
        if cpu.privilege.vector.is_none() {
            return Err(entry.error("vector register length requires the V extension"));
        }
        let length = number(entry, length)?;
        cpu.privilege.vector = Some(Box::new(
            crate::rv64::vector::VectorState::new(length).map_err(|error| entry.error(error))?,
        ));
    }
    let levels = match property(entry, "privilegeLevels") {
        None | Some("PrivilegeLevels.MachineSupervisorUser") => Levels::MachineSupervisorUser,
        Some("PrivilegeLevels.MachineUser") => Levels::MachineUser,
        Some("PrivilegeLevels.Machine") => Levels::Machine,
        Some(_) => return Err(entry.error("unsupported RV64 privilege levels")),
    };
    cpu.privilege.configure_levels(levels);
    if flag(entry, "renodeResetMstatus", false)? {
        cpu.privilege
            .initialize_renode_reset_status(isa.single || isa.double);
    }
    cpu.allow_unaligned = flag(entry, "allowUnalignedAccesses", false)?;
    cpu.privilege.renode_menvcfg = flag(entry, "renodeMenvcfg", false)?;
    let pmp_count =
        property(entry, "pmpEntryCount").map_or(Ok(64), |value| number(entry, value))?;
    let pmp_bits =
        property(entry, "pmpNumberOfAddrBits").map_or(Ok(54), |value| number(entry, value))?;
    if pmp_count > 64 || !(1..=54).contains(&pmp_bits) {
        return Err(entry.error("unsupported RV64 PMP configuration"));
    }
    if isa.smepmp && (pmp_count == 0 || external_pmp.is_some()) {
        return Err(entry.error("Smepmp requires internal PMP entries"));
    }
    if let Some(external) = external_pmp {
        let count = property(external, "numberOfPMPEntries")
            .map_or(Ok(64), |value| number(external, value))?;
        if !(1..=64).contains(&count) {
            return Err(external.error("unsupported RV64 external PMP entry count"));
        }
        let mut pmp = Pmp::external(count as usize, pmp_bits as u32);
        pmp.external_path = Some(format!("{}.{}", entry.name, external.name));
        cpu.privilege.pmp = Some(pmp);
    } else if pmp_count != 0 {
        let mut pmp = Pmp::new(pmp_count as usize, pmp_bits as u32);
        if isa.smepmp {
            pmp.enable_smepmp();
        }
        cpu.privilege.pmp = Some(pmp);
    }
    if entry
        .body
        .iter()
        .any(|(_, value)| value == "RegisterCustomCSR \"BPM\" 0x7C0 Machine")
    {
        cpu.privilege.bpm = Some(0);
    }
    if flag(entry, "traceStackAccess", false)? {
        cpu.stack_access_trace_name = Some(entry.name.clone());
    }
    cpu.trace_custom_csr = flag(entry, "traceCustomCsr", false)?;
    if cpu.trace_custom_csr && cpu.privilege.bpm.is_none() {
        return Err(entry.error("traceCustomCsr requires the custom BPM CSR registration"));
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
    let overlaps = overlaps
        || devices
            .mpfs_cache_way_enable
            .as_ref()
            .is_some_and(|device| {
                (start < u64::from(device.base) + MpfsCacheWayEnable::SIZE
                    && u64::from(device.base) < end)
                    || device.way_masks.is_some_and(|base| {
                        start < u64::from(base) + MpfsCacheWayEnable::WAY_MASK_SIZE
                            && u64::from(base) < end
                    })
            });
    let overlaps = overlaps
        || devices.mpfs_sysreg.as_ref().is_some_and(|device| {
            start < u64::from(device.base) + Sysreg::SIZE && u64::from(device.base) < end
        });
    let overlaps = overlaps
        || devices.mpfs_ddr.iter().any(|device| {
            start < u64::from(device.base) + DdrMock::SIZE && u64::from(device.base) < end
        });
    let overlaps = overlaps
        || devices
            .mpfs_usb
            .iter()
            .any(|usb| start < u64::from(usb.base) + Usb::SIZE && u64::from(usb.base) < end);
    let overlaps = overlaps
        || devices.mpfs_sd.iter().any(|device| {
            start < u64::from(device.base) + rv64_mpfs_sd::Controller::SIZE
                && u64::from(device.base) < end
        });
    let overlaps = overlaps
        || devices.mpfs_watchdogs.iter().any(|device| {
            start < u64::from(device.base) + Watchdog::SIZE && u64::from(device.base) < end
        });
    let overlaps = overlaps
        || devices
            .gems
            .iter()
            .flat_map(|gem| &gem.windows)
            .any(|&base| start < u64::from(base) + Gem::SIZE && u64::from(base) < end);
    let overlaps = overlaps
        || devices.mpfs_spi.iter().any(|device| {
            start < u64::from(device.base) + Spi::SIZE && u64::from(device.base) < end
        });
    let overlaps = overlaps
        || devices
            .mpfs_gpio
            .iter()
            .any(|gpio| start < u64::from(gpio.base) + Gpio::SIZE && u64::from(gpio.base) < end);
    let overlaps = overlaps
        || devices
            .mpfs_i2c
            .iter()
            .any(|i2c| start < u64::from(i2c.base) + I2c::SIZE && u64::from(i2c.base) < end)
        || devices
            .mpfs_qspi
            .iter()
            .any(|qspi| start < u64::from(qspi.base) + qspi.size && u64::from(qspi.base) < end);
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

fn registration_items(registrations: &str) -> impl Iterator<Item = &str> {
    let mut depth = 0;
    registrations
        .split(move |ch| {
            match ch {
                '{' => depth += 1,
                '}' => depth -= 1,
                _ => {}
            }
            ch == ';' && depth == 0
        })
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn typed_registration<'a>(
    entry: &Entry,
    value: &'a str,
) -> Result<Option<(bool, HashMap<&'a str, &'a str>)>, String> {
    let (point, properties) =
        if let Some(value) = value.strip_prefix("new Bus.BusPointRegistration") {
            (true, value)
        } else if let Some(value) = value.strip_prefix("new Bus.BusRangeRegistration") {
            (false, value)
        } else {
            return Ok(None);
        };
    let properties = properties
        .trim()
        .strip_prefix('{')
        .and_then(|text| text.strip_suffix('}'))
        .ok_or_else(|| entry.error("invalid typed bus registration"))?;
    let mut fields = HashMap::new();
    for field in properties
        .split(';')
        .map(str::trim)
        .filter(|field| !field.is_empty())
    {
        let (key, value) = field
            .split_once(':')
            .ok_or_else(|| entry.error("invalid bus registration property"))?;
        if !matches!(
            key.trim(),
            "address" | "size" | "offset" | "cpu" | "cluster"
        ) || fields.insert(key.trim(), value.trim()).is_some()
        {
            return Err(entry.error("unknown or duplicate bus registration property"));
        }
    }
    Ok(Some((point, fields)))
}

fn registration_address(entry: &Entry, fields: &HashMap<&str, &str>) -> Result<u64, String> {
    number(
        entry,
        fields
            .get("address")
            .ok_or_else(|| entry.error("missing bus registration address"))?,
    )
}

fn registration_harts(
    entry: &Entry,
    entries: &[Entry],
    fields: &HashMap<&str, &str>,
) -> Result<u64, String> {
    let mut cpus = entries.iter().filter(|cpu| {
        matches!(cpu.model.as_str(), "CPU.RiscV32" | "CPU.RiscV64") && cpu.registration.as_deref() != Some("none")
    }).map(|cpu| {
        let id = property(cpu, "hartId").map_or(Ok(0), |value| number(cpu, value))?;
        Ok((id, cpu))
    }).collect::<Result<Vec<_>, String>>()?;
    if cpus.len() > MAX_HARTS {
        return Err(entry.error("too many CPUs for a scoped bus registration"));
    }
    cpus.sort_unstable_by_key(|(id, _)| *id);
    let mut cpus = cpus.into_iter().map(|(_, cpu)| cpu).enumerate();
    match (fields.get("cpu"), fields.get("cluster")) {
        (None, None) => Ok(0),
        (Some(name), None) => cpus
            .find_map(|(slot, cpu)| (cpu.name == *name).then_some(1 << slot))
            .ok_or_else(|| entry.error("unknown CPU in bus registration")),
        (None, Some(name))
            if entries
                .iter()
                .any(|candidate| candidate.model == "CPU.Cluster" && candidate.name == *name) =>
        {
            let mut result = 0;
            for (slot, _) in cpus.filter(|(_, cpu)| cpu.registration.as_deref() == Some(*name)) {
                result |= 1 << slot;
            }
            if result == 0 {
                return Err(entry.error("bus registration cluster has no RV64 CPUs"));
            }
            Ok(result)
        }
        _ => Err(entry.error("invalid CPU or cluster bus registration")),
    }
}

fn uart_registration(entry: &Entry, entries: &[Entry]) -> Result<(u32, u64), String> {
    let Some(registrations) = entry
        .registration
        .as_deref()
        .and_then(|text| text.trim().strip_prefix('{'))
        .and_then(|text| text.strip_suffix('}'))
    else {
        return Ok((entry.address()?, 0));
    };
    let mut address = None;
    let mut harts = 0;
    for item in registration_items(registrations) {
        let text = item.strip_prefix("sysbus").unwrap_or("").trim();
        let Some((true, fields)) = typed_registration(entry, text)? else {
            return Err(entry.error("unsupported UART registration"));
        };
        let current = registration_address(entry, &fields)?;
        let scope = registration_harts(entry, entries, &fields)?;
        if scope == 0
            || harts & scope != 0
            || fields.contains_key("offset")
            || fields.contains_key("size")
            || address.is_some_and(|base| base != current)
        {
            return Err(
                entry.error("UART CPU registrations must be disjoint and share one address")
            );
        }
        address = Some(current);
        harts |= scope;
    }
    let base = address
        .and_then(|base| u32::try_from(base).ok())
        .ok_or_else(|| entry.error("missing or invalid CPU-scoped UART address"))?;
    Ok((base, harts))
}

fn map_memory(machine: &mut Machine, entry: &Entry, entries: &[Entry]) -> Result<(), String> {
    let size = required_number(entry, "size")?;
    let mut registrations = entry.registration.as_deref().unwrap().trim();
    if let Some(multiple) = registrations
        .strip_prefix('{')
        .and_then(|value| value.strip_suffix('}'))
    {
        registrations = multiple.trim();
    }
    let mut windows = Vec::new();
    for registration in registration_items(registrations) {
        let value = registration
            .strip_prefix("sysbus")
            .ok_or_else(|| entry.error("unsupported RAM registration"))?
            .trim();
        if let Some((point, fields)) = typed_registration(entry, value)? {
            let address = registration_address(entry, &fields)?;
            let length = if point {
                if fields.contains_key("size") {
                    return Err(entry.error("point registration cannot specify a size"));
                }
                size
            } else {
                number(
                    entry,
                    fields
                        .get("size")
                        .ok_or_else(|| entry.error("missing RAM registration size"))?,
                )?
            };
            let harts = registration_harts(entry, entries, &fields)?;
            windows.push((
                address,
                length,
                number(entry, fields.get("offset").copied().unwrap_or("0"))?,
                harts,
                fields.contains_key("cluster"),
            ));
            continue;
        }
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
        windows.push((address, length, 0, 0, false));
    }
    if windows.is_empty()
        || windows.iter().any(|&(_, length, offset, _, _)| {
            length == 0 || offset.checked_add(length).is_none_or(|end| end > size)
        })
    {
        return Err(entry.error("invalid RAM window size"));
    }
    let len = usize::try_from(size).map_err(|_| entry.error("RAM exceeds host limits"))?;
    let primary = windows
        .iter()
        .position(|&(_, _, offset, harts, _)| harts == 0 && offset == 0);
    let backing = if let Some(primary) = primary {
        let (base, length, _, _, _) = windows.remove(primary);
        let backing = machine.bus.map_ram_backing64(base, len, length as usize)?;
        if let (Ok(base), Ok(length)) = (u32::try_from(base), u32::try_from(length)) {
            machine.bus.register_traceable_memory(&entry.name, &[base], length);
        }
        Ok(backing)
    } else {
        machine.bus.allocate_ram_backing(len)
    }
    .map_err(|error| entry.error(&error))?;
    for (address, length, offset, cpu, cluster) in windows {
        machine
            .bus
            .map_alias64(
                backing,
                address,
                length as usize,
                offset as usize,
                cpu,
                cluster,
            )
            .map_err(|error| entry.error(&error))?;
    }
    Ok(())
}

fn hart_by_name(cpus: &[(usize, &Entry)], entry: &Entry, name: &str) -> Result<usize, String> {
    cpus.iter()
        .position(|&(_, cpu)| cpu.name == name)
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
