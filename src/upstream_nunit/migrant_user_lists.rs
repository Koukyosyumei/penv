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

const ARRAY_LIST: &str = "ArrayList";
const READ_ONLY: &str = "ReadOnlyCollection<object>";

const STACK: &str = "Stack";
const CHAR_STACK: &str = "Stack<char>";
const QUEUE: &str = "Queue<int>";

const STACK_FIELDS: &[(&str, FieldKind)] = &[
    ("_array", FieldKind::Reference),
    ("_size", FieldKind::Integer),
    ("_version", FieldKind::Integer),
];

const INT_SET: &str = "HashSet<int>";
const OBJECT_SET: &str = "HashSet<CustomObject>";
const INT_SLOT: &str = "HashSet<int>.Slot";
const OBJECT_SLOT: &str = "HashSet<CustomObject>.Slot";

const SET_FIELDS: &[(&str, FieldKind)] = &[
    ("m_buckets", FieldKind::Reference),
    ("m_slots", FieldKind::Reference),
    ("m_count", FieldKind::Integer),
    ("m_lastIndex", FieldKind::Integer),
    ("m_freeList", FieldKind::Integer),
    ("m_comparer", FieldKind::Reference),
    ("m_version", FieldKind::Integer),
];

const DICTIONARY_FIELDS: &[(&str, FieldKind)] = &[
    ("buckets", FieldKind::Reference),
    ("entries", FieldKind::Reference),
    ("count", FieldKind::Integer),
    ("version", FieldKind::Integer),
    ("freeList", FieldKind::Integer),
    ("freeCount", FieldKind::Integer),
    ("comparer", FieldKind::Reference),
];
const INT_DICT: DictionaryLayout = DictionaryLayout {
    class: "Dictionary<int,int>",
    entry: "Dictionary<int,int>.Entry",
    comparer: "GenericEqualityComparer<int>",
};
const STR_DICT: DictionaryLayout = DictionaryLayout {
    class: "Dictionary<string,string>",
    entry: "Dictionary<string,string>.Entry",
    comparer: "GenericEqualityComparer<string>",
};
const SHARED_DICT: DictionaryLayout = DictionaryLayout {
    class: "Dictionary<int,string>",
    entry: "Dictionary<int,string>.Entry",
    comparer: "GenericEqualityComparer<int>",
};
const CUSTOM_DICT: DictionaryLayout = DictionaryLayout {
    class: "Dictionary<CustomObject,string>",
    entry: "Dictionary<CustomObject,string>.Entry",
    comparer: "ObjectEqualityComparer<CustomObject>",
};

#[derive(Clone, Copy)]
struct DictionaryLayout {
    class: &'static str,
    entry: &'static str,
    comparer: &'static str,
}

const fn dictionary_stamp(
    class: &'static str,
    fields: &'static [(&'static str, FieldKind)],
) -> TypeStamp<'static> {
    TypeStamp {
        class,
        module: "mscorlib",
        revision: 0,
        fields,
    }
}

const HASHTABLE: &str = "Hashtable";
const HASHTABLE_BUCKET: &str = "Hashtable.bucket";

const CODECS: &[Codec] = &[
    generated_record!(HASHTABLE; buckets: reference, count: integer, occupancy: integer, load_size: integer, load_factor: float, version: integer, writer_busy: boolean, comparer: reference),
    generated_record!(HASHTABLE_BUCKET; key: reference, value: reference, hash_collision: integer),
    generated_record!("GenericEqualityComparer<string>";),
    generated_record!(INT_DICT.class; buckets: reference, entries: reference, count: integer, version: integer, free_list: integer, free_count: integer, comparer: reference),
    generated_record!(INT_DICT.entry; hash: integer, next: integer, key: integer, value: integer),
    generated_record!(STR_DICT.class; buckets: reference, entries: reference, count: integer, version: integer, free_list: integer, free_count: integer, comparer: reference),
    generated_record!(STR_DICT.entry; hash: integer, next: integer, key: reference, value: reference),
    generated_record!(SHARED_DICT.class; buckets: reference, entries: reference, count: integer, version: integer, free_list: integer, free_count: integer, comparer: reference),
    generated_record!(SHARED_DICT.entry; hash: integer, next: integer, key: integer, value: reference),
    generated_record!(CUSTOM_DICT.class; buckets: reference, entries: reference, count: integer, version: integer, free_list: integer, free_count: integer, comparer: reference),
    generated_record!(CUSTOM_DICT.entry; hash: integer, next: integer, key: reference, value: reference),
    generated_record!(INT_SET; buckets: reference, slots: reference, count: integer, last: integer, free: integer, comparer: reference, version: integer),
    generated_record!(OBJECT_SET; buckets: reference, slots: reference, count: integer, last: integer, free: integer, comparer: reference, version: integer),
    generated_record!(INT_SLOT; hash: integer, next: integer, value: integer),
    generated_record!(OBJECT_SLOT; hash: integer, next: integer, value: reference),
    generated_record!("GenericEqualityComparer<int>";),
    generated_record!("ObjectEqualityComparer<CustomObject>";),
    generated_record!("CustomObject"; field: reference),
    generated_record!("System.Object";),
    generated_record!(ARRAY_LIST; items: reference, size: integer, version: integer),
    generated_record!(READ_ONLY; list: reference),
    generated_record!("SimpleClass"; value: integer, string: reference),
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
    dictionary_stamp(
        HASHTABLE,
        &[
            ("buckets", FieldKind::Reference),
            ("count", FieldKind::Integer),
            ("occupancy", FieldKind::Integer),
            ("loadsize", FieldKind::Integer),
            ("loadFactor", FieldKind::Float),
            ("version", FieldKind::Integer),
            ("isWriterInProgress", FieldKind::Boolean),
            ("_keycomparer", FieldKind::Reference),
        ],
    ),
    dictionary_stamp(
        HASHTABLE_BUCKET,
        &[
            ("key", FieldKind::Reference),
            ("val", FieldKind::Reference),
            ("hash_coll", FieldKind::Integer),
        ],
    ),
    dictionary_stamp("GenericEqualityComparer<string>", &[]),
    dictionary_stamp(INT_DICT.class, DICTIONARY_FIELDS),
    dictionary_stamp(
        INT_DICT.entry,
        &[
            ("hashCode", FieldKind::Integer),
            ("next", FieldKind::Integer),
            ("key", FieldKind::Integer),
            ("value", FieldKind::Integer),
        ],
    ),
    dictionary_stamp(STR_DICT.class, DICTIONARY_FIELDS),
    dictionary_stamp(
        STR_DICT.entry,
        &[
            ("hashCode", FieldKind::Integer),
            ("next", FieldKind::Integer),
            ("key", FieldKind::Reference),
            ("value", FieldKind::Reference),
        ],
    ),
    dictionary_stamp(SHARED_DICT.class, DICTIONARY_FIELDS),
    dictionary_stamp(
        SHARED_DICT.entry,
        &[
            ("hashCode", FieldKind::Integer),
            ("next", FieldKind::Integer),
            ("key", FieldKind::Integer),
            ("value", FieldKind::Reference),
        ],
    ),
    dictionary_stamp(CUSTOM_DICT.class, DICTIONARY_FIELDS),
    dictionary_stamp(
        CUSTOM_DICT.entry,
        &[
            ("hashCode", FieldKind::Integer),
            ("next", FieldKind::Integer),
            ("key", FieldKind::Reference),
            ("value", FieldKind::Reference),
        ],
    ),
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
    TypeStamp {
        class: INT_SET,
        module: "System.Core",
        revision: 0,
        fields: SET_FIELDS,
    },
    TypeStamp {
        class: OBJECT_SET,
        module: "System.Core",
        revision: 0,
        fields: SET_FIELDS,
    },
    TypeStamp {
        class: INT_SLOT,
        module: "System.Core",
        revision: 0,
        fields: &[
            ("hashCode", FieldKind::Integer),
            ("next", FieldKind::Integer),
            ("value", FieldKind::Integer),
        ],
    },
    TypeStamp {
        class: OBJECT_SLOT,
        module: "System.Core",
        revision: 0,
        fields: &[
            ("hashCode", FieldKind::Integer),
            ("next", FieldKind::Integer),
            ("value", FieldKind::Reference),
        ],
    },
    TypeStamp {
        class: "GenericEqualityComparer<int>",
        module: "mscorlib",
        revision: 0,
        fields: &[],
    },
    TypeStamp {
        class: "ObjectEqualityComparer<CustomObject>",
        module: "mscorlib",
        revision: 0,
        fields: &[],
    },
    TypeStamp {
        class: "CustomObject",
        module: "Tests",
        revision: 0,
        fields: &[("Field", FieldKind::Reference)],
    },
    TypeStamp {
        class: "System.Object",
        module: "mscorlib",
        revision: 0,
        fields: &[],
    },
    user_list::type_stamp(ARRAY_LIST),
    TypeStamp {
        class: READ_ONLY,
        module: "mscorlib",
        revision: 0,
        fields: &[("list", FieldKind::Reference)],
    },
    TypeStamp {
        class: "SimpleClass",
        module: "Tests",
        revision: 0,
        fields: &[("Value", FieldKind::Integer), ("Str", FieldKind::Reference)],
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

fn string_ref(graph: &mut Graph, value: &str) -> Value {
    Value::Ref(graph.insert(Object::String(value.into())))
}

fn integer_ref(graph: &mut Graph, value: i64) -> Value {
    Value::Ref(graph.insert(record("System.Int32", vec![Value::Integer(value)])))
}

fn assert_string(graph: &Graph, item: &Value, expected: &str) {
    assert_eq!(
        graph.get(reference(item)),
        Some(&Object::String(expected.into()))
    );
}

fn assert_integer(graph: &Graph, item: &Value, expected: i64) {
    assert_eq!(
        graph.get(reference(item)),
        Some(&record("System.Int32", vec![Value::Integer(expected)]))
    );
}

fn mixed_array_list(options: Options) {
    let mut graph = Graph::default();
    let first = string_ref(&mut graph, "Word 1");
    let second = string_ref(&mut graph, "Word 2");
    let text = string_ref(&mut graph, "Word 4");
    let object = Value::Ref(graph.insert(record("SimpleClass", vec![Value::Integer(6), text])));
    let last = string_ref(&mut graph, "Word 3");
    let root = store(&mut graph, ARRAY_LIST, [first, second, object, last]);
    let (restored, copy) = options.restore(&graph, root);
    let items = UserList::restore(&restored, copy, ARRAY_LIST).unwrap();
    let [first, second, object, last] = items else {
        panic!("expected four array list items");
    };
    assert_string(&restored, first, "Word 1");
    assert_string(&restored, second, "Word 2");
    let Some(Object::Record { class, fields }) = restored.get(reference(object)) else {
        panic!("expected SimpleClass");
    };
    assert_eq!(class, "SimpleClass");
    assert_eq!(fields[0], Value::Integer(6));
    assert_string(&restored, &fields[1], "Word 4");
    assert_string(&restored, last, "Word 3");
}

fn primitive_array_list(options: Options) {
    let mut graph = Graph::default();
    let input = [1, 2, 3];
    let boxed = input.map(|value| integer_ref(&mut graph, value));
    let root = store(&mut graph, ARRAY_LIST, boxed);
    let (restored, copy) = options.restore(&graph, root);
    let items = UserList::restore(&restored, copy, ARRAY_LIST).unwrap();
    assert_eq!(items.len(), input.len());
    for (item, expected) in items.iter().zip(input) {
        assert_integer(&restored, item, expected);
    }
}

fn read_only_collection(options: Options) {
    let mut graph = Graph::default();
    let items = [
        string_ref(&mut graph, "asdasd"),
        integer_ref(&mut graph, 1),
        string_ref(&mut graph, "cvzxcv"),
    ];
    let list = store(&mut graph, OBJECTS, items);
    let root = graph.insert(record(READ_ONLY, vec![Value::Ref(list)]));
    let (restored, copy) = options.restore(&graph, root);
    let Some(Object::Record { class, fields }) = restored.get(copy) else {
        panic!("expected read-only collection");
    };
    assert_eq!(class, READ_ONLY);
    let items = UserList::restore(&restored, reference(&fields[0]), OBJECTS).unwrap();
    let [first, second, third] = items else {
        panic!("expected three collection items");
    };
    assert_string(&restored, first, "asdasd");
    assert_integer(&restored, second, 1);
    assert_string(&restored, third, "cvzxcv");
}

fn hash_set(options: Options, objects: bool) {
    let mut graph = Graph::default();
    let (class, slot, comparer) = if objects {
        (
            OBJECT_SET,
            OBJECT_SLOT,
            "ObjectEqualityComparer<CustomObject>",
        )
    } else {
        (INT_SET, INT_SLOT, "GenericEqualityComparer<int>")
    };
    let mut buckets = [0; 3];
    let mut slots = Vec::new();
    for index in 0..3 {
        let (hash, item) = if objects {
            let identity = graph.insert(record("System.Object", vec![]));
            let item = graph.insert(record("CustomObject", vec![Value::Ref(identity)]));
            // Assign a stable identity hash to each fixture object in this graph.
            (index * 3, Value::Ref(item))
        } else {
            (index + 1, Value::Integer((index + 1) as i64))
        };
        let bucket = hash % buckets.len();
        let next = buckets[bucket] - 1;
        buckets[bucket] = index as i64 + 1;
        slots.push(Value::Ref(graph.insert(record(
            slot,
            vec![Value::Integer(hash as i64), Value::Integer(next), item],
        ))));
    }
    let buckets = graph.insert(Object::Array(buckets.map(Value::Integer).to_vec()));
    let slots = graph.insert(Object::Array(slots));
    let comparer = graph.insert(record(comparer, vec![]));
    let root = graph.insert(record(
        class,
        vec![
            Value::Ref(buckets),
            Value::Ref(slots),
            Value::Integer(3),
            Value::Integer(3),
            Value::Integer(-1),
            Value::Ref(comparer),
            Value::Integer(3),
        ],
    ));

    let (restored, copy) = options.restore(&graph, root);
    let Some(Object::Record {
        class: actual,
        fields,
    }) = restored.get(copy)
    else {
        panic!("expected hash set");
    };
    assert_eq!(actual, class);
    assert_eq!(fields[2], Value::Integer(3));
    let Some(Object::Array(buckets)) = restored.get(reference(&fields[0])) else {
        panic!("expected buckets");
    };
    let Some(Object::Array(slots)) = restored.get(reference(&fields[1])) else {
        panic!("expected slots");
    };
    let mut visited = std::collections::HashSet::new();
    let mut integers = std::collections::BTreeSet::new();
    let mut identities = std::collections::HashSet::new();
    for (bucket, head) in buckets.iter().enumerate() {
        let Value::Integer(head) = head else {
            panic!("expected bucket index");
        };
        let mut next = head - 1;
        while next >= 0 {
            assert!(visited.insert(next));
            let Some(Object::Record { class, fields }) =
                restored.get(reference(&slots[next as usize]))
            else {
                panic!("expected slot");
            };
            assert_eq!(class, slot);
            let [Value::Integer(hash), Value::Integer(link), item] = fields.as_slice() else {
                panic!("expected slot fields");
            };
            assert_eq!(*hash as usize % buckets.len(), bucket);
            if objects {
                let Some(Object::Record { class, fields }) = restored.get(reference(item)) else {
                    panic!("expected custom object");
                };
                assert_eq!(class, "CustomObject");
                let identity = reference(&fields[0]);
                assert_eq!(
                    restored.get(identity),
                    Some(&record("System.Object", vec![]))
                );
                assert!(identities.insert(identity));
            } else {
                assert_eq!(item, &Value::Integer(*hash));
                integers.insert(*hash);
            }
            next = *link;
        }
    }
    assert_eq!(visited.len(), 3);
    if !objects {
        assert_eq!(integers, [1, 2, 3].into());
    }
}

impl DictionaryLayout {
    fn store(
        self,
        graph: &mut Graph,
        input: impl IntoIterator<Item = (i64, Value, Value)>,
        capacity: usize,
    ) -> ObjectId {
        let mut buckets = vec![-1; capacity];
        let mut entries = Vec::new();
        for (hash, key, value) in input {
            let bucket = (hash as usize) % capacity;
            let next = std::mem::replace(&mut buckets[bucket], entries.len() as i64);
            entries.push(Value::Ref(graph.insert(record(
                self.entry,
                vec![Value::Integer(hash), Value::Integer(next), key, value],
            ))));
        }
        let count = Value::Integer(entries.len() as i64);
        entries.resize(capacity, Value::Null);
        let buckets = graph.insert(Object::Array(
            buckets.into_iter().map(Value::Integer).collect(),
        ));
        let entries = graph.insert(Object::Array(entries));
        let comparer = graph.insert(record(self.comparer, vec![]));
        graph.insert(record(
            self.class,
            vec![
                Value::Ref(buckets),
                Value::Ref(entries),
                count.clone(),
                count,
                Value::Integer(-1),
                Value::Integer(0),
                Value::Ref(comparer),
            ],
        ))
    }

    fn restore(self, graph: &Graph, id: ObjectId) -> Vec<(&Value, &Value)> {
        let Some(Object::Record { class, fields }) = graph.get(id) else {
            panic!("expected dictionary");
        };
        assert_eq!(class, self.class);
        let [Value::Ref(buckets), Value::Ref(entries), Value::Integer(count), Value::Integer(_), Value::Integer(-1), Value::Integer(0), Value::Ref(comparer)] =
            fields.as_slice()
        else {
            panic!("expected populated dictionary without removed entries");
        };
        assert_eq!(graph.get(*comparer), Some(&record(self.comparer, vec![])));
        let Some(Object::Array(buckets)) = graph.get(*buckets) else {
            panic!("expected buckets");
        };
        let Some(Object::Array(entries)) = graph.get(*entries) else {
            panic!("expected entries");
        };
        let count = usize::try_from(*count).unwrap();
        let mut visited = vec![false; count];
        let mut result = Vec::with_capacity(count);
        for (bucket, head) in buckets.iter().enumerate() {
            let Value::Integer(mut index) = *head else {
                panic!("expected bucket index");
            };
            while index != -1 {
                let at = usize::try_from(index).unwrap();
                assert!(!std::mem::replace(&mut visited[at], true));
                let Some(Object::Record { class, fields }) = graph.get(reference(&entries[at]))
                else {
                    panic!("expected dictionary entry");
                };
                assert_eq!(class, self.entry);
                let [Value::Integer(hash), Value::Integer(next), key, value] = fields.as_slice()
                else {
                    panic!("expected entry fields");
                };
                assert_eq!(usize::try_from(*hash).unwrap() % buckets.len(), bucket);
                result.push((key, value));
                index = *next;
            }
        }
        assert_eq!(result.len(), count);
        result
    }
}

fn integer_dictionary(options: Options) {
    let mut graph = Graph::default();
    let root = INT_DICT.store(
        &mut graph,
        (0..100).map(|i| (i, Value::Integer(i), Value::Integer(i + 1))),
        197,
    );
    let (restored, copy) = options.restore(&graph, root);
    let actual: std::collections::BTreeMap<_, _> = INT_DICT
        .restore(&restored, copy)
        .into_iter()
        .map(|(key, value)| {
            let (Value::Integer(key), Value::Integer(value)) = (key, value) else {
                panic!("expected integer pair");
            };
            (*key, *value)
        })
        .collect();
    assert_eq!(actual, (0..100).map(|i| (i, i + 1)).collect());
}

fn string_from_ref<'g>(graph: &'g Graph, value: &Value) -> &'g str {
    let Some(Object::String(text)) = graph.get(reference(value)) else {
        panic!("expected string");
    };
    text
}

fn string_dictionary(options: Options) {
    let mut graph = Graph::default();
    let input = [("Ball", "Piłka"), ("Cat", "Kot"), ("Line", "Linia")];
    let entries = input.map(|(key, value)| {
        // The Rust fixture uses a deterministic UTF-16 hash for the graph's buckets.
        let hash = key.encode_utf16().fold(0i32, |hash, c| {
            hash.wrapping_mul(31).wrapping_add(i32::from(c))
        }) & i32::MAX;
        (
            i64::from(hash),
            string_ref(&mut graph, key),
            string_ref(&mut graph, value),
        )
    });
    let root = STR_DICT.store(&mut graph, entries, 3);
    let (restored, copy) = options.restore(&graph, root);
    let actual: std::collections::BTreeMap<_, _> = STR_DICT
        .restore(&restored, copy)
        .into_iter()
        .map(|(key, value)| {
            (
                string_from_ref(&restored, key),
                string_from_ref(&restored, value),
            )
        })
        .collect();
    assert_eq!(actual, input.into_iter().collect());
}

fn dictionary_identity(options: Options) {
    let mut graph = Graph::default();
    let shared = string_ref(&mut graph, "Word");
    let other = string_ref(&mut graph, "One");
    let entries = [(10, shared.clone()), (20, other), (30, shared)]
        .map(|(key, value)| (key, Value::Integer(key), value));
    let root = SHARED_DICT.store(&mut graph, entries, 3);
    let (restored, copy) = options.restore(&graph, root);
    let actual: std::collections::BTreeMap<_, _> = SHARED_DICT
        .restore(&restored, copy)
        .into_iter()
        .map(|(key, value)| {
            let Value::Integer(key) = key else {
                panic!("expected integer key");
            };
            (*key, value)
        })
        .collect();
    let contents: Vec<_> = actual
        .iter()
        .map(|(&key, value)| (key, string_from_ref(&restored, value)))
        .collect();
    assert_eq!(contents, [(10, "Word"), (20, "One"), (30, "Word")]);
    assert_eq!(reference(actual[&10]), reference(actual[&30]));
    assert_ne!(reference(actual[&10]), reference(actual[&20]));
}

fn custom_dictionary(options: Options) {
    let mut graph = Graph::default();
    let entries = ["test1", "test2", "test3"].map(|text| {
        let identity = graph.insert(record("System.Object", vec![]));
        let key = graph.insert(record("CustomObject", vec![Value::Ref(identity)]));
        (0, Value::Ref(key), string_ref(&mut graph, text))
    });
    let root = CUSTOM_DICT.store(&mut graph, entries, 3);
    let (restored, copy) = options.restore(&graph, root);
    assert_eq!(CUSTOM_DICT.restore(&restored, copy).len(), 3);
}

fn hashtable(options: Options) {
    let mut graph = Graph::default();
    let capacity = 197;
    let load_factor = 0.72_f32;
    // The fixture's integer hashes are all distinct and fit in the final table.
    let buckets = (0..capacity)
        .map(|bucket| {
            let fields = if bucket < 100 {
                vec![
                    integer_ref(&mut graph, bucket as i64),
                    integer_ref(&mut graph, bucket as i64 + 1),
                    Value::Integer(bucket as i64),
                ]
            } else {
                vec![Value::Null, Value::Null, Value::Integer(0)]
            };
            Value::Ref(graph.insert(record(HASHTABLE_BUCKET, fields)))
        })
        .collect();
    let buckets = graph.insert(Object::Array(buckets));
    let root = graph.insert(record(
        HASHTABLE,
        vec![
            Value::Ref(buckets),
            Value::Integer(100),
            Value::Integer(0),
            Value::Integer((load_factor * capacity as f32) as i64),
            Value::Float(f64::from(load_factor)),
            Value::Integer(105),
            Value::Boolean(false),
            Value::Null,
        ],
    ));

    let (restored, copy) = options.restore(&graph, root);
    let Some(Object::Record { class, fields }) = restored.get(copy) else {
        panic!("expected hashtable");
    };
    assert_eq!(class, HASHTABLE);
    assert_eq!(fields[1], Value::Integer(100));
    let Some(Object::Array(buckets)) = restored.get(reference(&fields[0])) else {
        panic!("expected hashtable buckets");
    };
    let mut seen = std::collections::BTreeSet::new();
    for (index, bucket) in buckets.iter().enumerate() {
        let Some(Object::Record { class, fields }) = restored.get(reference(bucket)) else {
            panic!("expected hashtable bucket");
        };
        assert_eq!(class, HASHTABLE_BUCKET);
        match fields.as_slice() {
            [Value::Null, Value::Null, Value::Integer(0)] => {}
            [key, value, Value::Integer(hash)] => {
                assert_eq!(usize::try_from(*hash).unwrap() % buckets.len(), index);
                assert_integer(&restored, key, *hash);
                assert_integer(&restored, value, *hash + 1);
                assert!(seen.insert(*hash));
            }
            _ => panic!("invalid bucket fields"),
        }
    }
    assert_eq!(seen, (0..100).collect());
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
configurations!(mixed_array_list, mixed_array_list);
configurations!(primitive_array_list, primitive_array_list);
configurations!(read_only_collection, read_only_collection);
configurations!(integer_set, |m| hash_set(m, false));
configurations!(custom_set, |m| hash_set(m, true));
configurations!(integer_dictionary, integer_dictionary);
configurations!(string_dictionary, string_dictionary);
configurations!(dictionary_identity, dictionary_identity);
configurations!(custom_dictionary, custom_dictionary);
configurations!(hashtable, hashtable);
