use super::*;

const MOCK_CPU: &str = "Antmicro.Renode.UnitTests.Mocks.MockCPU";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Model {
    Cpu,
    EmptyPeripheral,
    External,
}

impl Model {
    fn parse(name: &str) -> Option<Self> {
        match name {
            MOCK_CPU => Some(Self::Cpu),
            "Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral" => Some(Self::EmptyPeripheral),
            "Antmicro.Renode.UnitTests.Mocks.MockExternal" => Some(Self::External),
            _ => None,
        }
    }
}

#[derive(Default)]
pub(crate) struct Cpu {
    pub(crate) placeholder: Option<String>,
    other_cpu: Option<usize>,
    enum_value: Option<TwoState>,
}

enum TwoState {
    One,
    Two,
}

/// Objects and sysbus registrations created from a platform description.
pub(crate) struct CreatedPlatform {
    objects: Vec<CreatedObject>,
    registered: HashMap<String, usize>,
}

struct CreatedObject {
    model: Model,
    cpu: Option<Cpu>,
    range: Option<(u64, u64)>,
}

impl CreatedPlatform {
    pub(crate) fn cpu(&self, qualified_name: &str) -> Option<&Cpu> {
        self.registered
            .get(qualified_name)
            .and_then(|&id| self.objects[id].cpu.as_ref())
    }
}

pub(crate) fn create_objects(content: &str, root: &Path) -> Result<CreatedPlatform, String> {
    let entries = inline_entries(content, root)?;
    let mut globals = HashMap::new();
    let mut locals = HashMap::new();
    let mut targets = Vec::with_capacity(entries.len());
    let mut platform = CreatedPlatform {
        objects: Vec::new(),
        registered: HashMap::new(),
    };
    for entry in &entries {
        validate_entry_properties(&entry.body, |source, name| {
            format!("{source}: PropertyOrCtorNameUsedMoreThanOnce: {name}")
        })?;
        if entry.model.is_empty() {
            if entry.registration.is_none() && entry.body.is_empty() {
                return Err(entry.error("EmptyEntry: no type, registration, or properties"));
            }
            targets.push(None);
            continue;
        }
        let model = Model::parse(&entry.model)
            .ok_or_else(|| entry.error(&format!("TypeNotResolved: {}", entry.model)))?;
        let id = platform.objects.len();
        let previous = if entry.local {
            // An include reached through more than one path refers to the same local scope.
            locals.insert((entry.source.as_path(), entry.name.as_str()), id)
        } else {
            globals.insert(entry.name.as_str(), id)
        };
        if previous.is_some() {
            return Err(entry.error("VariableAlreadyDeclared: repeated type declaration"));
        }
        platform.objects.push(CreatedObject {
            model,
            cpu: (model == Model::Cpu).then(Cpu::default),
            range: None,
        });
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
        if let Some(registration) = &entry.registration {
            if platform.objects[id].model == Model::External {
                return Err(entry.error("NoUsableRegisterInterface: external object"));
            }
            let (parent, point) =
                registration_point(registration).map_err(|error| entry.error(error))?;
            if parent != "sysbus" {
                return Err(entry.error("NoUsableRegisterInterface: expected sysbus"));
            }
            match point {
                Some(RegistrationValue::Range { base, size }) => {
                    platform.objects[id].range = Some((base, size));
                }
                Some(RegistrationValue::Other(value)) if number(value).is_ok() => {}
                Some(RegistrationValue::Other(value)) => {
                    let target = resolve(&entry.source, value)
                        .ok_or_else(|| entry.error(&format!("MissingReference: {value}")))?;
                    let model = match platform.objects[target].model {
                        Model::Cpu => "CPU",
                        Model::EmptyPeripheral => "peripheral",
                        Model::External => "external",
                    };
                    return Err(entry.error(&format!(
                        "TypeMismatch: {model} is not a registration point"
                    )));
                }
                Some(RegistrationValue::String(_)) => {
                    return Err(entry.error("NoCtorForRegistrationPoint: string value"));
                }
                None => {}
            }
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
            let target = if name.trim() == "OtherCpu" && platform.objects[id].model == Model::Cpu {
                let target = resolve(&entry.source, value)
                    .ok_or_else(|| format!("{source}: MissingReference: {value}"))?;
                if platform.objects[target].model != Model::Cpu {
                    return Err(format!("{source}: TypeMismatch: OtherCpu requires a CPU"));
                }
                Some(target)
            } else {
                None
            };
            let Some(cpu) = &mut platform.objects[id].cpu else {
                return Err(format!("{source}: PropertyDoesNotExist: {name}"));
            };
            match name.trim() {
                "Placeholder" => {
                    cpu.placeholder = Some(
                        quoted_value(value)
                            .map_err(|error| format!("{source}: TypeMismatch: {error}"))?,
                    )
                }
                "OtherCpu" => cpu.other_cpu = target,
                "Model" => return Err(format!("{source}: PropertyNotWritable: Model")),
                "EnumValue" => {
                    cpu.enum_value = Some(match symbolic_value(value) {
                        Some(
                            SymbolicValue::Reference("One")
                            | SymbolicValue::Enum(Some("TwoStateEnum"), "One"),
                        ) => TwoState::One,
                        Some(
                            SymbolicValue::Reference("Two")
                            | SymbolicValue::Enum(Some("TwoStateEnum"), "Two"),
                        ) => TwoState::Two,
                        _ => return Err(format!("{source}: TypeMismatch: expected TwoStateEnum")),
                    });
                }
                name => return Err(format!("{source}: PropertyDoesNotExist: {name}")),
            }
        }
    }
    Ok(platform)
}
