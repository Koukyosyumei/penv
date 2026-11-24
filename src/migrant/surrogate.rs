//! Exact-type surrogate conversions for the graph stream.
use super::object_stream::{Object, ObjectId};
use std::collections::HashMap;
use std::io;

type Converter<'a> = Box<dyn FnMut(ObjectId, &Object) -> io::Result<Object> + 'a>;

#[derive(Default)]
pub struct Surrogates<'a> {
    converters: HashMap<String, Converter<'a>>,
}

impl<'a> Surrogates<'a> {
    pub fn register(
        &mut self,
        class: impl Into<String>,
        converter: impl FnMut(ObjectId, &Object) -> io::Result<Object> + 'a,
    ) {
        self.converters.insert(class.into(), Box::new(converter));
    }

    pub(super) fn convert(&mut self, id: ObjectId, object: &Object) -> io::Result<Option<Object>> {
        let Object::Record { class, .. } = object else {
            return Ok(None);
        };
        self.converters
            .get_mut(class)
            .map(|convert| convert(id, object))
            .transpose()
    }
}
