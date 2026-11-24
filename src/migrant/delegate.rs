use super::object_stream::{Graph, Object, ObjectId, Value};
use std::io;

pub const ENTRY: &str = "DelegateEntry";
pub type Callback = fn(&mut Graph, ObjectId) -> io::Result<()>;

/// Save a receiver together with the registered name of its method.
pub fn bind(graph: &mut Graph, method: ObjectId, target: ObjectId) -> Value {
    Value::Ref(graph.insert(Object::Record {
        class: ENTRY.into(),
        fields: vec![Value::Ref(method), Value::Ref(target)],
    }))
}

/// Invoke entries in subscription order, omitting transient receivers.
pub fn invoke(graph: &mut Graph, list: ObjectId, methods: &[(&str, Callback)]) -> io::Result<()> {
    let invalid = || {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid delegate invocation list",
        )
    };
    let Some(Object::Array(entries)) = graph.get(list) else {
        return Err(invalid());
    };
    let mut calls = Vec::with_capacity(entries.len());
    for entry in entries {
        let Value::Ref(entry) = entry else {
            return Err(invalid());
        };
        let Some(Object::Record { class, fields }) = graph.get(*entry) else {
            return Err(invalid());
        };
        if class != ENTRY {
            return Err(invalid());
        }
        let target = match fields.as_slice() {
            [Value::Ref(_), Value::Null] => continue,
            [Value::Ref(method), Value::Ref(target)] => {
                let Some(Object::String(name)) = graph.get(*method) else {
                    return Err(invalid());
                };
                let (_, callback) = methods
                    .iter()
                    .find(|(method, _)| method == name)
                    .ok_or_else(invalid)?;
                (*target, *callback)
            }
            _ => return Err(invalid()),
        };
        calls.push(target);
    }
    for (target, callback) in calls {
        callback(graph, target)?;
    }
    Ok(())
}
