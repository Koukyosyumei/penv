use std::{fmt::Debug, io::Cursor};

use crate::migrant::{
    blocking_collection::{self, BlockingCollection},
    object_stream::{Graph, Object, ObjectReader, ObjectWriter, References, Value},
};

fn round_trip<T: Eq + Ord + Debug>(
    element_type: &str,
    items: Vec<T>,
    stamped: bool,
    encode: impl Fn(&mut Graph, &T) -> Value,
    decode: impl Fn(&Graph, &Value) -> T,
) {
    let class = format!("System.Collections.Concurrent.BlockingCollection<{element_type}>");
    let mut graph = Graph::default();
    let collection = BlockingCollection::default();
    for item in &items {
        collection.add(encode(&mut graph, item));
    }
    let root = collection.store(&mut graph, &class);
    let stamps = [blocking_collection::type_stamp(&class)];
    let mut stream = Vec::new();
    let mut writer = ObjectWriter::new(&mut stream, &graph, References::Preserve, true);
    if stamped {
        writer = writer.with_type_stamps(&stamps);
    }
    writer.write_object(root).unwrap();
    writer.finish().unwrap();

    let mut reader = ObjectReader::new(Cursor::new(stream), References::Preserve, true);
    if stamped {
        reader = reader.with_type_stamps(&stamps);
    }
    let id = reader.read_object().unwrap();
    let copy = BlockingCollection::restore(reader.graph(), id, &class).unwrap();
    let mut expected: Vec<_> = collection
        .snapshot()
        .iter()
        .map(|value| decode(&graph, value))
        .collect();
    let mut actual: Vec<_> = copy
        .snapshot()
        .iter()
        .map(|value| decode(reader.graph(), value))
        .collect();
    expected.sort();
    actual.sort();
    assert_eq!(expected, actual);
    reader.finish().unwrap();
}

fn primitives(stamped: bool) {
    round_trip(
        "System.Int32",
        vec![1, 2, 3],
        stamped,
        |_, &value| Value::Integer(value),
        |_, value| {
            let Value::Integer(value) = value else {
                panic!("expected integer")
            };
            *value
        },
    );
}

fn nullable(stamped: bool) {
    round_trip(
        "System.Object",
        vec![Some(1), None, Some(3)],
        stamped,
        |_, &value| value.map_or(Value::Null, Value::Integer),
        |_, value| match value {
            Value::Null => None,
            Value::Integer(value) => Some(*value),
            _ => panic!("expected boxed integer or null"),
        },
    );
}

fn strings(stamped: bool) {
    round_trip(
        "System.String",
        vec!["One".to_owned(), "Two".to_owned(), "Three".to_owned()],
        stamped,
        |graph, value| Value::Ref(graph.insert(Object::String(value.clone()))),
        |graph, value| {
            let Value::Ref(id) = value else {
                panic!("expected string reference")
            };
            let Some(Object::String(value)) = graph.get(*id) else {
                panic!("expected string")
            };
            value.clone()
        },
    );
}

#[test]
fn primitives_plain() {
    primitives(false);
}
#[test]
fn nullable_plain() {
    nullable(false);
}
#[test]
fn strings_plain() {
    strings(false);
}
#[test]
fn primitives_stamped() {
    primitives(true);
}
#[test]
fn nullable_stamped() {
    nullable(true);
}
#[test]
fn strings_stamped() {
    strings(true);
}
