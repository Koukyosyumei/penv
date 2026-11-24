use crate::migrant::{
    generated_record::{Codec, generated_record},
    object_stream::{
        FieldKind, Graph, Object, ObjectReader, ObjectWriter, References, TypeStamp, Value,
        VersionPolicy, VersionToleranceError,
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
    if generated_write {
        writer = writer.with_generated_records(codecs);
    }
    writer.write_object(root).unwrap();
    writer.finish().unwrap();
    let mut reader = ObjectReader::new(stream.as_slice(), References::PerRoot, true)
        .with_type_stamps(&target)
        .with_version_policy(policy);
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
    };
    let result = round_trip(source, target, values, write, read, codecs, policy);
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
