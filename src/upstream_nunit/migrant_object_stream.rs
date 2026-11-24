use crate::migrant::object_stream::{
    FieldKind, Graph, Object, ObjectId, ObjectReader, ObjectWriter, References, TypeStamp, Value,
};
use std::io::{Cursor, Seek, SeekFrom};

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
        "SimpleClass",
        &[
            ("<Value>k__BackingField", FieldKind::Integer),
            ("<Str>k__BackingField", FieldKind::Reference),
        ],
    ),
    stamp(
        "SimpleContainer",
        &[
            ("<First>k__BackingField", FieldKind::Reference),
            ("<Second>k__BackingField", FieldKind::Reference),
        ],
    ),
    stamp(
        "CyclicRef",
        &[
            ("<Another>k__BackingField", FieldKind::Reference),
            ("<Val>k__BackingField", FieldKind::Integer),
        ],
    ),
    stamp(
        "ClassWithReadonly",
        &[("privateReadonlyField", FieldKind::Integer)],
    ),
    stamp(
        "ClassWithReferenceToTheObjectOfTheSameType",
        &[("Reference", FieldKind::Reference)],
    ),
    stamp(
        "Tuple<int,int>",
        &[
            ("m_Item1", FieldKind::Integer),
            ("m_Item2", FieldKind::Integer),
        ],
    ),
    stamp(
        "Tuple<int,double>",
        &[
            ("m_Item1", FieldKind::Integer),
            ("m_Item2", FieldKind::Float),
        ],
    ),
];

fn record(class: &str, fields: Vec<Value>) -> Object {
    Object::Record {
        class: class.into(),
        fields,
    }
}

fn string(value: &str) -> Object {
    Object::String(value.into())
}

fn fields(object: &Object) -> &[Value] {
    let Object::Record { fields, .. } = object else {
        panic!("expected an object record")
    };
    fields
}

fn reference(value: &Value) -> ObjectId {
    let Value::Ref(id) = value else {
        panic!("expected an object reference")
    };
    *id
}

fn deep_clone(
    graph: &Graph,
    root: ObjectId,
    stamped: bool,
) -> (ObjectReader<'static, 'static, Cursor<Vec<u8>>>, ObjectId) {
    let mut stream = Cursor::new(Vec::new());
    let mut writer = ObjectWriter::new(&mut stream, graph, References::Preserve, true);
    if stamped {
        writer = writer.with_type_stamps(STAMPS);
    }
    writer.write_object(root).unwrap();
    writer.finish().unwrap();
    stream.seek(SeekFrom::Start(0)).unwrap();
    let mut reader = ObjectReader::new(stream, References::Preserve, true);
    if stamped {
        reader = reader.with_type_stamps(STAMPS);
    }
    let root = reader.read_object().unwrap();
    (reader, root)
}

#[test]
fn should_serialize_object() {
    should_serialize_object_with_stamping(false);
}

#[test]
fn should_serialize_object_stamped() {
    should_serialize_object_with_stamping(true);
}

fn should_serialize_object_with_stamping(stamped: bool) {
    let mut graph = Graph::default();
    let object = graph.insert(record("System.Object", vec![]));
    let (reader, copy) = deep_clone(&graph, object, stamped);
    assert_eq!(
        reader.graph().get(copy),
        Some(&record("System.Object", vec![]))
    );
}

#[test]
fn should_serialize_simple_class() {
    should_serialize_simple_class_with_stamping(false);
}

#[test]
fn should_serialize_simple_class_stamped() {
    should_serialize_simple_class_with_stamping(true);
}

fn should_serialize_simple_class_with_stamping(stamped: bool) {
    let mut graph = Graph::default();
    let text = graph.insert(string("Magic"));
    let object = graph.insert(record(
        "SimpleClass",
        vec![Value::Integer(0x69), Value::Ref(text)],
    ));
    let (reader, copy) = deep_clone(&graph, object, stamped);
    let restored = reader.graph();
    let copy = restored.get(copy).unwrap();
    let (class, original) = (fields(copy), fields(graph.get(object).unwrap()));
    assert_eq!(
        copy,
        &record("SimpleClass", vec![original[0].clone(), class[1].clone()])
    );
    assert_eq!(
        restored.get(reference(&class[1])),
        graph.get(reference(&original[1]))
    );
}

fn cyclic_record(another: Value, val: i64) -> Object {
    record("CyclicRef", vec![another, Value::Integer(val)])
}

#[test]
fn should_serialize_cyclic_reference() {
    should_serialize_cyclic_reference_with_stamping(false);
}

#[test]
fn should_serialize_cyclic_reference_stamped() {
    should_serialize_cyclic_reference_with_stamping(true);
}

fn should_serialize_cyclic_reference_with_stamping(stamped: bool) {
    let mut graph = Graph::default();
    let one = graph.insert(cyclic_record(Value::Null, 0));
    let another = graph.insert(cyclic_record(Value::Ref(one), 1));
    *graph.get_mut(one).unwrap() = cyclic_record(Value::Ref(another), 0);
    let (reader, one_copy) = deep_clone(&graph, one, stamped);
    let graph = reader.graph();
    let another = reference(&fields(graph.get(one_copy).unwrap())[0]);
    assert_eq!(
        one_copy,
        reference(&fields(graph.get(another).unwrap())[0]),
        "Cyclic reference was not properly serialized."
    );
}

#[test]
fn should_serialize_reference_to_self() {
    should_serialize_reference_to_self_with_stamping(false);
}

#[test]
fn should_serialize_reference_to_self_stamped() {
    should_serialize_reference_to_self_with_stamping(true);
}

fn should_serialize_reference_to_self_with_stamping(stamped: bool) {
    let mut graph = Graph::default();
    let one = graph.insert(cyclic_record(Value::Null, 0));
    *graph.get_mut(one).unwrap() = cyclic_record(Value::Ref(one), 0);
    let (reader, copy) = deep_clone(&graph, one, stamped);
    assert_eq!(
        copy,
        reference(&fields(reader.graph().get(copy).unwrap())[0])
    );
}

#[test]
fn should_serialize_array_reference_to_self() {
    should_serialize_array_reference_to_self_with_stamping(false);
}

#[test]
fn should_serialize_array_reference_to_self_stamped() {
    should_serialize_array_reference_to_self_with_stamping(true);
}

fn should_serialize_array_reference_to_self_with_stamping(stamped: bool) {
    let mut graph = Graph::default();
    let array = graph.insert(Object::Array(vec![Value::Null; 2]));
    *graph.get_mut(array).unwrap() = Object::Array(vec![Value::Ref(array); 2]);
    let (reader, copy) = deep_clone(&graph, array, stamped);
    let Object::Array(elements) = reader.graph().get(copy).unwrap() else {
        panic!("expected array")
    };
    assert_eq!(copy, reference(&elements[0]));
    assert_eq!(reference(&elements[0]), reference(&elements[1]));
}

#[test]
fn should_serialize_null_reference() {
    should_serialize_null_reference_with_stamping(false);
}

#[test]
fn should_serialize_null_reference_stamped() {
    should_serialize_null_reference_with_stamping(true);
}

fn should_serialize_null_reference_with_stamping(stamped: bool) {
    let mut graph = Graph::default();
    let object = graph.insert(record("SimpleClass", vec![Value::Integer(0), Value::Null]));
    let (reader, copy) = deep_clone(&graph, object, stamped);
    assert_eq!(graph.get(object), reader.graph().get(copy));
}

#[test]
fn should_preserve_identity() {
    should_preserve_identity_with_stamping(false);
}

#[test]
fn should_preserve_identity_stamped() {
    should_preserve_identity_with_stamping(true);
}

fn should_preserve_identity_with_stamping(stamped: bool) {
    let mut graph = Graph::default();
    let text = graph.insert(string("This is a string"));
    let first = graph.insert(record(
        "SimpleClass",
        vec![Value::Integer(0), Value::Ref(text)],
    ));
    let second = graph.insert(record(
        "SimpleClass",
        vec![Value::Integer(0), Value::Ref(text)],
    ));
    let container = graph.insert(record(
        "SimpleContainer",
        vec![Value::Ref(first), Value::Ref(second)],
    ));
    let (reader, copy) = deep_clone(&graph, container, stamped);
    let graph = reader.graph();
    let container = fields(graph.get(copy).unwrap());
    let first = fields(graph.get(reference(&container[0])).unwrap());
    let second = fields(graph.get(reference(&container[1])).unwrap());
    assert_eq!(reference(&first[1]), reference(&second[1]));
}

#[test]
fn should_serialize_readonly_field() {
    should_serialize_readonly_field_with_stamping(false);
}

#[test]
fn should_serialize_readonly_field_stamped() {
    should_serialize_readonly_field_with_stamping(true);
}

fn should_serialize_readonly_field_with_stamping(stamped: bool) {
    let mut graph = Graph::default();
    let object = graph.insert(record("ClassWithReadonly", vec![Value::Integer(0xabcd)]));
    let (reader, copy) = deep_clone(&graph, object, stamped);
    assert_eq!(
        fields(reader.graph().get(copy).unwrap())[0],
        Value::Integer(0xabcd)
    );
}

#[test]
fn should_serialize_deep_graph() {
    should_serialize_deep_graph_with_stamping(false);
}

#[test]
fn should_serialize_deep_graph_stamped() {
    should_serialize_deep_graph_with_stamping(true);
}

fn should_serialize_deep_graph_with_stamping(stamped: bool) {
    let mut graph = Graph::default();
    let class = "ClassWithReferenceToTheObjectOfTheSameType";
    let root = graph.insert(record(class, vec![Value::Null]));
    let mut current = root;
    for _ in 0..45_000 {
        let next = graph.insert(record(class, vec![Value::Null]));
        *graph.get_mut(current).unwrap() = record(class, vec![Value::Ref(next)]);
        current = next;
    }
    let (reader, mut copy) = deep_clone(&graph, root, stamped);
    assert!(reader.graph().get(copy).is_some());
    let mut depth = 0;
    loop {
        let field = &fields(reader.graph().get(copy).unwrap())[0];
        if *field == Value::Null {
            break;
        }
        copy = reference(field);
        depth += 1;
    }
    assert_eq!(depth, 45_000);
    reader.finish().unwrap();
}

#[test]
fn should_handle_two_writes_and_reads() {
    let strings = ["One", "Two"];
    let mut graph = Graph::default();
    let ids = strings.map(|value| graph.insert(string(value)));
    let mut stream = Cursor::new(Vec::new());
    let mut writer =
        ObjectWriter::new(&mut stream, &graph, References::Preserve, true).with_type_stamps(STAMPS);
    for id in ids {
        writer.write_object(id).unwrap();
    }
    writer.finish().unwrap();
    let position = stream.position();
    stream.seek(SeekFrom::Start(0)).unwrap();
    let mut reader =
        ObjectReader::new(&mut stream, References::Preserve, true).with_type_stamps(STAMPS);
    for expected in strings {
        let id = reader.read_object().unwrap();
        assert_eq!(reader.graph().get(id), Some(&string(expected)));
    }
    reader.finish().unwrap();
    assert_eq!(position, stream.position());
}

fn open_stream(
    graph: &Graph,
    roots: &[ObjectId],
    references: References,
    buffered: bool,
) -> ObjectReader<'static, 'static, Cursor<Vec<u8>>> {
    let mut stream = Cursor::new(Vec::new());
    let mut writer =
        ObjectWriter::new(&mut stream, graph, references, buffered).with_type_stamps(STAMPS);
    for &root in roots {
        writer.write_object(root).unwrap();
    }
    writer.finish().unwrap();
    stream.seek(SeekFrom::Start(0)).unwrap();
    ObjectReader::new(stream, references, buffered).with_type_stamps(STAMPS)
}

fn tuple(first: i64, second: i64) -> Object {
    record(
        "Tuple<int,int>",
        vec![Value::Integer(first), Value::Integer(second)],
    )
}

fn tuples() -> (Graph, [ObjectId; 3]) {
    let mut graph = Graph::default();
    let first = graph.insert(tuple(0, 1));
    let second = graph.insert(tuple(1, 2));
    (graph, [first, first, second])
}

fn basic_open_stream(references: References) {
    let (graph, roots) = tuples();
    let mut reader = open_stream(&graph, &roots, references, true);
    let objects = roots.map(|_| reader.read_object().unwrap());
    for (expected, actual) in roots.into_iter().zip(objects) {
        assert_eq!(graph.get(expected), reader.graph().get(actual));
    }
    reader.finish().unwrap();
}

#[test]
fn basic_open_stream_do_not_preserve() {
    basic_open_stream(References::PerRoot);
}

#[test]
fn basic_open_stream_preserve() {
    basic_open_stream(References::Preserve);
}

#[test]
fn should_preserve_references() {
    let (graph, roots) = tuples();
    let mut reader = open_stream(&graph, &roots, References::Preserve, true);
    let [first, duplicate, second] = roots.map(|_| reader.read_object().unwrap());
    assert_eq!(first, duplicate);
    assert_ne!(duplicate, second);
    reader.finish().unwrap();
}

#[test]
fn shouldnt_preserve_references() {
    let mut graph = Graph::default();
    let object = graph.insert(record("System.Object", vec![]));
    let mut reader = open_stream(&graph, &[object, object], References::PerRoot, true);
    assert_ne!(reader.read_object().unwrap(), reader.read_object().unwrap());
    reader.finish().unwrap();
}

#[test]
fn should_deserialize_many() {
    let mut graph = Graph::default();
    let roots: Vec<_> = (1..=10)
        .map(|x| {
            graph.insert(record(
                "Tuple<int,double>",
                vec![Value::Integer(x), Value::Float(x as f64 + 0.5)],
            ))
        })
        .collect();
    let mut reader = open_stream(&graph, &roots, References::Preserve, false);
    let restored = reader.read_many().collect::<Result<Vec<_>, _>>().unwrap();
    let before: Vec<_> = roots.iter().map(|&id| graph.get(id).unwrap()).collect();
    let after: Vec<_> = restored
        .iter()
        .map(|&id| reader.graph().get(id).unwrap())
        .collect();
    assert_eq!(before, after);
    reader.finish().unwrap();
}

fn array_values(graph: &Graph, root: ObjectId) -> &[Value] {
    let Some(Object::Array(values)) = graph.get(root) else {
        panic!("expected an array");
    };
    values
}

fn primitives_array(stamped: bool) {
    let mut graph = Graph::default();
    let expected = [1, 2, 3, 4, 5, 6].map(Value::Integer);
    let root = graph.insert(Object::Array(expected.to_vec()));
    let (reader, copy) = deep_clone(&graph, root, stamped);
    assert_eq!(array_values(reader.graph(), copy), expected);
}

fn strings_in_array(graph: &Graph, root: ObjectId) -> Vec<&str> {
    array_values(graph, root)
        .iter()
        .filter_map(|value| {
            let Value::Ref(id) = value else { return None };
            match graph.get(*id) {
                Some(Object::String(text)) => Some(text.as_str()),
                _ => None,
            }
        })
        .collect()
}

fn strings_array(stamped: bool) {
    let mut graph = Graph::default();
    let expected = ["One", "Two", "Three"];
    let refs = expected.map(|text| Value::Ref(graph.insert(string(text))));
    let root = graph.insert(Object::Array(refs.to_vec()));
    let (reader, copy) = deep_clone(&graph, root, stamped);
    assert_eq!(strings_in_array(reader.graph(), copy), expected);
    assert_eq!(array_values(reader.graph(), copy).len(), expected.len());
}

fn mixed_array(stamped: bool) {
    let mut graph = Graph::default();
    let object = Value::Ref(graph.insert(record("System.Object", vec![])));
    let one = Value::Ref(graph.insert(string("One")));
    let two = Value::Ref(graph.insert(string("Two")));
    let other = Value::Ref(graph.insert(record("System.Object", vec![])));
    let root = graph.insert(Object::Array(vec![one, object.clone(), two, object, other]));
    let (reader, copy) = deep_clone(&graph, root, stamped);
    assert_eq!(
        strings_in_array(&graph, root),
        strings_in_array(reader.graph(), copy)
    );
    let values = array_values(reader.graph(), copy);
    assert_eq!(reference(&values[1]), reference(&values[3]));
    assert_ne!(reference(&values[3]), reference(&values[4]));
}

macro_rules! array_modes {
    ($plain:ident, $stamped:ident, $check:ident) => {
        #[test]
        fn $plain() {
            $check(false);
        }
        #[test]
        fn $stamped() {
            $check(true);
        }
    };
}

array_modes!(
    primitives_array_plain,
    primitives_array_stamped,
    primitives_array
);
array_modes!(strings_array_plain, strings_array_stamped, strings_array);
array_modes!(mixed_array_plain, mixed_array_stamped, mixed_array);

fn weak_open_stream(
    graph: &Graph,
    roots: &[ObjectId],
) -> ObjectReader<'static, 'static, Cursor<Vec<u8>>> {
    let mut stream = Cursor::new(Vec::new());
    let mut writer = ObjectWriter::new_weak(&mut stream, true).with_type_stamps(STAMPS);
    for &root in roots {
        writer.write_weak_object(graph, root).unwrap();
    }
    writer.finish().unwrap();
    stream.seek(SeekFrom::Start(0)).unwrap();
    ObjectReader::new(stream, References::Weak, true).with_type_stamps(STAMPS)
}

#[test]
fn basic_open_stream_weak() {
    let (graph, roots) = tuples();
    let mut reader = weak_open_stream(&graph, &roots);
    let objects = roots.map(|_| reader.read_weak_object().unwrap());
    for (expected, actual) in roots.into_iter().zip(objects) {
        assert_eq!(graph.get(expected), actual.get(actual.id()));
    }
    reader.finish().unwrap();
}

#[test]
fn should_preserve_weak_references() {
    let (graph, roots) = tuples();
    let mut reader = weak_open_stream(&graph, &roots);
    let [first, duplicate, second] = roots.map(|_| reader.read_weak_object().unwrap());
    let first_obj = first.get(first.id()).unwrap();
    let duplicate_obj = duplicate.get(duplicate.id()).unwrap();
    let second_obj = second.get(second.id()).unwrap();
    assert!(std::ptr::eq(first_obj, duplicate_obj));
    assert!(!std::ptr::eq(duplicate_obj, second_obj));
    reader.finish().unwrap();
}

#[test]
fn weak_reader_releases_a_cycle_when_its_last_root_is_dropped() {
    let mut graph = Graph::default();
    let root = graph.insert(Object::Array(vec![]));
    *graph.get_mut(root).unwrap() = Object::Array(vec![Value::Ref(root)]);
    let mut reader = weak_open_stream(&graph, &[root, root, root]);
    let one = reader.read_weak_object().unwrap();
    let two = reader.read_weak_object().unwrap();
    assert_eq!(
        one.get(one.id()),
        Some(&Object::Array(vec![Value::Ref(one.id())]))
    );
    assert!(std::ptr::eq(
        one.get(one.id()).unwrap(),
        two.get(two.id()).unwrap()
    ));
    drop(one);
    drop(two);
    assert_eq!(
        reader.read_weak_object().err().unwrap().kind(),
        std::io::ErrorKind::InvalidData
    );
    reader.finish().unwrap();
}

#[test]
fn weak_writer_keeps_separate_slots_for_independent_graphs() {
    let mut stream = Cursor::new(Vec::new());
    let mut writer = ObjectWriter::new_weak(&mut stream, false);
    for value in ["first", "second"] {
        let mut graph = Graph::default();
        let root = graph.insert(Object::String(value.into()));
        writer.write_weak_object(&graph, root).unwrap();
    }
    writer.finish().unwrap();
    stream.seek(SeekFrom::Start(0)).unwrap();
    let mut reader = ObjectReader::new(stream, References::Weak, false);
    let one = reader.read_weak_object().unwrap();
    let two = reader.read_weak_object().unwrap();
    assert_ne!(one.id(), two.id());
    assert_eq!(one.get(one.id()), Some(&Object::String("first".into())));
    assert_eq!(two.get(two.id()), Some(&Object::String("second".into())));
}

fn rectangular_primitives(stamped: bool) {
    let mut graph = Graph::default();
    let array = Object::Rectangular {
        dimensions: vec![3, 2],
        elements: [1, 2, 3, 4, 5, 6].map(Value::Integer).to_vec(),
    };
    let root = graph.insert(array.clone());
    let (reader, copy) = deep_clone(&graph, root, stamped);
    assert_eq!(reader.graph().get(copy), Some(&array));
}

fn rectangular_strings(stamped: bool) {
    let mut graph = Graph::default();
    let values = ["One", "Two", "Three", "Four"];
    let dimensions = vec![2, 2];
    let elements = values.map(|text| Value::Ref(graph.insert(string(text))));
    let root = graph.insert(Object::Rectangular {
        dimensions: dimensions.clone(),
        elements: elements.to_vec(),
    });
    let (reader, copy) = deep_clone(&graph, root, stamped);
    let Some(Object::Rectangular {
        dimensions: restored,
        elements,
    }) = reader.graph().get(copy)
    else {
        panic!("expected rectangular array");
    };
    assert_eq!(*restored, dimensions);
    assert_eq!(elements.len(), values.len());
    for (element, text) in elements.iter().zip(values) {
        assert_eq!(reader.graph().get(reference(element)), Some(&string(text)));
    }
}

fn four_dimensional_array(stamped: bool) {
    let mut graph = Graph::default();
    let mut values = [[[[0; 4]; 3]; 2]; 2];
    values[0][0][0][0] = 1;
    values[1][0][1][0] = 2;
    values[0][1][0][1] = 3;
    values[0][1][2][2] = 4;
    values[1][0][2][3] = 5;
    let array = Object::Rectangular {
        dimensions: vec![2, 2, 3, 4],
        elements: values
            .into_iter()
            .flatten()
            .flatten()
            .flatten()
            .map(Value::Integer)
            .collect(),
    };
    let root = graph.insert(array.clone());
    let (reader, copy) = deep_clone(&graph, root, stamped);
    assert_eq!(reader.graph().get(copy), Some(&array));
}

array_modes!(
    rectangular_primitives_plain,
    rectangular_primitives_stamped,
    rectangular_primitives
);
array_modes!(
    rectangular_strings_plain,
    rectangular_strings_stamped,
    rectangular_strings
);
array_modes!(
    four_dimensional_array_plain,
    four_dimensional_array_stamped,
    four_dimensional_array
);
