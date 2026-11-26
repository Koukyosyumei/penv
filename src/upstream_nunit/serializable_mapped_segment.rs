use crate::migrant::object_stream::{
    FieldKind, Graph, Object, ObjectReader, ObjectWriter, References, TypeStamp, Value,
};
use crate::serializable_mapped_segment::{SerializableMappedSegment, TYPE_STAMP};
use std::io::{Cursor, Seek};

const CLASS: &str = "PeripheralWithMappedSegment";
const STAMPS: &[TypeStamp<'static>] = &[
    TYPE_STAMP,
    TypeStamp {
        class: CLASS,
        module: "UnitTests",
        revision: 0,
        fields: &[("segments", FieldKind::Reference)],
    },
];

fn check_segments(touched: bool) {
    let segments = [
        SerializableMappedSegment::new(4096, 0),
        SerializableMappedSegment::new(4096, 8192),
    ];
    if touched {
        for segment in &segments {
            segment.touch();
        }
    }

    let mut graph = Graph::default();
    let ids = segments
        .iter()
        .map(|segment| Value::Ref(segment.snapshot(&mut graph).unwrap()))
        .collect();
    let array = graph.insert(Object::Array(ids));
    let root = graph.insert(Object::Record {
        class: CLASS.into(),
        fields: vec![Value::Ref(array)],
    });
    let mut stream = Cursor::new(Vec::new());
    let mut writer =
        ObjectWriter::new(&mut stream, &graph, References::Preserve, true).with_type_stamps(STAMPS);
    writer.write_object(root).unwrap();
    writer.finish().unwrap();
    stream.rewind().unwrap();

    let mut reader = ObjectReader::new(stream, References::Preserve, true).with_type_stamps(STAMPS);
    let root = reader.read_object().unwrap();
    let graph = reader.graph();
    let Some(Object::Record { class, fields }) = graph.get(root) else {
        panic!("expected peripheral");
    };
    assert_eq!(class, CLASS);
    let [Value::Ref(array)] = fields.as_slice() else {
        panic!("expected segment array reference");
    };
    let Some(Object::Array(segments)) = graph.get(*array) else {
        panic!("expected segment array");
    };
    assert_eq!(segments.len(), 2);
    for value in segments {
        let Value::Ref(id) = value else {
            panic!("expected segment reference");
        };
        let segment = SerializableMappedSegment::restore(graph, *id).unwrap();
        assert_eq!(segment.pointer().is_some(), touched);
    }
    reader.finish().unwrap();
}

#[test]
fn should_serialize_when_not_touched() {
    check_segments(false);
}

#[test]
fn should_serialize_when_touched() {
    check_segments(true);
}
