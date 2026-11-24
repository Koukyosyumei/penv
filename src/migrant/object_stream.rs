//! Reference-preserving object streams for Rust graph descriptors.
//!
//! This uses Migrant's primitive encoding with a Rust object-record format.
use super::primitive::{Reader, Writer};
use super::surrogate::Surrogates;
use std::collections::{HashMap, hash_map::Entry};
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
                        | (FieldKind::Float, Value::Float(_))
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

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Integer(i64),
    Float(f64),
    Ref(ObjectId),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Object {
    String(String),
    Record { class: String, fields: Vec<Value> },
    Array(Vec<Value>),
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
    surrogates: Option<&'a mut Surrogates<'s>>,
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
            surrogates: None,
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

    pub fn with_surrogates(mut self, surrogates: &'a mut Surrogates<'s>) -> Self {
        self.surrogates = Some(surrogates);
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

    fn write_from(&mut self, graph: &Graph, root: ObjectId) -> io::Result<()> {
        if self.references != References::Preserve {
            self.ids.clear();
        }
        self.frame_base = self.ids.len();
        self.pending.clear();
        self.replacements.clear();
        let root = match self.discover(graph, root) {
            Ok(root) => root,
            Err(error) => {
                for id in self.pending.drain(..) {
                    self.ids.remove(&id);
                }
                return Err(error);
            }
        };
        self.write_frame(graph, root)
    }

    fn discover(&mut self, graph: &Graph, root: ObjectId) -> io::Result<usize> {
        let root = self.intern(graph, root)?;
        let mut index = 0;
        // Reserve identities before visiting children, including forward and cyclic edges.
        while let Some(id) = self.pending.get(index).copied() {
            let original = graph.get(id).unwrap();
            let replacement = self
                .surrogates
                .as_mut()
                .map(|surrogates| surrogates.convert(id, original))
                .transpose()?
                .flatten();
            let object = replacement.as_ref().unwrap_or(original);
            let values = match object {
                Object::String(_) => &[],
                Object::Record { class, fields } => {
                    if let Some(stamps) = self.stamps {
                        stamp_for(stamps, class)?.validate(fields)?;
                    }
                    fields.as_slice()
                }
                Object::Array(fields) => fields.as_slice(),
            };
            if values.len() > MAX_ITEMS {
                return Err(invalid("object field limit exceeded"));
            }
            for value in values {
                if let Value::Ref(id) = value {
                    self.intern(graph, *id)?;
                }
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
                Object::String(value) => {
                    self.data.bytes(&[0])?;
                    self.data.string(value)?;
                }
                Object::Record { class, fields } => {
                    self.data.bytes(&[1])?;
                    self.data.string(class)?;
                    if let Some(stamps) = self.stamps {
                        stamp_for(stamps, class)?.write(&mut self.data)?;
                    }
                    Self::write_values(&mut self.data, &self.ids, fields)?;
                }
                Object::Array(values) => {
                    self.data.bytes(&[2])?;
                    Self::write_values(&mut self.data, &self.ids, values)?;
                }
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
                Value::Ref(id) => {
                    data.bytes(&[3])?;
                    data.unsigned(ids[id] as u64)?;
                }
            }
        }
        Ok(())
    }

    pub fn finish(self) -> io::Result<()> {
        self.data.finish()
    }
}

pub struct ObjectReader<'a, 's, R: Read> {
    data: Reader<R>,
    graph: Graph,
    weak_history: Vec<Weak<Object>>,
    references: References,
    stamps: Option<&'a [TypeStamp<'a>]>,
    surrogates: Option<&'a mut Surrogates<'s>>,
}

impl<'a, 's, R: Read> ObjectReader<'a, 's, R> {
    pub fn new(stream: R, references: References, buffered: bool) -> Self {
        Self {
            data: Reader::new(stream, buffered),
            graph: Graph::default(),
            weak_history: Vec::new(),
            references,
            stamps: None,
            surrogates: None,
        }
    }

    pub fn with_surrogates(mut self, surrogates: &'a mut Surrogates<'s>) -> Self {
        self.surrogates = Some(surrogates);
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
                        Object::Record { fields, .. } | Object::Array(fields) => {
                            pending.extend(fields.iter().filter_map(|value| match value {
                                Value::Ref(id) => Some(*id),
                                _ => None,
                            }));
                        }
                        Object::String(_) => {}
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
        for _ in 0..count {
            objects.push(match self.data.byte()? {
                0 => Object::String(self.data.string()?),
                1 => {
                    let class = self.data.string()?;
                    let stamp = self
                        .stamps
                        .map(|stamps| stamp_for(stamps, &class))
                        .transpose()?;
                    if let Some(stamp) = stamp {
                        stamp.verify(&mut self.data)?;
                    }
                    let fields = self.read_values(base, limit)?;
                    if let Some(stamp) = stamp {
                        stamp.validate(&fields)?;
                    }
                    Object::Record { class, fields }
                }
                2 => Object::Array(self.read_values(base, limit)?),
                _ => return Err(invalid("unknown object kind")),
            });
        }
        if let Some(surrogates) = self.surrogates.as_mut() {
            for (index, object) in objects.iter_mut().enumerate() {
                let id = ObjectId(old_len + index);
                if let Some(replacement) = surrogates.convert(id, object)? {
                    let fields = match &replacement {
                        Object::String(_) => &[][..],
                        Object::Record { fields, .. } | Object::Array(fields) => fields,
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
        // Publish all objects together after validating every edge.
        self.graph
            .objects
            .extend(objects.into_iter().map(|object| Some(Arc::new(object))));
        Ok(Some(ObjectId(base + root)))
    }

    fn count(&mut self) -> io::Result<usize> {
        usize::try_from(self.data.unsigned()?)
            .ok()
            .filter(|&count| count <= MAX_ITEMS)
            .ok_or_else(|| invalid("object stream item limit exceeded"))
    }

    fn read_values(&mut self, base: usize, limit: usize) -> io::Result<Vec<Value>> {
        let count = self.count()?;
        let mut values = Vec::new();
        for _ in 0..count {
            values.push(match self.data.byte()? {
                0 => Value::Null,
                1 => Value::Integer(self.data.signed()?),
                2 => {
                    let mut bytes = [0; 8];
                    self.data.bytes(&mut bytes)?;
                    Value::Float(f64::from_le_bytes(bytes))
                }
                3 => {
                    let id = self.count()?;
                    if id >= limit {
                        return Err(invalid("object reference outside stream table"));
                    }
                    Value::Ref(ObjectId(base + id))
                }
                _ => return Err(invalid("unknown value kind")),
            });
        }
        Ok(values)
    }

    pub fn finish(self) -> io::Result<()> {
        self.data.finish()
    }
}
