use crate::migrant::{
    generated_record::{generated_record, Codec},
    graph_serializer::GraphSerializer,
    object_stream::{FieldKind, Graph, Object, ObjectId, TypeStamp, Value},
    user_list::{self, UserList},
};

const INTS: &str = "List<int>";
const NESTED: &str = "List<List<int>>";
const OBJECTS: &str = "List<object>";
const BOXES: &str = "List<Box>";

const INTERFACES: &str = "List<IInterface>";
const INTERFACE_CONTAINER: &str = "ClassWithTheListOfInterfaces";
const IMPLEMENTATIONS: [&str; 2] = ["ImplementingInterfaceOne", "ImplementingInterfaceTwo"];

const STACK: &str = "Stack";
const CHAR_STACK: &str = "Stack<char>";
const QUEUE: &str = "Queue<int>";

const STACK_FIELDS: &[(&str, FieldKind)] = &[
    ("_array", FieldKind::Reference),
    ("_size", FieldKind::Integer),
    ("_version", FieldKind::Integer),
];

const CODECS: &[Codec] = &[
    generated_record!(STACK; array: reference, size: integer, version: integer),
    generated_record!(CHAR_STACK; array: reference, size: integer, version: integer),
    generated_record!(QUEUE; array: reference, head: integer, tail: integer, size: integer, version: integer),
    generated_record!("System.Char"; value: integer),
    generated_record!(INTS; items: reference, size: integer, version: integer),
    generated_record!(NESTED; items: reference, size: integer, version: integer),
    generated_record!(OBJECTS; items: reference, size: integer, version: integer),
    generated_record!(BOXES; items: reference, size: integer, version: integer),
    generated_record!("Box"; element: reference),
    generated_record!("System.Int32"; value: integer),
    generated_record!(INTERFACES; items: reference, size: integer, version: integer),
    generated_record!(INTERFACE_CONTAINER; list: reference),
    generated_record!(IMPLEMENTATIONS[0];),
    generated_record!(IMPLEMENTATIONS[1];),
];

const STAMPS: &[TypeStamp<'static>] = &[
    TypeStamp {
        class: STACK,
        module: "mscorlib",
        revision: 0,
        fields: STACK_FIELDS,
    },
    TypeStamp {
        class: CHAR_STACK,
        module: "mscorlib",
        revision: 0,
        fields: STACK_FIELDS,
    },
    TypeStamp {
        class: QUEUE,
        module: "mscorlib",
        revision: 0,
        fields: &[
            ("_array", FieldKind::Reference),
            ("_head", FieldKind::Integer),
            ("_tail", FieldKind::Integer),
            ("_size", FieldKind::Integer),
            ("_version", FieldKind::Integer),
        ],
    },
    TypeStamp {
        class: "System.Char",
        module: "mscorlib",
        revision: 0,
        fields: &[("m_value", FieldKind::Integer)],
    },
    user_list::type_stamp(INTS),
    user_list::type_stamp(NESTED),
    user_list::type_stamp(OBJECTS),
    user_list::type_stamp(BOXES),
    user_list::type_stamp(INTERFACES),
    TypeStamp {
        class: "System.Int32",
        module: "mscorlib",
        revision: 0,
        fields: &[("m_value", FieldKind::Integer)],
    },
    TypeStamp {
        class: INTERFACE_CONTAINER,
        module: "Tests",
        revision: 0,
        fields: &[("List", FieldKind::Reference)],
    },
    TypeStamp {
        class: IMPLEMENTATIONS[0],
        module: "Tests",
        revision: 0,
        fields: &[],
    },
    TypeStamp {
        class: IMPLEMENTATIONS[1],
        module: "Tests",
        revision: 0,
        fields: &[],
    },
    TypeStamp {
        class: "Box",
        module: "Tests",
        revision: 0,
        fields: &[("Element", FieldKind::Reference)],
    },
];

#[derive(Clone, Copy)]
struct Options {
    write: bool,
    read: bool,
    stamp: bool,
}

impl Options {
    fn restore(self, graph: &Graph, root: ObjectId) -> (Graph, ObjectId) {
        GraphSerializer::default()
            .with_collections_as_user_objects(true)
            .with_generated_methods(self.write.then_some(CODECS), self.read.then_some(CODECS))
            .clone_graph(graph, root, self.stamp.then_some(STAMPS))
            .unwrap()
    }
}

fn store(graph: &mut Graph, class: &str, values: impl IntoIterator<Item = Value>) -> ObjectId {
    let default = if class == INTS {
        Value::Integer(0)
    } else {
        Value::Null
    };
    let mut list = UserList::new(default);
    for value in values {
        list.push(value);
    }
    list.store(graph, class)
}

fn reference(value: &Value) -> ObjectId {
    let Value::Ref(id) = value else {
        panic!("expected reference")
    };
    *id
}

fn primitive_list(options: Options, range: std::ops::Range<i64>) {
    let mut graph = Graph::default();
    let root = store(&mut graph, INTS, range.clone().map(Value::Integer));
    let (restored, root) = options.restore(&graph, root);
    assert_eq!(
        UserList::restore(&restored, root, INTS).unwrap(),
        range.map(Value::Integer).collect::<Vec<_>>()
    );
}

fn nested_lists(options: Options) {
    let mut graph = Graph::default();
    let input = [[1, 2, 3], [97, 98, 99]];
    let nested = input.map(|items| Value::Ref(store(&mut graph, INTS, items.map(Value::Integer))));
    let root = store(&mut graph, NESTED, nested);
    let (restored, root) = options.restore(&graph, root);
    let actual = UserList::restore(&restored, root, NESTED).unwrap();
    for (index, expected) in input.into_iter().enumerate() {
        assert_eq!(
            UserList::restore(&restored, reference(&actual[index]), INTS).unwrap(),
            expected.map(Value::Integer),
        );
    }
}

fn list_with_null(options: Options) {
    let mut graph = Graph::default();
    let one = Value::Ref(graph.insert(Object::String("One".into())));
    let two = Value::Ref(graph.insert(Object::String("Two".into())));
    let root = store(&mut graph, OBJECTS, [one, Value::Null, two]);
    let (restored, root) = options.restore(&graph, root);
    let actual = UserList::restore(&restored, root, OBJECTS).unwrap();
    assert_eq!(actual.len(), 3);
    assert_eq!(
        restored.get(reference(&actual[0])),
        Some(&Object::String("One".into()))
    );
    assert_eq!(actual[1], Value::Null);
    assert_eq!(
        restored.get(reference(&actual[2])),
        Some(&Object::String("Two".into()))
    );
}

fn list_identity(options: Options) {
    let mut graph = Graph::default();
    let root = graph.insert(Object::Array(Vec::new()));
    let items = std::array::from_fn::<_, 2, _>(|_| {
        Value::Ref(graph.insert(Object::Record {
            class: "Box".into(),
            fields: vec![Value::Ref(root)],
        }))
    });
    let list = store(&mut graph, BOXES, items);
    *graph.get_mut(root).unwrap() = graph.get(list).unwrap().clone();
    let (restored, copy) = options.restore(&graph, root);
    let actual = UserList::restore(&restored, copy, BOXES).unwrap();
    let element = |index: usize| {
        let Some(Object::Record { fields, .. }) = restored.get(reference(&actual[index])) else {
            panic!("expected box");
        };
        reference(&fields[0])
    };
    assert_eq!(copy, element(0));
    assert_eq!(element(0), element(1));
}

fn record(class: &str, fields: Vec<Value>) -> Object {
    Object::Record {
        class: class.into(),
        fields,
    }
}

fn boxed_identity(options: Options) {
    let mut graph = Graph::default();
    let primitive = graph.insert(record("System.Int32", vec![Value::Integer(1234)]));
    let boxes = std::array::from_fn::<_, 2, _>(|_| {
        Value::Ref(graph.insert(record("Box", vec![Value::Ref(primitive)])))
    });
    let root = store(&mut graph, BOXES, boxes);
    let (restored, copy) = options.restore(&graph, root);
    let items = UserList::restore(&restored, copy, BOXES).unwrap();
    let element = |index: usize| {
        let Some(Object::Record { fields, .. }) = restored.get(reference(&items[index])) else {
            panic!("expected box");
        };
        reference(&fields[0])
    };
    assert_eq!(element(0), element(1));
}

fn interface_list(options: Options) {
    let mut graph = Graph::default();
    let items: Vec<_> = (0..100)
        .map(|i| Value::Ref(graph.insert(record(IMPLEMENTATIONS[i % 2], vec![]))))
        .collect();
    let list = store(&mut graph, INTERFACES, items);
    let root = graph.insert(record(INTERFACE_CONTAINER, vec![Value::Ref(list)]));
    let (restored, copy) = options.restore(&graph, root);
    let Some(Object::Record { class, fields }) = restored.get(copy) else {
        panic!("expected restored interface list owner");
    };
    assert_eq!(class, INTERFACE_CONTAINER);
    let items = UserList::restore(&restored, reference(&fields[0]), INTERFACES).unwrap();
    assert_eq!(items.len(), 100);
}

fn character_stack(options: Options, boxed: bool) {
    let mut graph = Graph::default();
    let input: Vec<_> = "It works!".encode_utf16().map(i64::from).collect();
    let mut backing: Vec<_> = input
        .iter()
        .map(|&ch| {
            let ch = Value::Integer(ch);
            if boxed {
                Value::Ref(graph.insert(record("System.Char", vec![ch])))
            } else {
                ch
            }
        })
        .collect();
    let size = Value::Integer(backing.len() as i64);
    let class = if boxed {
        backing.resize(10, Value::Null);
        STACK
    } else {
        backing.resize(16, Value::Integer(0));
        CHAR_STACK
    };
    let backing = graph.insert(Object::Array(backing));
    let root = graph.insert(record(class, vec![Value::Ref(backing), size.clone(), size]));
    let (restored, copy) = options.restore(&graph, root);
    let Some(Object::Record {
        class: actual,
        fields,
    }) = restored.get(copy)
    else {
        panic!("expected stack");
    };
    assert_eq!(actual, class);
    let [Value::Ref(array), Value::Integer(size), Value::Integer(_)] = fields.as_slice() else {
        panic!("expected stack fields");
    };
    let Some(Object::Array(items)) = restored.get(*array) else {
        panic!("expected stack backing array");
    };
    let actual: Vec<_> = items[..usize::try_from(*size).unwrap()]
        .iter()
        .rev()
        .map(|item| {
            let item = if boxed {
                let Some(Object::Record { class, fields }) = restored.get(reference(item)) else {
                    panic!("expected boxed character");
                };
                assert_eq!(class, "System.Char");
                &fields[0]
            } else {
                item
            };
            let Value::Integer(ch) = item else {
                panic!("expected character");
            };
            *ch
        })
        .collect();
    assert_eq!(actual, input.into_iter().rev().collect::<Vec<_>>());
}

fn integer_queue(options: Options) {
    let mut graph = Graph::default();
    let input = [1, 3, 5].map(Value::Integer);
    let mut backing = input.to_vec();
    backing.resize(4, Value::Integer(0));
    let array = graph.insert(Object::Array(backing));
    let root = graph.insert(record(
        QUEUE,
        vec![
            Value::Ref(array),
            Value::Integer(0),
            Value::Integer(3),
            Value::Integer(3),
            Value::Integer(4),
        ],
    ));
    let (restored, copy) = options.restore(&graph, root);
    let Some(Object::Record { class, fields }) = restored.get(copy) else {
        panic!("expected queue");
    };
    assert_eq!(class, QUEUE);
    let [Value::Ref(array), Value::Integer(head), Value::Integer(_), Value::Integer(size), Value::Integer(_)] =
        fields.as_slice()
    else {
        panic!("expected queue fields");
    };
    let Some(Object::Array(items)) = restored.get(*array) else {
        panic!("expected queue backing array");
    };
    let head = usize::try_from(*head).unwrap();
    let actual: Vec<_> = (0..usize::try_from(*size).unwrap())
        .map(|offset| items[(head + offset) % items.len()].clone())
        .collect();
    assert_eq!(actual, input);
}

macro_rules! configurations {
    ($module:ident, $case:expr) => {
        mod $module {
            use super::*;
            #[test]
            fn reflection_reflection_plain() {
                ($case)(Options {
                    write: false,
                    read: false,
                    stamp: false,
                });
            }
            #[test]
            fn reflection_reflection_stamped() {
                ($case)(Options {
                    write: false,
                    read: false,
                    stamp: true,
                });
            }
            #[test]
            fn reflection_generated_plain() {
                ($case)(Options {
                    write: false,
                    read: true,
                    stamp: false,
                });
            }
            #[test]
            fn reflection_generated_stamped() {
                ($case)(Options {
                    write: false,
                    read: true,
                    stamp: true,
                });
            }
            #[test]
            fn generated_reflection_plain() {
                ($case)(Options {
                    write: true,
                    read: false,
                    stamp: false,
                });
            }
            #[test]
            fn generated_reflection_stamped() {
                ($case)(Options {
                    write: true,
                    read: false,
                    stamp: true,
                });
            }
            #[test]
            fn generated_generated_plain() {
                ($case)(Options {
                    write: true,
                    read: true,
                    stamp: false,
                });
            }
            #[test]
            fn generated_generated_stamped() {
                ($case)(Options {
                    write: true,
                    read: true,
                    stamp: true,
                });
            }
        }
    };
}
configurations!(primitives, |m| primitive_list(m, 1..4));
configurations!(long, |m| primitive_list(m, 0..100_000));
configurations!(empty, |m| primitive_list(m, 0..0));
configurations!(nested, nested_lists);
configurations!(with_null, list_with_null);
configurations!(identity, list_identity);
configurations!(boxed_identity, boxed_identity);
configurations!(interface_list, interface_list);
configurations!(nongeneric_stack, |m| character_stack(m, true));
configurations!(generic_stack, |m| character_stack(m, false));
configurations!(queue, integer_queue);
