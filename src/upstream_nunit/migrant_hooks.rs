use crate::migrant::object_stream::{
    FieldInitializer, FieldKind, Graph, Object, ObjectId, ObjectReader, ObjectWriter, ReadHook,
    ReadHookPhase, References, RestoredFrame, Transients, TypeStamp, Value,
};
use std::{
    cell::Cell,
    io::{self, Cursor, Seek},
};

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
    stamp(
        "PostDeserializationMock",
        &[("Invoked", FieldKind::Boolean)],
    ),
    stamp(
        "PostDeserializationMockDerived",
        &[
            ("Invoked", FieldKind::Boolean),
            ("DerivedInvoked", FieldKind::Boolean),
        ],
    ),
    stamp(
        "ClassWithTheReference",
        &[
            ("Id", FieldKind::Integer),
            ("Value", FieldKind::Integer),
            ("RefOne", FieldKind::Reference),
            ("RefTwo", FieldKind::Reference),
        ],
    ),
    stamp(
        "ReferencingPostDeserializationMock",
        &[("mock", FieldKind::Reference)],
    ),
    stamp(
        "ReferencedPostDeserializationMock",
        &[("TestObject", FieldKind::Reference)],
    ),
    stamp("System.Object", &[]),
    stamp(
        "CyclicReferenceMockA",
        &[("B", FieldKind::Reference), ("Str", FieldKind::Reference)],
    ),
    stamp(
        "CyclicReferenceMockB",
        &[("A", FieldKind::Reference), ("Str", FieldKind::Reference)],
    ),
    stamp("LateDeserializationMockA", &[("B", FieldKind::Reference)]),
    stamp(
        "DeserializationMockB",
        &[("PostDeserialized", FieldKind::Boolean)],
    ),
    stamp("DerivedFromClassB", &[]),
];

fn record(graph: &mut Graph, class: &str, fields: Vec<Value>) -> ObjectId {
    graph.insert(Object::Record {
        class: class.into(),
        fields,
    })
}

fn fields(object: Option<&Object>) -> &[Value] {
    let Some(Object::Record { fields, .. }) = object else {
        panic!("expected a record");
    };
    fields
}

fn id(field: &Value) -> ObjectId {
    let Value::Ref(id) = field else {
        panic!("expected a reference");
    };
    *id
}

fn restore(
    graph: &Graph,
    root: ObjectId,
    stamped: bool,
    hooks: &[ReadHook<'_>],
    transients: Option<&Transients<'_>>,
    check: impl FnOnce(&Graph, ObjectId),
) {
    let mut stream = Cursor::new(Vec::new());
    let mut writer = ObjectWriter::new(&mut stream, graph, References::Preserve, true);
    if stamped {
        writer = writer.with_type_stamps(STAMPS);
    }
    if let Some(layout) = transients {
        writer = writer.with_transients(layout);
    }
    writer.write_object(root).unwrap();
    writer.finish().unwrap();
    stream.rewind().unwrap();
    let mut reader = ObjectReader::new(stream, References::Preserve, true).with_read_hooks(hooks);
    if stamped {
        reader = reader.with_type_stamps(STAMPS);
    }
    if let Some(layout) = transients {
        reader = reader.with_transients(layout);
    }
    let root = reader.read_object().unwrap();
    check(reader.graph(), root);
    reader.finish().unwrap();
}

fn set_flag(frame: &mut RestoredFrame<'_>, object: ObjectId, index: usize) -> io::Result<()> {
    frame.fields_mut(object).unwrap()[index] = Value::Boolean(true);
    Ok(())
}

fn post_deserialization(stamped: bool, derived: bool) {
    let class = if derived {
        "PostDeserializationMockDerived"
    } else {
        "PostDeserializationMock"
    };
    let mut graph = Graph::default();
    let original = vec![Value::Boolean(false); if derived { 2 } else { 1 }];
    let root = record(&mut graph, class, original.clone());
    let hooks = [
        ReadHook {
            class,
            phase: ReadHookPhase::Late,
            apply: |frame, id| set_flag(frame, id, 0),
        },
        ReadHook {
            class,
            phase: ReadHookPhase::Late,
            apply: |frame, id| set_flag(frame, id, 1),
        },
    ];
    restore(
        &graph,
        root,
        stamped,
        &hooks[..original.len()],
        None,
        |copy, root| {
            assert_eq!(
                fields(copy.get(root)),
                vec![Value::Boolean(true); original.len()]
            );
        },
    );
    assert_eq!(fields(graph.get(root)), original);
}

thread_local! { static COUNTER: Cell<i64> = const { Cell::new(0) }; }

fn chained_hooks(stamped: bool) {
    COUNTER.set(0);
    let mut graph = Graph::default();
    let class = "ClassWithTheReference";
    let nodes: [ObjectId; 6] = std::array::from_fn(|i| {
        record(
            &mut graph,
            class,
            vec![
                Value::Integer(i as i64),
                Value::Integer(0),
                Value::Null,
                Value::Null,
            ],
        )
    });
    for (source, first, second) in [(0, 1, 2), (1, 3, 4), (2, 1, 5)] {
        let Some(Object::Record { fields, .. }) = graph.get_mut(nodes[source]) else {
            unreachable!()
        };
        fields[2] = Value::Ref(nodes[first]);
        fields[3] = Value::Ref(nodes[second]);
    }
    let root = graph.insert(Object::Array(nodes.map(Value::Ref).to_vec()));
    let hooks = [ReadHook {
        class,
        phase: ReadHookPhase::Immediate,
        apply: |frame, id| {
            let value = COUNTER.with(|counter| {
                let value = counter.get() + 1;
                counter.set(value);
                value
            });
            frame.fields_mut(id).unwrap()[1] = Value::Integer(value);
            Ok(())
        },
    }];
    restore(&graph, root, stamped, &hooks, None, |copy, root| {
        let Some(Object::Array(restored)) = copy.get(root) else {
            panic!("expected array");
        };
        for (index, value) in [(5, 1), (4, 2), (3, 3), (2, 4), (1, 5), (0, 6)] {
            assert_eq!(
                fields(copy.get(id(&restored[index])))[1],
                Value::Integer(value)
            );
        }
    });
}

fn referenced_object_ready(stamped: bool) {
    let mut graph = Graph::default();
    let leaf = record(&mut graph, "System.Object", vec![]);
    let child = record(
        &mut graph,
        "ReferencedPostDeserializationMock",
        vec![Value::Ref(leaf)],
    );
    let class = "ReferencingPostDeserializationMock";
    let root = record(&mut graph, class, vec![Value::Ref(child)]);
    let hooks = [ReadHook {
        class,
        phase: ReadHookPhase::Late,
        apply: |frame, root| {
            let child = fields(frame.get(root))
                .first()
                .and_then(|value| match value {
                    Value::Ref(id) => frame.get(*id),
                    _ => None,
                });
            let leaf = fields(child).first().and_then(|value| match value {
                Value::Ref(id) => frame.get(*id),
                _ => None,
            });
            if leaf.is_none() {
                return Err(io::Error::other("referenced mock is not ready"));
            }
            Ok(())
        },
    }];
    restore(&graph, root, stamped, &hooks, None, |_, _| {});
}

fn cyclic_objects_ready(stamped: bool) {
    let mut graph = Graph::default();
    let text_a = graph.insert(Object::String("Something".into()));
    let text_b = graph.insert(Object::String("Something different".into()));
    let a = record(
        &mut graph,
        "CyclicReferenceMockA",
        vec![Value::Null, Value::Ref(text_a)],
    );
    let b = record(
        &mut graph,
        "CyclicReferenceMockB",
        vec![Value::Ref(a), Value::Ref(text_b)],
    );
    let Some(Object::Record {
        fields: a_fields, ..
    }) = graph.get_mut(a)
    else {
        unreachable!()
    };
    a_fields[0] = Value::Ref(b);
    let hooks = ["CyclicReferenceMockA", "CyclicReferenceMockB"].map(|class| ReadHook {
        class,
        phase: ReadHookPhase::Late,
        apply: |frame, root| {
            let [Value::Ref(other), _] = fields(frame.get(root)) else {
                return Err(io::Error::other("missing cycle edge"));
            };
            let [_, Value::Ref(text)] = fields(frame.get(*other)) else {
                return Err(io::Error::other("missing cyclic object text"));
            };
            if !matches!(frame.get(*text), Some(Object::String(_))) {
                return Err(io::Error::other("cyclic object is not ready"));
            }
            Ok(())
        },
    });
    restore(&graph, a, stamped, &hooks, None, |_, _| {});
}

fn inheritance_order(stamped: bool) {
    let class = "DerivedFromClassB";
    let mut graph = Graph::default();
    let root = record(&mut graph, class, vec![Value::Boolean(false); 3]);
    const LAYOUT: Transients<'static> = Transients {
        classes: &[],
        fields: &[(
            "DerivedFromClassB",
            &[
                (0, FieldInitializer::Default(Value::Boolean(false))),
                (1, FieldInitializer::Default(Value::Boolean(false))),
                (2, FieldInitializer::Default(Value::Boolean(false))),
            ],
        )],
    };
    let hooks = [
        ReadHook {
            class,
            phase: ReadHookPhase::Immediate,
            apply: |frame, id| set_flag(frame, id, 0),
        },
        ReadHook {
            class,
            phase: ReadHookPhase::Immediate,
            apply: |frame, id| {
                assert_eq!(fields(frame.get(id))[0], Value::Boolean(true));
                set_flag(frame, id, 1)
            },
        },
        ReadHook {
            class,
            phase: ReadHookPhase::Immediate,
            apply: |frame, id| {
                assert_eq!(
                    fields(frame.get(id))[..2],
                    [Value::Boolean(true), Value::Boolean(true)]
                );
                set_flag(frame, id, 2)
            },
        },
    ];
    restore(
        &graph,
        root,
        stamped,
        &hooks,
        Some(&LAYOUT),
        |copy, root| {
            assert_eq!(fields(copy.get(root))[2], Value::Boolean(true));
        },
    );
}

macro_rules! modes {
    ($plain:ident, $stamped:ident, $case:expr) => {
        #[test]
        fn $plain() {
            ($case)(false);
        }
        #[test]
        fn $stamped() {
            ($case)(true);
        }
    };
}

modes!(post_plain, post_stamped, |stamped| post_deserialization(
    stamped, false
));
modes!(derived_post_plain, derived_post_stamped, |stamped| {
    post_deserialization(stamped, true)
});
modes!(chain_plain, chain_stamped, chained_hooks);
modes!(
    referenced_plain,
    referenced_stamped,
    referenced_object_ready
);
modes!(cyclic_plain, cyclic_stamped, cyclic_objects_ready);
modes!(inheritance_plain, inheritance_stamped, inheritance_order);

fn reject_surrogate_with_late_hook(stamped: bool) {
    use crate::migrant::surrogate::Surrogates;

    let class = "LateDeserializationMockA";
    let mut graph = Graph::default();
    let b = record(
        &mut graph,
        "DeserializationMockB",
        vec![Value::Boolean(false)],
    );
    let root = record(&mut graph, class, vec![Value::Ref(b)]);
    let mut stream = Cursor::new(Vec::new());
    let mut writer = ObjectWriter::new(&mut stream, &graph, References::Preserve, true);
    if stamped {
        writer = writer.with_type_stamps(STAMPS);
    }
    writer.write_object(root).unwrap();
    writer.finish().unwrap();
    stream.rewind().unwrap();

    let hooks = [
        ReadHook {
            class: "DeserializationMockB",
            phase: ReadHookPhase::Immediate,
            apply: |frame, id| set_flag(frame, id, 0),
        },
        ReadHook {
            class,
            phase: ReadHookPhase::Late,
            apply: |frame, root| {
                let target = id(&fields(frame.get(root))[0]);
                assert_eq!(fields(frame.get(target))[0], Value::Boolean(true));
                Ok(())
            },
        },
    ];

    let mut surrogates = Surrogates::default();
    surrogates
        .register(class, |_, _| {
            Ok(Object::Record {
                class: "System.Object".into(),
                fields: vec![],
            })
        })
        .unwrap();
    let mut reader = ObjectReader::new(stream, References::Preserve, true)
        .with_read_hooks(&hooks)
        .with_surrogates(&mut surrogates);
    if stamped {
        reader = reader.with_type_stamps(STAMPS);
    }
    let error = reader.read_object().unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert!(
        error
            .to_string()
            .contains("cannot be used with a surrogate")
    );
}

modes!(
    surrogate_conflict_plain,
    surrogate_conflict_stamped,
    reject_surrogate_with_late_hook
);

fn global_observers(stamped: bool) {
    let pre_serialization = Cell::new(0);
    let post_serialization = Cell::new(0);
    let post_deserialization = Cell::new(0);
    let mut graph = Graph::default();
    let entries = ["One", "Two"].map(|text| Value::Ref(graph.insert(Object::String(text.into()))));
    let root = graph.insert(Object::Array(entries.to_vec()));
    let mut stream = Cursor::new(Vec::new());
    let mut writer = ObjectWriter::new(&mut stream, &graph, References::Preserve, true)
        .with_write_observers(
            |_| pre_serialization.set(pre_serialization.get() + 1),
            |_| post_serialization.set(post_serialization.get() + 1),
        );
    if stamped {
        writer = writer.with_type_stamps(STAMPS);
    }
    writer.write_object(root).unwrap();
    writer.finish().unwrap();

    assert_eq!(pre_serialization.get(), 3);
    assert_eq!(post_serialization.get(), 3);
    assert_eq!(post_deserialization.get(), 0);

    stream.rewind().unwrap();
    let mut reader = ObjectReader::new(stream, References::Preserve, true)
        .with_read_observer(|_| post_deserialization.set(post_deserialization.get() + 1));
    if stamped {
        reader = reader.with_type_stamps(STAMPS);
    }
    reader.read_object().unwrap();
    assert_eq!(post_deserialization.get(), 3);
    reader.finish().unwrap();
}

modes!(global_plain, global_stamped, global_observers);
