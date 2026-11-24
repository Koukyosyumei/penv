use super::object_stream::{FieldKind, Graph, Object, ObjectId, TypeStamp, Value};
use std::io;

/// Field layout for a list serialized as a user object.
pub struct UserList {
    items: Vec<Value>,
    capacity: usize,
    version: i32,
    default: Value,
}

impl UserList {
    pub fn new(default: Value) -> Self {
        Self {
            items: Vec::new(),
            capacity: 0,
            version: 0,
            default,
        }
    }

    pub fn push(&mut self, value: Value) {
        if self.items.len() == self.capacity {
            self.capacity = if self.capacity == 0 {
                4
            } else {
                self.capacity * 2
            };
        }
        self.items.push(value);
        self.version = self.version.wrapping_add(1);
    }

    pub fn store(self, graph: &mut Graph, class: &str) -> ObjectId {
        let count = self.items.len();
        let mut backing = self.items;
        backing.resize(self.capacity, self.default);
        let backing = graph.insert(Object::Array(backing));
        graph.insert(Object::Record {
            class: class.into(),
            fields: vec![
                Value::Ref(backing),
                Value::Integer(count as i64),
                Value::Integer(self.version.into()),
            ],
        })
    }

    pub fn restore<'a>(graph: &'a Graph, id: ObjectId, class: &str) -> io::Result<&'a [Value]> {
        let invalid = || io::Error::new(io::ErrorKind::InvalidData, "invalid user list record");
        let Some(Object::Record {
            class: actual,
            fields,
        }) = graph.get(id)
        else {
            return Err(invalid());
        };
        let [Value::Ref(items), Value::Integer(count), Value::Integer(version)] = fields.as_slice()
        else {
            return Err(invalid());
        };
        if actual != class || i32::try_from(*version).is_err() {
            return Err(invalid());
        }
        let Some(Object::Array(items)) = graph.get(*items) else {
            return Err(invalid());
        };
        let count = usize::try_from(*count).map_err(|_| invalid())?;
        items.get(..count).ok_or_else(invalid)
    }
}

pub const fn type_stamp(class: &str) -> TypeStamp<'_> {
    TypeStamp {
        class,
        module: "mscorlib",
        revision: 0,
        fields: &[
            ("_items", FieldKind::Reference),
            ("_size", FieldKind::Integer),
            ("_version", FieldKind::Integer),
        ],
    }
}
