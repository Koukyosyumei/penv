//! Surrogate selection for graph records with explicitly registered type relationships.
use super::object_stream::{Object, ObjectId};
use std::collections::{HashMap, HashSet};
use std::io;

type Converter<'a> = Box<dyn FnMut(ObjectId, &Object) -> io::Result<Object> + 'a>;

struct Registration<'a> {
    class: String,
    converter: Option<Converter<'a>>,
}

#[derive(Default)]
pub struct Surrogates<'a> {
    parents: HashMap<String, Vec<String>>,
    registrations: Vec<Registration<'a>>,
}

impl<'a> Surrogates<'a> {
    /// Describe direct matches such as a base class, an implemented interface,
    /// or the generic definition of a closed type.
    pub fn describe(&mut self, class: &str, parents: &[&str]) {
        self.parents.insert(
            class.into(),
            parents.iter().map(|parent| (*parent).into()).collect(),
        );
    }

    pub fn register(
        &mut self,
        class: impl Into<String>,
        converter: impl FnMut(ObjectId, &Object) -> io::Result<Object> + 'a,
    ) {
        self.set(class.into(), Some(Box::new(converter)));
    }

    /// Stop fallback to a more general registration when this type matches.
    pub fn disable(&mut self, class: impl Into<String>) {
        self.set(class.into(), None);
    }

    fn set(&mut self, class: String, converter: Option<Converter<'a>>) {
        if let Some(existing) = self
            .registrations
            .iter_mut()
            .find(|item| item.class == class)
        {
            existing.converter = converter;
        } else {
            self.registrations.push(Registration { class, converter });
        }
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
