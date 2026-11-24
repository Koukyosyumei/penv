use crate::migrant::{
    generated_record::{Codec, generated_record},
    object_stream::{
        FieldKind, Graph, Object, ObjectId, ObjectReader, ObjectWriter, References, TypeHierarchy,
        TypeStamp, Value, VersionPolicy,
    },
};

fn stamp<'a>(class: &'a str, revision: u64, fields: &'a [(&'a str, FieldKind)]) -> TypeStamp<'a> {
    TypeStamp {
        class,
        module: "Dynamic",
        revision,
        fields,
    }
}

fn record(graph: &mut Graph, class: &str, fields: Vec<Value>) -> ObjectId {
    graph.insert(Object::Record {
        class: class.into(),
        fields,
    })
}

fn text(graph: &mut Graph, value: &str) -> Value {
    Value::Ref(graph.insert(Object::String(value.into())))
}

fn fields(graph: &Graph, object: ObjectId) -> &[Value] {
    let Some(Object::Record { fields, .. }) = graph.get(object) else {
        panic!("expected a restored record");
    };
    fields
}

fn reference(value: &Value) -> ObjectId {
    let Value::Ref(id) = value else {
        panic!("expected object reference");
    };
    *id
}

fn transfer(
    graph: &Graph,
    root: ObjectId,
    source: &[TypeStamp<'_>],
    target: &[TypeStamp<'_>],
    codes: &[Codec],
    modes: (bool, bool),
    mut policy: VersionPolicy,
    bases: Option<(&[&str], &[&str])>,
) -> (Graph, ObjectId) {
    policy.allow_module_revision_change = true;
    let mut bytes = Vec::new();
    let mut output =
        ObjectWriter::new(&mut bytes, graph, References::PerRoot, true).with_type_stamps(source);
    let source_hierarchy = [TypeHierarchy {
        class: "A",
        bases: bases.map_or(&[], |(before, _)| before),
    }];
    if bases.is_some() {
        output = output.with_type_hierarchies(source, &source_hierarchy);
    }
    if modes.0 {
        output = output.with_generated_records(codes);
    }
    output.write_object(root).unwrap();
    output.finish().unwrap();

    let mut input = ObjectReader::new(bytes.as_slice(), References::PerRoot, true)
        .with_type_stamps(target)
        .with_version_policy(policy);
    let target_hierarchy = [TypeHierarchy {
        class: "A",
        bases: bases.map_or(&[], |(_, after)| after),
    }];
    if bases.is_some() {
        input = input.with_type_hierarchies(target, &target_hierarchy);
    }
    if modes.1 {
        input = input.with_generated_records(codes);
    }
    let restored = input.read_object().unwrap();
    (input.finish_into_graph().unwrap(), restored)
}

const SHORT: &[(&str, FieldKind)] = &[("a", FieldKind::Reference), ("c", FieldKind::Reference)];
const LONG: &[(&str, FieldKind)] = &[
    ("a", FieldKind::Reference),
    ("b", FieldKind::Integer),
    ("c", FieldKind::Reference),
];

fn field_removal(write: bool, read: bool, class: &str, codes: &[Codec]) {
    let mut graph = Graph::default();
    let first = text(&mut graph, "testing");
    let last = text(&mut graph, "finish");
    let root = record(&mut graph, class, vec![first, Value::Integer(147), last]);
    let (graph, root) = transfer(
        &graph,
        root,
        &[stamp(class, 1, LONG)],
        &[stamp(class, 2, SHORT)],
        codes,
        (write, read),
        VersionPolicy {
            allow_field_removal: true,
            ..VersionPolicy::default()
        },
        None,
    );
    let result = fields(&graph, root);
    assert_eq!(
        graph.get(reference(&result[0])),
        Some(&Object::String("testing".into()))
    );
    assert_eq!(
        graph.get(reference(&result[1])),
        Some(&Object::String("finish".into()))
    );
}

fn class_field_removal(write: bool, read: bool) {
    const CODECS: &[Codec] = &[generated_record!("A"; a: reference, b: integer, c: reference)];
    field_removal(write, read, "A", CODECS);
}

fn struct_field_removal(write: bool, read: bool) {
    const CODECS: &[Codec] =
        &[generated_record!("struct A"; a: reference, b: integer, c: reference)];
    field_removal(write, read, "struct A", CODECS);
}

fn field_insertion(write: bool, read: bool) {
    const CODECS: &[Codec] = &[generated_record!("A"; a: reference, c: reference)];
    let mut graph = Graph::default();
    let first = text(&mut graph, "testing");
    let last = text(&mut graph, "finish");
    let root = record(&mut graph, "A", vec![first, last]);
    let (graph, root) = transfer(
        &graph,
        root,
        &[stamp("A", 1, SHORT)],
        &[stamp("A", 2, LONG)],
        CODECS,
        (write, read),
        VersionPolicy {
            allow_field_addition: true,
            ..VersionPolicy::default()
        },
        None,
    );
    let result = fields(&graph, root);
    assert_eq!(
        graph.get(reference(&result[0])),
        Some(&Object::String("testing".into()))
    );
    assert_eq!(result[1], Value::Integer(0));
    assert_eq!(
        graph.get(reference(&result[2])),
        Some(&Object::String("finish".into()))
    );
}

fn nested_struct_removal(write: bool, read: bool) {
    const OUTER: &[(&str, FieldKind)] = &[("a", FieldKind::Reference), ("b", FieldKind::Reference)];
    const NESTED: &[(&str, FieldKind)] = &[("a", FieldKind::Reference), ("b", FieldKind::Integer)];
    const CODECS: &[Codec] = &[
        generated_record!("A"; a: reference, b: reference),
        generated_record!("struct B"; a: reference, b: integer),
    ];
    let mut graph = Graph::default();
    let first = text(&mut graph, "testing");
    let nested_text = text(&mut graph, "text");
    let nested = record(
        &mut graph,
        "struct B",
        vec![nested_text, Value::Integer(147)],
    );
    let root = record(&mut graph, "A", vec![first, Value::Ref(nested)]);
    let (graph, root) = transfer(
        &graph,
        root,
        &[stamp("A", 1, OUTER), stamp("struct B", 1, NESTED)],
        &[stamp("A", 2, OUTER), stamp("struct B", 2, &NESTED[1..])],
        CODECS,
        (write, read),
        VersionPolicy {
            allow_field_removal: true,
            ..VersionPolicy::default()
        },
        None,
    );
    let outer = fields(&graph, root);
    assert_eq!(
        graph.get(reference(&outer[0])),
        Some(&Object::String("testing".into()))
    );
    assert_eq!(fields(&graph, reference(&outer[1]))[0], Value::Integer(147));
}

fn base_change(write: bool, read: bool, add: bool) {
    const FIELD: &[(&str, FieldKind)] = &[("f", FieldKind::Reference)];
    const CODECS: &[Codec] = &[
        generated_record!("A"; f: reference),
        generated_record!("System.Object";),
    ];
    let mut graph = Graph::default();
    let value = record(&mut graph, "System.Object", vec![]);
    let root = record(&mut graph, "A", vec![Value::Ref(value)]);
    let bases: (&[&str], &[&str]) = if add { (&[], &["X"]) } else { (&["X"], &[]) };
    let (graph, root) = transfer(
        &graph,
        root,
        &[stamp("A", 1, FIELD), stamp("System.Object", 0, &[])],
        &[stamp("A", 2, FIELD), stamp("System.Object", 0, &[])],
        CODECS,
        (write, read),
        VersionPolicy {
            allow_inheritance_chain_change: true,
            ..VersionPolicy::default()
        },
        Some(bases),
    );
    assert!(graph.get(reference(&fields(&graph, root)[0])).is_some());
}

fn base_addition(write: bool, read: bool) {
    base_change(write, read, true);
}
fn base_removal(write: bool, read: bool) {
    base_change(write, read, false);
}

macro_rules! configurations {
    ($module:ident, $case:ident) => {
        mod $module {
            use super::*;
            #[test]
            fn write_false_read_false() {
                $case(false, false);
            }
            #[test]
            fn write_false_read_true() {
                $case(false, true);
            }
            #[test]
            fn write_true_read_false() {
                $case(true, false);
            }
            #[test]
            fn write_true_read_true() {
                $case(true, true);
            }
        }
    };
}
configurations!(class_field_removal, class_field_removal);
configurations!(struct_field_removal, struct_field_removal);
configurations!(nested_struct_removal, nested_struct_removal);
configurations!(field_insertion, field_insertion);
configurations!(base_addition, base_addition);
configurations!(base_removal, base_removal);
