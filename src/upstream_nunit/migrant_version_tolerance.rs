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
