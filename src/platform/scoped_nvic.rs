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
                keys(entry, &[])?;
                nvics.insert(entry.name.as_str(), (entry, Nvic::new(0xff, None)));
            }
            "CPU.CortexM" => {}
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
        let properties = keys(entry, &["cpuType", "nvic"])?;
        let property = |key| {
            properties
                .iter()
                .find_map(|&(name, value)| (name == key).then_some(value))
        };
        let kind = property("cpuType").ok_or_else(|| entry.error("missing Cortex-M CPU type"))?;
        let kind = crate::repl_syntax::quoted_string(kind)
            .filter(|kind| matches!(kind.as_str(), "cortex-m0" | "cortex-m0+"))
            .ok_or_else(|| {
                entry.error("unsupported CPU type for CPU-scoped NVIC platform check")
            })?;
        let name = property("nvic").ok_or_else(|| entry.error("missing NVIC reference"))?;
        if !nvics.contains_key(name) {
            return Err(entry.error("unknown NVIC reference"));
        }
        let handle = PeripheralHandle::default();
        let slot = machine
            .bus
            .register_cpu_identity(&handle, &kind, Some(CpuRegistrationPoint::default()))
            .map_err(|e| entry.error(&format!("CPU registration failed: {e:?}")))?;
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
    Ok(machine)
}
