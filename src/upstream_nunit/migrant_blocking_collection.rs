use std::fmt::Debug;

use crate::migrant::{
    blocking_collection::{self, BlockingCollection},
    generated_record::{generated_record, Codec},
    graph_serializer::GraphSerializer,
    object_stream::{FieldKind, Graph, Object, TypeStamp, Value},
};

const INTS: &str = "System.Collections.Concurrent.BlockingCollection<System.Int32>";
const OBJECTS: &str = "System.Collections.Concurrent.BlockingCollection<System.Object>";
const STRINGS: &str = "System.Collections.Concurrent.BlockingCollection<System.String>";

const CODECS: &[Codec] = &[
    generated_record!(INTS; items: reference),
    generated_record!(OBJECTS; items: reference),
    generated_record!(STRINGS; items: reference),
    generated_record!("System.Int32"; value: integer),
];

#[derive(Clone, Copy, Default)]
struct Settings {
    stamped: bool,
    xml: bool,
    writer: bool,
    reader: bool,
}

fn round_trip<T: Eq + Ord + Debug>(
    class: &str,
    items: Vec<T>,
    settings: Settings,
    encode: impl Fn(&mut Graph, &T) -> Value,
    decode: impl Fn(&Graph, &Value) -> T,
) {
    let mut graph = Graph::default();
    let collection = BlockingCollection::default();
    for item in &items {
        collection.add(encode(&mut graph, item));
    }
    let root = collection.store(&mut graph, class);
    let stamps = [
        blocking_collection::type_stamp(class),
        TypeStamp {
            class: "System.Int32",
            module: "mscorlib",
            revision: 0,
            fields: &[("m_value", FieldKind::Integer)],
        },
    ];
    let (copy_graph, id) = GraphSerializer::default()
        .with_xml_serializable(settings.xml)
        .with_generated_methods(
            settings.writer.then_some(CODECS),
            settings.reader.then_some(CODECS),
        )
        .clone_graph(&graph, root, settings.stamped.then_some(&stamps))
        .unwrap();
    let copy = BlockingCollection::restore(&copy_graph, id, class).unwrap();
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

fn primitives(settings: Settings) {
    round_trip(
        INTS,
        vec![1, 2, 3],
        settings,
        |_, &value| Value::Integer(value),
        |_, value| {
            let Value::Integer(value) = value else {
                panic!("expected integer")
            };
            *value
        },
    );
}

fn nullable(settings: Settings) {
    round_trip(
        OBJECTS,
        vec![Some(1), None, Some(3)],
        settings,
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

fn strings(settings: Settings) {
    round_trip(
        STRINGS,
        vec!["One".to_owned(), "Two".to_owned(), "Three".to_owned()],
        settings,
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
    primitives(Settings {
        stamped: false,
        xml: false,
        ..Settings::default()
    });
}
#[test]
fn nullable_plain() {
    nullable(Settings {
        stamped: false,
        xml: false,
        ..Settings::default()
    });
}
#[test]
fn strings_plain() {
    strings(Settings {
        stamped: false,
        xml: false,
        ..Settings::default()
    });
}
#[test]
fn primitives_stamped() {
    primitives(Settings {
        stamped: true,
        xml: false,
        ..Settings::default()
    });
}
#[test]
fn nullable_stamped() {
    nullable(Settings {
        stamped: true,
        xml: false,
        ..Settings::default()
    });
}
#[test]
fn strings_stamped() {
    strings(Settings {
        stamped: true,
        xml: false,
        ..Settings::default()
    });
}

#[test]
fn primitives_xml_plain() {
    primitives(Settings {
        stamped: false,
        xml: true,
        ..Settings::default()
    });
}
#[test]
fn nullable_xml_plain() {
    nullable(Settings {
        stamped: false,
        xml: true,
        ..Settings::default()
    });
}
#[test]
fn strings_xml_plain() {
    strings(Settings {
        stamped: false,
        xml: true,
        ..Settings::default()
    });
}
#[test]
fn primitives_xml_stamped() {
    primitives(Settings {
        stamped: true,
        xml: true,
        ..Settings::default()
    });
}
#[test]
fn nullable_xml_stamped() {
    nullable(Settings {
        stamped: true,
        xml: true,
        ..Settings::default()
    });
}
#[test]
fn strings_xml_stamped() {
    strings(Settings {
        stamped: true,
        xml: true,
        ..Settings::default()
    });
}

macro_rules! cases {
    ($w:literal, $r:literal, $xml:literal, $stamp:literal; $p:ident, $n:ident, $s:ident) => {
        #[test]
        fn $p() {
            primitives(Settings {
                writer: $w,
                reader: $r,
                xml: $xml,
                stamped: $stamp,
            });
        }
        #[test]
        fn $n() {
            nullable(Settings {
                writer: $w,
                reader: $r,
                xml: $xml,
                stamped: $stamp,
            });
        }
        #[test]
        fn $s() {
            strings(Settings {
                writer: $w,
                reader: $r,
                xml: $xml,
                stamped: $stamp,
            });
        }
    };
}
cases!(false,true,false,false; primitives_generated_reader_plain, nullable_generated_reader_plain, strings_generated_reader_plain);
cases!(false,true,false,true; primitives_generated_reader_stamped, nullable_generated_reader_stamped, strings_generated_reader_stamped);
cases!(false,true,true,false; primitives_generated_reader_xml_plain, nullable_generated_reader_xml_plain, strings_generated_reader_xml_plain);
cases!(false,true,true,true; primitives_generated_reader_xml_stamped, nullable_generated_reader_xml_stamped, strings_generated_reader_xml_stamped);
cases!(true,false,false,false; primitives_generated_writer_plain, nullable_generated_writer_plain, strings_generated_writer_plain);
cases!(true,false,false,true; primitives_generated_writer_stamped, nullable_generated_writer_stamped, strings_generated_writer_stamped);
cases!(true,false,true,false; primitives_generated_writer_xml_plain, nullable_generated_writer_xml_plain, strings_generated_writer_xml_plain);
cases!(true,false,true,true; primitives_generated_writer_xml_stamped, nullable_generated_writer_xml_stamped, strings_generated_writer_xml_stamped);
cases!(true,true,false,false; primitives_generated_both_plain, nullable_generated_both_plain, strings_generated_both_plain);
cases!(true,true,false,true; primitives_generated_both_stamped, nullable_generated_both_stamped, strings_generated_both_stamped);
cases!(true,true,true,false; primitives_generated_both_xml_plain, nullable_generated_both_xml_plain, strings_generated_both_xml_plain);
cases!(true,true,true,true; primitives_generated_both_xml_stamped, nullable_generated_both_xml_stamped, strings_generated_both_xml_stamped);
