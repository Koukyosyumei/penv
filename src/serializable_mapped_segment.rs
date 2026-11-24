use crate::migrant::object_stream::{FieldKind, Graph, Object, ObjectId, TypeStamp, Value};
use std::io;
use std::sync::OnceLock;

const CLASS: &str = "Antmicro.Renode.Core.SerializableMappedSegment";

pub const TYPE_STAMP: TypeStamp<'static> = TypeStamp {
    class: CLASS,
    module: "Infrastructure",
    revision: 0,
    fields: &[
        ("Size", FieldKind::Integer),
        ("StartingOffset", FieldKind::Integer),
        ("buffer", FieldKind::Reference),
    ],
};

pub struct SerializableMappedSegment {
    size: usize,
    starting_offset: u64,
    buffer: OnceLock<Box<[u8]>>,
}

impl SerializableMappedSegment {
    pub fn new(size: usize, starting_offset: u64) -> Self {
        Self {
            size,
            starting_offset,
            buffer: OnceLock::new(),
        }
    }

    pub fn touch(&self) {
        self.buffer
            .get_or_init(|| vec![0; self.size].into_boxed_slice());
    }

    pub fn pointer(&self) -> Option<*const u8> {
        self.buffer.get().map(|buffer| buffer.as_ptr())
    }

    pub fn snapshot(&self, graph: &mut Graph) -> io::Result<ObjectId> {
        let size = i64::try_from(self.size).map_err(|_| invalid_segment())?;
        let contents = self.buffer.get().map_or(Value::Null, |buffer| {
            Value::Ref(graph.insert(Object::Bytes(buffer.to_vec())))
        });
        Ok(graph.insert(Object::Record {
            class: CLASS.into(),
            fields: vec![
                Value::Integer(size),
                Value::Integer(self.starting_offset as i64),
                contents,
            ],
        }))
    }

    pub fn restore(graph: &Graph, id: ObjectId) -> io::Result<Self> {
        let Some(Object::Record { class, fields }) = graph.get(id) else {
            return Err(invalid_segment());
        };
        let [
            Value::Integer(size),
            Value::Integer(starting_offset),
            contents,
        ] = fields.as_slice()
        else {
            return Err(invalid_segment());
        };
        if class != CLASS {
            return Err(invalid_segment());
        }
        let size = usize::try_from(*size).map_err(|_| invalid_segment())?;
        let buffer = match contents {
            Value::Null => OnceLock::new(),
            Value::Ref(id) => {
                let Some(Object::Bytes(bytes)) = graph.get(*id) else {
                    return Err(invalid_segment());
                };
                if bytes.len() != size {
                    return Err(invalid_segment());
                }
                OnceLock::from(bytes.clone().into_boxed_slice())
            }
            _ => return Err(invalid_segment()),
        };
        Ok(Self {
            size,
            starting_offset: *starting_offset as u64,
            buffer,
        })
    }
}

fn invalid_segment() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "invalid mapped segment snapshot",
    )
}
