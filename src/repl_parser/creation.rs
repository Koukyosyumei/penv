use super::*;

const MOCK_CPU: &str = "Antmicro.Renode.UnitTests.Mocks.MockCPU";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Model {
    Cpu,
    EmptyPeripheral,
    EnumPeripheral,
    CollectionPeripheral,
    ScalarPeripheral(ScalarKind),
    External,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ScalarKind {
    Integer,
    String,
    Range,
    Object,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ScalarValue {
    Integer(i32),
    String(Option<String>),
    Range(u64, u64),
    Object(Option<usize>),
}

impl ScalarKind {
    fn bind(
        self,
        name: &str,
        value: &str,
        resolve: impl Fn(&str) -> Option<usize>,
    ) -> Option<ScalarValue> {
        match (self, name, value) {
            (Self::Integer, "mockInt", "empty") => Some(ScalarValue::Integer(0)),
            (Self::Integer, "mockInt", value) => value.parse().ok().map(ScalarValue::Integer),
            (Self::String, "mockString", "empty") => Some(ScalarValue::String(None)),
            (Self::String, "mockString", value) => quoted_value(value)
                .ok()
                .map(|s| ScalarValue::String(Some(s))),
            (Self::Range, "mockRange", "empty") => Some(ScalarValue::Range(0, 0)),
            (Self::Range, "mockRange", value) => range_value(value)
                .ok()
                .map(|(base, size)| ScalarValue::Range(base, size)),
            (Self::Object, "mockObject", "empty") => Some(ScalarValue::Object(None)),
            (Self::Object, "mockObject", value) => {
                resolve(value).map(|id| ScalarValue::Object(Some(id)))
            }
            _ => None,
        }
    }
}

impl Model {
    fn parse(name: &str) -> Option<Self> {
        match name {
            "Antmicro.Renode.Tests.UnitTests.Mocks.MockPeripheralWithEnumAttribute" => {
                Some(Self::EnumPeripheral)
            }
            "Antmicro.Renode.Tests.UnitTests.Mocks.MockPeripheralWithObjectAttribute" => {
                Some(Self::ScalarPeripheral(ScalarKind::Object))
            }
            "Antmicro.Renode.Tests.UnitTests.Mocks.MockPeripheralWithRangeAttribute" => {
                Some(Self::ScalarPeripheral(ScalarKind::Range))
            }
            "Antmicro.Renode.Tests.UnitTests.Mocks.MockPeripheralWithStringAttribute" => {
                Some(Self::ScalarPeripheral(ScalarKind::String))
            }
            "Antmicro.Renode.Tests.UnitTests.Mocks.MockPeripheralWithNumericalAttrubute" => {
                Some(Self::ScalarPeripheral(ScalarKind::Integer))
            }
            "Antmicro.Renode.Tests.UnitTests.Mocks.MockPeripheralWithCollectionAttributes" => {
                Some(Self::CollectionPeripheral)
            }
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
    pub(crate) enum_value: TwoState,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum TwoState {
    #[default]
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
    ranges: Vec<(u64, u64)>,
    enum_arguments: EnumArguments,
    scalar: Option<ScalarValue>,
    collections: Collections,
}

struct EnumArguments {
    strict: u8,
    any_numeric: u8,
}

impl Default for EnumArguments {
    fn default() -> Self {
        Self {
            strict: 1,
            any_numeric: 1,
        }
    }
}

impl EnumArguments {
    fn bind(&mut self, name: &str, value: &str, source: &SourceLine) -> Result<(), String> {
        let (type_name, argument, allow_any_numeric) = match name {
            "mockEnum" => ("MockEnum", &mut self.strict, false),
            "mockEnumWithAttribute" => ("MockEnumWithAttribute", &mut self.any_numeric, true),
            _ => return Err(format!("{source}: PropertyDoesNotExist: {name}")),
        };
        *argument = match symbolic_value(value) {
            Some(SymbolicValue::Enum(qualifier, "ValidValue"))
                if qualifier.is_none() || qualifier == Some(type_name) =>
            {
                1
            }
            _ => number(value)
                .ok()
                .and_then(|n| u8::try_from(n).ok())
                .filter(|&n| allow_any_numeric || n == 1)
                .ok_or_else(|| {
                    format!("{source}: NoCtor: {value} is not a valid {type_name} argument")
                })?,
        };
        Ok(())
    }
}

#[derive(Default)]
pub(crate) struct Collections {
    pub(crate) integers: Option<Vec<i32>>,
    pub(crate) array: Option<Vec<i32>>,
    pub(crate) strings: Option<Vec<String>>,
    pub(crate) dictionary: Option<std::collections::BTreeMap<i32, String>>,
}

fn bind_list<T>(
    value: &str,
    convert: impl Fn(ListItem<'_>) -> Option<T>,
) -> Result<Option<Vec<T>>, String> {
    if value == "empty" {
        return Ok(None);
    }
    list_value(value)
        .map_err(str::to_owned)?
        .into_iter()
        .map(|item| {
            convert(item).ok_or_else(|| "TypeMismatch: incompatible collection element".into())
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

fn signed_integer(item: ListItem<'_>) -> Option<i32> {
    let ListItem::Number(value) = item else {
        return None;
    };
    value.parse().ok()
}

impl Collections {
    fn bind(&mut self, name: &str, value: &str) -> Result<(), String> {
        match name {
            "mockIntStringDict" => {
                self.dictionary = if value == "empty" {
                    None
                } else {
                    let mut items = std::collections::BTreeMap::new();
                    for (key, value) in dictionary_value(value)? {
                        let key = key
                            .parse()
                            .map_err(|_| "TypeMismatch: expected integer key")?;
                        let value = quoted_value(value)
                            .map_err(|_| "TypeMismatch: expected string value")?;
                        if items.insert(key, value).is_some() {
                            return Err("An item with the same key has already been added.".into());
                        }
                    }
                    Some(items)
                };
            }
            "mockIntList" => self.integers = bind_list(value, signed_integer)?,
            "mockIntArray" => self.array = bind_list(value, signed_integer)?,
            "mockStringList" => {
                self.strings = bind_list(value, |item| match item {
                    ListItem::String(value) => Some(value),
                    _ => None,
                })?
            }
            _ => return Err(format!("PropertyDoesNotExist: {name}")),
        }
        Ok(())
    }
}

impl CreatedPlatform {
    #[cfg(test)]
    pub(crate) fn collections(&self, qualified_name: &str) -> Option<&Collections> {
        let object = &self.objects[*self.registered.get(qualified_name)?];
        (object.model == Model::CollectionPeripheral).then_some(&object.collections)
    }

    #[cfg(test)]
    pub(crate) fn scalar(&self, qualified_name: &str) -> Option<&ScalarValue> {
        self.objects[*self.registered.get(qualified_name)?]
            .scalar
            .as_ref()
    }

    #[cfg(test)]
    pub(crate) fn enum_values(&self, qualified_name: &str) -> Option<(u8, u8)> {
        let object = &self.objects[*self.registered.get(qualified_name)?];
        (object.model == Model::EnumPeripheral).then_some((
            object.enum_arguments.strict,
            object.enum_arguments.any_numeric,
        ))
    }

    #[cfg(test)]
    pub(crate) fn peripheral_ranges(&self, qualified_name: &str) -> Option<&[(u64, u64)]> {
        let object = &self.objects[*self.registered.get(qualified_name)?];
        (object.model == Model::EmptyPeripheral).then_some(object.ranges.as_slice())
    }

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
            ranges: Vec::new(),
            enum_arguments: EnumArguments::default(),
            scalar: None,
            collections: Collections::default(),
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
            let points = if let Some(list) = registration
                .strip_prefix('{')
                .and_then(|list| list.strip_suffix('}'))
            {
                registration_points(list).map_err(|error| entry.error(error))?
            } else {
                vec![registration_point(registration).map_err(|error| entry.error(error))?]
            };
            for (parent, point) in points {
                if parent != "sysbus" {
                    return Err(entry.error("NoUsableRegisterInterface: expected sysbus"));
                }
                match point {
                    Some(RegistrationValue::Range { base, size }) => {
                        platform.objects[id].ranges.push((base, size));
                    }
                    Some(RegistrationValue::Other(value)) if number(value).is_ok() => {}
                    Some(RegistrationValue::Other(value)) if value.starts_with("new ") => {
                        let (object, _) =
                            object_prefix(value).map_err(|error| entry.error(error))?;
                        Model::parse(object.model).ok_or_else(|| {
                            entry.error(&format!("TypeNotResolved: {}", object.model))
                        })?;
                        return Err(
                            entry.error("TypeMismatch: inline object is not a registration point")
                        );
                    }
                    Some(RegistrationValue::Other(value)) => {
                        let target = resolve(&entry.source, value)
                            .ok_or_else(|| entry.error(&format!("MissingReference: {value}")))?;
                        let model = match platform.objects[target].model {
                            Model::Cpu => "CPU",
                            Model::EmptyPeripheral
                            | Model::EnumPeripheral
                            | Model::ScalarPeripheral(_)
                            | Model::CollectionPeripheral => "peripheral",
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
            platform.set_property(
                id,
                name.trim(),
                value,
                source,
                |name| resolve(&entry.source, name),
                0,
            )?;
        }
    }
    for (entry, object) in entries
        .iter()
        .filter(|entry| !entry.model.is_empty())
        .zip(&platform.objects)
    {
        if matches!(object.model, Model::ScalarPeripheral(_)) && object.scalar.is_none() {
            return Err(entry.error("NoCtor: missing required scalar constructor argument"));
        }
    }
    Ok(platform)
}

impl CreatedPlatform {
    fn set_property(
        &mut self,
        id: usize,
        name: &str,
        value: &str,
        source: &SourceLine,
        resolve: impl Fn(&str) -> Option<usize> + Copy,
        depth: usize,
    ) -> Result<(), String> {
        if depth >= 64 {
            return Err(format!("{source}: inline object nesting limit exceeded"));
        }
        if self.objects[id].model == Model::CollectionPeripheral {
            return self.objects[id]
                .collections
                .bind(name, value)
                .map_err(|error| format!("{source}: {error}"));
        }
        if self.objects[id].model == Model::EnumPeripheral {
            return self.objects[id].enum_arguments.bind(name, value, source);
        }
        if let Model::ScalarPeripheral(kind) = self.objects[id].model {
            self.objects[id].scalar = Some(kind.bind(name, value, resolve).ok_or_else(|| {
                format!("{source}: NoCtor: invalid scalar constructor argument {name}")
            })?);
            return Ok(());
        }
        let target = if name == "OtherCpu" && self.objects[id].model == Model::Cpu {
            Some(if value.starts_with("new ") {
                let (object, tail) =
                    object_prefix(value).map_err(|error| format!("{source}: {error}"))?;
                if !tail.trim().is_empty() {
                    return Err(format!("{source}: unexpected text after inline object"));
                }
                let model = Model::parse(object.model)
                    .ok_or_else(|| format!("{source}: TypeNotResolved: {}", object.model))?;
                if model != Model::Cpu {
                    return Err(format!("{source}: TypeMismatch: OtherCpu requires a CPU"));
                }
                let target = self.objects.len();
                self.objects.push(CreatedObject {
                    model,
                    cpu: Some(Cpu::default()),
                    ranges: Vec::new(),
                    enum_arguments: EnumArguments::default(),
                    scalar: None,
                    collections: Collections::default(),
                });
                let mut seen = HashSet::new();
                for (key, value) in object.fields {
                    if !seen.insert(key) {
                        return Err(format!(
                            "{source}: PropertyOrCtorNameUsedMoreThanOnce: {key}"
                        ));
                    }
                    self.set_property(target, key, value, source, resolve, depth + 1)?;
                }
                target
            } else {
                let target =
                    resolve(value).ok_or_else(|| format!("{source}: MissingReference: {value}"))?;
                if self.objects[target].model != Model::Cpu {
                    return Err(format!("{source}: TypeMismatch: OtherCpu requires a CPU"));
                }
                target
            })
        } else {
            None
        };
        let Some(cpu) = &mut self.objects[id].cpu else {
            return Err(format!("{source}: PropertyDoesNotExist: {name}"));
        };
        match name {
            "Placeholder" => {
                cpu.placeholder = Some(
                    crate::platform::multiline_value(value)
                        .map(Ok)
                        .unwrap_or_else(|| quoted_value(value))
                        .map_err(|error| format!("{source}: TypeMismatch: {error}"))?,
                )
            }
            "OtherCpu" => cpu.other_cpu = target,
            "Model" => return Err(format!("{source}: PropertyNotWritable: Model")),
            "EnumValue" => {
                cpu.enum_value = match symbolic_value(value) {
                    Some(
                        SymbolicValue::Reference("One" | "none" | "empty")
                        | SymbolicValue::Enum(Some("TwoStateEnum"), "One"),
                    ) => TwoState::One,
                    Some(
                        SymbolicValue::Reference("Two")
                        | SymbolicValue::Enum(Some("TwoStateEnum"), "Two"),
                    ) => TwoState::Two,
                    _ => return Err(format!("{source}: TypeMismatch: expected TwoStateEnum")),
                };
            }
            name => return Err(format!("{source}: PropertyDoesNotExist: {name}")),
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn other_cpu(&self, cpu: &Cpu) -> Option<&Cpu> {
        self.objects
            .get(cpu.other_cpu?)
            .and_then(|object| object.cpu.as_ref())
    }
}
