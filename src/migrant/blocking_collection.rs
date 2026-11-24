use std::{
    collections::VecDeque,
    io,
    sync::{Condvar, Mutex},
};

use super::object_stream::{FieldKind, Graph, Object, ObjectId, TypeStamp, Value};

#[derive(Default)]
pub struct BlockingCollection {
    entries: Mutex<VecDeque<Value>>,
    ready: Condvar,
}

impl BlockingCollection {
    pub fn add(&self, value: Value) {
        self.entries.lock().unwrap().push_back(value);
        self.ready.notify_one();
    }

    pub fn take(&self) -> Value {
        let entries = self.entries.lock().unwrap();
        self.ready
            .wait_while(entries, |entries| entries.is_empty())
            .unwrap()
            .pop_front()
            .unwrap()
    }

    pub fn snapshot(&self) -> Vec<Value> {
        self.entries.lock().unwrap().iter().cloned().collect()
    }

    pub fn store(&self, graph: &mut Graph, class: &str) -> ObjectId {
        let entries = graph.insert(Object::Array(self.snapshot()));
        graph.insert(Object::Record {
            class: class.into(),
            fields: vec![Value::Ref(entries)],
        })
    }

    pub fn restore(graph: &Graph, id: ObjectId, class: &str) -> io::Result<Self> {
        let invalid = || io::Error::new(io::ErrorKind::InvalidData, "invalid blocking collection");
        let Some(Object::Record {
            class: stored_class,
            fields,
        }) = graph.get(id)
        else {
            return Err(invalid());
        };
        let [Value::Ref(entries)] = fields.as_slice() else {
            return Err(invalid());
        };
        let Some(Object::Array(values)) = graph.get(*entries) else {
            return Err(invalid());
        };
        if stored_class != class {
            return Err(invalid());
        }
        let copy = Self::default();
        for value in values {
            copy.add(value.clone());
        }
        Ok(copy)
    }
}

pub fn type_stamp(class: &str) -> TypeStamp<'_> {
    TypeStamp {
        class,
        module: "System",
        revision: 0,
        fields: &[("items", FieldKind::Reference)],
    }
}
