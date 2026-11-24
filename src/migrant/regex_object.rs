//! Stream representation of a compiled regular expression.
use super::object_stream::{FieldKind, Graph, Object, ObjectId, TypeStamp, Value};
use regex::Regex;
use std::io;

const CLASS: &str = "System.Text.RegularExpressions.Regex";

pub const TYPE_STAMP: TypeStamp<'static> = TypeStamp {
    class: CLASS,
    module: "System.Text.RegularExpressions",
    revision: 0,
    fields: &[("pattern", FieldKind::Reference)],
};

pub fn store(graph: &mut Graph, value: &Regex) -> ObjectId {
    let pattern = graph.insert(Object::String(value.as_str().into()));
    graph.insert(Object::Record {
        class: CLASS.into(),
        fields: vec![Value::Ref(pattern)],
    })
}

pub fn restore(graph: &Graph, id: ObjectId) -> io::Result<Regex> {
    let invalid = || {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid regular expression record",
        )
    };
    let Some(Object::Record { class, fields }) = graph.get(id) else {
        return Err(invalid());
    };
    let [Value::Ref(pattern)] = fields.as_slice() else {
        return Err(invalid());
    };
    if class != CLASS {
        return Err(invalid());
    }
    let Some(Object::String(pattern)) = graph.get(*pattern) else {
        return Err(invalid());
    };
    Regex::new(pattern).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}
