//! Reference-preserving object streams for Rust graph descriptors.
//!
//! This uses Migrant's primitive encoding with a Rust object-record format.
use super::generated_record::{Codec, RecordInput, RecordOutput};
use super::primitive::{Decimal, Guid, Reader, Writer};
use super::surrogate::Surrogates;
use std::collections::{hash_map::Entry, HashMap};
use std::io::{self, Read, Write};
use std::sync::{Arc, Weak};

const MAX_ITEMS: usize = 1_000_000;
const ROOT: u8 = 1;
const STAMPED_ROOT: u8 = 2;

#[derive(Clone, Copy)]
#[repr(u8)]
pub enum FieldKind {
    Integer = 1,
    Float = 2,
    Reference = 3,
    NullableInteger = 4,
    Guid = 5,
    BigInteger = 6,
    NativeResource = 7,
    Decimal = 8,
    NullableDecimal = 9,
    Boolean = 10,
}

#[derive(Clone, Copy)]
pub struct TypeStamp<'a> {
    pub class: &'a str,
    pub module: &'a str,
    pub revision: u64,
    pub fields: &'a [(&'a str, FieldKind)],
}

fn stamp_for<'a>(stamps: &'a [TypeStamp<'a>], class: &str) -> io::Result<TypeStamp<'a>> {
    stamps
        .iter()
        .copied()
        .find(|stamp| stamp.class == class)
        .ok_or_else(|| invalid("missing type stamp for object class"))
}

impl TypeStamp<'_> {
    fn validate(self, values: &[Value]) -> io::Result<()> {
        if self.fields.len() != values.len()
            || !self.fields.iter().zip(values).all(|((_, kind), value)| {
                matches!(
                    (kind, value),
                    (FieldKind::Integer, Value::Integer(_))
                        | (FieldKind::NullableInteger, Value::Null | Value::Integer(_))
                        | (FieldKind::Float, Value::Float(_))
                        | (FieldKind::Guid, Value::Guid(_))
                        | (FieldKind::BigInteger, Value::BigInteger(_))
                        | (FieldKind::NativeResource, Value::NativeResource(_))
                        | (FieldKind::Decimal, Value::Decimal(_))
                        | (FieldKind::Boolean, Value::Boolean(_))
                        | (FieldKind::NullableDecimal, Value::Null | Value::Decimal(_))
                        | (FieldKind::Reference, Value::Null | Value::Ref(_))
                )
            })
        {
            return Err(invalid("object fields do not match type stamp"));
        }
        Ok(())
    }

    fn write(self, data: &mut Writer<impl Write>) -> io::Result<()> {
        data.string(self.module)?;
        data.unsigned(self.revision)?;
        data.unsigned(self.fields.len() as u64)?;
        for &(name, kind) in self.fields {
            data.string(name)?;
            data.bytes(&[kind as u8])?;
        }
        Ok(())
    }

    fn verify(self, data: &mut Reader<impl Read>) -> io::Result<()> {
        if data.string()? != self.module
            || data.unsigned()? != self.revision
            || data.unsigned()? != self.fields.len() as u64
        {
            return Err(invalid("object type stamp differs from local schema"));
        }
        for &(name, kind) in self.fields {
            if data.string()? != name || data.byte()? != kind as u8 {
                return Err(invalid("object field stamp differs from local schema"));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ObjectId(usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeResource {
    IntPtr(usize),
    Pointer(usize),
    SpinLock,
    ThreadLocal,
    ManualResetEvent { signaled: bool },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NonSerializableResource {
    pub resource: NativeResource,
    pub path: Vec<String>,
}

impl std::fmt::Display for NonSerializableResource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "native resource {:?} cannot be serialized",
            self.resource
        )?;
        if !self.path.is_empty() {
            write!(formatter, " at {}", self.path.join(" -> "))?;
        }
        Ok(())
    }
}

impl std::error::Error for NonSerializableResource {}

fn unsupported(resource: NativeResource, path: Vec<String>) -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        NonSerializableResource { resource, path },
    )
}

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Integer(i64),
    Float(f64),
    Guid(Guid),
    /// Little-endian signed two's-complement bytes.
    BigInteger(Vec<u8>),
    NativeResource(NativeResource),
    Decimal(Decimal),
    Boolean(bool),
    Ref(ObjectId),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Object {
    String(String),
    Bytes(Vec<u8>),
    Record {
        class: String,
        fields: Vec<Value>,
    },
    Array(Vec<Value>),
    Rectangular {
        dimensions: Vec<usize>,
        elements: Vec<Value>,
    },
}

fn check_rectangular(dimensions: &[usize], elements: &[Value]) -> io::Result<()> {
    let count = dimensions
        .iter()
        .try_fold(1usize, |size, &axis| size.checked_mul(axis));
    if !(2..=32).contains(&dimensions.len())
        || dimensions.iter().any(|&axis| axis > MAX_ITEMS)
        || count != Some(elements.len())
        || elements.len() > MAX_ITEMS
    {
        return Err(invalid("invalid rectangular array shape"));
    }
    Ok(())
}

pub enum FieldInitializer {
    Default(Value),
    Constructor(fn() -> Value),
}

impl FieldInitializer {
    fn restore(&self) -> Value {
        match self {
            Self::Default(value) => value.clone(),
            Self::Constructor(constructor) => constructor(),
        }
    }
}

/// Runtime field layout for transient values excluded from an object stream.
pub struct Transients<'a> {
    pub classes: &'a [&'a str],
    /// Field initializers indexed in the complete, in-memory record layout.
    pub fields: &'a [(&'a str, &'a [(usize, FieldInitializer)])],
}

impl Transients<'_> {
    fn contains(&self, graph: &Graph, id: ObjectId) -> bool {
        matches!(graph.get(id), Some(Object::Record { class, .. }) if self.classes.contains(&class.as_str()))
    }

    fn initializers(&self, class: &str) -> &[(usize, FieldInitializer)] {
        self.fields
            .iter()
            .find(|(name, _)| *name == class)
            .map_or(&[], |(_, fields)| fields)
    }

    fn project(&self, graph: &Graph, object: &Object) -> Option<Object> {
        let values = match object {
            Object::Record { fields, .. }
            | Object::Array(fields)
            | Object::Rectangular {
                elements: fields, ..
            } => fields,
            _ => return None,
        };
        let initializers = match object {
            Object::Record { class, .. } => self.initializers(class),
            _ => &[],
        };
        if initializers.is_empty()
            && !values
                .iter()
                .any(|value| matches!(value, Value::Ref(id) if self.contains(graph, *id)))
        {
            return None;
        }
        let projected = values
            .iter()
            .enumerate()
            .filter(|(index, _)| !initializers.iter().any(|(omitted, _)| omitted == index))
            .map(|(_, value)| match value {
                Value::Ref(id) if self.contains(graph, *id) => Value::Null,
                _ => value.clone(),
            })
            .collect();
        Some(match object {
            Object::Record { class, .. } => Object::Record {
                class: class.clone(),
                fields: projected,
            },
            Object::Array(_) => Object::Array(projected),
            Object::Rectangular { dimensions, .. } => Object::Rectangular {
                dimensions: dimensions.clone(),
                elements: projected,
            },
            _ => unreachable!(),
        })
    }

    fn restore_fields(&self, class: &str, fields: &mut Vec<Value>) -> io::Result<()> {
        let mut previous = None;
        for (index, initializer) in self.initializers(class) {
            if *index > fields.len() || previous.is_some_and(|previous| previous >= *index) {
                return Err(invalid("invalid transient field layout"));
            }
            let value = initializer.restore();
            if matches!(value, Value::Ref(_)) {
                return Err(invalid(
                    "initializer returned an unmanaged object reference",
                ));
            }
            fields.insert(*index, value);
            previous = Some(*index);
        }
        Ok(())
    }
}

#[derive(Default)]
pub struct Graph {
    objects: Vec<Option<Arc<Object>>>,
    identity: Arc<()>,
}

impl Graph {
    pub fn insert(&mut self, object: Object) -> ObjectId {
        let id = ObjectId(self.objects.len());
        self.objects.push(Some(Arc::new(object)));
        id
    }

    pub fn get(&self, id: ObjectId) -> Option<&Object> {
        self.objects.get(id.0)?.as_deref()
    }

    pub fn get_mut(&mut self, id: ObjectId) -> Option<&mut Object> {
        Some(Arc::make_mut(self.objects.get_mut(id.0)?.as_mut()?))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum References {
    PerRoot,
    Preserve,
    Weak,
}

/// A read result owns exactly the objects reachable from its root. Edges are
/// object IDs, so even a cycle expires after the last owner is dropped.
pub struct RetainedRoot {
    id: ObjectId,
    objects: HashMap<ObjectId, Arc<Object>>,
}

impl RetainedRoot {
    pub fn id(&self) -> ObjectId {
        self.id
    }

    pub fn get(&self, id: ObjectId) -> Option<&Object> {
        self.objects.get(&id).map(Arc::as_ref)
    }
}

type SourceObject = (*const (), ObjectId);

struct WeakWriteHistory {
    ids: HashMap<SourceObject, (Weak<()>, usize)>,
    slots: usize,
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

type ObjectObserver<'a> = Box<dyn FnMut(&Object) + 'a>;

pub struct ObjectWriter<'a, 's, W: Write> {
    data: Writer<W>,
    graph: Option<&'a Graph>,
    weak: Option<WeakWriteHistory>,
    references: References,
    ids: HashMap<ObjectId, usize>,
    pending: Vec<ObjectId>,
    frame_base: usize,
    replacements: HashMap<ObjectId, Object>,
    stamps: Option<&'a [TypeStamp<'a>]>,
    allow_xml_surrogates: bool,
    generated: Option<&'a [Codec]>,
    transients: Option<&'a Transients<'a>>,
    surrogates: Option<&'a mut Surrogates<'s>>,
    restore_prototype: Option<&'a mut Surrogates<'s>>,
    restore_classes: HashMap<ObjectId, String>,
    before_write: Option<ObjectObserver<'s>>,
    after_write: Option<ObjectObserver<'s>>,
}

impl<'a, 's, W: Write> ObjectWriter<'a, 's, W> {
    pub fn new(stream: W, graph: &'a Graph, references: References, buffered: bool) -> Self {
        Self::with_source(stream, Some(graph), references, buffered)
    }

    fn with_source(
        stream: W,
        graph: Option<&'a Graph>,
        references: References,
        buffered: bool,
    ) -> Self {
        Self {
            data: Writer::new(stream, buffered),
            graph,
            weak: (references == References::Weak).then(|| WeakWriteHistory {
                ids: HashMap::new(),
                slots: 0,
            }),
            references,
            ids: HashMap::new(),
            pending: Vec::new(),
            frame_base: 0,
            replacements: HashMap::new(),
            stamps: None,
            allow_xml_surrogates: true,
            generated: None,
            transients: None,
            surrogates: None,
            restore_prototype: None,
            restore_classes: HashMap::new(),
            before_write: None,
            after_write: None,
        }
    }

    /// In weak mode, sources are borrowed only for the duration of each root write.
    pub fn new_weak(stream: W, buffered: bool) -> Self {
        Self::with_source(stream, None, References::Weak, buffered)
    }

    pub fn write_weak_object(&mut self, graph: &Graph, root: ObjectId) -> io::Result<()> {
        let history = self
            .weak
            .as_mut()
            .ok_or_else(|| invalid("writer is not in weak mode"))?;
        history
            .ids
            .retain(|_, (object, _)| object.strong_count() != 0);
        self.write_from(graph, root)?;
        let history = self.weak.as_mut().unwrap();
        for id in &self.pending {
            let source = (Arc::as_ptr(&graph.identity), *id);
            history
                .ids
                .insert(source, (Arc::downgrade(&graph.identity), self.ids[id]));
        }
        history.slots += self.pending.len();
        Ok(())
    }

    pub fn with_write_observers(
        mut self,
        before: impl FnMut(&Object) + 's,
        after: impl FnMut(&Object) + 's,
    ) -> Self {
        self.before_write = Some(Box::new(before));
        self.after_write = Some(Box::new(after));
        self
    }

    pub fn with_surrogates(mut self, surrogates: &'a mut Surrogates<'s>) -> Self {
        self.surrogates = Some(surrogates);
        self
    }

    /// Record the restored class by invoking each matching read factory once
    /// when a new surrogate record is discovered.
    pub fn with_restore_prototypes(mut self, factories: &'a mut Surrogates<'s>) -> Self {
        self.restore_prototype = Some(factories);
        self
    }

    pub fn with_transients(mut self, transients: &'a Transients<'a>) -> Self {
        self.transients = Some(transients);
        self
    }

    pub(super) fn allow_xml_surrogates(mut self, allow: bool) -> Self {
        self.allow_xml_surrogates = allow;
        self
    }

    pub fn with_generated_records(mut self, codecs: &'a [Codec]) -> Self {
        self.generated = Some(codecs);
        self
    }

    pub fn with_type_stamps(mut self, stamps: &'a [TypeStamp<'a>]) -> Self {
        self.stamps = Some(stamps);
        self
    }

    fn intern(&mut self, graph: &Graph, id: ObjectId) -> io::Result<usize> {
        match self.ids.entry(id) {
            Entry::Occupied(entry) => Ok(*entry.get()),
            Entry::Vacant(entry) => {
                if graph.get(id).is_none() {
                    return Err(invalid("invalid graph reference"));
                }
                let next = if let Some(history) = &self.weak {
                    let source = (Arc::as_ptr(&graph.identity), id);
                    if let Some((_, slot)) = history.ids.get(&source) {
                        entry.insert(*slot);
                        return Ok(*slot);
                    }
                    history.slots + self.pending.len()
                } else {
                    self.pending.len() + self.frame_base
                };
                if next >= MAX_ITEMS {
                    return Err(invalid("object limit exceeded"));
                }
                entry.insert(next);
                self.pending.push(id);
                Ok(next)
            }
        }
    }

    pub fn write_object(&mut self, root: ObjectId) -> io::Result<()> {
        if self.references == References::Weak {
            return Err(invalid("weak mode requires write_weak_object"));
        }
        self.write_from(self.graph.unwrap(), root)
    }

    pub fn write_root(&mut self, root: Option<ObjectId>) -> io::Result<()> {
        self.write_object(
            root.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "null object root"))?,
        )
    }

    fn write_from(&mut self, graph: &Graph, root: ObjectId) -> io::Result<()> {
        if self
            .transients
            .is_some_and(|transients| transients.contains(graph, root))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "transient object root",
            ));
        }
        if self.references != References::Preserve {
            self.ids.clear();
        }
        self.frame_base = self.ids.len();
        self.pending.clear();
        self.replacements.clear();
        self.restore_classes.clear();
        let root = match self.discover(graph, root) {
            Ok(root) => root,
            Err(error) => {
                for id in self.pending.drain(..) {
                    self.ids.remove(&id);
                }
                return Err(error);
            }
        };
        self.write_frame(graph, root)?;
        if let Some(surrogates) = self.surrogates.as_mut() {
            surrogates.close_registration();
        }
        Ok(())
    }

    fn discover(&mut self, graph: &Graph, root: ObjectId) -> io::Result<usize> {
        let root = self.intern(graph, root)?;
        let mut parents = HashMap::new();
        let mut index = 0;
        // Reserve identities before visiting children, including forward and cyclic edges.
        while let Some(id) = self.pending.get(index).copied() {
            let original = graph.get(id).unwrap();
            if let Some(observer) = &mut self.before_write {
                observer(original);
            }
            let replacement = self
                .surrogates
                .as_mut()
                .map(|surrogates| {
                    if self.generated.is_some() {
                        surrogates.convert_generated(id, original)
                    } else {
                        surrogates.convert(id, original)
                    }
                })
                .transpose()?
                .flatten();
            let projected = self.transients.and_then(|transients| {
                transients.project(graph, replacement.as_ref().unwrap_or(original))
            });
            let replacement = projected.or(replacement);
            let object = replacement.as_ref().unwrap_or(original);
            if let Some(factories) = &mut self.restore_prototype {
                if let Some(prototype) = factories.convert(id, object)? {
                    let Object::Record { class, .. } = prototype else {
                        return Err(invalid("restore prototype must be a record"));
                    };
                    self.restore_classes.insert(id, class);
                }
            }
            let values = match object {
                Object::String(_) => &[],
                Object::Bytes(bytes) => {
                    if bytes.len() > MAX_ITEMS {
                        return Err(invalid("byte array limit exceeded"));
                    }
                    &[]
                }
                Object::Record { class, fields } => {
                    if !self.allow_xml_surrogates && class == super::xml_surrogate::CLASS {
                        return Err(super::xml_surrogate::disabled());
                    }
                    if let Some(stamps) = self.stamps {
                        stamp_for(stamps, class)?.validate(fields)?;
                    }
                    fields.as_slice()
                }
                Object::Array(fields) => fields.as_slice(),
                Object::Rectangular {
                    dimensions,
                    elements,
                } => {
                    check_rectangular(dimensions, elements)?;
                    elements
                }
            };
            if values.len() > MAX_ITEMS {
                return Err(invalid("object field limit exceeded"));
            }
            for value in values {
                match value {
                    Value::NativeResource(resource) => {
                        let mut path = Vec::new();
                        let mut current = id;
                        loop {
                            let object = if current == id {
                                object
                            } else {
                                self.replacements
                                    .get(&current)
                                    .or_else(|| graph.get(current))
                                    .unwrap()
                            };
                            path.push(match object {
                                Object::Record { class, .. } => class.clone(),
                                _ => "array".into(),
                            });
                            let Some(&parent) = parents.get(&current) else {
                                break;
                            };
                            current = parent;
                        }
                        path.reverse();
                        return Err(unsupported(*resource, path));
                    }
                    Value::Ref(child) => {
                        let ready = self.pending.len();
                        self.intern(graph, *child)?;
                        if self.pending.len() != ready {
                            parents.insert(*child, id);
                        }
                    }
                    Value::BigInteger(bytes) if bytes.is_empty() || bytes.len() > MAX_ITEMS => {
                        return Err(invalid("invalid big integer length"));
                    }
                    _ => {}
                }
            }
            // Detect unsupported native values before selecting the field codec.
            // This preserves the full resource path in both write methods.
            if let (Some(codecs), Object::Record { class, fields }) = (self.generated, object) {
                let codec = find_codec(codecs, class)?;
                (codec.write)(&mut ValidateGeneratedFields, fields)?;
            }
            if let Some(replacement) = replacement {
                self.replacements.insert(id, replacement);
            }
            index += 1;
        }

        Ok(root)
    }

    fn write_frame(&mut self, graph: &Graph, root: usize) -> io::Result<()> {
        self.data.bytes(&[if self.stamps.is_some() {
            STAMPED_ROOT
        } else {
            ROOT
        }])?;
        self.data.unsigned(root as u64)?;
        self.data.unsigned(self.pending.len() as u64)?;
        for id in &self.pending {
            match self
                .replacements
                .get(id)
                .or_else(|| graph.get(*id))
                .unwrap()
            {
                Object::Bytes(bytes) => {
                    self.data.bytes(&[4])?;
                    self.data.unsigned(bytes.len() as u64)?;
                    self.data.bytes(bytes)?;
                }
                Object::String(value) => {
                    self.data.bytes(&[0])?;
                    self.data.string(value)?;
                }
                Object::Record { class, fields } => {
                    if let Some(restored_class) = self.restore_classes.get(id) {
                        self.data.bytes(&[5])?;
                        self.data.string(restored_class)?;
                    } else {
                        self.data.bytes(&[1])?;
                    }
                    self.data.string(class)?;
                    if let Some(stamps) = self.stamps {
                        stamp_for(stamps, class)?.write(&mut self.data)?;
                    }
                    if let Some(codecs) = self.generated {
                        let codec = find_codec(codecs, class)?;
                        (codec.write)(
                            &mut GeneratedOutput {
                                data: &mut self.data,
                                ids: &self.ids,
                            },
                            fields,
                        )?;
                    } else {
                        Self::write_values(&mut self.data, &self.ids, fields)?;
                    }
                }
                Object::Array(values) => {
                    self.data.bytes(&[2])?;
                    Self::write_sequence(
                        &mut self.data,
                        &self.ids,
                        values,
                        self.generated.is_some(),
                    )?;
                }
                Object::Rectangular {
                    dimensions,
                    elements,
                } => {
                    self.data.bytes(&[3])?;
                    self.data.unsigned(dimensions.len() as u64)?;
                    for &size in dimensions {
                        self.data.unsigned(size as u64)?;
                    }
                    Self::write_sequence(
                        &mut self.data,
                        &self.ids,
                        elements,
                        self.generated.is_some(),
                    )?;
                }
            }
            if let Some(observer) = &mut self.after_write {
                observer(graph.get(*id).unwrap());
            }
        }
        Ok(())
    }

    fn write_sequence(
        data: &mut Writer<W>,
        ids: &HashMap<ObjectId, usize>,
        values: &[Value],
        generated: bool,
    ) -> io::Result<()> {
        if !generated {
            return Self::write_values(data, ids, values);
        }
        let mut output = GeneratedOutput { data, ids };
        output.count(values.len())?;
        for value in values {
            match value {
                Value::Integer(_) => output.integer(value)?,
                Value::Null | Value::Ref(_) => output.reference(value)?,
                _ => Self::write_value(output.data, output.ids, value)?,
            }
        }
        Ok(())
    }

    fn write_values(
        data: &mut Writer<W>,
        ids: &HashMap<ObjectId, usize>,
        values: &[Value],
    ) -> io::Result<()> {
        data.unsigned(values.len() as u64)?;
        for value in values {
            Self::write_value(data, ids, value)?;
        }
        Ok(())
    }

    fn write_value(
        data: &mut Writer<W>,
        ids: &HashMap<ObjectId, usize>,
        value: &Value,
    ) -> io::Result<()> {
        match value {
            Value::Null => data.bytes(&[0])?,
            Value::Integer(value) => {
                data.bytes(&[1])?;
                data.signed(*value)?;
            }
            Value::Float(value) => {
                data.bytes(&[2])?;
                data.bytes(&value.to_le_bytes())?;
            }
            Value::Guid(value) => {
                data.bytes(&[4])?;
                data.guid(*value)?;
            }
            Value::NativeResource(resource) => return Err(unsupported(*resource, Vec::new())),
            Value::Boolean(value) => data.bytes(&[7, u8::from(*value)])?,
            Value::Decimal(value) => {
                data.bytes(&[6])?;
                data.decimal(*value)?;
            }
            Value::BigInteger(bytes) => {
                data.bytes(&[5])?;
                data.unsigned(bytes.len() as u64)?;
                data.bytes(bytes)?;
            }
            Value::Ref(id) => {
                data.bytes(&[3])?;
                data.unsigned(ids[id] as u64)?;
            }
        }
        Ok(())
    }

    pub fn finish(self) -> io::Result<()> {
        self.data.finish()
    }
}

/// A callback applied after all records and surrogates in a frame are restored.
pub struct LateRecordHook<'a> {
    pub class: &'a str,
    pub apply: fn(&mut [Value]) -> io::Result<()>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ReadHookPhase {
    Immediate,
    Late,
}

pub struct ReadHook<'a> {
    pub class: &'a str,
    pub phase: ReadHookPhase,
    pub apply: fn(&mut RestoredFrame<'_>, ObjectId) -> io::Result<()>,
}

pub struct RestoredFrame<'a> {
    previous: &'a Graph,
    objects: &'a mut [Object],
}

impl RestoredFrame<'_> {
    pub fn get(&self, id: ObjectId) -> Option<&Object> {
        match id.0.checked_sub(self.previous.objects.len()) {
            Some(index) => self.objects.get(index),
            None => self.previous.get(id),
        }
    }

    pub fn fields_mut(&mut self, id: ObjectId) -> Option<&mut [Value]> {
        let index = id.0.checked_sub(self.previous.objects.len())?;
        match self.objects.get_mut(index)? {
            Object::Record { fields, .. } => Some(fields),
            _ => None,
        }
    }

    fn run(&mut self, hooks: &[ReadHook<'_>]) -> io::Result<()> {
        for phase in [ReadHookPhase::Immediate, ReadHookPhase::Late] {
            for index in (0..self.objects.len()).rev() {
                let Object::Record { class, .. } = &self.objects[index] else {
                    continue;
                };
                let class = class.clone();
                let id = ObjectId(self.previous.objects.len() + index);
                for hook in hooks
                    .iter()
                    .filter(|hook| hook.phase == phase && hook.class == class)
                {
                    (hook.apply)(self, id)?;
                }
            }
        }
        Ok(())
    }
}

pub struct ObjectReader<'a, 's, R: Read> {
    data: Reader<R>,
    graph: Graph,
    weak_history: Vec<Weak<Object>>,
    late_hooks: &'a [LateRecordHook<'a>],
    read_hooks: &'a [ReadHook<'a>],
    references: References,
    stamps: Option<&'a [TypeStamp<'a>]>,
    allow_xml_surrogates: bool,
    generated: Option<&'a [Codec]>,
    transients: Option<&'a Transients<'a>>,
    surrogates: Option<&'a mut Surrogates<'s>>,
    after_read: Option<ObjectObserver<'s>>,
}

impl<'a, 's, R: Read> ObjectReader<'a, 's, R> {
    pub fn new(stream: R, references: References, buffered: bool) -> Self {
        Self {
            data: Reader::new(stream, buffered),
            graph: Graph::default(),
            weak_history: Vec::new(),
            late_hooks: &[],
            read_hooks: &[],
            references,
            stamps: None,
            allow_xml_surrogates: true,
            generated: None,
            transients: None,
            surrogates: None,
            after_read: None,
        }
    }

    pub fn with_read_observer(mut self, observer: impl FnMut(&Object) + 's) -> Self {
        self.after_read = Some(Box::new(observer));
        self
    }

    pub fn with_read_hooks(mut self, hooks: &'a [ReadHook<'a>]) -> Self {
        self.read_hooks = hooks;
        self
    }

    pub fn with_late_hooks(mut self, hooks: &'a [LateRecordHook<'a>]) -> Self {
        self.late_hooks = hooks;
        self
    }

    pub fn with_surrogates(mut self, surrogates: &'a mut Surrogates<'s>) -> Self {
        self.surrogates = Some(surrogates);
        self
    }

    pub fn with_transients(mut self, transients: &'a Transients<'a>) -> Self {
        self.transients = Some(transients);
        self
    }

    pub(super) fn allow_xml_surrogates(mut self, allow: bool) -> Self {
        self.allow_xml_surrogates = allow;
        self
    }

    pub fn with_generated_records(mut self, codecs: &'a [Codec]) -> Self {
        self.generated = Some(codecs);
        self
    }

    pub fn with_type_stamps(mut self, stamps: &'a [TypeStamp<'a>]) -> Self {
        self.stamps = Some(stamps);
        self
    }

    pub fn graph(&self) -> &Graph {
        &self.graph
    }

    pub fn read_weak_object(&mut self) -> io::Result<RetainedRoot> {
        if self.references != References::Weak {
            return Err(invalid("reader is not in weak mode"));
        }
        self.graph.objects = self.weak_history.iter().map(Weak::upgrade).collect();
        let result = self.next_impl().and_then(|root| {
            let root = root.ok_or(io::ErrorKind::UnexpectedEof)?;
            let mut retained = RetainedRoot {
                id: root,
                objects: HashMap::new(),
            };
            let mut pending = vec![root];
            while let Some(id) = pending.pop() {
                if let Entry::Vacant(entry) = retained.objects.entry(id) {
                    let object = self
                        .graph
                        .objects
                        .get(id.0)
                        .and_then(Option::as_ref)
                        .ok_or_else(|| invalid("weak stream reference has expired"))?;
                    match object.as_ref() {
                        Object::Record { fields, .. }
                        | Object::Array(fields)
                        | Object::Rectangular {
                            elements: fields, ..
                        } => {
                            pending.extend(fields.iter().filter_map(|value| match value {
                                Value::Ref(id) => Some(*id),
                                _ => None,
                            }));
                        }
                        Object::String(_) | Object::Bytes(_) => {}
                    }
                    entry.insert(Arc::clone(object));
                }
            }
            Ok(retained)
        });
        self.weak_history = self
            .graph
            .objects
            .drain(..)
            .map(|object| object.as_ref().map_or_else(Weak::new, Arc::downgrade))
            .collect();
        result
    }

    pub fn read_object(&mut self) -> io::Result<ObjectId> {
        if self.references == References::Weak {
            return Err(invalid("weak mode requires read_weak_object"));
        }
        self.next_impl()?
            .ok_or_else(|| io::ErrorKind::UnexpectedEof.into())
    }

    pub fn read_many(&mut self) -> impl Iterator<Item = io::Result<ObjectId>> + '_ {
        let mut done = false;
        std::iter::from_fn(move || {
            if done {
                return None;
            }
            let result = if self.references == References::Weak {
                Err(invalid("weak mode requires read_weak_object"))
            } else {
                self.next_impl()
            };
            match result {
                Ok(Some(id)) => Some(Ok(id)),
                Ok(None) => {
                    done = true;
                    None
                }
                Err(error) => {
                    done = true;
                    Some(Err(error))
                }
            }
        })
    }

    fn next_impl(&mut self) -> io::Result<Option<ObjectId>> {
        match self.data.byte() {
            Ok(ROOT) if self.stamps.is_none() => {}
            Ok(STAMPED_ROOT) if self.stamps.is_some() => {}
            Ok(ROOT | STAMPED_ROOT) => {
                return Err(invalid("inconsistent object stream stamp configuration"));
            }
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(error) => return Err(error),
            Ok(_) => return Err(invalid("invalid object record marker")),
        }
        let old_len = self.graph.objects.len();
        let base = if self.references != References::PerRoot {
            0
        } else {
            old_len
        };
        let root = self.count()?;
        let count = self.count()?;
        let end = old_len
            .checked_add(count)
            .ok_or_else(|| invalid("object limit exceeded"))?;
        let limit = end - base;
        if limit > MAX_ITEMS || root >= limit {
            return Err(invalid("invalid root reference or object limit exceeded"));
        }
        let mut objects = Vec::new();
        let mut restore_classes = HashMap::new();
        for index in 0..count {
            let kind = self.data.byte()?;
            objects.push(match kind {
                0 => Object::String(self.data.string()?),
                1 | 5 => {
                    if kind == 5 {
                        restore_classes.insert(index, self.data.string()?);
                    }
                    let class = self.data.string()?;
                    if !self.allow_xml_surrogates && class == super::xml_surrogate::CLASS {
                        return Err(super::xml_surrogate::disabled());
                    }
                    let stamp = self
                        .stamps
                        .map(|stamps| stamp_for(stamps, &class))
                        .transpose()?;
                    if let Some(stamp) = stamp {
                        stamp.verify(&mut self.data)?;
                    }
                    let mut fields = if let Some(codecs) = self.generated {
                        let codec = find_codec(codecs, &class)?;
                        (codec.read)(&mut GeneratedInput {
                            data: &mut self.data,
                            base,
                            limit,
                        })?
                    } else {
                        self.read_values(base, limit)?
                    };
                    if let Some(stamp) = stamp {
                        stamp.validate(&fields)?;
                    }
                    if let Some(transients) = self.transients {
                        transients.restore_fields(&class, &mut fields)?;
                    }
                    Object::Record { class, fields }
                }
                2 => Object::Array(self.read_sequence(base, limit)?),
                3 => {
                    let rank = self.count()?;
                    if !(2..=32).contains(&rank) {
                        return Err(invalid("invalid rectangular array rank"));
                    }
                    let dimensions = (0..rank)
                        .map(|_| self.count())
                        .collect::<io::Result<Vec<_>>>()?;
                    let elements = self.read_sequence(base, limit)?;
                    check_rectangular(&dimensions, &elements)?;
                    Object::Rectangular {
                        dimensions,
                        elements,
                    }
                }
                4 => {
                    let mut bytes = vec![0; self.count()?];
                    self.data.bytes(&mut bytes)?;
                    Object::Bytes(bytes)
                }
                _ => return Err(invalid("unknown object kind")),
            });
        }
        if let Some(surrogates) = self.surrogates.as_mut() {
            for (index, object) in objects.iter_mut().enumerate().rev() {
                let id = ObjectId(old_len + index);
                if let Object::Record { class, .. } = object {
                    let has_late_hook = self.late_hooks.iter().any(|hook| &hook.class == class)
                        || self
                            .read_hooks
                            .iter()
                            .any(|hook| hook.phase == ReadHookPhase::Late && &hook.class == class);
                    if has_late_hook && surrogates.has_conversion(class) {
                        return Err(invalid(
                            "late post-deserialization hook cannot be used with a surrogate",
                        ));
                    }
                }
                if let Some(replacement) = surrogates.convert(id, object)? {
                    if let Some(expected) = restore_classes.remove(&index) {
                        if !matches!(&replacement, Object::Record { class, .. } if *class == expected)
                        {
                            return Err(invalid("restored class differs from stream prototype"));
                        }
                    }
                    let fields = match &replacement {
                        Object::String(_) | Object::Bytes(_) => &[][..],
                        Object::Record { fields, .. } | Object::Array(fields) => fields,
                        Object::Rectangular {
                            dimensions,
                            elements,
                        } => {
                            check_rectangular(dimensions, elements)?;
                            elements
                        }
                    };
                    if fields.iter().any(
                        |field| matches!(field, Value::Ref(id) if !(base..end).contains(&id.0)),
                    ) {
                        return Err(invalid("surrogate reference outside stream table"));
                    }
                    *object = replacement;
                }
            }
        }
        if !restore_classes.is_empty() {
            return Err(invalid("missing restore factory for stream prototype"));
        }
        if !self.read_hooks.is_empty() {
            RestoredFrame {
                previous: &self.graph,
                objects: &mut objects,
            }
            .run(self.read_hooks)?;
        }
        if !self.late_hooks.is_empty() || !self.read_hooks.is_empty() {
            for object in objects.iter_mut().rev() {
                if let Object::Record { class, fields } = object {
                    for hook in self.late_hooks.iter().filter(|hook| hook.class == class) {
                        (hook.apply)(fields)?;
                    }
                    if fields.iter().any(
                        |value| matches!(value, Value::Ref(id) if !(base..end).contains(&id.0)),
                    ) {
                        return Err(invalid("hook reference outside stream table"));
                    }
                }
            }
        }
        if let Some(observer) = &mut self.after_read {
            for object in objects.iter().rev() {
                observer(object);
            }
        }
        // Publish all objects together after validating every edge.
        self.graph
            .objects
            .extend(objects.into_iter().map(|object| Some(Arc::new(object))));
        if let Some(surrogates) = self.surrogates.as_mut() {
            surrogates.close_registration();
        }
        Ok(Some(ObjectId(base + root)))
    }

    fn count(&mut self) -> io::Result<usize> {
        usize::try_from(self.data.unsigned()?)
            .ok()
            .filter(|&count| count <= MAX_ITEMS)
            .ok_or_else(|| invalid("object stream item limit exceeded"))
    }

    fn read_sequence(&mut self, base: usize, limit: usize) -> io::Result<Vec<Value>> {
        if self.generated.is_none() {
            return self.read_values(base, limit);
        }
        let count = self.count()?;
        let mut values = Vec::with_capacity(count);
        for _ in 0..count {
            let tag = self.data.byte()?;
            values.push(match tag {
                0 => Value::Null,
                1 => Value::Integer(self.data.signed()?),
                3 => read_reference(&mut self.data, base, limit)?,
                _ => self.read_value(tag, base, limit)?,
            });
        }
        Ok(values)
    }

    fn read_values(&mut self, base: usize, limit: usize) -> io::Result<Vec<Value>> {
        let count = self.count()?;
        let mut values = Vec::new();
        for _ in 0..count {
            let tag = self.data.byte()?;
            values.push(self.read_value(tag, base, limit)?);
        }
        Ok(values)
    }

    fn read_value(&mut self, tag: u8, base: usize, limit: usize) -> io::Result<Value> {
        Ok(match tag {
            0 => Value::Null,
            1 => Value::Integer(self.data.signed()?),
            2 => {
                let mut bytes = [0; 8];
                self.data.bytes(&mut bytes)?;
                Value::Float(f64::from_le_bytes(bytes))
            }
            3 => read_reference(&mut self.data, base, limit)?,
            4 => Value::Guid(self.data.guid()?),
            5 => {
                let length = self.count()?;
                if length == 0 {
                    return Err(invalid("invalid big integer length"));
                }
                let mut bytes = vec![0; length];
                self.data.bytes(&mut bytes)?;
                Value::BigInteger(bytes)
            }
            6 => Value::Decimal(self.data.decimal()?),
            7 => match self.data.byte()? {
                0 => Value::Boolean(false),
                1 => Value::Boolean(true),
                _ => return Err(invalid("invalid boolean value")),
            },
            _ => return Err(invalid("unknown value kind")),
        })
    }

    pub(super) fn finish_into_graph(self) -> io::Result<Graph> {
        self.data.finish()?;
        Ok(self.graph)
    }

    pub fn finish(self) -> io::Result<()> {
        self.data.finish()
    }
}

fn find_codec<'a>(codecs: &'a [Codec], class: &str) -> io::Result<&'a Codec> {
    codecs
        .iter()
        .find(|codec| codec.class == class)
        .ok_or_else(|| invalid("no generated codec for record"))
}

struct ValidateGeneratedFields;

impl RecordOutput for ValidateGeneratedFields {
    fn count(&mut self, _: usize) -> io::Result<()> {
        Ok(())
    }
    fn boolean(&mut self, value: &Value) -> io::Result<()> {
        if matches!(value, Value::Boolean(_)) {
            Ok(())
        } else {
            Err(invalid("expected generated boolean field"))
        }
    }

    fn nullable_integer(&mut self, value: &Value) -> io::Result<()> {
        if matches!(value, Value::Null) {
            Ok(())
        } else {
            self.integer(value)
        }
    }

    fn integer(&mut self, value: &Value) -> io::Result<()> {
        if matches!(value, Value::Integer(_)) {
            Ok(())
        } else {
            Err(invalid("expected generated integer field"))
        }
    }
    fn float(&mut self, value: &Value) -> io::Result<()> {
        if matches!(value, Value::Float(_)) {
            Ok(())
        } else {
            Err(invalid("expected generated float field"))
        }
    }

    fn decimal(&mut self, value: &Value) -> io::Result<()> {
        if matches!(value, Value::Decimal(_)) {
            Ok(())
        } else {
            Err(invalid("expected generated decimal field"))
        }
    }

    fn guid(&mut self, value: &Value) -> io::Result<()> {
        if matches!(value, Value::Guid(_)) {
            Ok(())
        } else {
            Err(invalid("expected generated GUID field"))
        }
    }

    fn big_integer(&mut self, value: &Value) -> io::Result<()> {
        if matches!(value, Value::BigInteger(bytes) if !bytes.is_empty() && bytes.len() <= MAX_ITEMS)
        {
            Ok(())
        } else {
            Err(invalid("invalid generated BigInteger field"))
        }
    }

    fn reference(&mut self, value: &Value) -> io::Result<()> {
        if matches!(value, Value::Null | Value::Ref(_)) {
            Ok(())
        } else {
            Err(invalid("expected generated reference field"))
        }
    }
}

struct GeneratedOutput<'a, W: Write> {
    data: &'a mut Writer<W>,
    ids: &'a HashMap<ObjectId, usize>,
}
impl<W: Write> RecordOutput for GeneratedOutput<'_, W> {
    fn count(&mut self, count: usize) -> io::Result<()> {
        self.data.unsigned(count as u64)
    }

    fn boolean(&mut self, value: &Value) -> io::Result<()> {
        let Value::Boolean(value) = value else {
            return Err(invalid("expected generated boolean field"));
        };
        self.data.bytes(&[7, u8::from(*value)])
    }

    fn nullable_integer(&mut self, value: &Value) -> io::Result<()> {
        if matches!(value, Value::Null) {
            self.data.bytes(&[0])
        } else {
            self.integer(value)
        }
    }

    fn integer(&mut self, value: &Value) -> io::Result<()> {
        let Value::Integer(value) = value else {
            return Err(invalid("expected generated integer field"));
        };
        self.data.bytes(&[1])?;
        self.data.signed(*value)
    }

    fn float(&mut self, value: &Value) -> io::Result<()> {
        let Value::Float(value) = value else {
            return Err(invalid("expected generated float field"));
        };
        self.data.bytes(&[2])?;
        self.data.bytes(&value.to_le_bytes())
    }

    fn decimal(&mut self, value: &Value) -> io::Result<()> {
        let Value::Decimal(value) = value else {
            return Err(invalid("expected generated decimal field"));
        };
        self.data.bytes(&[6])?;
        self.data.decimal(*value)
    }

    fn guid(&mut self, value: &Value) -> io::Result<()> {
        let Value::Guid(value) = value else {
            return Err(invalid("expected generated GUID field"));
        };
        self.data.bytes(&[4])?;
        self.data.guid(*value)
    }

    fn big_integer(&mut self, value: &Value) -> io::Result<()> {
        ValidateGeneratedFields.big_integer(value)?;
        let Value::BigInteger(bytes) = value else {
            unreachable!()
        };
        self.data.bytes(&[5])?;
        self.data.unsigned(bytes.len() as u64)?;
        self.data.bytes(bytes)
    }

    fn reference(&mut self, value: &Value) -> io::Result<()> {
        match value {
            Value::Null => self.data.bytes(&[0]),
            Value::Ref(id) => {
                let id = self
                    .ids
                    .get(id)
                    .ok_or_else(|| invalid("unknown generated reference"))?;
                self.data.bytes(&[3])?;
                self.data.unsigned(*id as u64)
            }
            _ => Err(invalid("expected generated reference field")),
        }
    }
}

struct GeneratedInput<'a, R: Read> {
    data: &'a mut Reader<R>,
    base: usize,
    limit: usize,
}
impl<R: Read> RecordInput for GeneratedInput<'_, R> {
    fn expect_count(&mut self, count: usize) -> io::Result<()> {
        if self.data.unsigned()? != count as u64 {
            return Err(invalid("incorrect generated record field count"));
        }
        Ok(())
    }

    fn boolean(&mut self) -> io::Result<Value> {
        if self.data.byte()? != 7 {
            return Err(invalid("expected generated boolean field"));
        }
        match self.data.byte()? {
            0 => Ok(Value::Boolean(false)),
            1 => Ok(Value::Boolean(true)),
            _ => Err(invalid("invalid boolean value")),
        }
    }

    fn nullable_integer(&mut self) -> io::Result<Value> {
        match self.data.byte()? {
            0 => Ok(Value::Null),
            1 => Ok(Value::Integer(self.data.signed()?)),
            _ => Err(invalid("expected generated nullable integer field")),
        }
    }

    fn integer(&mut self) -> io::Result<Value> {
        if self.data.byte()? != 1 {
            return Err(invalid("expected generated integer field"));
        }
        Ok(Value::Integer(self.data.signed()?))
    }

    fn float(&mut self) -> io::Result<Value> {
        if self.data.byte()? != 2 {
            return Err(invalid("expected generated float field"));
        }
        let mut bytes = [0; 8];
        self.data.bytes(&mut bytes)?;
        Ok(Value::Float(f64::from_le_bytes(bytes)))
    }

    fn decimal(&mut self) -> io::Result<Value> {
        if self.data.byte()? != 6 {
            return Err(invalid("expected generated decimal field"));
        }
        Ok(Value::Decimal(self.data.decimal()?))
    }

    fn guid(&mut self) -> io::Result<Value> {
        if self.data.byte()? != 4 {
            return Err(invalid("expected generated GUID field"));
        }
        Ok(Value::Guid(self.data.guid()?))
    }

    fn big_integer(&mut self) -> io::Result<Value> {
        if self.data.byte()? != 5 {
            return Err(invalid("expected generated BigInteger field"));
        }
        let count = self.data.unsigned()?;
        if count == 0 || count > MAX_ITEMS as u64 {
            return Err(invalid("invalid BigInteger field length"));
        }
        let mut bytes = vec![0; count as usize];
        self.data.bytes(&mut bytes)?;
        Ok(Value::BigInteger(bytes))
    }

    fn reference(&mut self) -> io::Result<Value> {
        match self.data.byte()? {
            0 => Ok(Value::Null),
            3 => read_reference(self.data, self.base, self.limit),
            _ => Err(invalid("expected generated reference field")),
        }
    }
}

fn read_reference(data: &mut Reader<impl Read>, base: usize, limit: usize) -> io::Result<Value> {
    let id = data.unsigned()?;
    if id >= limit as u64 {
        return Err(invalid("object reference outside stream table"));
    }
    Ok(Value::Ref(ObjectId(base + id as usize)))
}
