use crate::migrant::generated_record::{generated_record, Codec};
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
    stamp("PreSerializationMock", &[("Invoked", FieldKind::Boolean)]),
    stamp(
        "PreSerializationMockDerived",
        &[
            ("Invoked", FieldKind::Boolean),
            ("DerivedInvoked", FieldKind::Boolean),
        ],
    ),
    stamp(
        "ImmediatePostSerializationMock",
        &[("Invoked", FieldKind::Boolean)],
    ),
    stamp(
        "ImmediatePostSerializationMockDerived",
        &[
            ("Invoked", FieldKind::Boolean),
            ("DerivedInvoked", FieldKind::Boolean),
        ],
    ),
    stamp("StaticImmediatePostSerializationMock", &[]),
    stamp("LatePostSerializationMockA", &[("B", FieldKind::Reference)]),
    stamp(
        "PostSerializationMockB",
        &[("PostSerialized", FieldKind::Boolean)],
    ),
    stamp("ClassSendingExcetpionDuringSerialization", &[]),
    stamp(
        "PrePostSerializationMock",
        &[
            ("PreExecuted", FieldKind::Boolean),
            ("PostExecuted", FieldKind::Boolean),
            ("SendingException", FieldKind::Reference),
        ],
    ),
    stamp(
        "LatePrePostSerializationMock",
        &[
            ("PreExecuted", FieldKind::Boolean),
            ("PostExecuted", FieldKind::Boolean),
            ("SendingException", FieldKind::Reference),
        ],
    ),
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
    stamp(
        "ForOrderTestA",
        &[
            ("HookInvokedOn", FieldKind::Integer),
            ("B", FieldKind::Reference),
        ],
    ),
    stamp("ForOrderTestB", &[("HookInvokedOn", FieldKind::Integer)]),
    stamp(
        "ForLateOrderTestA",
        &[
            ("HookInvokedOn", FieldKind::Integer),
            ("B", FieldKind::Reference),
        ],
    ),
    stamp(
        "ForLateOrderTestB",
        &[("HookInvokedOn", FieldKind::Integer)],
    ),
    stamp(
        "MockWithVirtualDerived",
        &[
            ("counterValue", FieldKind::Integer),
            ("BasePreSerializationCounter", FieldKind::Integer),
            ("DerivedPreSerializationCounter", FieldKind::Integer),
            ("BasePostSerializationCounter", FieldKind::Integer),
            ("DerivedPostSerializationCounter", FieldKind::Integer),
            ("BasePostDeserializationCounter", FieldKind::Integer),
            ("DerivedPostDeserializationCounter", FieldKind::Integer),
        ],
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

const CODECS: &[Codec] = &[
    generated_record!("PreSerializationMock"; invoked: boolean),
    generated_record!("PreSerializationMockDerived"; invoked: boolean, derived_invoked: boolean),
    generated_record!("ImmediatePostSerializationMock"; invoked: boolean),
    generated_record!("ImmediatePostSerializationMockDerived"; invoked: boolean, derived_invoked: boolean),
    generated_record!("StaticImmediatePostSerializationMock"; ),
    generated_record!("LatePostSerializationMockA"; b: reference),
    generated_record!("PostSerializationMockB"; post_serialized: boolean),
    generated_record!("ClassSendingExcetpionDuringSerialization"; ),
    generated_record!("PrePostSerializationMock"; pre_executed: boolean, post_executed: boolean, sending_exception: reference),
    generated_record!("LatePrePostSerializationMock"; pre_executed: boolean, post_executed: boolean, sending_exception: reference),
    generated_record!("PostDeserializationMock"; invoked: boolean),
    generated_record!("PostDeserializationMockDerived"; invoked: boolean, derived_invoked: boolean),
    generated_record!("ClassWithTheReference"; id: integer, value: integer, ref_one: reference, ref_two: reference),
    generated_record!("ReferencingPostDeserializationMock"; mock: reference),
    generated_record!("ReferencedPostDeserializationMock"; test_object: reference),
    generated_record!("ForOrderTestA"; hook_invoked_on: integer, b: reference),
    generated_record!("ForOrderTestB"; hook_invoked_on: integer),
    generated_record!("ForLateOrderTestA"; hook_invoked_on: integer, b: reference),
    generated_record!("ForLateOrderTestB"; hook_invoked_on: integer),
    generated_record!("MockWithVirtualDerived"; counter_value: integer, base_pre_serialization_counter: integer, derived_pre_serialization_counter: integer, base_post_serialization_counter: integer, derived_post_serialization_counter: integer, base_post_deserialization_counter: integer, derived_post_deserialization_counter: integer),
    generated_record!("System.Object"; ),
    generated_record!("CyclicReferenceMockA"; b: reference, str: reference),
    generated_record!("CyclicReferenceMockB"; a: reference, str: reference),
    generated_record!("LateDeserializationMockA"; b: reference),
    generated_record!("DeserializationMockB"; post_deserialized: boolean),
    generated_record!("DerivedFromClassB"; ),
];

#[derive(Clone, Copy, Default)]
struct Method {
    stamped: bool,
    write: bool,
    read: bool,
}

impl Method {
    const fn new(stamped: bool, write: bool, read: bool) -> Self {
        Self {
            stamped,
            write,
            read,
        }
    }

    fn writer<'a, 's, W: io::Write>(
        self,
        writer: ObjectWriter<'a, 's, W>,
    ) -> ObjectWriter<'a, 's, W> {
        if self.write {
            writer.with_generated_records(CODECS)
        } else {
            writer
        }
    }

    fn reader<'a, 's, R: io::Read>(
        self,
        reader: ObjectReader<'a, 's, R>,
    ) -> ObjectReader<'a, 's, R> {
        if self.read {
            reader.with_generated_records(CODECS)
        } else {
            reader
        }
    }

    fn write_hooked(
        self,
        stream: impl io::Write,
        graph: &mut Graph,
        root: ObjectId,
        hooks: &[WriteHook<'_>],
    ) -> io::Result<()> {
        let stamps = self.stamped.then_some(STAMPS);
        if self.write {
            write_hooked_with_codecs(stream, graph, root, stamps, hooks, Some(CODECS))
        } else {
            write_hooked(stream, graph, root, stamps, hooks)
        }
    }
}

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
    method: Method,
    hooks: &[ReadHook<'_>],
    transients: Option<&Transients<'_>>,
    check: impl FnOnce(&Graph, ObjectId),
) {
    let mut stream = Cursor::new(Vec::new());
    let mut writer = ObjectWriter::new(&mut stream, graph, References::Preserve, true);
    writer = method.writer(writer);
    if method.stamped {
        writer = writer.with_type_stamps(STAMPS);
    }
    if let Some(layout) = transients {
        writer = writer.with_transients(layout);
    }
    writer.write_object(root).unwrap();
    writer.finish().unwrap();
    stream.rewind().unwrap();
    let mut reader = ObjectReader::new(stream, References::Preserve, true).with_read_hooks(hooks);
    reader = method.reader(reader);
    if method.stamped {
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

fn post_deserialization(method: Method, derived: bool) {
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
        method,
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

fn chained_hooks(method: Method) {
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
    restore(&graph, root, method, &hooks, None, |copy, root| {
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

fn referenced_object_ready(method: Method) {
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
    restore(&graph, root, method, &hooks, None, |_, _| {});
}

fn cyclic_objects_ready(method: Method) {
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
    restore(&graph, a, method, &hooks, None, |_, _| {});
}

fn inheritance_order(method: Method) {
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
    restore(&graph, root, method, &hooks, Some(&LAYOUT), |copy, root| {
        assert_eq!(fields(copy.get(root))[2], Value::Boolean(true));
    });
}

macro_rules! modes {
    ($plain:ident, $stamped:ident, $case:expr) => {
        #[test]
        fn $plain() {
            ($case)(Method::default());
        }
        #[test]
        fn $stamped() {
            ($case)(Method::new(true, false, false));
        }
        mod $plain {
            use super::*;
            #[test]
            fn reader_plain() {
                ($case)(Method::new(false, false, true));
            }
            #[test]
            fn reader_stamped() {
                ($case)(Method::new(true, false, true));
            }
            #[test]
            fn writer_plain() {
                ($case)(Method::new(false, true, false));
            }
            #[test]
            fn writer_stamped() {
                ($case)(Method::new(true, true, false));
            }
            #[test]
            fn both_plain() {
                ($case)(Method::new(false, true, true));
            }
            #[test]
            fn both_stamped() {
                ($case)(Method::new(true, true, true));
            }
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

fn reject_surrogate_with_late_hook(method: Method) {
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
    writer = method.writer(writer);
    if method.stamped {
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
    reader = method.reader(reader);
    if method.stamped {
        reader = reader.with_type_stamps(STAMPS);
    }
    let error = reader.read_object().unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert!(error
        .to_string()
        .contains("cannot be used with a surrogate"));
}

modes!(
    surrogate_conflict_plain,
    surrogate_conflict_stamped,
    reject_surrogate_with_late_hook
);

fn global_observers(method: Method) {
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
    writer = method.writer(writer);
    if method.stamped {
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
    reader = method.reader(reader);
    if method.stamped {
        reader = reader.with_type_stamps(STAMPS);
    }
    reader.read_object().unwrap();
    assert_eq!(post_deserialization.get(), 3);
    reader.finish().unwrap();
}

modes!(global_plain, global_stamped, global_observers);

use crate::migrant::write_hooks::{
    write_hooked, write_hooked_with_codecs, WriteHook, WriteHookPhase,
};

fn write_and_restore(
    graph: &mut Graph,
    root: ObjectId,
    method: Method,
    hooks: &[WriteHook<'_>],
) -> (ObjectReader<'static, 'static, Cursor<Vec<u8>>>, ObjectId) {
    write_and_restore_with_read_hooks(graph, root, method, hooks, &[])
}

fn write_and_restore_with_read_hooks<'a>(
    graph: &mut Graph,
    root: ObjectId,
    method: Method,
    hooks: &[WriteHook<'_>],
    read_hooks: &'a [ReadHook<'a>],
) -> (ObjectReader<'a, 'static, Cursor<Vec<u8>>>, ObjectId) {
    let mut stream = Cursor::new(Vec::new());
    method
        .write_hooked(&mut stream, graph, root, hooks)
        .unwrap();
    stream.rewind().unwrap();
    let mut reader =
        ObjectReader::new(stream, References::Preserve, true).with_read_hooks(read_hooks);
    reader = method.reader(reader);
    if method.stamped {
        reader = reader.with_type_stamps(STAMPS);
    }
    let root = reader.read_object().unwrap();
    (reader, root)
}

fn set_original_flag(graph: &mut Graph, object: ObjectId, index: usize) -> io::Result<()> {
    let Some(Object::Record { fields, .. }) = graph.get_mut(object) else {
        panic!("expected a record");
    };
    fields[index] = Value::Boolean(true);
    Ok(())
}

fn phase_flags(method: Method, before: bool, derived: bool) {
    let (class, phase) = match (before, derived) {
        (true, false) => ("PreSerializationMock", WriteHookPhase::Before),
        (true, true) => ("PreSerializationMockDerived", WriteHookPhase::Before),
        (false, false) => ("ImmediatePostSerializationMock", WriteHookPhase::After),
        (false, true) => (
            "ImmediatePostSerializationMockDerived",
            WriteHookPhase::After,
        ),
    };
    let count = if derived { 2 } else { 1 };
    let mut graph = Graph::default();
    let root = record(&mut graph, class, vec![Value::Boolean(false); count]);
    let hooks = [
        WriteHook {
            class,
            phase,
            apply: |graph, id| set_original_flag(graph, id, 0),
        },
        WriteHook {
            class,
            phase,
            apply: |graph, id| set_original_flag(graph, id, 1),
        },
    ];
    let (reader, copy) = write_and_restore(&mut graph, root, method, &hooks[..count]);
    assert_eq!(fields(graph.get(root)), vec![Value::Boolean(true); count]);
    assert_eq!(
        fields(reader.graph().get(copy)),
        vec![Value::Boolean(before); count]
    );
}

thread_local! { static STATIC_INVOKED: Cell<bool> = const { Cell::new(false) }; }

fn static_post(method: Method) {
    STATIC_INVOKED.set(false);
    let mut graph = Graph::default();
    let class = "StaticImmediatePostSerializationMock";
    let root = record(&mut graph, class, vec![]);
    let hooks = [WriteHook {
        class,
        phase: WriteHookPhase::After,
        apply: |_, _| {
            STATIC_INVOKED.set(true);
            Ok(())
        },
    }];
    write_and_restore(&mut graph, root, method, &hooks);
    assert!(STATIC_INVOKED.get());
}

fn child_immediate_before_late(method: Method) {
    let mut graph = Graph::default();
    let child = record(
        &mut graph,
        "PostSerializationMockB",
        vec![Value::Boolean(false)],
    );
    let root = record(
        &mut graph,
        "LatePostSerializationMockA",
        vec![Value::Ref(child)],
    );
    let hooks = [
        WriteHook {
            class: "PostSerializationMockB",
            phase: WriteHookPhase::After,
            apply: |graph, id| set_original_flag(graph, id, 0),
        },
        WriteHook {
            class: "LatePostSerializationMockA",
            phase: WriteHookPhase::Late,
            apply: |graph, root| {
                let child = id(&fields(graph.get(root))[0]);
                if fields(graph.get(child))[0] != Value::Boolean(true) {
                    return Err(io::Error::other(
                        "child immediate serialization hook has not run",
                    ));
                }
                Ok(())
            },
        },
    ];
    write_and_restore(&mut graph, root, method, &hooks);
}

fn post_even_on_error(method: Method, late: bool) {
    let mut graph = Graph::default();
    let child_class = "ClassSendingExcetpionDuringSerialization";
    let child = record(&mut graph, child_class, vec![]);
    let class = if late {
        "LatePrePostSerializationMock"
    } else {
        "PrePostSerializationMock"
    };
    let root = record(
        &mut graph,
        class,
        vec![
            Value::Boolean(false),
            Value::Boolean(false),
            Value::Ref(child),
        ],
    );
    let hooks = [
        WriteHook {
            class,
            phase: WriteHookPhase::Before,
            apply: |graph, id| set_original_flag(graph, id, 0),
        },
        WriteHook {
            class,
            phase: if late {
                WriteHookPhase::Late
            } else {
                WriteHookPhase::After
            },
            apply: |graph, id| set_original_flag(graph, id, 1),
        },
        WriteHook {
            class: child_class,
            phase: WriteHookPhase::Before,
            apply: |_, _| Err(io::Error::other("serialization hook failure")),
        },
    ];
    let error = method
        .write_hooked(Vec::new(), &mut graph, root, &hooks)
        .unwrap_err();
    assert_eq!(error.to_string(), "serialization hook failure");
    assert_eq!(
        fields(graph.get(root))[..2],
        [Value::Boolean(true), Value::Boolean(true)]
    );
}

modes!(pre_write_plain, pre_write_stamped, |s| phase_flags(
    s, true, false
));
modes!(derived_pre_write_plain, derived_pre_write_stamped, |s| {
    phase_flags(s, true, true)
});
modes!(post_write_plain, post_write_stamped, |s| phase_flags(
    s, false, false
));
modes!(derived_post_write_plain, derived_post_write_stamped, |s| {
    phase_flags(s, false, true)
});
modes!(static_post_plain, static_post_stamped, static_post);
modes!(
    late_post_write_plain,
    late_post_write_stamped,
    child_immediate_before_late
);
// The upstream method uses the same serialization object and assertions.
modes!(
    late_post_read_named_plain,
    late_post_read_named_stamped,
    child_immediate_before_late
);
modes!(error_early_plain, error_early_stamped, |s| {
    post_even_on_error(s, false)
});
modes!(
    error_late_plain,
    error_late_stamped,
    |s| post_even_on_error(s, true)
);

fn mark_time(fields: &mut [Value]) -> io::Result<()> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_nanos();
    fields[0] = Value::Integer(i64::try_from(nanos).map_err(io::Error::other)?);
    std::thread::sleep(std::time::Duration::from_millis(100));
    Ok(())
}

fn child_before_parent(method: Method, late: bool) {
    let (parent, child, write_phase, read_phase) = if late {
        (
            "ForLateOrderTestA",
            "ForLateOrderTestB",
            WriteHookPhase::Late,
            ReadHookPhase::Late,
        )
    } else {
        (
            "ForOrderTestA",
            "ForOrderTestB",
            WriteHookPhase::After,
            ReadHookPhase::Immediate,
        )
    };
    let mut graph = Graph::default();
    let b = record(&mut graph, child, vec![Value::Integer(0)]);
    let root = record(&mut graph, parent, vec![Value::Integer(0), Value::Ref(b)]);
    let write_hooks = [parent, child].map(|class| WriteHook {
        class,
        phase: write_phase,
        apply: |graph, id| {
            let Some(Object::Record { fields, .. }) = graph.get_mut(id) else {
                panic!("expected record")
            };
            mark_time(fields)
        },
    });
    let read_hooks = [parent, child].map(|class| ReadHook {
        class,
        phase: read_phase,
        apply: |frame, id| mark_time(frame.fields_mut(id).unwrap()),
    });
    let (reader, copy) =
        write_and_restore_with_read_hooks(&mut graph, root, method, &write_hooks, &read_hooks);
    for (graph, root) in [(&graph, root), (reader.graph(), copy)] {
        let root = fields(graph.get(root));
        let child = fields(graph.get(id(&root[1])));
        let (Value::Integer(parent_time), Value::Integer(child_time)) = (&root[0], &child[0])
        else {
            panic!("expected hook timestamps");
        };
        assert!(child_time < parent_time);
    }
}

trait VirtualHook {
    fn invoke(&self, fields: &mut [Value], destination: usize);
}

struct VirtualBase;

impl VirtualHook for VirtualBase {
    fn invoke(&self, fields: &mut [Value], destination: usize) {
        let Value::Integer(counter) = &mut fields[0] else {
            panic!("expected counter")
        };
        *counter += 1;
        fields[destination] = Value::Integer(*counter);
    }
}

struct VirtualDerived;

impl VirtualHook for VirtualDerived {
    fn invoke(&self, fields: &mut [Value], destination: usize) {
        VirtualBase.invoke(fields, destination);
        VirtualBase.invoke(fields, destination + 1);
    }
}

fn invoke_virtual(fields: &mut [Value], destination: usize) -> io::Result<()> {
    let hook: &dyn VirtualHook = &VirtualDerived;
    hook.invoke(fields, destination);
    Ok(())
}

fn write_virtual(graph: &mut Graph, id: ObjectId, destination: usize) -> io::Result<()> {
    let Some(Object::Record { fields, .. }) = graph.get_mut(id) else {
        panic!("expected record")
    };
    invoke_virtual(fields, destination)
}

fn virtual_hook_dispatch(method: Method) {
    let mut graph = Graph::default();
    let class = "MockWithVirtualDerived";
    let root = record(&mut graph, class, vec![Value::Integer(0); 7]);
    let write = [
        WriteHook {
            class,
            phase: WriteHookPhase::Before,
            apply: |graph, id| write_virtual(graph, id, 1),
        },
        WriteHook {
            class,
            phase: WriteHookPhase::After,
            apply: |graph, id| write_virtual(graph, id, 3),
        },
    ];
    let read = [ReadHook {
        class,
        phase: ReadHookPhase::Immediate,
        apply: |frame, id| invoke_virtual(frame.fields_mut(id).unwrap(), 5),
    }];
    let (reader, copy) = write_and_restore_with_read_hooks(&mut graph, root, method, &write, &read);
    assert_eq!(
        fields(graph.get(root))[1..5],
        [1, 2, 3, 4].map(Value::Integer)
    );
    assert_eq!(
        fields(reader.graph().get(copy))[5..7],
        [3, 4].map(Value::Integer)
    );
}

modes!(immediate_order_plain, immediate_order_stamped, |s| {
    child_before_parent(s, false)
});
modes!(late_order_plain, late_order_stamped, |s| {
    child_before_parent(s, true)
});
modes!(
    virtual_dispatch_plain,
    virtual_dispatch_stamped,
    virtual_hook_dispatch
);
