use super::{Entry, keys, map_pl011, multi_registration, number, object_fields};
use crate::{
    arm_gic::{Gic, Version, Window, WindowKind},
    arm32::{Cpu, Profile},
    machine::Machine,
};

pub(super) fn matches(entries: &[Entry]) -> bool {
    entries
        .iter()
        .any(|entry| matches!(entry.model.as_str(), "CPU.ARMv7A" | "CPU.ARMv7R"))
}

pub(super) fn construct(entries: &[Entry], require_ram: bool) -> Result<Machine, String> {
    let mut machine = Machine::without_default_ram();
    let cpus: Vec<_> = entries
        .iter()
        .filter(|entry| matches!(entry.model.as_str(), "CPU.ARMv7A" | "CPU.ARMv7R"))
        .collect();
    if cpus.len() > crate::machine::MAX_HARTS {
        return Err("too many ARMv7 CPUs".into());
    }
    machine.configure_harts(cpus.len());
    let controller = entries
        .iter()
        .find(|entry| entry.model == "IRQControllers.ARM_GenericInterruptController");
    let mut affinities = Vec::new();
    for (hart, entry) in cpus.iter().enumerate() {
        if entry.registration.as_deref() != Some("sysbus") {
            return Err(entry.error("ARMv7 CPU must be registered on sysbus"));
        }
        let fields = keys(
            entry,
            &[
                "cpuType",
                "numberOfMPURegions",
                "cpuId",
                "genericInterruptController",
            ],
        )?;
        let mut affinity = hart as u32;
        for &(key, value) in &fields {
            match key {
                "cpuId" => affinity = number(value).map_err(|error| entry.error(&error))?,
                "genericInterruptController"
                    if !controller.is_some_and(|gic| gic.name == value) =>
                {
                    return Err(entry.error("unknown ARM interrupt controller"));
                }
                _ => {}
            }
        }
        if affinities.contains(&affinity) {
            return Err(entry.error("duplicate ARM CPU affinity"));
        }
        affinities.push(affinity);
        let model = fields
            .iter()
            .find(|(key, _)| *key == "cpuType")
            .and_then(|(_, value)| crate::repl_syntax::quoted_string(value))
            .ok_or_else(|| entry.error("missing quoted ARMv7 cpuType"))?;
        let profile = match (entry.model.as_str(), model.as_str()) {
            ("CPU.ARMv7A", "cortex-a7" | "cortex-a9" | "cortex-a15") => Profile::Application,
            ("CPU.ARMv7R", "cortex-r4" | "cortex-r5" | "cortex-r7" | "cortex-r8") => {
                Profile::RealTime
            }
            _ => return Err(entry.error("unsupported ARMv7 CPU type")),
        };
        let cpu = machine.hart_mut(hart);
        cpu.name = Some(entry.name.clone());
        cpu.arm32 = Some(Box::new(Cpu::new(profile)));
        machine.bus.guest_cpu_names.push(entry.name.clone());
    }
    let mut mapped = false;
    for entry in entries {
        match entry.model.as_str() {
            "CPU.ARMv7A" | "CPU.ARMv7R" => {}
            "Memory.MappedMemory" | "Memory.ArrayMemory" => {
                map_memory(&mut machine, entry)?;
                mapped = true;
            }
            "UART.PL011" => map_pl011(entry, controller, &mut machine.bus)?,
            "IRQControllers.ARM_GenericInterruptController" => {
                map_gic(&mut machine, entry, &affinities)?
            }
            _ => return Err(entry.error("unsupported ARMv7 platform peripheral")),
        }
    }
    if require_ram && !mapped {
        return Err("ARMv7 platform has no mapped RAM".into());
    }
    Ok(machine)
}

fn map_gic(machine: &mut Machine, entry: &Entry, affinities: &[u32]) -> Result<(), String> {
    let mut version = Version::V2;
    let mut shared = 960;
    for (key, value) in keys(
        entry,
        &[
            "architectureVersion",
            "sharedPeripheralCount",
            "supportsTwoSecurityStates",
        ],
    )? {
        match (key, value) {
            ("architectureVersion", ".GICv1" | ".GICv2") => version = Version::V2,
            ("architectureVersion", ".GICv3") => version = Version::V3,
            ("sharedPeripheralCount", value) => {
                shared = number(value).map_err(|error| entry.error(&error))? as usize
            }
            ("supportsTwoSecurityStates", "false") => {}
            _ => return Err(entry.error("unsupported ARM GIC configuration")),
        }
    }
    let registrations = entry
        .registration_list()
        .ok_or_else(|| entry.error("ARM GIC requires bus registrations"))?;
    let mut windows = Vec::new();
    for registration in registrations {
        if let Some((base, size, region)) = multi_registration(entry, registration)? {
            let kind = match region {
                "distributor" => WindowKind::Distributor,
                "cpuInterface" => WindowKind::CpuInterface,
                _ => return Err(entry.error("unknown ARM GIC bus region")),
            };
            windows.push(Window { base, size, kind });
            continue;
        }
        let body = registration
            .strip_prefix("sysbus new IRQControllers.ArmGicRedistributorRegistration")
            .and_then(|properties| properties.trim().strip_prefix('{'))
            .and_then(|properties| properties.strip_suffix('}'))
            .ok_or_else(|| entry.error("unsupported ARM GIC registration"))?;
        let mut base = None;
        let mut cpu = None;
        for (key, value) in object_fields(body).map_err(|error| entry.error(error))? {
            match key {
                "address" if base.is_none() => {
                    base = Some(number(value).map_err(|error| entry.error(&error))?)
                }
                "attachedCPU" if cpu.is_none() => {
                    cpu = Some(
                        machine
                            .bus
                            .scoped_cpu_index(value)
                            .map_err(|error| entry.error(&error))?,
                    )
                }
                _ => return Err(entry.error("invalid ARM GIC redistributor property")),
            }
        }
        let (Some(base), Some(cpu)) = (base, cpu) else {
            return Err(entry.error("ARM GIC redistributor requires an address and CPU"));
        };
        windows.push(Window {
            base,
            size: 0x2_0000,
            kind: WindowKind::Redistributor(cpu),
        });
    }
    let gic = Gic::new(version, shared, affinities).map_err(|error| entry.error(&error))?;
    machine
        .bus
        .map_arm_gic(gic, windows)
        .map_err(|error| entry.error(&error))
}

fn map_memory(machine: &mut Machine, entry: &Entry) -> Result<(), String> {
    let fields = keys(entry, &["size"])?;
    let [("size", value)] = fields.as_slice() else {
        return Err(entry.error("ARMv7 memory requires a size"));
    };
    let size = crate::parse_address64(value).map_err(|error| entry.error(&error))?;
    let size =
        usize::try_from(size).map_err(|_| entry.error("ARMv7 memory exceeds host capacity"))?;
    let registrations = entry
        .registration_list()
        .or_else(|| entry.registration.as_deref().map(|point| vec![point]))
        .ok_or_else(|| entry.error("ARMv7 memory requires a sysbus registration"))?;
    let mut windows = Vec::new();
    for registration in registrations {
        let point = registration
            .strip_prefix("sysbus")
            .ok_or_else(|| entry.error("expected a sysbus registration"))?
            .trim();
        if let Some(fields) = point
            .strip_prefix("new Bus.BusPointRegistration")
            .and_then(|fields| fields.trim().strip_prefix('{'))
            .and_then(|fields| fields.strip_suffix('}'))
        {
            let mut address = None;
            let mut hart = None;
            for (key, value) in object_fields(fields).map_err(|error| entry.error(error))? {
                match key {
                    "address" if address.is_none() => {
                        address = Some(
                            crate::parse_address64(value).map_err(|error| entry.error(&error))?,
                        )
                    }
                    "cpu" if hart.is_none() => {
                        hart = Some(
                            machine
                                .bus
                                .scoped_cpu_index(value)
                                .map_err(|error| entry.error(&error))?,
                        )
                    }
                    _ => return Err(entry.error("invalid ARMv7 bus registration field")),
                }
            }
            windows.push((
                address.ok_or_else(|| entry.error("missing address"))?,
                hart.map_or(0, |hart| 1 << hart),
            ));
        } else {
            windows.push((
                crate::parse_address64(point).map_err(|error| entry.error(&error))?,
                0,
            ));
        }
    }
    if windows.is_empty() || size == 0 {
        return Err(entry.error("empty ARMv7 memory registration"));
    }
    let primary = windows.iter().position(|(_, mask)| *mask == 0);
    let backing = if let Some(index) = primary {
        let (base, _) = windows.remove(index);
        if let (Ok(address), Ok(length)) = (u32::try_from(base), u32::try_from(size)) {
            machine
                .bus
                .register_traceable_memory(&entry.name, &[address], length);
        }
        machine.bus.map_ram_backing64(base, size, size)
    } else {
        machine.bus.allocate_ram_backing(size)
    }
    .map_err(|error| entry.error(&error))?;
    for (address, mask) in windows {
        machine
            .bus
            .map_alias64(backing, address, size, 0, mask, false)
            .map_err(|error| entry.error(&error))?;
    }
    Ok(())
}
