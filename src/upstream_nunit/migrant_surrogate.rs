use crate::migrant::object_stream::{
    FieldKind, Graph, Object, ObjectId, ObjectReader, ObjectWriter, References, TypeStamp, Value,
};
use crate::migrant::surrogate::{RegistrationClosed, Surrogates};
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
    stamp("SurrogateMockD", &[]),
    stamp("SurrogateMockE", &[]),
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
    restore
        .register("SurrogateMockB", |_, _| Ok(a(999)))
        .unwrap();
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
    encode.register("SurrogateMockB", |_, _| Ok(a(1))).unwrap();
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
    encode
        .register("SurrogateMockB", |_, _| {
            let value = counter.get();
            counter.set(value + 1);
            Ok(a(value))
        })
        .unwrap();
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
    encode.register("SurrogateMockB", |_, _| Ok(a(1))).unwrap();
    restore
        .register("SurrogateMockA", |_, _| {
            Ok(record("SurrogateMockC", vec![]))
        })
        .unwrap();
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
    restore
        .register("System.Object", |id, _| {
            Ok(record("SurrogateMockJ", vec![Value::Ref(id)]))
        })
        .unwrap();
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

const C: &str = "SurrogateMockC";
const D: &str = "SurrogateMockD";
const E: &str = "SurrogateMockE";
const H: &str = "SurrogateMockH";
const IE: &str = "ISurrogateMockE";
const IG: &str = "ISurrogateMockG";
const OPEN_F: &str = "SurrogateMockF<>";
const OPEN_I: &str = "SurrogateMockI<>";
const F_STRING: &str = "SurrogateMockF<string>";
const I_STRING: &str = "SurrogateMockI<string>";

fn describe_types(rules: &mut Surrogates<'_>) {
    rules.describe(D, &[C]).unwrap();
    rules.describe(E, &[IE]).unwrap();
    rules.describe(H, &[IE, IG]).unwrap();
    rules.describe(F_STRING, &[OPEN_F]).unwrap();
    rules.describe(OPEN_I, &[OPEN_F]).unwrap();
    rules.describe(I_STRING, &[F_STRING, OPEN_I]).unwrap();
}

fn empty_case(
    stamped: bool,
    class: &str,
    encode: &mut Surrogates<'_>,
    restore: &mut Surrogates<'_>,
    expected: Object,
) {
    let mut graph = Graph::default();
    let root = graph.insert(record(class, vec![]));
    round_trip(&graph, root, stamped, encode, restore, |copy, id| {
        assert_eq!(copy.get(id), Some(&expected));
    });
}

fn match_parent(stamped: bool, source: &str, target: &str, on_read: bool) {
    let mut encode = Surrogates::default();
    let mut restore = Surrogates::default();
    let rules = if on_read { &mut restore } else { &mut encode };
    describe_types(rules);
    rules
        .register(target, |_, _| Ok(record("SurrogateMockB", vec![])))
        .unwrap();
    empty_case(
        stamped,
        source,
        &mut encode,
        &mut restore,
        record("SurrogateMockB", vec![]),
    );
}

fn prefer_specific(stamped: bool) {
    let mut encode = Surrogates::default();
    describe_types(&mut encode);
    encode.register(C, |_, _| Ok(a(1))).unwrap();
    encode
        .register(D, |_, _| Ok(record("SurrogateMockB", vec![])))
        .unwrap();
    empty_case(
        stamped,
        D,
        &mut encode,
        &mut Surrogates::default(),
        record("SurrogateMockB", vec![]),
    );
}

fn prefer_first_interface(stamped: bool) {
    let mut encode = Surrogates::default();
    describe_types(&mut encode);
    encode
        .register(IG, |_, _| Ok(Object::String("g".into())))
        .unwrap();
    encode
        .register(IE, |_, _| Ok(Object::String("e".into())))
        .unwrap();
    empty_case(
        stamped,
        H,
        &mut encode,
        &mut Surrogates::default(),
        Object::String("g".into()),
    );
}

fn generic_case(
    stamped: bool,
    class: &str,
    value: &str,
    configure: impl for<'g> FnOnce(&'g Graph, &mut Surrogates<'g>),
    expected: Option<&str>,
) {
    let mut graph = Graph::default();
    let value = graph.insert(Object::String(value.into()));
    let root = graph.insert(record(class, vec![Value::Ref(value)]));
    let mut encode = Surrogates::default();
    describe_types(&mut encode);
    configure(&graph, &mut encode);
    round_trip(
        &graph,
        root,
        stamped,
        &mut encode,
        &mut Surrogates::default(),
        |copy, id| {
            let Some(Object::String(result)) = copy.get(id) else {
                panic!("expected a string");
            };
            if let Some(expected) = expected {
                assert_eq!(result, expected);
            }
        },
    );
}

fn extract_generic_field(stamped: bool) {
    generic_case(
        stamped,
        F_STRING,
        "test",
        |graph, encode| {
            // As in the upstream callback, use the first instance field.
            encode
                .register(OPEN_F, move |_, object| {
                    let Object::Record { fields, .. } = object else {
                        unreachable!()
                    };
                    Ok(graph.get(ref_id(&fields[0])).unwrap().clone())
                })
                .unwrap();
        },
        None,
    );
}

fn prefer_closed_generic(stamped: bool) {
    generic_case(
        stamped,
        F_STRING,
        "test",
        |_, encode| {
            encode
                .register(OPEN_F, |_, _| Ok(Object::String("general".into())))
                .unwrap();
            encode
                .register(F_STRING, |_, _| Ok(Object::String("special".into())))
                .unwrap();
        },
        Some("special"),
    );
}

fn match_generic_base(stamped: bool, also_register_derived: bool) {
    generic_case(
        stamped,
        I_STRING,
        "something",
        |_, encode| {
            encode
                .register(OPEN_F, move |_, _| {
                    Ok(Object::String(
                        if also_register_derived {
                            "fail"
                        } else {
                            "success"
                        }
                        .into(),
                    ))
                })
                .unwrap();
            if also_register_derived {
                encode
                    .register(OPEN_I, |_, _| Ok(Object::String("success".into())))
                    .unwrap();
            }
        },
        Some("success"),
    );
}

fn disable_specific(stamped: bool) {
    let mut encode = Surrogates::default();
    describe_types(&mut encode);
    encode
        .register(C, |_, _| Ok(Object::String("fail".into())))
        .unwrap();
    encode.disable(D).unwrap();
    empty_case(
        stamped,
        D,
        &mut encode,
        &mut Surrogates::default(),
        record(D, vec![]),
    );
}

modes!(
    derived_read_plain,
    derived_read_stamped,
    match_parent,
    D,
    C,
    true
);
modes!(
    derived_write_plain,
    derived_write_stamped,
    match_parent,
    D,
    C,
    false
);
modes!(
    interface_read_plain,
    interface_read_stamped,
    match_parent,
    E,
    IE,
    true
);
modes!(
    interface_write_plain,
    interface_write_stamped,
    match_parent,
    E,
    IE,
    false
);
modes!(specific_plain, specific_stamped, prefer_specific);
modes!(first_plain, first_stamped, prefer_first_interface);
modes!(
    generic_field_plain,
    generic_field_stamped,
    extract_generic_field
);
modes!(closed_plain, closed_stamped, prefer_closed_generic);
modes!(
    generic_base_plain,
    generic_base_stamped,
    match_generic_base,
    false
);
modes!(
    generic_specific_plain,
    generic_specific_stamped,
    match_generic_base,
    true
);
modes!(disable_plain, disable_stamped, disable_specific);

fn should_reject_registration_after_write(stamped: bool) {
    let mut graph = Graph::default();
    let root = graph.insert(record("System.Object", vec![]));
    let mut rules = Surrogates::default();
    let mut writer = ObjectWriter::new(std::io::sink(), &graph, References::Preserve, true)
        .with_surrogates(&mut rules);
    if stamped {
        writer = writer.with_type_stamps(STAMPS);
    }
    writer.write_object(root).unwrap();
    writer.finish().unwrap();
    assert_eq!(
        rules.register("System.Object", |_, _| Ok(record("System.Object", vec![]))),
        Err(RegistrationClosed)
    );
}

fn should_reject_registration_after_read(stamped: bool) {
    let mut graph = Graph::default();
    let root = graph.insert(record("System.Object", vec![]));
    let mut rules = Surrogates::default();
    round_trip(
        &graph,
        root,
        stamped,
        &mut Surrogates::default(),
        &mut rules,
        |_, _| {},
    );
    assert_eq!(
        rules.register("System.Object", |_, _| Ok(record("System.Object", vec![]))),
        Err(RegistrationClosed)
    );
}

modes!(
    registration_after_write_plain,
    registration_after_write_stamped,
    should_reject_registration_after_write
);
modes!(
    registration_after_read_plain,
    registration_after_read_stamped,
    should_reject_registration_after_read
);
