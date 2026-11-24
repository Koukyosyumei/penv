use super::*;

const MOCK_CPU: &str = "Antmicro.Renode.UnitTests.Mocks.MockCPU";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Model {
    Sysbus,
    IrqSender,
    DualIrqSender,
    NumberedGpio,
    IrqReceiver,
    Cpu,
    EmptyPeripheral,
    Register,
    AmbiguousRegister,
    MockRegistrationPoint,
    NonPeripheralRegister,
    DependentPeripheral,
    EnumPeripheral,
    CollectionPeripheral,
    ScalarPeripheral(ScalarKind),
    External,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ScalarKind {
    Integer,
    Boolean,
    PeripheralReference,
    String,
    Range,
    Object,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ScalarValue {
    Integer(i32),
    Boolean(bool),
    PeripheralReference(Option<usize>),
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
            (Self::Boolean, "mockBool", "empty") => Some(ScalarValue::Boolean(false)),
            (Self::Boolean, "mockBool", value) => boolean_value(value).map(ScalarValue::Boolean),
            (Self::PeripheralReference, "mockReference", "empty") => {
                Some(ScalarValue::PeripheralReference(None))
            }
            (Self::PeripheralReference, "mockReference", value) => {
                resolve(value).map(|id| ScalarValue::PeripheralReference(Some(id)))
            }
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
    fn irq_outputs(self) -> &'static [&'static str] {
        match self {
            Model::IrqSender => &["Irq"],
            Model::DualIrqSender => &["Irq", "AnotherIrq"],
            Model::NumberedGpio => &["Irq", "OtherIrq"],
            Model::IrqReceiver => &["IRQ"],
            _ => &[],
        }
    }

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
            "Antmicro.Renode.Tests.UnitTests.Mocks.MockPeripheralWithBoolAttribute" => {
                Some(Self::ScalarPeripheral(ScalarKind::Boolean))
            }
            "Antmicro.Renode.Tests.UnitTests.Mocks.MockPeripheralUsingReferenceAttribute" => {
                Some(Self::ScalarPeripheral(ScalarKind::PeripheralReference))
            }
            "Antmicro.Renode.UnitTests.Mocks.MockPeripheralWithDependency" => {
                Some(Self::DependentPeripheral)
            }
            "Antmicro.Renode.UnitTests.Mocks.NullRegister"
            | "Antmicro.Renode.UnitTests.Mocks.MockRegister" => Some(Self::Register),
            "Antmicro.Renode.UnitTests.Mocks.AmbiguousRegister" => Some(Self::AmbiguousRegister),
            "Antmicro.Renode.UnitTests.Mocks.MockRegistrationPoint" => {
                Some(Self::MockRegistrationPoint)
            }
            "Antmicro.Renode.UnitTests.Mocks.EmptyInterestingType" => {
                Some(Self::NonPeripheralRegister)
            }
            "Antmicro.Renode.UnitTests.Mocks.MockIrqSender" => Some(Self::IrqSender),
            "Antmicro.Renode.UnitTests.Mocks.MockIrqSenderWithTwoInterrupts" => {
                Some(Self::DualIrqSender)
            }
            "Antmicro.Renode.UnitTests.Mocks.MockGPIOByNumberConnectorPeripheral" => {
                Some(Self::NumberedGpio)
            }
            "Antmicro.Renode.UnitTests.Mocks.MockReceiver" => Some(Self::IrqReceiver),
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

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum IrqOutput {
    Named(&'static str),
    Pin(u32),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Endpoint {
    pub(crate) receiver: usize,
    pub(crate) pin: u32,
    pub(crate) local: Option<u32>,
}

struct CreatedObject {
    model: Model,
    cpu: Option<Cpu>,
    ranges: Vec<(u64, u64)>,
    enum_arguments: EnumArguments,
    scalar: Option<ScalarValue>,
    empty_peripheral_counter: u32,
    empty_peripheral_value: Option<String>,
    dependency: Option<usize>,
    throw_on_construction: bool,
    gpio_count: u32,
    endpoints: HashMap<IrqOutput, Vec<Endpoint>>,
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
    pub(crate) cpu_list: Option<Vec<usize>>,
    pub(crate) int_cpu: Option<std::collections::BTreeMap<i32, usize>>,
    pub(crate) cpu_int: Option<std::collections::BTreeMap<usize, i32>>,
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

fn bind_dictionary<K: Ord, V>(
    value: &str,
    key: impl Fn(&str) -> Result<K, String>,
    item: impl Fn(&str) -> Result<V, String>,
) -> Result<Option<std::collections::BTreeMap<K, V>>, String> {
    if value == "empty" {
        return Ok(None);
    }
    let mut items = std::collections::BTreeMap::new();
    for (k, v) in dictionary_value(value)? {
        if items.insert(key(k)?, item(v)?).is_some() {
            return Err("An item with the same key has already been added.".into());
        }
    }
    Ok(Some(items))
}

impl Collections {
    fn bind(
        &mut self,
        name: &str,
        value: &str,
        resolve_cpu: impl Fn(&str) -> Result<usize, String>,
    ) -> Result<(), String> {
        let integer = |value: &str| {
            value
                .parse()
                .map_err(|_| "TypeMismatch: expected integer".into())
        };
        match name {
            "mockIntStringDict" => {
                self.dictionary = bind_dictionary(value, integer, |value| {
                    quoted_value(value).map_err(|_| "TypeMismatch: expected string value".into())
                })?
            }
            "mockIntCpuDict" => self.int_cpu = bind_dictionary(value, integer, resolve_cpu)?,
            "mockCpuIntDict" => self.cpu_int = bind_dictionary(value, resolve_cpu, integer)?,
            "mockCpuList" => {
                self.cpu_list = if value == "empty" {
                    None
                } else {
                    Some(
                        list_value(value)
                            .map_err(str::to_owned)?
                            .into_iter()
                            .map(|item| match item {
                                ListItem::Symbol(SymbolicValue::Reference(name)) => {
                                    resolve_cpu(name)
                                }
                                _ => Err("TypeMismatch: expected CPU reference".into()),
                            })
                            .collect::<Result<Vec<_>, _>>()?,
                    )
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
    pub(crate) fn object_id(&self, name: &str) -> Option<usize> {
        self.registered.get(name).copied()
    }

    #[cfg(test)]
    pub(crate) fn irq_endpoints(&self, id: usize, name: &str) -> &[Endpoint] {
        let object = &self.objects[id];
        let output = object
            .model
            .irq_outputs()
            .iter()
            .copied()
            .find(|&output| output == name)
            .map(IrqOutput::Named)
            .or_else(|| name.parse().ok().map(IrqOutput::Pin))
            .unwrap();
        object.endpoints.get(&output).map_or(&[], Vec::as_slice)
    }

    #[cfg(test)]
    pub(crate) fn cpu_id(&self, qualified_name: &str) -> Option<usize> {
        let id = *self.registered.get(qualified_name)?;
        (self.objects[id].model == Model::Cpu).then_some(id)
    }

    #[cfg(test)]
    pub(crate) fn collections(&self, qualified_name: &str) -> Option<&Collections> {
        let object = &self.objects[*self.registered.get(qualified_name)?];
        (object.model == Model::CollectionPeripheral).then_some(&object.collections)
    }

    #[cfg(test)]
    pub(crate) fn empty_peripheral_counter(&self, qualified_name: &str) -> Option<u32> {
        let object = &self.objects[*self.registered.get(qualified_name)?];
        (object.model == Model::EmptyPeripheral).then_some(object.empty_peripheral_counter)
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

pub(crate) trait InitHandler {
    fn validate(&mut self, name: Option<&str>, command: &str) -> Result<(), String>;
    fn execute(&mut self, name: Option<&str>, commands: &[&str]);
    fn register_reset(&mut self, _: &str, _: &[&str]) {}
}

struct NoopInit;

impl InitHandler for NoopInit {
    fn validate(&mut self, _: Option<&str>, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn execute(&mut self, _: Option<&str>, _: &[&str]) {}
}

pub(crate) fn create_objects(content: &str, root: &Path) -> Result<CreatedPlatform, String> {
    create_with_init(content, root, &mut NoopInit)
}

pub(crate) fn create_with_init(
    content: &str,
    root: &Path,
    handler: &mut impl InitHandler,
) -> Result<CreatedPlatform, String> {
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
        if entry.alias.is_some() {
            match entry.registration.as_deref() {
                None => return Err(entry.error("AliasWithoutRegistration: missing registration")),
                Some("none") => {
                    return Err(entry.error("AliasWithNoneRegistration: registration cancelled"));
                }
                _ => {}
            }
        }
        if entry.model.is_empty()
            && entry.name == "sysbus"
            && entry
                .body
                .iter()
                .any(|(_, property)| property_key(property).is_some())
        {
            return Err(entry.error("CtorAttributesInNonCreatingEntry: sysbus already exists"));
        }
        if entry.model == "Antmicro.Renode.UnitTests.Mocks.MockPeripheralWithProtectedConstructor" {
            return Err(entry.error("NoCtor: model has no public constructor"));
        }
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
            empty_peripheral_counter: 1,
            empty_peripheral_value: None,
            dependency: None,
            throw_on_construction: false,
            gpio_count: 0,
            endpoints: HashMap::new(),
            collections: Collections::default(),
        });
        targets.push(Some(id));
    }
    let sysbus = platform.objects.len();
    platform.objects.push(CreatedObject {
        model: Model::Sysbus,
        cpu: None,
        ranges: Vec::new(),
        enum_arguments: EnumArguments::default(),
        scalar: None,
        empty_peripheral_counter: 1,
        empty_peripheral_value: None,
        dependency: None,
        throw_on_construction: false,
        gpio_count: 0,
        endpoints: HashMap::new(),
        collections: Collections::default(),
    });
    globals.insert("sysbus", sysbus);
    let mut init = Vec::new();
    let mut preinit = Vec::new();
    let mut reset = Vec::new();
    let resolve = |scope: &Path, name: &str| {
        locals
            .get(&(scope, name))
            .or_else(|| globals.get(name))
            .copied()
    };

    let mut pending = HashMap::new();
    for (entry, declared) in entries.iter().zip(targets) {
        let id = declared
            .or_else(|| resolve(&entry.source, &entry.name))
            .ok_or_else(|| entry.error("MissingReference: no declaration for overlay"))?;
        if let Some(registration) = &entry.registration {
            if platform.objects[id].model == Model::External {
                return Err(entry.error("NoUsableRegisterInterface: external object"));
            }
            if registration == "none" {
                pending.remove(&id);
            } else {
                let mut parents = Vec::new();
                platform.objects[id].ranges.clear();
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
                        let target = resolve(&entry.source, parent)
                            .ok_or_else(|| entry.error(&format!("MissingReference: {parent}")))?;
                        match platform.objects[target].model {
                            Model::NonPeripheralRegister => {
                                return Err(entry.error(
                                    "CastException: registration parent must be a peripheral",
                                ));
                            }
                            Model::AmbiguousRegister
                                if platform.objects[id].model == Model::EmptyPeripheral =>
                            {
                                let diagnosis = match point {
                                    Some(RegistrationValue::Other(value))
                                        if number(value).is_ok() =>
                                    {
                                        "AmbiguousCtorForRegistrationPoint: both bus and double point accept integer"
                                    }
                                    Some(RegistrationValue::Other(value))
                                        if value.parse::<f64>().is_ok_and(f64::is_finite) =>
                                    {
                                        "AmbiguousRegistree: both byte and double-word registration accept double point"
                                    }
                                    Some(RegistrationValue::Other(value))
                                        if resolve(&entry.source, value).is_some_and(
                                            |target| {
                                                platform.objects[target].model
                                                    == Model::MockRegistrationPoint
                                            },
                                        ) =>
                                    {
                                        "AmbiguousRegistrationPointType: the point implements both supported interfaces"
                                    }
                                    _ => {
                                        "NoUsableRegisterInterface: unsupported registration point"
                                    }
                                };
                                return Err(entry.error(diagnosis));
                            }
                            _ => {}
                        }
                        if platform.objects[target].model != Model::Register || point.is_some() {
                            return Err(entry
                                .error("NoUsableRegisterInterface: unsupported parent or point"));
                        }
                        parents.push(Some(target));
                        continue;
                    }
                    parents.push(None);
                    match point {
                        Some(RegistrationValue::Range { base, size }) => {
                            if platform.objects[id].model == Model::Cpu {
                                return Err(entry.error(
                                    "NoCtorForRegistrationPoint: CPU does not accept a bus range",
                                ));
                            }
                            platform.objects[id].ranges.push((base, size));
                        }
                        Some(RegistrationValue::Other(value)) if number(value).is_ok() => {}
                        Some(RegistrationValue::Other(value)) if value.starts_with("new ") => {
                            let (object, _) =
                                object_prefix(value).map_err(|error| entry.error(error))?;
                            if object.model
                                == "Antmicro.Renode.Peripherals.Bus.BusRangeRegistration"
                            {
                                let [("range", value)] = object.fields.as_slice() else {
                                    return Err(entry.error(
                                        "NoCtorForRegistrationPoint: expected range argument",
                                    ));
                                };
                                platform.objects[id]
                                    .ranges
                                    .push(range_value(value).map_err(|error| entry.error(error))?);
                                continue;
                            }
                            Model::parse(object.model).ok_or_else(|| {
                                entry.error(&format!("TypeNotResolved: {}", object.model))
                            })?;
                            return Err(entry
                                .error("TypeMismatch: inline object is not a registration point"));
                        }
                        Some(RegistrationValue::Other(value)) => {
                            let target = resolve(&entry.source, value).ok_or_else(|| {
                                entry.error(&format!("MissingReference: {value}"))
                            })?;
                            let model = match platform.objects[target].model {
                                Model::Cpu => "CPU",
                                Model::EmptyPeripheral
                                | Model::DependentPeripheral
                                | Model::Register
                                | Model::EnumPeripheral
                                | Model::ScalarPeripheral(_)
                                | Model::CollectionPeripheral => "peripheral",
                                Model::External
                                | Model::NonPeripheralRegister
                                | Model::AmbiguousRegister => "non-peripheral",
                                Model::MockRegistrationPoint => "mock registration point",
                                Model::Sysbus => "system bus",
                                Model::IrqSender
                                | Model::DualIrqSender
                                | Model::NumberedGpio
                                | Model::IrqReceiver => "IRQ peripheral",
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
                pending.insert(id, (entry.alias.as_deref().unwrap_or(&entry.name), parents));
            }
        }
        let mut connections = Vec::new();
        let mut sections = entry.body.iter().peekable();
        while sections
            .peek()
            .is_some_and(|(_, text)| init_header(text).is_none())
        {
            let (source, property) = sections.next().unwrap();
            if property.contains("->") && property_key(property).is_none() {
                connections.push((source, property));
                continue;
            }
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
        check_irq_source_overlap(platform.objects[id].model, &connections)?;
        for (source, property) in connections {
            platform
                .connect_irq(id, property, |name| resolve(&entry.source, name))
                .map_err(|error| format!("{source}: {error}"))?;
        }
        while let Some((_, marker)) = sections.next() {
            let (section, append) = init_header(marker).unwrap();
            let mut additions = Vec::new();
            while sections
                .peek()
                .is_some_and(|(_, text)| init_header(text).is_none())
            {
                additions.push(sections.next().unwrap().1.as_str());
            }
            let plan = match section {
                ScriptSection::Init => &mut init,
                ScriptSection::Preinit => &mut preinit,
                ScriptSection::Reset => &mut reset,
            };
            merge_script(plan, id, &entry.name, append, additions);
        }
    }
    preinit.sort_by_key(|(id, _, _)| *id != sysbus);
    run_scripts(handler, preinit, true)?;
    for (entry, object) in entries
        .iter()
        .filter(|entry| !entry.model.is_empty())
        .zip(&mut platform.objects)
    {
        object
            .finish_constructor()
            .map_err(|error| entry.error(&error))?;
    }
    let mut ranges = Vec::new();
    for &id in pending.keys() {
        ranges.extend(
            platform.objects[id]
                .ranges
                .iter()
                .map(|&(base, size)| (base, size)),
        );
    }
    ranges.sort_unstable();
    let mut end = 0;
    for (base, size) in ranges {
        if size == 0 {
            continue;
        }
        if base < end {
            return Err("RegistrationException: overlapping sysbus ranges".into());
        }
        end = base
            .checked_add(size)
            .ok_or("RegistrationException: registration range overflow")?;
    }
    while !pending.is_empty() {
        let ready: Vec<_> = pending
            .iter()
            .filter_map(|(&id, (_, parents))| {
                parents
                    .iter()
                    .all(|parent| {
                        parent.is_none_or(|parent| {
                            platform
                                .registered
                                .values()
                                .any(|&registered| registered == parent)
                        })
                    })
                    .then_some(id)
            })
            .collect();
        if ready.is_empty() {
            return Err("RegistrationException: unregistered or cyclic parent".into());
        }
        for id in ready {
            let (name, parents) = pending.remove(&id).unwrap();
            for parent in parents {
                let paths: Vec<_> = match parent {
                    None => vec![format!("sysbus.{name}")],
                    Some(parent) => platform
                        .registered
                        .iter()
                        .filter(|(_, registered)| **registered == parent)
                        .map(|(path, _)| format!("{path}.{name}"))
                        .collect(),
                };
                for path in paths {
                    if platform
                        .registered
                        .insert(path, id)
                        .is_some_and(|previous| previous != id)
                    {
                        return Err("NameSettingException: duplicate registration name".into());
                    }
                }
            }
        }
    }
    platform.check_constructor_cycles()?;
    run_scripts(handler, init, false)?;
    for (_, name, commands) in reset {
        handler.register_reset(name, &commands);
    }
    Ok(platform)
}

fn check_irq_source_overlap(
    model: Model,
    connections: &[(&SourceLine, &String)],
) -> Result<(), String> {
    let mut named = HashSet::new();
    let mut ranges = Vec::new();
    for (line, text) in connections {
        let connection = irq_connection(text).map_err(|error| format!("{line}: {error}"))?;
        if let Some(sources) = connection.sources {
            for source in sources {
                match source {
                    IrqEnd::Property(name) => {
                        if !named.insert(name) {
                            return Err(format!("{line}: IrqSourceUsedMoreThanOnce: {name}"));
                        }
                    }
                    IrqEnd::Number(pin) => ranges.push((pin, pin, *line)),
                    IrqEnd::Range(a, b) => ranges.push((a.min(b), a.max(b), *line)),
                }
            }
        } else if let [name] = model.irq_outputs()
            && !named.insert(name)
        {
            return Err(format!("{line}: IrqSourceUsedMoreThanOnce: {name}"));
        }
    }
    ranges.sort_unstable_by_key(|&(start, end, _)| (start, end));
    let mut previous_end = None;
    for (start, end, line) in ranges {
        if previous_end.is_some_and(|limit| start <= limit) {
            return Err(format!(
                "{line}: IrqSourceUsedMoreThanOnce: overlapping pin groups"
            ));
        }
        previous_end = Some(end);
    }
    Ok(())
}

type ScriptPlan<'a> = Vec<(usize, &'a str, Vec<&'a str>)>;

fn merge_script<'a>(
    plan: &mut ScriptPlan<'a>,
    id: usize,
    name: &'a str,
    append: bool,
    commands: Vec<&'a str>,
) {
    if let Some((_, _, existing)) = plan.iter_mut().find(|(target, _, _)| *target == id) {
        if !append {
            existing.clear();
        }
        existing.extend(commands);
    } else {
        plan.push((id, name, commands));
    }
}

fn run_scripts(
    handler: &mut impl InitHandler,
    plan: ScriptPlan<'_>,
    preinit: bool,
) -> Result<(), String> {
    let diagnostic = if preinit {
        "PreinitSectionValidationError"
    } else {
        "InitSectionValidationError"
    };
    for (_, name, commands) in &plan {
        for command in commands {
            handler
                .validate((!preinit).then_some(*name), command)
                .map_err(|error| format!("{diagnostic}: {name}: {error}"))?;
        }
    }
    for (_, name, commands) in plan {
        handler.execute((!preinit).then_some(name), &commands);
    }
    Ok(())
}

impl CreatedObject {
    fn finish_constructor(&mut self) -> Result<(), String> {
        if self.throw_on_construction {
            return Err("ConstructionException: Fake exception".into());
        }
        if let Some(value) = &self.empty_peripheral_value {
            if value.parse::<i32>().is_ok() {
                // An integer fits both the int overload with optional enabled and the double overload.
                return Err("AmbiguousCtor: multiple public constructors accept value".into());
            }
            if value.parse::<f64>().is_ok_and(f64::is_finite) {
                self.empty_peripheral_counter = 2;
            } else {
                return Err("NoCtor: invalid value argument".into());
            }
        }
        if matches!(self.model, Model::ScalarPeripheral(_)) && self.scalar.is_none() {
            return Err("NoCtor: missing required scalar constructor argument".into());
        }
        Ok(())
    }
}

impl CreatedPlatform {
    fn check_constructor_cycles(&self) -> Result<(), String> {
        // Each mock constructor has at most one dependency. Walk each chain once.
        let mut state = vec![0; self.objects.len()];
        for start in 0..state.len() {
            let mut current = Some(start);
            let mut path = Vec::new();
            while let Some(id) = current {
                match state[id] {
                    1 => return Err("CreationOrderCycle: cyclic constructor dependency".into()),
                    2 => break,
                    _ => {
                        state[id] = 1;
                        path.push(id);
                        current = self.objects[id].dependency;
                    }
                }
            }
            for id in path {
                state[id] = 2;
            }
        }
        Ok(())
    }

    fn connect_irq(
        &mut self,
        id: usize,
        text: &str,
        resolve: impl Fn(&str) -> Option<usize>,
    ) -> Result<(), String> {
        let connection = irq_connection(text)?;
        let sender = &self.objects[id];
        let outputs = sender.model.irq_outputs();
        let mut receivers = Vec::new();
        for destination in &connection.destinations {
            let target =
                resolve(destination.name).ok_or("IrqDestinationDoesNotExist: missing receiver")?;
            if !matches!(
                self.objects[target].model,
                Model::IrqReceiver | Model::NumberedGpio
            ) {
                return Err(
                    "IrqDestinationIsNotIrqReceiver: model does not implement IGPIOReceiver".into(),
                );
            }
            receivers.push(target);
        }
        let width = |end: &IrqEnd<'_>| match end {
            IrqEnd::Range(a, b) => u64::from(a.abs_diff(*b)) + 1,
            _ => 1,
        };
        let source_width = if let Some(sources) = &connection.sources {
            for source in sources {
                match source {
                    IrqEnd::Property(name) if outputs.contains(name) => {}
                    IrqEnd::Property(_) => {
                        return Err("IrqSourceDoesNotExist: unknown output".into());
                    }
                    IrqEnd::Number(_) | IrqEnd::Range(_, _)
                        if sender.model != Model::NumberedGpio =>
                    {
                        return Err(
                            "IrqSourceIsNotNumberedGpioOutput: model has no numbered outputs"
                                .into(),
                        );
                    }
                    IrqEnd::Number(pin) if *pin >= sender.gpio_count => {
                        return Err("IrqSourcePinDoesNotExist: pin outside GPIO count".into());
                    }
                    IrqEnd::Range(a, b) if *a.max(b) >= sender.gpio_count => {
                        return Err("IrqSourcePinDoesNotExist: pin outside GPIO count".into());
                    }
                    _ => {}
                }
            }
            sources.iter().map(width).sum()
        } else {
            match outputs.len() {
                0 => return Err("IrqSourceDoesNotExist: no default output".into()),
                1 => 1,
                _ => return Err("AmbiguousDefaultIrqSource: more than one output".into()),
            }
        };
        if connection
            .destinations
            .iter()
            .any(|destination| destination.groups.iter().map(width).sum::<u64>() != source_width)
        {
            return Err("WrongIrqArity: source and destination widths differ".into());
        }
        let mut sources = Vec::new();
        match connection.sources {
            None => sources.push(IrqOutput::Named(outputs[0])),
            Some(groups) => {
                for group in groups {
                    match group {
                        IrqEnd::Property(name) => {
                            let name = outputs
                                .iter()
                                .copied()
                                .find(|&output| output == name)
                                .unwrap();
                            sources.push(IrqOutput::Named(name));
                        }
                        IrqEnd::Number(pin) => sources.push(IrqOutput::Pin(pin)),
                        IrqEnd::Range(a, b) => sources.extend(irq_range(a, b).map(IrqOutput::Pin)),
                    }
                }
            }
        }
        let endpoints = &mut self.objects[id].endpoints;
        if connection.destinations.is_empty() {
            for output in sources {
                endpoints.remove(&output);
            }
            return Ok(());
        }
        for (destination, receiver) in connection.destinations.into_iter().zip(receivers) {
            let pins = destination
                .groups
                .into_iter()
                .flat_map(|group| match group {
                    IrqEnd::Number(pin) => irq_range(pin, pin),
                    IrqEnd::Range(a, b) => irq_range(a, b),
                    IrqEnd::Property(_) => unreachable!("destination groups consist of pins"),
                });
            for (&output, pin) in sources.iter().zip(pins) {
                endpoints.entry(output).or_default().push(Endpoint {
                    receiver,
                    pin,
                    local: destination.local,
                });
            }
        }
        Ok(())
    }

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
        if self.objects[id].model == Model::NumberedGpio && name == "gpios" {
            self.objects[id].gpio_count =
                number(value).map_err(|error| format!("{source}: {error}"))?;
            return Ok(());
        }
        if self.objects[id].model == Model::DependentPeripheral {
            if name == "throwException" {
                self.objects[id].throw_on_construction = boolean_value(value).ok_or_else(|| {
                    format!("{source}: TypeMismatch: expected boolean constructor argument")
                })?;
                return Ok(());
            }
            if name != "other" {
                return Err(format!(
                    "{source}: NoCtor: unknown constructor argument {name}"
                ));
            }
            let target = if value == "empty" || value == "none" {
                None
            } else if value.starts_with("new ") {
                let (object, tail) =
                    object_prefix(value).map_err(|error| format!("{source}: {error}"))?;
                if !tail.trim().is_empty() {
                    return Err(format!("{source}: unexpected text after inline object"));
                }
                let model = Model::parse(object.model)
                    .ok_or_else(|| format!("{source}: TypeNotResolved: {}", object.model))?;
                let target = self.objects.len();
                self.objects.push(CreatedObject {
                    model,
                    cpu: (model == Model::Cpu).then(Cpu::default),
                    ranges: Vec::new(),
                    enum_arguments: EnumArguments::default(),
                    scalar: None,
                    empty_peripheral_counter: 1,
                    empty_peripheral_value: None,
                    dependency: None,
                    throw_on_construction: false,
                    gpio_count: 0,
                    endpoints: HashMap::new(),
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
                self.objects[target]
                    .finish_constructor()
                    .map_err(|error| format!("{source}: {error}"))?;
                Some(target)
            } else {
                Some(resolve(value).ok_or_else(|| format!("{source}: MissingReference: {value}"))?)
            };
            if target.is_some_and(|target| self.objects[target].model == Model::External) {
                return Err(format!(
                    "{source}: TypeMismatch: other requires a peripheral"
                ));
            }
            self.objects[id].dependency = target;
            return Ok(());
        }
        if self.objects[id].model == Model::CollectionPeripheral {
            let mut collections = std::mem::take(&mut self.objects[id].collections);
            collections
                .bind(name, value, |name| {
                    let target =
                        resolve(name).ok_or_else(|| format!("MissingReference: {name}"))?;
                    if self.objects[target].model != Model::Cpu {
                        return Err(format!("TypeMismatch: {name} is not a CPU"));
                    }
                    Ok(target)
                })
                .map_err(|error| format!("{source}: {error}"))?;
            self.objects[id].collections = collections;
            return Ok(());
        }
        if self.objects[id].model == Model::EnumPeripheral {
            return self.objects[id].enum_arguments.bind(name, value, source);
        }
        if self.objects[id].model == Model::EmptyPeripheral {
            if name == "ThrowingProperty" {
                value
                    .parse::<i32>()
                    .map_err(|_| format!("{source}: TypeMismatch: expected integer property"))?;
                return Err(format!(
                    "{source}: PropertySettingException: Fake exception"
                ));
            }
            if value.starts_with("new ") {
                let (object, _) =
                    object_prefix(value).map_err(|error| format!("{source}: {error}"))?;
                Model::parse(object.model)
                    .ok_or_else(|| format!("{source}: TypeNotResolved: {}", object.model))?;
            }
            if name != "value" {
                return Err(format!(
                    "{source}: NoCtor: unknown constructor argument {name}"
                ));
            }
            self.objects[id].empty_peripheral_value = (value != "none").then(|| value.to_owned());
            return Ok(());
        }
        if let Model::ScalarPeripheral(kind) = self.objects[id].model {
            self.objects[id].scalar = Some(
                kind.bind(name, value, |name| {
                    let target = resolve(name)?;
                    (kind != ScalarKind::PeripheralReference
                        || self.objects[target].model != Model::External)
                        .then_some(target)
                })
                .ok_or_else(|| {
                    format!("{source}: NoCtor: invalid scalar constructor argument {name}")
                })?,
            );
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
                    empty_peripheral_counter: 1,
                    empty_peripheral_value: None,
                    dependency: None,
                    throw_on_construction: false,
                    gpio_count: 0,
                    endpoints: HashMap::new(),
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
