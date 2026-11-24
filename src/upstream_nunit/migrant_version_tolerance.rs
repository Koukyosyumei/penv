use crate::migrant::{
    generated_record::{Codec, generated_record},
    object_stream::{
        FieldKind, Graph, Object, ObjectReader, ObjectWriter, References, TypeHierarchy, TypeStamp,
        Value, VersionPolicy, VersionToleranceError,
    },
};

const GUID_FIELDS: &[(&str, FieldKind)] = &[("Field", FieldKind::Integer)];
const GUID_CODECS: &[Codec] = &[generated_record!("A"; field: integer)];

#[derive(Clone, Copy)]
enum Level {
    Exact,
    Inheritance,
    Add,
    Remove,
    AddAndRemove,
    Guid,
}

fn round_trip(
    source: TypeStamp<'_>,
    target: TypeStamp<'_>,
    values: Vec<Value>,
    generated_write: bool,
    generated_read: bool,
    codecs: &[Codec],
    policy: VersionPolicy,
    hierarchy: Option<(&[&str], &[&str])>,
) -> std::io::Result<Vec<Value>> {
    let mut graph = Graph::default();
    let root = graph.insert(Object::Record {
        class: source.class.into(),
        fields: values,
    });
    let mut stream = Vec::new();
    let source = [source];
    let target = [target];
    let mut writer =
        ObjectWriter::new(&mut stream, &graph, References::PerRoot, true).with_type_stamps(&source);
    let source_hierarchy = [TypeHierarchy {
        class: source[0].class,
        bases: hierarchy.map_or(&[], |(bases, _)| bases),
    }];
    if hierarchy.is_some() {
        writer = writer.with_type_hierarchies(&source, &source_hierarchy);
    }
    if generated_write {
        writer = writer.with_generated_records(codecs);
    }
    writer.write_object(root).unwrap();
    writer.finish().unwrap();
    let mut reader = ObjectReader::new(stream.as_slice(), References::PerRoot, true)
        .with_type_stamps(&target)
        .with_version_policy(policy);
    let target_hierarchy = [TypeHierarchy {
        class: target[0].class,
        bases: hierarchy.map_or(&[], |(_, bases)| bases),
    }];
    if hierarchy.is_some() {
        reader = reader.with_type_hierarchies(&target, &target_hierarchy);
    }
    if generated_read {
        reader = reader.with_generated_records(codecs);
    }
    let root = reader.read_object()?;
    let Object::Record { fields, .. } = reader.graph().get(root).unwrap() else {
        panic!("expected restored record");
    };
    let fields = fields.clone();
    reader.finish()?;
    Ok(fields)
}

fn assert_tolerance(result: std::io::Result<Vec<Value>>, succeeds: bool) {
    match result {
        Ok(_) => assert!(succeeds, "unexpected successful restoration"),
        Err(error) => {
            assert!(!succeeds, "{error}");
            assert!(
                error.get_ref().unwrap().is::<VersionToleranceError>(),
                "{error}"
            );
        }
    }
}

fn guid(write: bool, read: bool, level: Level) {
    let source = TypeStamp {
        class: "A",
        module: "Dynamic",
        revision: 1,
        fields: GUID_FIELDS,
    };
    let target = TypeStamp {
        revision: 2,
        ..source
    };
    let allowed = !matches!(level, Level::Exact);
    let result = round_trip(
        source,
        target,
        vec![Value::Integer(0)],
        write,
        read,
        GUID_CODECS,
        VersionPolicy {
            allow_module_revision_change: allowed,
            ..VersionPolicy::default()
        },
        None,
    );
    assert_tolerance(result, allowed);
}

mod module_revision {
    use super::*;
    #[test]
    fn write_false_read_false_exact() {
        guid(false, false, Level::Exact);
    }
    #[test]
    fn write_false_read_false_inheritance() {
        guid(false, false, Level::Inheritance);
    }
    #[test]
    fn write_false_read_false_add() {
        guid(false, false, Level::Add);
    }
    #[test]
    fn write_false_read_false_remove() {
        guid(false, false, Level::Remove);
    }
    #[test]
    fn write_false_read_false_guid() {
        guid(false, false, Level::Guid);
    }
    #[test]
    fn write_false_read_true_exact() {
        guid(false, true, Level::Exact);
    }
    #[test]
    fn write_false_read_true_inheritance() {
        guid(false, true, Level::Inheritance);
    }
    #[test]
    fn write_false_read_true_add() {
        guid(false, true, Level::Add);
    }
    #[test]
    fn write_false_read_true_remove() {
        guid(false, true, Level::Remove);
    }
    #[test]
    fn write_false_read_true_guid() {
        guid(false, true, Level::Guid);
    }
    #[test]
    fn write_true_read_false_exact() {
        guid(true, false, Level::Exact);
    }
    #[test]
    fn write_true_read_false_inheritance() {
        guid(true, false, Level::Inheritance);
    }
    #[test]
    fn write_true_read_false_add() {
        guid(true, false, Level::Add);
    }
    #[test]
    fn write_true_read_false_remove() {
        guid(true, false, Level::Remove);
    }
    #[test]
    fn write_true_read_false_guid() {
        guid(true, false, Level::Guid);
    }
    #[test]
    fn write_true_read_true_exact() {
        guid(true, true, Level::Exact);
    }
    #[test]
    fn write_true_read_true_inheritance() {
        guid(true, true, Level::Inheritance);
    }
    #[test]
    fn write_true_read_true_add() {
        guid(true, true, Level::Add);
    }
    #[test]
    fn write_true_read_true_remove() {
        guid(true, true, Level::Remove);
    }
    #[test]
    fn write_true_read_true_guid() {
        guid(true, true, Level::Guid);
    }
}

const SHORT: &[(&str, FieldKind)] = &[("a", FieldKind::Integer)];
const LONG: &[(&str, FieldKind)] = &[("a", FieldKind::Integer), ("b", FieldKind::Float)];
const SHORT_CODECS: &[Codec] = &[generated_record!("A"; a: integer)];
const LONG_CODECS: &[Codec] = &[generated_record!("A"; a: integer, b: float)];

fn field_change(write: bool, read: bool, level: Level, add: bool) {
    let (source_fields, target_fields, values, codecs) = if add {
        (SHORT, LONG, vec![Value::Integer(0)], SHORT_CODECS)
    } else {
        (
            LONG,
            SHORT,
            vec![Value::Integer(0), Value::Float(0.0)],
            LONG_CODECS,
        )
    };
    let source = TypeStamp {
        class: "A",
        module: "Dynamic",
        revision: 1,
        fields: source_fields,
    };
    let target = TypeStamp {
        revision: 2,
        fields: target_fields,
        ..source
    };
    let policy = VersionPolicy {
        // The upstream helper grants GUID changes independently of this test parameter.
        allow_module_revision_change: true,
        allow_field_addition: matches!(level, Level::Add),
        allow_field_removal: matches!(level, Level::Remove),
        ..VersionPolicy::default()
    };
    let result = round_trip(source, target, values, write, read, codecs, policy, None);
    let expected = if add {
        policy.allow_field_addition
    } else {
        policy.allow_field_removal
    };
    if expected {
        let expected = if add {
            vec![Value::Integer(0), Value::Float(0.0)]
        } else {
            vec![Value::Integer(0)]
        };
        assert_eq!(result.as_ref().unwrap(), &expected);
    }
    assert_tolerance(result, expected);
}

fn addition(write: bool, read: bool, level: Level) {
    field_change(write, read, level, true);
}

fn removal(write: bool, read: bool, level: Level) {
    field_change(write, read, level, false);
}

mod field_addition {
    use super::*;
    #[test]
    fn write_false_read_false_exact() {
        addition(false, false, Level::Exact);
    }
    #[test]
    fn write_false_read_false_inheritance() {
        addition(false, false, Level::Inheritance);
    }
    #[test]
    fn write_false_read_false_add() {
        addition(false, false, Level::Add);
    }
    #[test]
    fn write_false_read_false_remove() {
        addition(false, false, Level::Remove);
    }
    #[test]
    fn write_false_read_false_guid() {
        addition(false, false, Level::Guid);
    }
    #[test]
    fn write_false_read_true_exact() {
        addition(false, true, Level::Exact);
    }
    #[test]
    fn write_false_read_true_inheritance() {
        addition(false, true, Level::Inheritance);
    }
    #[test]
    fn write_false_read_true_add() {
        addition(false, true, Level::Add);
    }
    #[test]
    fn write_false_read_true_remove() {
        addition(false, true, Level::Remove);
    }
    #[test]
    fn write_false_read_true_guid() {
        addition(false, true, Level::Guid);
    }
    #[test]
    fn write_true_read_false_exact() {
        addition(true, false, Level::Exact);
    }
    #[test]
    fn write_true_read_false_inheritance() {
        addition(true, false, Level::Inheritance);
    }
    #[test]
    fn write_true_read_false_add() {
        addition(true, false, Level::Add);
    }
    #[test]
    fn write_true_read_false_remove() {
        addition(true, false, Level::Remove);
    }
    #[test]
    fn write_true_read_false_guid() {
        addition(true, false, Level::Guid);
    }
    #[test]
    fn write_true_read_true_exact() {
        addition(true, true, Level::Exact);
    }
    #[test]
    fn write_true_read_true_inheritance() {
        addition(true, true, Level::Inheritance);
    }
    #[test]
    fn write_true_read_true_add() {
        addition(true, true, Level::Add);
    }
    #[test]
    fn write_true_read_true_remove() {
        addition(true, true, Level::Remove);
    }
    #[test]
    fn write_true_read_true_guid() {
        addition(true, true, Level::Guid);
    }
}

mod field_removal {
    use super::*;
    #[test]
    fn write_false_read_false_exact() {
        removal(false, false, Level::Exact);
    }
    #[test]
    fn write_false_read_false_inheritance() {
        removal(false, false, Level::Inheritance);
    }
    #[test]
    fn write_false_read_false_add() {
        removal(false, false, Level::Add);
    }
    #[test]
    fn write_false_read_false_remove() {
        removal(false, false, Level::Remove);
    }
    #[test]
    fn write_false_read_false_guid() {
        removal(false, false, Level::Guid);
    }
    #[test]
    fn write_false_read_true_exact() {
        removal(false, true, Level::Exact);
    }
    #[test]
    fn write_false_read_true_inheritance() {
        removal(false, true, Level::Inheritance);
    }
    #[test]
    fn write_false_read_true_add() {
        removal(false, true, Level::Add);
    }
    #[test]
    fn write_false_read_true_remove() {
        removal(false, true, Level::Remove);
    }
    #[test]
    fn write_false_read_true_guid() {
        removal(false, true, Level::Guid);
    }
    #[test]
    fn write_true_read_false_exact() {
        removal(true, false, Level::Exact);
    }
    #[test]
    fn write_true_read_false_inheritance() {
        removal(true, false, Level::Inheritance);
    }
    #[test]
    fn write_true_read_false_add() {
        removal(true, false, Level::Add);
    }
    #[test]
    fn write_true_read_false_remove() {
        removal(true, false, Level::Remove);
    }
    #[test]
    fn write_true_read_false_guid() {
        removal(true, false, Level::Guid);
    }
    #[test]
    fn write_true_read_true_exact() {
        removal(true, true, Level::Exact);
    }
    #[test]
    fn write_true_read_true_inheritance() {
        removal(true, true, Level::Inheritance);
    }
    #[test]
    fn write_true_read_true_add() {
        removal(true, true, Level::Add);
    }
    #[test]
    fn write_true_read_true_remove() {
        removal(true, true, Level::Remove);
    }
    #[test]
    fn write_true_read_true_guid() {
        removal(true, true, Level::Guid);
    }
}

const EMPTY_CODECS: &[Codec] = &[generated_record!("A";)];

fn hierarchy_change(
    write: bool,
    read: bool,
    level: Level,
    source_bases: &[&str],
    target_bases: &[&str],
) {
    let source = TypeStamp {
        class: "A",
        module: "Dynamic",
        revision: 1,
        fields: &[],
    };
    let target = TypeStamp {
        revision: 2,
        ..source
    };
    let policy = VersionPolicy {
        allow_module_revision_change: true,
        allow_inheritance_chain_change: matches!(level, Level::Inheritance),
        allow_field_addition: matches!(level, Level::Add),
        allow_field_removal: matches!(level, Level::Remove),
    };
    let result = round_trip(
        source,
        target,
        vec![],
        write,
        read,
        EMPTY_CODECS,
        policy,
        Some((source_bases, target_bases)),
    );
    assert_tolerance(result, policy.allow_inheritance_chain_change);
}

fn base_insertion(write: bool, read: bool, level: Level) {
    hierarchy_change(write, read, level, &[], &["BaseA"]);
}
fn base_removal(write: bool, read: bool, level: Level) {
    hierarchy_change(write, read, level, &["BaseA"], &[]);
}
fn base_renaming(write: bool, read: bool, level: Level) {
    hierarchy_change(write, read, level, &["BaseA"], &["NewBaseA"]);
}

mod base_insertion {
    use super::*;
    #[test]
    fn write_false_read_false_exact() {
        base_insertion(false, false, Level::Exact);
    }
    #[test]
    fn write_false_read_false_inheritance() {
        base_insertion(false, false, Level::Inheritance);
    }
    #[test]
    fn write_false_read_false_add() {
        base_insertion(false, false, Level::Add);
    }
    #[test]
    fn write_false_read_false_remove() {
        base_insertion(false, false, Level::Remove);
    }
    #[test]
    fn write_false_read_false_guid() {
        base_insertion(false, false, Level::Guid);
    }
    #[test]
    fn write_false_read_true_exact() {
        base_insertion(false, true, Level::Exact);
    }
    #[test]
    fn write_false_read_true_inheritance() {
        base_insertion(false, true, Level::Inheritance);
    }
    #[test]
    fn write_false_read_true_add() {
        base_insertion(false, true, Level::Add);
    }
    #[test]
    fn write_false_read_true_remove() {
        base_insertion(false, true, Level::Remove);
    }
    #[test]
    fn write_false_read_true_guid() {
        base_insertion(false, true, Level::Guid);
    }
    #[test]
    fn write_true_read_false_exact() {
        base_insertion(true, false, Level::Exact);
    }
    #[test]
    fn write_true_read_false_inheritance() {
        base_insertion(true, false, Level::Inheritance);
    }
    #[test]
    fn write_true_read_false_add() {
        base_insertion(true, false, Level::Add);
    }
    #[test]
    fn write_true_read_false_remove() {
        base_insertion(true, false, Level::Remove);
    }
    #[test]
    fn write_true_read_false_guid() {
        base_insertion(true, false, Level::Guid);
    }
    #[test]
    fn write_true_read_true_exact() {
        base_insertion(true, true, Level::Exact);
    }
    #[test]
    fn write_true_read_true_inheritance() {
        base_insertion(true, true, Level::Inheritance);
    }
    #[test]
    fn write_true_read_true_add() {
        base_insertion(true, true, Level::Add);
    }
    #[test]
    fn write_true_read_true_remove() {
        base_insertion(true, true, Level::Remove);
    }
    #[test]
    fn write_true_read_true_guid() {
        base_insertion(true, true, Level::Guid);
    }
}

mod base_removal {
    use super::*;
    #[test]
    fn write_false_read_false_exact() {
        base_removal(false, false, Level::Exact);
    }
    #[test]
    fn write_false_read_false_inheritance() {
        base_removal(false, false, Level::Inheritance);
    }
    #[test]
    fn write_false_read_false_add() {
        base_removal(false, false, Level::Add);
    }
    #[test]
    fn write_false_read_false_remove() {
        base_removal(false, false, Level::Remove);
    }
    #[test]
    fn write_false_read_false_guid() {
        base_removal(false, false, Level::Guid);
    }
    #[test]
    fn write_false_read_true_exact() {
        base_removal(false, true, Level::Exact);
    }
    #[test]
    fn write_false_read_true_inheritance() {
        base_removal(false, true, Level::Inheritance);
    }
    #[test]
    fn write_false_read_true_add() {
        base_removal(false, true, Level::Add);
    }
    #[test]
    fn write_false_read_true_remove() {
        base_removal(false, true, Level::Remove);
    }
    #[test]
    fn write_false_read_true_guid() {
        base_removal(false, true, Level::Guid);
    }
    #[test]
    fn write_true_read_false_exact() {
        base_removal(true, false, Level::Exact);
    }
    #[test]
    fn write_true_read_false_inheritance() {
        base_removal(true, false, Level::Inheritance);
    }
    #[test]
    fn write_true_read_false_add() {
        base_removal(true, false, Level::Add);
    }
    #[test]
    fn write_true_read_false_remove() {
        base_removal(true, false, Level::Remove);
    }
    #[test]
    fn write_true_read_false_guid() {
        base_removal(true, false, Level::Guid);
    }
    #[test]
    fn write_true_read_true_exact() {
        base_removal(true, true, Level::Exact);
    }
    #[test]
    fn write_true_read_true_inheritance() {
        base_removal(true, true, Level::Inheritance);
    }
    #[test]
    fn write_true_read_true_add() {
        base_removal(true, true, Level::Add);
    }
    #[test]
    fn write_true_read_true_remove() {
        base_removal(true, true, Level::Remove);
    }
    #[test]
    fn write_true_read_true_guid() {
        base_removal(true, true, Level::Guid);
    }
}

mod base_renaming {
    use super::*;
    #[test]
    fn write_false_read_false_exact() {
        base_renaming(false, false, Level::Exact);
    }
    #[test]
    fn write_false_read_false_inheritance() {
        base_renaming(false, false, Level::Inheritance);
    }
    #[test]
    fn write_false_read_false_add() {
        base_renaming(false, false, Level::Add);
    }
    #[test]
    fn write_false_read_false_remove() {
        base_renaming(false, false, Level::Remove);
    }
    #[test]
    fn write_false_read_false_guid() {
        base_renaming(false, false, Level::Guid);
    }
    #[test]
    fn write_false_read_true_exact() {
        base_renaming(false, true, Level::Exact);
    }
    #[test]
    fn write_false_read_true_inheritance() {
        base_renaming(false, true, Level::Inheritance);
    }
    #[test]
    fn write_false_read_true_add() {
        base_renaming(false, true, Level::Add);
    }
    #[test]
    fn write_false_read_true_remove() {
        base_renaming(false, true, Level::Remove);
    }
    #[test]
    fn write_false_read_true_guid() {
        base_renaming(false, true, Level::Guid);
    }
    #[test]
    fn write_true_read_false_exact() {
        base_renaming(true, false, Level::Exact);
    }
    #[test]
    fn write_true_read_false_inheritance() {
        base_renaming(true, false, Level::Inheritance);
    }
    #[test]
    fn write_true_read_false_add() {
        base_renaming(true, false, Level::Add);
    }
    #[test]
    fn write_true_read_false_remove() {
        base_renaming(true, false, Level::Remove);
    }
    #[test]
    fn write_true_read_false_guid() {
        base_renaming(true, false, Level::Guid);
    }
    #[test]
    fn write_true_read_true_exact() {
        base_renaming(true, true, Level::Exact);
    }
    #[test]
    fn write_true_read_true_inheritance() {
        base_renaming(true, true, Level::Inheritance);
    }
    #[test]
    fn write_true_read_true_add() {
        base_renaming(true, true, Level::Add);
    }
    #[test]
    fn write_true_read_true_remove() {
        base_renaming(true, true, Level::Remove);
    }
    #[test]
    fn write_true_read_true_guid() {
        base_renaming(true, true, Level::Guid);
    }
}

fn moved_field(write: bool, read: bool, level: Level) {
    // Names are qualified by their declaring class so these represent
    // different fields even though their simple names and value types coincide.
    const BEFORE: &[(&str, FieldKind)] = &[
        ("BaseA:a", FieldKind::Reference),
        ("A:b", FieldKind::Integer),
    ];
    const AFTER: &[(&str, FieldKind)] =
        &[("A:a", FieldKind::Reference), ("A:b", FieldKind::Integer)];
    const CODECS: &[Codec] = &[generated_record!("A"; a: reference, b: integer)];
    let source = TypeStamp {
        class: "A",
        module: "Dynamic",
        revision: 1,
        fields: BEFORE,
    };
    let target = TypeStamp {
        revision: 2,
        fields: AFTER,
        ..source
    };
    let policy = VersionPolicy {
        allow_module_revision_change: true,
        allow_inheritance_chain_change: matches!(level, Level::Inheritance),
        allow_field_addition: matches!(level, Level::Add | Level::AddAndRemove),
        allow_field_removal: matches!(level, Level::Remove | Level::AddAndRemove),
    };
    let result = round_trip(
        source,
        target,
        vec![Value::Null, Value::Integer(0)],
        write,
        read,
        CODECS,
        policy,
        Some((&["BaseA"], &["BaseA"])),
    );
    assert_tolerance(
        result,
        policy.allow_field_addition && policy.allow_field_removal,
    );
}

mod moved_fields {
    use super::*;
    #[test]
    fn write_false_read_false_addandremove() {
        moved_field(false, false, Level::AddAndRemove);
    }
    #[test]
    fn write_false_read_false_inheritance() {
        moved_field(false, false, Level::Inheritance);
    }
    #[test]
    fn write_false_read_false_add() {
        moved_field(false, false, Level::Add);
    }
    #[test]
    fn write_false_read_false_remove() {
        moved_field(false, false, Level::Remove);
    }
    #[test]
    fn write_false_read_false_guid() {
        moved_field(false, false, Level::Guid);
    }
    #[test]
    fn write_false_read_true_addandremove() {
        moved_field(false, true, Level::AddAndRemove);
    }
    #[test]
    fn write_false_read_true_inheritance() {
        moved_field(false, true, Level::Inheritance);
    }
    #[test]
    fn write_false_read_true_add() {
        moved_field(false, true, Level::Add);
    }
    #[test]
    fn write_false_read_true_remove() {
        moved_field(false, true, Level::Remove);
    }
    #[test]
    fn write_false_read_true_guid() {
        moved_field(false, true, Level::Guid);
    }
    #[test]
    fn write_true_read_false_addandremove() {
        moved_field(true, false, Level::AddAndRemove);
    }
    #[test]
    fn write_true_read_false_inheritance() {
        moved_field(true, false, Level::Inheritance);
    }
    #[test]
    fn write_true_read_false_add() {
        moved_field(true, false, Level::Add);
    }
    #[test]
    fn write_true_read_false_remove() {
        moved_field(true, false, Level::Remove);
    }
    #[test]
    fn write_true_read_false_guid() {
        moved_field(true, false, Level::Guid);
    }
    #[test]
    fn write_true_read_true_addandremove() {
        moved_field(true, true, Level::AddAndRemove);
    }
    #[test]
    fn write_true_read_true_inheritance() {
        moved_field(true, true, Level::Inheritance);
    }
    #[test]
    fn write_true_read_true_add() {
        moved_field(true, true, Level::Add);
    }
    #[test]
    fn write_true_read_true_remove() {
        moved_field(true, true, Level::Remove);
    }
    #[test]
    fn write_true_read_true_guid() {
        moved_field(true, true, Level::Guid);
    }
}

fn check_declared_stamp(write: bool, read: bool, pattern: &str) {
    use crate::migrant::declared_types::DeclaredFieldTypes;

    const CLASS: &str = "ClassWithGenerics";
    const STAMPS: &[TypeStamp<'static>] = &[TypeStamp {
        class: CLASS,
        module: "Migrant.Tests",
        revision: 0,
        fields: &[
            ("IntegerNumber", FieldKind::Integer),
            ("SecondIntegerNumber", FieldKind::Integer),
            ("ListOfIntegerNumbers", FieldKind::Reference),
            ("FloatNumber", FieldKind::Float),
            ("ListOfFloatNumbers", FieldKind::Reference),
        ],
    }];
    const FIELDS: &[DeclaredFieldTypes<'static>] = &[DeclaredFieldTypes {
        class: CLASS,
        fields: &[
            &["System.Int32"],
            &["System.Int32"],
            &["System.Collections.Generic.List`1", "System.Int32"],
            &["System.Single"],
            &["System.Collections.Generic.List`1", "System.Single"],
        ],
    }];
    const CODECS: &[Codec] = &[generated_record!(CLASS;
        first: integer, second: integer, integers: reference, number: float, floats: reference
    )];
    let mut graph = Graph::default();
    let original = Object::Record {
        class: CLASS.into(),
        fields: vec![
            Value::Integer(0),
            Value::Integer(0),
            Value::Null,
            Value::Float(0.0),
            Value::Null,
        ],
    };
    let root = graph.insert(original.clone());
    let mut stream = Vec::new();
    let mut writer = ObjectWriter::new(&mut stream, &graph, References::PerRoot, true)
        .with_declared_field_types(STAMPS, FIELDS);
    if write {
        writer = writer.with_generated_records(CODECS);
    }
    writer.write_object(root).unwrap();
    writer.finish().unwrap();
    let printable: String = stream
        .iter()
        .filter(|byte| (b' '..=b'~').contains(byte))
        .map(|&byte| char::from(byte))
        .collect();
    assert_eq!(printable.match_indices(pattern).count(), 1);

    let mut reader = ObjectReader::new(stream.as_slice(), References::PerRoot, true)
        .with_declared_field_types(STAMPS, FIELDS);
    if read {
        reader = reader.with_generated_records(CODECS);
    }
    let restored = reader.read_object().unwrap();
    assert_eq!(reader.graph().get(restored), Some(&original));
    reader.finish().unwrap();
}

mod declared_types {
    use super::*;
    #[test]
    fn primitive_write_false_read_false() {
        check_declared_stamp(false, false, "System.Int32");
    }
    #[test]
    fn generic_write_false_read_false() {
        check_declared_stamp(false, false, "System.Collections.Generic.List`1");
    }
    #[test]
    fn primitive_write_false_read_true() {
        check_declared_stamp(false, true, "System.Int32");
    }
    #[test]
    fn generic_write_false_read_true() {
        check_declared_stamp(false, true, "System.Collections.Generic.List`1");
    }
    #[test]
    fn primitive_write_true_read_false() {
        check_declared_stamp(true, false, "System.Int32");
    }
    #[test]
    fn generic_write_true_read_false() {
        check_declared_stamp(true, false, "System.Collections.Generic.List`1");
    }
    #[test]
    fn primitive_write_true_read_true() {
        check_declared_stamp(true, true, "System.Int32");
    }
    #[test]
    fn generic_write_true_read_true() {
        check_declared_stamp(true, true, "System.Collections.Generic.List`1");
    }
}
