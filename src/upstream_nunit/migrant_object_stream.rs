use crate::migrant::object_stream::{
    Graph, Object, ObjectId, ObjectReader, ObjectWriter, References, Value,
};
use std::io::{Cursor, Seek, SeekFrom};

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

fn deep_clone(graph: &Graph, root: ObjectId) -> (ObjectReader<Cursor<Vec<u8>>>, ObjectId) {
    let mut stream = Cursor::new(Vec::new());
    let mut writer = ObjectWriter::new(&mut stream, graph, References::Preserve, true);
    writer.write_object(root).unwrap();
    writer.finish().unwrap();
    stream.seek(SeekFrom::Start(0)).unwrap();
    let mut reader = ObjectReader::new(stream, References::Preserve, true);
    let root = reader.read_object().unwrap();
    (reader, root)
}

#[test]
fn should_serialize_object() {
    let mut graph = Graph::default();
    let object = graph.insert(record("System.Object", vec![]));
    let (reader, copy) = deep_clone(&graph, object);
    assert_eq!(
        reader.graph().get(copy),
        Some(&record("System.Object", vec![]))
    );
}

#[test]
fn should_serialize_simple_class() {
    let mut graph = Graph::default();
    let text = graph.insert(string("Magic"));
    let object = graph.insert(record(
        "SimpleClass",
        vec![Value::Integer(0x69), Value::Ref(text)],
    ));
    let (reader, copy) = deep_clone(&graph, object);
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
    let mut graph = Graph::default();
    let one = graph.insert(cyclic_record(Value::Null, 0));
    let another = graph.insert(cyclic_record(Value::Ref(one), 1));
    *graph.get_mut(one).unwrap() = cyclic_record(Value::Ref(another), 0);
    let (reader, one_copy) = deep_clone(&graph, one);
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
    let mut graph = Graph::default();
    let one = graph.insert(cyclic_record(Value::Null, 0));
    *graph.get_mut(one).unwrap() = cyclic_record(Value::Ref(one), 0);
    let (reader, copy) = deep_clone(&graph, one);
    assert_eq!(
        copy,
        reference(&fields(reader.graph().get(copy).unwrap())[0])
    );
}

#[test]
fn should_serialize_array_reference_to_self() {
    let mut graph = Graph::default();
    let array = graph.insert(Object::Array(vec![Value::Null; 2]));
    *graph.get_mut(array).unwrap() = Object::Array(vec![Value::Ref(array); 2]);
    let (reader, copy) = deep_clone(&graph, array);
    let Object::Array(elements) = reader.graph().get(copy).unwrap() else {
        panic!("expected array")
    };
    assert_eq!(copy, reference(&elements[0]));
    assert_eq!(reference(&elements[0]), reference(&elements[1]));
}

#[test]
fn should_serialize_null_reference() {
    let mut graph = Graph::default();
    let object = graph.insert(record("SimpleClass", vec![Value::Integer(0), Value::Null]));
    let (reader, copy) = deep_clone(&graph, object);
    assert_eq!(graph.get(object), reader.graph().get(copy));
}

#[test]
fn should_preserve_identity() {
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
    let (reader, copy) = deep_clone(&graph, container);
    let graph = reader.graph();
    let container = fields(graph.get(copy).unwrap());
    let first = fields(graph.get(reference(&container[0])).unwrap());
    let second = fields(graph.get(reference(&container[1])).unwrap());
    assert_eq!(reference(&first[1]), reference(&second[1]));
}

#[test]
fn should_serialize_readonly_field() {
    let mut graph = Graph::default();
    let object = graph.insert(record("ClassWithReadonly", vec![Value::Integer(0xabcd)]));
    let (reader, copy) = deep_clone(&graph, object);
    assert_eq!(
        fields(reader.graph().get(copy).unwrap())[0],
        Value::Integer(0xabcd)
    );
}

#[test]
fn should_serialize_deep_graph() {
    let mut graph = Graph::default();
    let class = "ClassWithReferenceToTheObjectOfTheSameType";
    let root = graph.insert(record(class, vec![Value::Null]));
    let mut current = root;
    for _ in 0..45_000 {
        let next = graph.insert(record(class, vec![Value::Null]));
        *graph.get_mut(current).unwrap() = record(class, vec![Value::Ref(next)]);
        current = next;
    }
    let (reader, mut copy) = deep_clone(&graph, root);
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
    let mut writer = ObjectWriter::new(&mut stream, &graph, References::Preserve, true);
    for id in ids {
        writer.write_object(id).unwrap();
    }
    writer.finish().unwrap();
    let position = stream.position();
    stream.seek(SeekFrom::Start(0)).unwrap();
    let mut reader = ObjectReader::new(&mut stream, References::Preserve, true);
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
) -> ObjectReader<Cursor<Vec<u8>>> {
    let mut stream = Cursor::new(Vec::new());
    let mut writer = ObjectWriter::new(&mut stream, graph, references, buffered);
    for &root in roots {
        writer.write_object(root).unwrap();
    }
    writer.finish().unwrap();
    stream.seek(SeekFrom::Start(0)).unwrap();
    ObjectReader::new(stream, references, buffered)
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
