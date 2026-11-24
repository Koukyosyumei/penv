//! Surrogate selection for graph records with explicitly registered type relationships.
use super::object_stream::{Object, ObjectId};
use std::collections::{HashMap, HashSet};
use std::io;

type Converter<'a> = Box<dyn FnMut(ObjectId, &Object) -> io::Result<Object> + 'a>;

struct Registration<'a> {
    class: String,
    converter: Option<Converter<'a>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RegistrationClosed;

#[derive(Default)]
pub struct Surrogates<'a> {
    parents: HashMap<String, Vec<String>>,
    registrations: Vec<Registration<'a>>,
    closed: bool,
}

impl<'a> Surrogates<'a> {
    /// Describe direct matches such as a base class, an implemented interface,
    /// or the generic definition of a closed type.
    pub fn describe(&mut self, class: &str, parents: &[&str]) -> Result<(), RegistrationClosed> {
        self.check_open()?;
        self.parents.insert(
            class.into(),
            parents.iter().map(|parent| (*parent).into()).collect(),
        );
        Ok(())
    }

    pub fn register(
        &mut self,
        class: impl Into<String>,
        converter: impl FnMut(ObjectId, &Object) -> io::Result<Object> + 'a,
    ) -> Result<(), RegistrationClosed> {
        self.set(class.into(), Some(Box::new(converter)))
    }

    /// Convert a closed generic record using the same type arguments on a new definition.
    pub fn register_generic_record(
        &mut self,
        source_definition: &str,
        target_definition: &str,
        mut construct: impl FnMut(ObjectId, &Object) -> io::Result<Vec<super::object_stream::Value>>
        + 'a,
    ) -> Result<(), RegistrationClosed> {
        let target = target_definition.to_owned();
        self.register(source_definition, move |id, object| {
            let Object::Record { class, .. } = object else {
                unreachable!()
            };
            let arguments = class
                .split_once('<')
                .and_then(|(_, arguments)| arguments.strip_suffix('>'))
                .filter(|arguments| !arguments.is_empty())
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "expected a closed generic record",
                    )
                })?;
            Ok(Object::Record {
                class: format!("{target}<{arguments}>"),
                fields: construct(id, object)?,
            })
        })
    }

    pub fn registration_count(&self) -> usize {
        self.registrations.len()
    }

    /// Stop fallback to a more general registration when this type matches.
    pub fn disable(&mut self, class: impl Into<String>) -> Result<(), RegistrationClosed> {
        self.set(class.into(), None)
    }

    pub(super) fn close_registration(&mut self) {
        self.closed = true;
    }

    fn check_open(&self) -> Result<(), RegistrationClosed> {
        if self.closed {
            Err(RegistrationClosed)
        } else {
            Ok(())
        }
    }

    fn set(
        &mut self,
        class: String,
        converter: Option<Converter<'a>>,
    ) -> Result<(), RegistrationClosed> {
        self.check_open()?;
        if let Some(existing) = self
            .registrations
            .iter_mut()
            .find(|item| item.class == class)
        {
            existing.converter = converter;
        } else {
            self.registrations.push(Registration { class, converter });
        }
        Ok(())
    }

    fn matches(&self, candidate: &str, class: &str) -> bool {
        let mut pending = vec![class];
        let mut seen = HashSet::new();
        while let Some(current) = pending.pop() {
            if current == candidate {
                return true;
            }
            if seen.insert(current) {
                if let Some(parents) = self.parents.get(current) {
                    pending.extend(parents.iter().map(String::as_str));
                }
            }
        }
        false
    }

    fn find(&self, class: &str) -> Option<usize> {
        // Match the upstream insertion order: insert each specialization before
        // the first existing registration that also matches it.
        let mut candidates: Vec<usize> = Vec::new();
        for (index, item) in self.registrations.iter().enumerate() {
            if !self.matches(&item.class, class) {
                continue;
            }
            let position = candidates
                .iter()
                .position(|&previous| {
                    self.matches(&self.registrations[previous].class, &item.class)
                })
                .unwrap_or(candidates.len());
            candidates.insert(position, index);
        }
        candidates.first().copied()
    }

    pub(super) fn has_conversion(&self, class: &str) -> bool {
        self.find(class)
            .is_some_and(|index| self.registrations[index].converter.is_some())
    }

    pub(super) fn convert(&mut self, id: ObjectId, object: &Object) -> io::Result<Option<Object>> {
        let Object::Record { class, .. } = object else {
            return Ok(None);
        };
        let Some(index) = self.find(class) else {
            return Ok(None);
        };
        self.registrations[index]
            .converter
            .as_mut()
            .map(|convert| convert(id, object))
            .transpose()
    }
}
