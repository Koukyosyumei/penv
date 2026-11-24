use super::*;

const MOCK_CPU: &str = "Antmicro.Renode.UnitTests.Mocks.MockCPU";

#[derive(Default)]
pub(crate) struct Cpu {
    pub(crate) placeholder: Option<String>,
    other_cpu: Option<usize>,
}

/// The CPU objects and sysbus registrations created from a platform description.
pub(crate) struct CpuPlatform {
    cpus: Vec<Cpu>,
    registered: HashMap<String, usize>,
}

impl CpuPlatform {
    pub(crate) fn cpu(&self, qualified_name: &str) -> Option<&Cpu> {
        self.registered
            .get(qualified_name)
            .map(|&id| &self.cpus[id])
    }
}

pub(crate) fn create_cpus(content: &str, root: &Path) -> Result<CpuPlatform, String> {
    let entries = inline_entries(content, root)?;
    let mut globals = HashMap::new();
    let mut locals = HashMap::new();
    let mut targets = Vec::with_capacity(entries.len());
    let mut platform = CpuPlatform {
        cpus: Vec::new(),
        registered: HashMap::new(),
    };
    for entry in &entries {
        if entry.model.is_empty() {
            targets.push(None);
            continue;
        }
        if entry.model != MOCK_CPU {
            return Err(entry.error("unsupported CPU model"));
        }
        let id = platform.cpus.len();
        let previous = if entry.local {
            // An include reached through more than one path refers to the same local scope.
            locals.insert((entry.source.as_path(), entry.name.as_str()), id)
        } else {
            globals.insert(entry.name.as_str(), id)
        };
        if previous.is_some() {
            return Err(entry.error("VariableAlreadyDeclared"));
        }
        platform.cpus.push(Cpu::default());
        targets.push(Some(id));
    }
    let resolve = |scope: &Path, name: &str| {
        locals
            .get(&(scope, name))
            .or_else(|| globals.get(name))
            .copied()
    };

    for (entry, declared) in entries.iter().zip(targets) {
        let id = declared
            .or_else(|| resolve(&entry.source, &entry.name))
            .ok_or_else(|| entry.error("MissingReference: no declaration for overlay"))?;
        let cpu = &mut platform.cpus[id];
        if entry.registration.is_some() {
            entry.sysbus()?;
            let name = format!("sysbus.{}", entry.alias.as_deref().unwrap_or(&entry.name));
            if platform.registered.insert(name, id).is_some() {
                return Err(entry.error("duplicate sysbus registration"));
            }
        }
        for (source, property) in &entry.body {
            let (name, value) = property
                .split_once(':')
                .ok_or_else(|| format!("{source}: expected CPU property"))?;
            let value = value.trim();
            match name.trim() {
                "Placeholder" => {
                    cpu.placeholder =
                        Some(quoted_value(value).map_err(|error| format!("{source}: {error}"))?)
                }
                "OtherCpu" => {
                    cpu.other_cpu = Some(
                        resolve(&entry.source, value)
                            .ok_or_else(|| format!("{source}: MissingReference: {value}"))?,
                    )
                }
                name => return Err(format!("{source}: unknown CPU property {name}")),
            }
        }
    }
    Ok(platform)
}
