use super::{Entry, keys, number, object_fields};
use crate::{
    bus::CpuRegistrationPoint, bus_translation::PeripheralHandle, machine::Machine, nvic::Nvic,
};
use std::collections::{HashMap, HashSet};

pub(super) fn matches(entries: &[Entry]) -> bool {
    entries
        .iter()
        .filter(|entry| entry.model == "CPU.CortexM")
        .count()
        > 1
        && entries.iter().any(|entry| {
            entry.model == "IRQControllers.NVIC"
                && entry.registration.as_deref().is_some_and(|registration| {
                    registration.starts_with("sysbus new Bus.BusPointRegistration")
                })
        })
}

/// Construct the CPU and NVIC registration graph for a platform whose
/// controllers all use CPU-scoped bus points.
pub(super) fn construct(entries: &[Entry]) -> Result<Machine, String> {
    let mut names = HashSet::new();
    for entry in entries {
        if !names.insert(entry.name.as_str()) {
            return Err(entry.error("duplicate device name"));
        }
    }

    let mut machine = Machine::without_default_ram();
    let mut cpus = HashMap::new();
    // CPU construction depends on the NVIC object, but the NVIC bus
    // registration depends on the CPU. Build the objects before registering.
    let mut nvics = HashMap::new();
    for entry in entries {
        match entry.model.as_str() {
            "IRQControllers.NVIC" => {
                let body: Vec<_> = entry.body.iter().filter(|(_, text)| !text.contains("->")).cloned().collect();
                let properties = super::keys_in(entry, &body, &["priorityMask", "systickFrequency"])?;
                let mut priority = 0xff;
                for (key, value) in properties {
                    let value = number(value).map_err(|error| entry.error(&error))?;
                    match key {
                        "priorityMask" => priority = u8::try_from(value).map_err(|_| entry.error("invalid NVIC priority mask"))?,
                        "systickFrequency" if value != 0 => {}
                        _ => return Err(entry.error("invalid NVIC property")),
                    }
                }
                nvics.insert(entry.name.as_str(), (entry, Nvic::new(priority, None)));
            }
            "CPU.CortexM" | "Memory.MappedMemory" | "Memory.ArrayMemory" => {}
            _ => {
                return Err(
                    entry.error("CPU-scoped NVIC platform check does not support this model")
                );
            }
        }
    }
    for entry in entries.iter().filter(|entry| entry.model == "CPU.CortexM") {
        if entry.sysbus()?.is_some() {
            return Err(entry.error("unexpected Cortex-M bus address"));
        }
        let properties = keys(entry, &["cpuType", "nvic", "id"])?;
        let property = |key| {
            properties
                .iter()
                .find_map(|&(name, value)| (name == key).then_some(value))
        };
        let kind = property("cpuType").ok_or_else(|| entry.error("missing Cortex-M CPU type"))?;
        let kind = crate::repl_syntax::quoted_string(kind)
            .filter(|kind| matches!(kind.as_str(), "cortex-m0" | "cortex-m0+" | "cortex-m4" | "cortex-m4f" | "cortex-m55"))
            .ok_or_else(|| {
                entry.error("unsupported CPU type for CPU-scoped NVIC platform check")
            })?;
        let name = property("nvic").ok_or_else(|| entry.error("missing NVIC reference"))?;
        if !nvics.contains_key(name) {
            return Err(entry.error("unknown NVIC reference"));
        }
        let handle = PeripheralHandle::default();
        let requested = property("id").map(|value| number(value)).transpose().map_err(|error| entry.error(&error))?;
        let slot = machine
            .bus
            .register_cpu_identity(&handle, &kind, Some(CpuRegistrationPoint { slot: requested.map(|slot| slot as usize) }))
            .map_err(|e| entry.error(&format!("CPU registration failed: {e:?}")))?;
        if slot >= crate::machine::MAX_HARTS {
            return Err(entry.error("CPU-scoped slot exceeds the machine limit"));
        }
        if machine.bus.guest_cpu_names.len() <= slot {
            machine.bus.guest_cpu_names.resize(slot + 1, String::new());
        }
        machine.bus.guest_cpu_names[slot] = entry.name.clone();
        cpus.insert(entry.name.as_str(), (slot, name));
    }
    let scope_count = nvics.len();
    for (name, (entry, controller)) in nvics {
        let body = entry
            .registration
            .as_deref()
            .and_then(|value| value.strip_prefix("sysbus new Bus.BusPointRegistration"))
            .and_then(|value| value.trim().strip_prefix('{'))
            .and_then(|value| value.strip_suffix('}'))
            .ok_or_else(|| entry.error("expected a CPU-scoped NVIC bus point"))?;
        let mut address = None;
        let mut cpu = None;
        for (key, value) in object_fields(body).map_err(|e| entry.error(e))? {
            match key {
                "address" if address.is_none() => {
                    address = Some(number(value).map_err(|e| entry.error(&e))?)
                }
                "cpu" if cpu.is_none() => cpu = Some(value),
                _ => return Err(entry.error("invalid or duplicate NVIC bus point field")),
            }
        }
        let address = address.ok_or_else(|| entry.error("missing NVIC bus point address"))?;
        if address != crate::nvic::BASE {
            return Err(entry.error("invalid NVIC address"));
        }
        let (slot, referenced) = cpu
            .and_then(|cpu| cpus.get(cpu))
            .ok_or_else(|| entry.error("unknown NVIC registration CPU"))?;
        if *referenced != name {
            return Err(entry.error("conflicting NVIC registration for Cortex-M CPU"));
        }
        machine
            .bus
            .register_scoped_nvic(*slot, address, controller)
            .map_err(|error| entry.error(&error))?;
    }
    if scope_count != cpus.len() {
        return Err("expected one CPU-scoped NVIC per Cortex-M CPU".into());
    }
    for entry in entries.iter().filter(|entry| matches!(entry.model.as_str(), "Memory.MappedMemory" | "Memory.ArrayMemory")) {
        let properties = keys(entry, &["size"])?;
        let [("size", size)] = properties.as_slice() else { return Err(entry.error("scoped memory requires a size")); };
        let size = number(size).map_err(|error| entry.error(&error))?;
        let registration = entry.registration.as_deref().ok_or_else(|| entry.error("scoped memory needs a bus registration"))?;
        let registration = registration.trim().strip_prefix('{')
            .and_then(|registration| registration.strip_suffix('}')).map(str::trim).unwrap_or(registration);
        let registration = registration.strip_prefix("sysbus new Bus.BusPointRegistration")
            .and_then(|fields| fields.trim().strip_prefix('{')).and_then(|fields| fields.strip_suffix('}'))
            .ok_or_else(|| entry.error("expected a CPU-scoped memory bus point"))?;
        let mut address = None;
        let mut cpu = None;
        for (key, value) in object_fields(registration).map_err(|error| entry.error(error))? {
            match key {
                "address" if address.is_none() => {
                    address = Some(crate::parse_address64(value).map_err(|error| entry.error(&error))?);
                }
                "cpu" if cpu.is_none() => cpu = cpus.get(value).map(|(slot, _)| *slot),
                _ => return Err(entry.error("invalid or duplicate scoped memory property")),
            }
        }
        machine.bus.map_named_cpu_ram(&entry.name, cpu.ok_or_else(|| entry.error("unknown scoped memory CPU"))?,
            address.ok_or_else(|| entry.error("missing scoped memory address"))?, size)
            .map_err(|error| entry.error(&error))?;
    }
    Ok(machine)
}
