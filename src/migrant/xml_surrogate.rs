use super::object_stream::{FieldKind, Graph, Object, ObjectId, TypeStamp, Value};
use std::io;

pub(super) const CLASS: &str = "Antmicro.Migrant.BultinSurrogates.SurrogateForIXmlSerializable";

pub const TYPE_STAMP: TypeStamp<'static> = TypeStamp {
    class: CLASS,
    module: "Migrant",
    revision: 0,
    fields: &[
        ("assemblyQualifiedName", FieldKind::Reference),
        ("bytes", FieldKind::Reference),
    ],
};

pub trait XmlSerializable: Sized {
    const TYPE_NAME: &'static str;
    fn write_xml(&self) -> io::Result<Vec<u8>>;
    fn read_xml(bytes: &[u8]) -> io::Result<Self>;
}

pub fn store<T: XmlSerializable>(graph: &mut Graph, value: &T) -> io::Result<ObjectId> {
    let bytes = value.write_xml()?;
    let name = graph.insert(Object::String(T::TYPE_NAME.into()));
    let bytes = graph.insert(Object::Bytes(bytes));
    Ok(graph.insert(Object::Record {
        class: CLASS.into(),
        fields: vec![Value::Ref(name), Value::Ref(bytes)],
    }))
}

pub fn restore<T: XmlSerializable>(graph: &Graph, id: ObjectId) -> io::Result<T> {
    let invalid = || io::Error::new(io::ErrorKind::InvalidData, "invalid XML surrogate record");
    let Some(Object::Record { class, fields }) = graph.get(id) else {
        return Err(invalid());
    };
    let [Value::Ref(name), Value::Ref(bytes)] = fields.as_slice() else {
        return Err(invalid());
    };
    if class != CLASS
        || !matches!(graph.get(*name), Some(Object::String(name)) if name == T::TYPE_NAME)
    {
        return Err(invalid());
    }
    let Some(Object::Bytes(bytes)) = graph.get(*bytes) else {
        return Err(invalid());
    };
    T::read_xml(bytes)
}

pub(super) fn disabled() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "XML serialization support is disabled",
    )
}
