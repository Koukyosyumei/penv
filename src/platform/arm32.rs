use super::{Entry, keys, object_fields};
use crate::{
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
    for (hart, entry) in cpus.iter().enumerate() {
        if entry.registration.as_deref() != Some("sysbus") {
            return Err(entry.error("ARMv7 CPU must be registered on sysbus"));
        }
        let fields = keys(entry, &["cpuType", "numberOfMPURegions", "cpuId"])?;
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
            _ => return Err(entry.error("unsupported ARMv7 platform peripheral")),
        }
    }
    if require_ram && !mapped {
        return Err("ARMv7 platform has no mapped RAM".into());
    }
    Ok(machine)
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
