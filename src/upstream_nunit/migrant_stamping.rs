use crate::migrant::generated_record::{generated_record, Codec};
use crate::migrant::object_stream::{
    FieldKind, Graph, Object, ObjectReader, ObjectWriter, References, TypeStamp, Value,
};
use std::io::{Cursor, Seek, SeekFrom};
use std::time::{SystemTime, UNIX_EPOCH};

const CLASS: &str = "Antmicro.Migrant.Tests.StampingTests+TestClass";
const STAMPS: [TypeStamp<'static>; 1] = [TypeStamp {
    class: CLASS,
    module: "Tests",
    revision: 0,
    fields: &[
        ("number", FieldKind::Integer),
        ("obj", FieldKind::Reference),
    ],
}];

const GENERATED: &[Codec] = &[generated_record!(CLASS; number: integer, obj: reference)];

fn write_self_cycle(stamped: bool, generated: bool) -> (Cursor<Vec<u8>>, i64) {
    let number = i64::from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .subsec_millis(),
    );
    let mut graph = Graph::default();
    let root = graph.insert(Object::Record {
        class: CLASS.into(),
        fields: vec![Value::Integer(number), Value::Null],
    });
    let Object::Record { fields, .. } = graph.get_mut(root).unwrap() else {
        unreachable!()
    };
    fields[1] = Value::Ref(root);
    let mut stream = Cursor::new(Vec::new());
    let mut writer = ObjectWriter::new(&mut stream, &graph, References::Preserve, true);
    if stamped {
        writer = writer.with_type_stamps(&STAMPS);
    }
    if generated {
        writer = writer.with_generated_records(GENERATED);
    }
    writer.write_object(root).unwrap();
    writer.finish().unwrap();
    stream.seek(SeekFrom::Start(0)).unwrap();
    (stream, number)
}

fn check_config(
    write_generated: bool,
    read_generated: bool,
    serialize_with_stamps: bool,
    deserialize_with_stamps: bool,
) {
    let (stream, number) = write_self_cycle(serialize_with_stamps, write_generated);
    let mut reader = ObjectReader::new(stream, References::Preserve, true);
    if deserialize_with_stamps {
        reader = reader.with_type_stamps(&STAMPS);
    }
    if read_generated {
        reader = reader.with_generated_records(GENERATED);
    }
    let result = reader.read_object();
    if serialize_with_stamps == deserialize_with_stamps {
        let root = result.expect("Should deserialize when stream configuration is correct.");
        assert_eq!(
            reader.graph().get(root),
            Some(&Object::Record {
                class: CLASS.into(),
                fields: vec![Value::Integer(number), Value::Ref(root)],
            })
        );
        reader.finish().unwrap();
    } else {
        assert_eq!(
            result.unwrap_err().kind(),
            std::io::ErrorKind::InvalidData,
            "Should not deserialize when stream configuration is inconsistent."
        );
    }
}

#[test]
fn stamped_to_stamped() {
    check_config(false, false, true, true);
}

#[test]
fn stamped_to_unstamped() {
    check_config(false, false, true, false);
}

#[test]
fn unstamped_to_stamped() {
    check_config(false, false, false, true);
}

#[test]
fn unstamped_to_unstamped() {
    check_config(false, false, false, false);
}

#[test]
fn rejects_a_different_field_stamp_before_restoring_the_cycle() {
    let (stream, _) = write_self_cycle(true, false);
    let incompatible = [TypeStamp {
        fields: &[
            ("renamed", FieldKind::Integer),
            ("obj", FieldKind::Reference),
        ],
        ..STAMPS[0]
    }];
    let mut reader =
        ObjectReader::new(stream, References::Preserve, true).with_type_stamps(&incompatible);
    assert_eq!(
        reader.read_object().unwrap_err().kind(),
        std::io::ErrorKind::InvalidData
    );
}

#[test]
fn reflection_generated_stamped_to_stamped() {
    check_config(false, true, true, true);
}

#[test]
fn reflection_generated_stamped_to_unstamped() {
    check_config(false, true, true, false);
}

#[test]
fn reflection_generated_unstamped_to_stamped() {
    check_config(false, true, false, true);
}

#[test]
fn reflection_generated_unstamped_to_unstamped() {
    check_config(false, true, false, false);
}

#[test]
fn generated_reflection_stamped_to_stamped() {
    check_config(true, false, true, true);
}

#[test]
fn generated_reflection_stamped_to_unstamped() {
    check_config(true, false, true, false);
}

#[test]
fn generated_reflection_unstamped_to_stamped() {
    check_config(true, false, false, true);
}

#[test]
fn generated_reflection_unstamped_to_unstamped() {
    check_config(true, false, false, false);
}

#[test]
fn generated_generated_stamped_to_stamped() {
    check_config(true, true, true, true);
}

#[test]
fn generated_generated_stamped_to_unstamped() {
    check_config(true, true, true, false);
}

#[test]
fn generated_generated_unstamped_to_stamped() {
    check_config(true, true, false, true);
}

#[test]
fn generated_generated_unstamped_to_unstamped() {
    check_config(true, true, false, false);
}
