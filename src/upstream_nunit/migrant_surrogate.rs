use crate::migrant::object_stream::{
    FieldKind, Graph, Object, ObjectId, ObjectReader, ObjectWriter, References, TypeStamp, Value,
};
use crate::migrant::surrogate::Surrogates;
use std::io::{Cursor, Seek};

const fn stamp(
    class: &'static str,
    fields: &'static [(&'static str, FieldKind)],
) -> TypeStamp<'static> {
    TypeStamp {
        class,
        module: "Tests",
        revision: 0,
        fields,
    }
}

const STAMPS: &[TypeStamp<'static>] = &[
    stamp("System.Object", &[]),
    stamp(
        "SurrogateMockA",
        &[("<Field>k__BackingField", FieldKind::Integer)],
    ),
    stamp("SurrogateMockB", &[]),
    stamp("List<object>", &[("items", FieldKind::Reference)]),
];

fn record(class: &str, fields: Vec<Value>) -> Object {
    Object::Record {
        class: class.into(),
        fields,
    }
}

fn a(number: i64) -> Object {
    record("SurrogateMockA", vec![Value::Integer(number)])
}

fn round_trip(
    graph: &Graph,
    root: ObjectId,
    stamped: bool,
    encode: &mut Surrogates<'_>,
    restore: &mut Surrogates<'_>,
    check: impl FnOnce(&Graph, ObjectId),
) {
    let mut stream = Cursor::new(Vec::new());
    let mut writer =
        ObjectWriter::new(&mut stream, graph, References::Preserve, true).with_surrogates(encode);
    if stamped {
        writer = writer.with_type_stamps(STAMPS);
    }
    writer.write_object(root).unwrap();
    writer.finish().unwrap();
    stream.rewind().unwrap();
    let mut reader = ObjectReader::new(stream, References::Preserve, true).with_surrogates(restore);
    if stamped {
        reader = reader.with_type_stamps(STAMPS);
    }
    let root = reader.read_object().unwrap();
    check(reader.graph(), root);
    reader.finish().unwrap();
}

fn place_object_for_surrogate(stamped: bool) {
    let mut graph = Graph::default();
    let root = graph.insert(record("SurrogateMockB", vec![]));
    let mut restore = Surrogates::default();
    restore.register("SurrogateMockB", |_, _| Ok(a(999)));
    round_trip(
        &graph,
        root,
        stamped,
        &mut Surrogates::default(),
        &mut restore,
        |copy, id| assert_eq!(copy.get(id), Some(&a(999))),
    );
}

fn place_surrogate_for_object(stamped: bool) {
    let mut graph = Graph::default();
    let root = graph.insert(record("SurrogateMockB", vec![]));
    let mut encode = Surrogates::default();
    encode.register("SurrogateMockB", |_, _| Ok(a(1)));
    round_trip(
        &graph,
        root,
        stamped,
        &mut encode,
        &mut Surrogates::default(),
        |copy, id| assert_eq!(copy.get(id), Some(&a(1))),
    );
}

fn ref_id(value: &Value) -> ObjectId {
    let Value::Ref(id) = value else {
        panic!("expected a reference");
    };
    *id
}

fn place_surrogate_for_object_preserving_identity(stamped: bool) {
    let mut graph = Graph::default();
    let b = graph.insert(record("SurrogateMockB", vec![]));
    let second = graph.insert(record("SurrogateMockB", vec![]));
    let items = graph.insert(Object::Array(vec![
        Value::Ref(b),
        Value::Ref(second),
        Value::Ref(b),
    ]));
    let root = graph.insert(record("List<object>", vec![Value::Ref(items)]));
    let counter = std::cell::Cell::new(0);
    let mut encode = Surrogates::default();
    encode.register("SurrogateMockB", |_, _| {
        let value = counter.get();
        counter.set(value + 1);
        Ok(a(value))
    });
    round_trip(
        &graph,
        root,
        stamped,
        &mut encode,
        &mut Surrogates::default(),
        |copy, id| {
            let Some(Object::Record { class, fields }) = copy.get(id) else {
                panic!("expected a list");
            };
            assert_eq!(class, "List<object>");
            let Some(Object::Array(list)) = copy.get(ref_id(&fields[0])) else {
                panic!("expected list items");
            };
            assert_eq!(list[0], list[2]);
            assert_ne!(list[0], list[1]);
            assert_eq!(copy.get(ref_id(&list[0])), Some(&a(counter.get() - 2)));
            assert_eq!(copy.get(ref_id(&list[1])), Some(&a(counter.get() - 1)));
        },
    );
}

fn do_surrogate_object_swap(stamped: bool, times: usize) {
    let mut graph = Graph::default();
    let root = graph.insert(record("SurrogateMockB", vec![]));
    let mut encode = Surrogates::default();
    let mut restore = Surrogates::default();
    encode.register("SurrogateMockB", |_, _| Ok(a(1)));
    restore.register("SurrogateMockA", |_, _| {
        Ok(record("SurrogateMockC", vec![]))
    });
    for _ in 0..times {
        round_trip(
            &graph,
            root,
            stamped,
            &mut encode,
            &mut restore,
            |copy, id| assert_eq!(copy.get(id), Some(&record("SurrogateMockC", vec![]))),
        );
    }
}

fn deserialize_surrogate_pointing_to_itself(stamped: bool) {
    let mut graph = Graph::default();
    let root = graph.insert(record("System.Object", vec![]));
    let mut restore = Surrogates::default();
    restore.register("System.Object", |id, _| {
        Ok(record("SurrogateMockJ", vec![Value::Ref(id)]))
    });
    round_trip(
        &graph,
        root,
        stamped,
        &mut Surrogates::default(),
        &mut restore,
        |copy, id| {
            assert_eq!(
                copy.get(id),
                Some(&record("SurrogateMockJ", vec![Value::Ref(id)]))
            )
        },
    );
}

macro_rules! modes {
    ($plain:ident, $stamped:ident, $check:ident $(, $argument:expr)*) => {
        #[test]
        fn $plain() { $check(false $(, $argument)*); }
        #[test]
        fn $stamped() { $check(true $(, $argument)*); }
    };
}

modes!(
    place_read_plain,
    place_read_stamped,
    place_object_for_surrogate
);
modes!(
    place_write_plain,
    place_write_stamped,
    place_surrogate_for_object
);
modes!(
    place_write_identity_plain,
    place_write_identity_stamped,
    place_surrogate_for_object_preserving_identity
);
modes!(swap_plain, swap_stamped, do_surrogate_object_swap, 1);
modes!(
    swap_twice_plain,
    swap_twice_stamped,
    do_surrogate_object_swap,
    2
);
modes!(
    self_plain,
    self_stamped,
    deserialize_surrogate_pointing_to_itself
);
