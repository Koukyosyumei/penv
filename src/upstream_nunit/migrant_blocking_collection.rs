use std::fmt::Debug;

use crate::migrant::{
    blocking_collection::{self, BlockingCollection},
    graph_serializer::GraphSerializer,
    object_stream::{FieldKind, Graph, Object, TypeStamp, Value},
};

fn round_trip<T: Eq + Ord + Debug>(
    element_type: &str,
    items: Vec<T>,
    stamped: bool,
    xml: bool,
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
    let stamps = [
        blocking_collection::type_stamp(&class),
        TypeStamp {
            class: "System.Int32",
            module: "mscorlib",
            revision: 0,
            fields: &[("m_value", FieldKind::Integer)],
        },
    ];
    let (copy_graph, id) = GraphSerializer::default()
        .with_xml_serializable(xml)
        .clone_graph(&graph, root, stamped.then_some(&stamps))
        .unwrap();
    let copy = BlockingCollection::restore(&copy_graph, id, &class).unwrap();
    let mut expected: Vec<_> = collection
        .snapshot()
        .iter()
        .map(|value| decode(&graph, value))
        .collect();
    let mut actual: Vec<_> = copy
        .snapshot()
        .iter()
        .map(|value| decode(&copy_graph, value))
        .collect();
    expected.sort();
    actual.sort();
    assert_eq!(expected, actual);
}

fn primitives(stamped: bool, xml: bool) {
    round_trip(
        "System.Int32",
        vec![1, 2, 3],
        stamped,
        xml,
        |_, &value| Value::Integer(value),
        |_, value| {
            let Value::Integer(value) = value else {
                panic!("expected integer")
            };
            *value
        },
    );
}

fn nullable(stamped: bool, xml: bool) {
    round_trip(
        "System.Object",
        vec![Some(1), None, Some(3)],
        stamped,
        xml,
        |graph, &value| {
            value.map_or(Value::Null, |value| {
                Value::Ref(graph.insert(Object::Record {
                    class: "System.Int32".into(),
                    fields: vec![Value::Integer(value)],
                }))
            })
        },
        |graph, value| match value {
            Value::Null => None,
            Value::Ref(id) => {
                let Some(Object::Record { class, fields }) = graph.get(*id) else {
                    panic!("expected boxed integer");
                };
                assert_eq!(class, "System.Int32");
                let [Value::Integer(value)] = fields.as_slice() else {
                    panic!("expected boxed integer value");
                };
                Some(*value)
            }
            _ => panic!("expected boxed integer or null"),
        },
    );
}

fn strings(stamped: bool, xml: bool) {
    round_trip(
        "System.String",
        vec!["One".to_owned(), "Two".to_owned(), "Three".to_owned()],
        stamped,
        xml,
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
    primitives(false, false);
}
#[test]
fn nullable_plain() {
    nullable(false, false);
}
#[test]
fn strings_plain() {
    strings(false, false);
}
#[test]
fn primitives_stamped() {
    primitives(true, false);
}
#[test]
fn nullable_stamped() {
    nullable(true, false);
}
#[test]
fn strings_stamped() {
    strings(true, false);
}

#[test]
fn primitives_xml_plain() {
    primitives(false, true);
}
#[test]
fn nullable_xml_plain() {
    nullable(false, true);
}
#[test]
fn strings_xml_plain() {
    strings(false, true);
}
#[test]
fn primitives_xml_stamped() {
    primitives(true, true);
}
#[test]
fn nullable_xml_stamped() {
    nullable(true, true);
}
#[test]
fn strings_xml_stamped() {
    strings(true, true);
}
