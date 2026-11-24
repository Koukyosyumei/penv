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
    stamp("Dictionary<int,int>", &[("items", FieldKind::Reference)]),
    stamp("Hashtable", &[("items", FieldKind::Reference)]),
    stamp(
        "Dictionary<string,string>",
        &[("items", FieldKind::Reference)],
    ),
    stamp("Dictionary<int,string>", &[("items", FieldKind::Reference)]),
    stamp(
        "ConcurrentDictionary<int,int>",
        &[("items", FieldKind::Reference)],
    ),
    stamp("Queue<int>", &[("items", FieldKind::Reference)]),
    stamp("HashSet<int>", &[("items", FieldKind::Reference)]),
    stamp("ArrayList", &[("items", FieldKind::Reference)]),
    stamp(
        "ReadOnlyCollection<object>",
        &[("list", FieldKind::Reference)],
    ),
    stamp("List<int>", &[("items", FieldKind::Reference)]),
    stamp("List<List<int>>", &[("items", FieldKind::Reference)]),
    stamp("List<Box>", &[("items", FieldKind::Reference)]),
    stamp("List<object>", &[("items", FieldKind::Reference)]),
    stamp("System.Int32", &[("m_value", FieldKind::Integer)]),
    stamp(
        "GenericBox<int?>",
        &[("<Element>k__BackingField", FieldKind::NullableInteger)],
    ),
    stamp("Box", &[("<Element>k__BackingField", FieldKind::Reference)]),
    stamp(
        "GenericBox<long>",
        &[("<Element>k__BackingField", FieldKind::Integer)],
    ),
    stamp(
        "Tuple<string,string,string>",
        &[
            ("m_Item1", FieldKind::Reference),
            ("m_Item2", FieldKind::Reference),
            ("m_Item3", FieldKind::Reference),
        ],
    ),
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

macro_rules! stamp_modes {
    ($plain:ident, $stamped:ident, $check:expr) => {
        #[test]
        fn $plain() {
            ($check)(false);
        }
        #[test]
        fn $stamped() {
            ($check)(true);
        }
    };
}

stamp_modes!(
    primitives_array_plain,
    primitives_array_stamped,
    primitives_array
);
stamp_modes!(strings_array_plain, strings_array_stamped, strings_array);
stamp_modes!(mixed_array_plain, mixed_array_stamped, mixed_array);

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

stamp_modes!(
    rectangular_primitives_plain,
    rectangular_primitives_stamped,
    rectangular_primitives
);
stamp_modes!(
    rectangular_strings_plain,
    rectangular_strings_stamped,
    rectangular_strings
);
stamp_modes!(
    four_dimensional_array_plain,
    four_dimensional_array_stamped,
    four_dimensional_array
);

fn box_with_lazy_scan(stamped: bool) {
    let mut graph = Graph::default();
    let text = graph.insert(string("Something"));
    let root = graph.insert(record("Box", vec![Value::Ref(text)]));
    let (reader, copy) = deep_clone(&graph, root, stamped);
    let field = &fields(reader.graph().get(copy).unwrap())[0];
    assert_eq!(reader.graph().get(reference(field)), graph.get(text));
}

fn field_with_different_formal_and_actual_type(stamped: bool) {
    let mut graph = Graph::default();
    let inner = graph.insert(record("Box", vec![Value::Null]));
    let root = graph.insert(record("Box", vec![Value::Ref(inner)]));
    let (reader, copy) = deep_clone(&graph, root, stamped);
    let field = &fields(reader.graph().get(copy).unwrap())[0];
    let Some(Object::Record { class, .. }) = reader.graph().get(reference(field)) else {
        panic!("expected the concrete record type");
    };
    assert_eq!(class, "Box");
}

fn box_with_long(stamped: bool) {
    let mut graph = Graph::default();
    let root = graph.insert(record("GenericBox<long>", vec![Value::Integer(1234)]));
    let (reader, copy) = deep_clone(&graph, root, stamped);
    assert_eq!(
        fields(reader.graph().get(copy).unwrap())[0],
        fields(graph.get(root).unwrap())[0]
    );
}

fn string_tuple(stamped: bool) {
    let mut graph = Graph::default();
    let contents = ["One", "Two", "Three"];
    let ids = contents.map(|text| Value::Ref(graph.insert(string(text))));
    let root = graph.insert(record("Tuple<string,string,string>", ids.to_vec()));
    let (reader, copy) = deep_clone(&graph, root, stamped);
    let Some(Object::Record { class, fields }) = reader.graph().get(copy) else {
        panic!("expected a tuple");
    };
    assert_eq!(class, "Tuple<string,string,string>");
    assert_eq!(fields.len(), contents.len());
    for (field, content) in fields.iter().zip(contents) {
        assert_eq!(reader.graph().get(reference(field)), Some(&string(content)));
    }
}

stamp_modes!(
    box_with_lazy_scan_plain,
    box_with_lazy_scan_stamped,
    box_with_lazy_scan
);
stamp_modes!(
    field_actual_type_plain,
    field_actual_type_stamped,
    field_with_different_formal_and_actual_type
);
stamp_modes!(box_with_long_plain, box_with_long_stamped, box_with_long);
stamp_modes!(string_tuple_plain, string_tuple_stamped, string_tuple);

fn nullable_field(stamped: bool, value: Option<i32>) {
    let mut graph = Graph::default();
    let element = value.map_or(Value::Null, |value| Value::Integer(i64::from(value)));
    let root = graph.insert(record("GenericBox<int?>", vec![element]));
    let (reader, copy) = deep_clone(&graph, root, stamped);
    assert_eq!(
        fields(reader.graph().get(copy).unwrap())[0],
        fields(graph.get(root).unwrap())[0]
    );
}

fn boxed_integer(graph: &mut Graph, value: i32) -> Value {
    Value::Ref(graph.insert(record(
        "System.Int32",
        vec![Value::Integer(i64::from(value))],
    )))
}

fn boxed_nullable_field(stamped: bool, value: Option<i32>) {
    let mut graph = Graph::default();
    let element = value.map_or(Value::Null, |value| boxed_integer(&mut graph, value));
    let root = graph.insert(record("Box", vec![element.clone()]));
    let (reader, copy) = deep_clone(&graph, root, stamped);
    let actual = &fields(reader.graph().get(copy).unwrap())[0];
    match element {
        Value::Ref(id) => assert_eq!(reader.graph().get(reference(actual)), graph.get(id)),
        Value::Null => assert_eq!(*actual, Value::Null),
        _ => unreachable!(),
    }
}

fn non_null_nullable(stamped: bool) {
    nullable_field(stamped, Some(3));
}
fn null_nullable(stamped: bool) {
    nullable_field(stamped, None);
}
// The upstream method names are reversed relative to their inputs.
fn boxed_null_nullable(stamped: bool) {
    boxed_nullable_field(stamped, Some(66));
}
fn boxed_non_null_nullable(stamped: bool) {
    boxed_nullable_field(stamped, None);
}

stamp_modes!(
    non_null_nullable_plain,
    non_null_nullable_stamped,
    non_null_nullable
);
stamp_modes!(null_nullable_plain, null_nullable_stamped, null_nullable);
stamp_modes!(
    boxed_null_nullable_plain,
    boxed_null_nullable_stamped,
    boxed_null_nullable
);
stamp_modes!(
    boxed_non_null_nullable_plain,
    boxed_non_null_nullable_stamped,
    boxed_non_null_nullable
);

fn two_serializations(stamped: bool) {
    let mut graph = Graph::default();
    let one = Value::Ref(graph.insert(string("One")));
    let two = boxed_integer(&mut graph, 2);
    let four = Value::Ref(graph.insert(string("Four")));
    let root = graph.insert(Object::Array(vec![one, two, Value::Null, four]));
    let mut stream = Cursor::new(Vec::new());
    // Serializer.Serialize starts a fresh write with an independently flushed buffer.
    for _ in 0..2 {
        let mut writer = ObjectWriter::new(&mut stream, &graph, References::PerRoot, true);
        if stamped {
            writer = writer.with_type_stamps(STAMPS);
        }
        writer.write_object(root).unwrap();
        writer.finish().unwrap();
    }
    stream.rewind().unwrap();
    for _ in 0..2 {
        let mut reader = ObjectReader::new(&mut stream, References::PerRoot, true);
        if stamped {
            reader = reader.with_type_stamps(STAMPS);
        }
        let copy = reader.read_object().unwrap();
        let actual = array_values(reader.graph(), copy);
        let expected = array_values(&graph, root);
        assert_eq!(actual.len(), expected.len());
        for (actual, expected) in actual.iter().zip(expected) {
            match expected {
                Value::Ref(id) => assert_eq!(reader.graph().get(reference(actual)), graph.get(*id)),
                Value::Null => assert_eq!(*actual, Value::Null),
                _ => unreachable!(),
            }
        }
        reader.finish().unwrap();
    }
}

stamp_modes!(
    two_serializations_plain,
    two_serializations_stamped,
    two_serializations
);

fn list(graph: &mut Graph, class: &str, items: Vec<Value>) -> ObjectId {
    let backing = graph.insert(Object::Array(items));
    graph.insert(record(class, vec![Value::Ref(backing)]))
}

fn list_values<'a>(graph: &'a Graph, id: ObjectId, class: &str) -> &'a [Value] {
    let Some(Object::Record {
        class: actual,
        fields,
    }) = graph.get(id)
    else {
        panic!("expected list record");
    };
    assert_eq!(actual, class);
    array_values(graph, reference(&fields[0]))
}

fn primitive_list(stamped: bool, range: std::ops::Range<i64>) {
    let mut graph = Graph::default();
    let elements: Vec<_> = range.map(Value::Integer).collect();
    let root = list(&mut graph, "List<int>", elements.clone());
    let (reader, root) = deep_clone(&graph, root, stamped);
    assert_eq!(list_values(reader.graph(), root, "List<int>"), elements);
}

fn list_of_lists(stamped: bool) {
    let mut graph = Graph::default();
    let inputs = [[1, 2, 3], [97, 98, 99]].map(|values| values.map(Value::Integer));
    let roots = inputs
        .clone()
        .map(|values| Value::Ref(list(&mut graph, "List<int>", values.to_vec())));
    let root = list(&mut graph, "List<List<int>>", roots.to_vec());
    let (reader, copy) = deep_clone(&graph, root, stamped);
    let restored = list_values(reader.graph(), copy, "List<List<int>>");
    for index in 0..2 {
        assert_eq!(
            list_values(reader.graph(), reference(&restored[index]), "List<int>"),
            inputs[index]
        );
    }
}

fn list_self_identity(stamped: bool) {
    let mut graph = Graph::default();
    let root = list(&mut graph, "List<Box>", vec![]);
    let first = graph.insert(record("Box", vec![Value::Ref(root)]));
    let second = graph.insert(record("Box", vec![Value::Ref(root)]));
    let backing = reference(&fields(graph.get(root).unwrap())[0]);
    *graph.get_mut(backing).unwrap() = Object::Array(vec![Value::Ref(first), Value::Ref(second)]);
    let (reader, copy) = deep_clone(&graph, root, stamped);
    let graph = reader.graph();
    let items = list_values(graph, copy, "List<Box>");
    let first = reference(&fields(graph.get(reference(&items[0])).unwrap())[0]);
    let second = reference(&fields(graph.get(reference(&items[1])).unwrap())[0]);
    assert_eq!(copy, first);
    assert_eq!(first, second);
}

fn boxed_primitive(stamped: bool) {
    let mut graph = Graph::default();
    let three = boxed_integer(&mut graph, 3);
    let root = graph.insert(record("Box", vec![three]));
    let (reader, copy) = deep_clone(&graph, root, stamped);
    let element = reference(&fields(reader.graph().get(copy).unwrap())[0]);
    assert_eq!(
        reader.graph().get(element),
        Some(&record("System.Int32", vec![Value::Integer(3)]))
    );
}

fn boxed_identity(stamped: bool) {
    let mut graph = Graph::default();
    let primitive = boxed_integer(&mut graph, 1234);
    let first = graph.insert(record("Box", vec![primitive.clone()]));
    let second = graph.insert(record("Box", vec![primitive]));
    let root = list(
        &mut graph,
        "List<Box>",
        vec![Value::Ref(first), Value::Ref(second)],
    );
    let (reader, copy) = deep_clone(&graph, root, stamped);
    let graph = reader.graph();
    let items = list_values(graph, copy, "List<Box>");
    let element =
        |index: usize| reference(&fields(graph.get(reference(&items[index])).unwrap())[0]);
    assert_eq!(element(0), element(1));
}

fn list_with_null(stamped: bool) {
    let mut graph = Graph::default();
    let one = graph.insert(string("One"));
    let two = graph.insert(string("Two"));
    let root = list(
        &mut graph,
        "List<object>",
        vec![Value::Ref(one), Value::Null, Value::Ref(two)],
    );
    let (reader, copy) = deep_clone(&graph, root, stamped);
    let graph = reader.graph();
    let actual = list_values(graph, copy, "List<object>");
    assert_eq!(actual.len(), 3);
    assert_eq!(graph.get(reference(&actual[0])), Some(&string("One")));
    assert_eq!(actual[1], Value::Null);
    assert_eq!(graph.get(reference(&actual[2])), Some(&string("Two")));
}

stamp_modes!(primitive_list_plain, primitive_list_stamped, |stamped| {
    primitive_list(stamped, 1..4)
});
stamp_modes!(
    long_primitive_list_plain,
    long_primitive_list_stamped,
    |stamped| primitive_list(stamped, 0..100_000)
);
stamp_modes!(list_of_lists_plain, list_of_lists_stamped, list_of_lists);
stamp_modes!(
    list_self_identity_plain,
    list_self_identity_stamped,
    list_self_identity
);
stamp_modes!(
    boxed_primitive_plain,
    boxed_primitive_stamped,
    boxed_primitive
);
stamp_modes!(boxed_identity_plain, boxed_identity_stamped, boxed_identity);
stamp_modes!(list_with_null_plain, list_with_null_stamped, list_with_null);

fn dictionary(
    graph: &mut Graph,
    class: &str,
    entries: impl IntoIterator<Item = (Value, Value)>,
) -> ObjectId {
    list(
        graph,
        class,
        entries
            .into_iter()
            .flat_map(|(key, value)| [key, value])
            .collect(),
    )
}

fn dictionary_entries<'a>(
    graph: &'a Graph,
    root: ObjectId,
    class: &str,
) -> impl Iterator<Item = (&'a Value, &'a Value)> {
    let items = list_values(graph, root, class);
    assert_eq!(items.len() % 2, 0);
    items.chunks_exact(2).map(|entry| (&entry[0], &entry[1]))
}

fn integer_value(graph: &Graph, item: &Value) -> i64 {
    let value = match item {
        Value::Ref(id) => {
            let object = graph.get(*id).unwrap();
            let Object::Record { class, .. } = object else {
                panic!("expected boxed integer")
            };
            assert_eq!(class, "System.Int32");
            &fields(object)[0]
        }
        value => value,
    };
    let Value::Integer(value) = value else {
        panic!("expected integer")
    };
    *value
}

fn string_value<'a>(graph: &'a Graph, value: &Value) -> &'a str {
    let Some(Object::String(text)) = graph.get(reference(value)) else {
        panic!("expected string")
    };
    text
}

fn integer_dictionary(
    stamped: bool,
    class: &str,
    size: i32,
    values: impl Fn(i32) -> i32,
    boxed: bool,
) {
    let mut graph = Graph::default();
    let expected: std::collections::BTreeMap<_, _> = (0..size)
        .map(|i| (i64::from(i), i64::from(values(i))))
        .collect();
    let entries: Vec<_> = (0..size)
        .map(|i| {
            if boxed {
                (
                    boxed_integer(&mut graph, i),
                    boxed_integer(&mut graph, values(i)),
                )
            } else {
                (
                    Value::Integer(i64::from(i)),
                    Value::Integer(i64::from(values(i))),
                )
            }
        })
        .collect();
    let root = dictionary(&mut graph, class, entries);
    let (reader, root) = deep_clone(&graph, root, stamped);
    let graph = reader.graph();
    let actual: Vec<_> = dictionary_entries(graph, root, class)
        .map(|(key, value)| (integer_value(graph, key), integer_value(graph, value)))
        .collect();
    assert_eq!(actual.len(), expected.len());
    assert_eq!(
        actual
            .into_iter()
            .collect::<std::collections::BTreeMap<_, _>>(),
        expected
    );
}

fn string_dictionary(stamped: bool) {
    let mut graph = Graph::default();
    let expected = [("Ball", "Piłka"), ("Cat", "Kot"), ("Line", "Linia")];
    let entries = expected.map(|(key, value)| {
        (
            Value::Ref(graph.insert(string(key))),
            Value::Ref(graph.insert(string(value))),
        )
    });
    let class = "Dictionary<string,string>";
    let root = dictionary(&mut graph, class, entries);
    let (reader, root) = deep_clone(&graph, root, stamped);
    let graph = reader.graph();
    let actual: Vec<_> = dictionary_entries(graph, root, class)
        .map(|(key, value)| (string_value(graph, key), string_value(graph, value)))
        .collect();
    assert_eq!(actual.len(), expected.len());
    assert_eq!(
        actual
            .into_iter()
            .collect::<std::collections::BTreeMap<_, _>>(),
        expected.into_iter().collect()
    );
}

fn dictionary_identity(stamped: bool) {
    let mut graph = Graph::default();
    let word = graph.insert(string("Word"));
    let one = graph.insert(string("One"));
    let class = "Dictionary<int,string>";
    let root = dictionary(
        &mut graph,
        class,
        [
            (Value::Integer(10), Value::Ref(word)),
            (Value::Integer(20), Value::Ref(one)),
            (Value::Integer(30), Value::Ref(word)),
        ],
    );
    let (reader, root) = deep_clone(&graph, root, stamped);
    let graph = reader.graph();
    let restored: Vec<_> = dictionary_entries(graph, root, class).collect();
    assert_eq!(restored.len(), 3);
    let by_key: std::collections::BTreeMap<_, _> = restored
        .into_iter()
        .map(|(key, value)| (integer_value(graph, key), value))
        .collect();
    let values: Vec<_> = by_key
        .iter()
        .map(|(&key, value)| (key, string_value(graph, value)))
        .collect();
    assert_eq!(values, [(10, "Word"), (20, "One"), (30, "Word")]);
    assert_eq!(reference(by_key[&10]), reference(by_key[&30]));
    assert_ne!(reference(by_key[&10]), reference(by_key[&20]));
}

fn queue_and_set(stamped: bool, queue: bool) {
    let mut graph = Graph::default();
    let (class, input) = if queue {
        ("Queue<int>", [1, 3, 5])
    } else {
        ("HashSet<int>", [1, 2, 3])
    };
    let root = list(&mut graph, class, input.map(Value::Integer).to_vec());
    let (reader, root) = deep_clone(&graph, root, stamped);
    let mut actual: Vec<_> = list_values(reader.graph(), root, class)
        .iter()
        .map(|value| integer_value(reader.graph(), value))
        .collect();
    if !queue {
        actual.sort_unstable();
    }
    assert_eq!(actual, input);
}

stamp_modes!(int_dictionary_plain, int_dictionary_stamped, |s| {
    integer_dictionary(s, "Dictionary<int,int>", 100, |i| i + 1, false)
});
stamp_modes!(hashtable_plain, hashtable_stamped, |s| integer_dictionary(
    s,
    "Hashtable",
    100,
    |i| i + 1,
    true
));
stamp_modes!(
    concurrent_dictionary_plain,
    concurrent_dictionary_stamped,
    |s| integer_dictionary(s, "ConcurrentDictionary<int,int>", 10, |i| 2 * i, false)
);
stamp_modes!(
    string_dictionary_plain,
    string_dictionary_stamped,
    string_dictionary
);
stamp_modes!(
    dictionary_identity_plain,
    dictionary_identity_stamped,
    dictionary_identity
);
stamp_modes!(queue_plain, queue_stamped, |s| queue_and_set(s, true));
stamp_modes!(hash_set_plain, hash_set_stamped, |s| queue_and_set(
    s, false
));

fn array_list_with_strings(stamped: bool) {
    let mut graph = Graph::default();
    let first = graph.insert(string("Word 1"));
    let second = graph.insert(string("Word 2"));
    let text = graph.insert(string("Word 4"));
    let object = graph.insert(record(
        "SimpleClass",
        vec![Value::Integer(6), Value::Ref(text)],
    ));
    let last = graph.insert(string("Word 3"));
    let root = list(
        &mut graph,
        "ArrayList",
        vec![first, second, object, last]
            .into_iter()
            .map(Value::Ref)
            .collect(),
    );
    let (reader, root) = deep_clone(&graph, root, stamped);
    let graph = reader.graph();
    let restored = list_values(graph, root, "ArrayList");
    assert_eq!(restored.len(), 4);
    assert_eq!(string_value(graph, &restored[0]), "Word 1");
    assert_eq!(string_value(graph, &restored[1]), "Word 2");
    let object = graph.get(reference(&restored[2])).unwrap();
    let Object::Record { class, fields } = object else {
        panic!("expected SimpleClass");
    };
    assert_eq!(class, "SimpleClass");
    assert_eq!(fields[0], Value::Integer(6));
    assert_eq!(string_value(graph, &fields[1]), "Word 4");
    assert_eq!(string_value(graph, &restored[3]), "Word 3");
}

fn array_list_with_primitives(stamped: bool) {
    let mut graph = Graph::default();
    let boxed = [1, 2, 3].map(|value| boxed_integer(&mut graph, value));
    let root = list(&mut graph, "ArrayList", boxed.to_vec());
    let (reader, root) = deep_clone(&graph, root, stamped);
    let graph = reader.graph();
    let restored: Vec<_> = list_values(graph, root, "ArrayList")
        .iter()
        .map(|value| integer_value(graph, value))
        .collect();
    assert_eq!(restored, [1, 2, 3]);
}

fn read_only_collection(stamped: bool) {
    let mut graph = Graph::default();
    let first = Value::Ref(graph.insert(string("asdasd")));
    let second = boxed_integer(&mut graph, 1);
    let third = Value::Ref(graph.insert(string("cvzxcv")));
    let inner = list(&mut graph, "List<object>", vec![first, second, third]);
    let root = graph.insert(record(
        "ReadOnlyCollection<object>",
        vec![Value::Ref(inner)],
    ));
    let (reader, root) = deep_clone(&graph, root, stamped);
    let graph = reader.graph();
    let Some(Object::Record { class, fields }) = graph.get(root) else {
        panic!("expected collection");
    };
    assert_eq!(class, "ReadOnlyCollection<object>");
    let restored = list_values(graph, reference(&fields[0]), "List<object>");
    assert_eq!(restored.len(), 3);
    assert_eq!(string_value(graph, &restored[0]), "asdasd");
    assert_eq!(integer_value(graph, &restored[1]), 1);
    assert_eq!(string_value(graph, &restored[2]), "cvzxcv");
}

stamp_modes!(
    array_list_strings_plain,
    array_list_strings_stamped,
    array_list_with_strings
);
stamp_modes!(
    array_list_primitives_plain,
    array_list_primitives_stamped,
    array_list_with_primitives
);
stamp_modes!(
    read_only_collection_plain,
    read_only_collection_stamped,
    read_only_collection
);

fn byte_array(stamped: bool) {
    let mut graph = Graph::default();
    let expected = Object::Bytes(vec![2, 4, 6, 8, 10]);
    let root = graph.insert(expected.clone());
    let (reader, copy) = deep_clone(&graph, root, stamped);
    assert_eq!(reader.graph().get(copy), Some(&expected));
}

fn empty_array(stamped: bool) {
    let mut graph = Graph::default();
    let root = graph.insert(Object::Array(Vec::new()));
    let (reader, copy) = deep_clone(&graph, root, stamped);
    assert!(array_values(reader.graph(), copy).is_empty());
}

fn empty_rectangular(stamped: bool, dimensions: &[usize]) {
    let mut graph = Graph::default();
    let root = graph.insert(Object::Rectangular {
        dimensions: dimensions.to_vec(),
        elements: Vec::new(),
    });
    let (reader, copy) = deep_clone(&graph, root, stamped);
    let Some(Object::Rectangular {
        dimensions: actual,
        elements,
    }) = reader.graph().get(copy)
    else {
        panic!("expected a rectangular array");
    };
    assert_eq!(actual, dimensions);
    assert!(elements.is_empty());
}

stamp_modes!(byte_array_plain, byte_array_stamped, byte_array);
stamp_modes!(empty_array_plain, empty_array_stamped, empty_array);
stamp_modes!(empty_list_plain, empty_list_stamped, |s| primitive_list(
    s,
    0..0
));
stamp_modes!(empty_2d_plain, empty_2d_stamped, |s| empty_rectangular(
    s,
    &[0, 0]
));
stamp_modes!(empty_3d_plain, empty_3d_stamped, |s| empty_rectangular(
    s,
    &[2, 0, 2]
));
