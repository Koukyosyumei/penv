use crate::migrant::generated_record::{generated_record, Codec};
use crate::migrant::object_stream::{
    FieldInitializer, FieldKind, Graph, LateRecordHook, NativeResource, NonSerializableResource,
    Object, ObjectId, ObjectReader, ObjectWriter, References, Transients, TypeStamp, Value,
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
    stamp("SimpleEnum", &[("value__", FieldKind::Integer)]),
    stamp("EnumByte", &[("value__", FieldKind::Integer)]),
    stamp("EnumLong", &[("value__", FieldKind::Integer)]),
    stamp("EnumShort", &[("value__", FieldKind::Integer)]),
    stamp("TestEnumFlags", &[("value__", FieldKind::Integer)]),
    stamp(
        "ClassWithNullableEnum",
        &[("E", FieldKind::NullableInteger)],
    ),
    stamp(
        "KeyValuePair<int,int>",
        &[("key", FieldKind::Integer), ("value", FieldKind::Integer)],
    ),
    stamp("Stack", &[("items", FieldKind::Reference)]),
    stamp("Stack<char>", &[("items", FieldKind::Reference)]),
    stamp("System.Char", &[("m_value", FieldKind::Integer)]),
    stamp(
        "SimpleDerivedClass",
        &[
            ("SimpleBaseClass.BaseField", FieldKind::Integer),
            (
                "SimpleDerivedClass.<DerivedField>k__BackingField",
                FieldKind::Integer,
            ),
        ],
    ),
    stamp(
        "SimplerStruct",
        &[("A", FieldKind::Integer), ("B", FieldKind::Integer)],
    ),
    stamp(
        "SimpleStruct",
        &[("A", FieldKind::Integer), ("B", FieldKind::Reference)],
    ),
    stamp(
        "ComplexStruct",
        &[
            ("A", FieldKind::Integer),
            ("B.A", FieldKind::Integer),
            ("B.B", FieldKind::Reference),
        ],
    ),
    stamp(
        "GenericBox<SimpleStruct>",
        &[
            ("Element.A", FieldKind::Integer),
            ("Element.B", FieldKind::Reference),
        ],
    ),
    stamp("CustomObject", &[("Field", FieldKind::Reference)]),
    stamp(
        "Dictionary<CustomObject,string>",
        &[("items", FieldKind::Reference)],
    ),
    stamp("HashSet<CustomObject>", &[("items", FieldKind::Reference)]),
    stamp(
        "GenericBox<Guid>",
        &[("<Element>k__BackingField", FieldKind::Guid)],
    ),
    stamp(
        "ClassWithGuid",
        &[
            ("<Id>k__BackingField", FieldKind::Guid),
            ("<Number>k__BackingField", FieldKind::Integer),
            ("<Str>k__BackingField", FieldKind::Reference),
        ],
    ),
    stamp(
        "GenericBox<BigInteger>",
        &[("<Element>k__BackingField", FieldKind::BigInteger)],
    ),
    stamp(
        "System.Numerics.BigInteger",
        &[("value", FieldKind::BigInteger)],
    ),
    stamp(
        "ClassWithTransientField",
        &[("A", FieldKind::Integer), ("C", FieldKind::Integer)],
    ),
    stamp(
        "ClassWithIntPtr",
        &[("<Ptr>k__BackingField", FieldKind::NativeResource)],
    ),
    stamp(
        "ClassWithPointer",
        &[("Pointer", FieldKind::NativeResource)],
    ),
    stamp(
        "ClassWithSpinLock",
        &[("<SLock>k__BackingField", FieldKind::NativeResource)],
    ),
    stamp(
        "ClassWithThreadLocal",
        &[("<ThreadLocal>k__BackingField", FieldKind::NativeResource)],
    ),
    stamp(
        "GenericBox<AnotherContainingType>",
        &[("Element", FieldKind::Reference)],
    ),
    stamp(
        "AnotherContainingType",
        &[("WithIntPtr", FieldKind::Reference)],
    ),
    stamp(
        "Issue60OuterClass",
        &[
            ("Field1", FieldKind::Decimal),
            ("Inner", FieldKind::Reference),
        ],
    ),
    stamp(
        "Issue60InnerClass",
        &[
            ("Field2", FieldKind::NullableDecimal),
            ("Field3", FieldKind::NullableDecimal),
        ],
    ),
    stamp(
        "SpeciallySerializable",
        &[("payload", FieldKind::Reference)],
    ),
    stamp("BadlySerializable", &[("payload", FieldKind::Reference)]),
    stamp(
        "StructGenericBox<SpeciallySerializable>",
        &[("Element", FieldKind::Reference)],
    ),
    stamp(
        "GenericClassWithNestedGenericArgument<string>",
        &[("Field", FieldKind::Reference)],
    ),
    stamp("GenericClass<string>", &[("Field", FieldKind::Reference)]),
    stamp(
        "ClassWithTheListOfInterfaces",
        &[("List", FieldKind::Reference)],
    ),
    stamp("List<IInterface>", &[("items", FieldKind::Reference)]),
    stamp("ImplementingInterfaceOne", &[]),
    stamp("ImplementingInterfaceTwo", &[]),
    stamp("StructWithConstructorAttribute", &[]),
    stamp("StructWithConstructorAttributeOnNestedStruct", &[]),
    stamp("WithConstructorAttribute", &[]),
    stamp("GenericClassWithLatePostDeserializationHook", &[]),
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
    deep_clone_transient(graph, root, stamped, None)
}

fn deep_clone_transient(
    graph: &Graph,
    root: ObjectId,
    stamped: bool,
    transients: Option<&'static Transients<'static>>,
) -> (ObjectReader<'static, 'static, Cursor<Vec<u8>>>, ObjectId) {
    deep_clone_with_hooks(graph, root, stamped, transients, &[])
}

fn deep_clone_with_hooks(
    graph: &Graph,
    root: ObjectId,
    stamped: bool,
    transients: Option<&'static Transients<'static>>,
    hooks: &'static [LateRecordHook<'static>],
) -> (ObjectReader<'static, 'static, Cursor<Vec<u8>>>, ObjectId) {
    clone_using_methods(graph, root, stamped, transients, hooks, None, None)
}

fn clone_using_methods(
    graph: &Graph,
    root: ObjectId,
    stamped: bool,
    transients: Option<&'static Transients<'static>>,
    hooks: &'static [LateRecordHook<'static>],
    write_codecs: Option<&'static [Codec]>,
    read_codecs: Option<&'static [Codec]>,
) -> (ObjectReader<'static, 'static, Cursor<Vec<u8>>>, ObjectId) {
    let mut stream = Cursor::new(Vec::new());
    let mut writer = ObjectWriter::new(&mut stream, graph, References::Preserve, true);
    if stamped {
        writer = writer.with_type_stamps(STAMPS);
    }
    if let Some(transients) = transients {
        writer = writer.with_transients(transients);
    }
    if let Some(codecs) = write_codecs {
        writer = writer.with_generated_records(codecs);
    }
    writer.write_object(root).unwrap();
    writer.finish().unwrap();
    stream.seek(SeekFrom::Start(0)).unwrap();
    let mut reader = ObjectReader::new(stream, References::Preserve, true).with_late_hooks(hooks);
    if stamped {
        reader = reader.with_type_stamps(STAMPS);
    }
    if let Some(transients) = transients {
        reader = reader.with_transients(transients);
    }
    if let Some(codecs) = read_codecs {
        reader = reader.with_generated_records(codecs);
    }
    let root = reader.read_object().unwrap();
    (reader, root)
}

const GRAPH_CODECS: &[Codec] = &[
    generated_record!("System.Object";),
    generated_record!("SimpleClass"; value: integer, string: reference),
    generated_record!("CyclicRef"; another: reference, value: integer),
    generated_record!("SimpleContainer"; first: reference, second: reference),
    generated_record!("ClassWithReadonly"; value: integer),
    generated_record!("ClassWithReferenceToTheObjectOfTheSameType"; next: reference),
    generated_record!("SimpleEnum"; value: integer),
    generated_record!("EnumByte"; value: integer),
    generated_record!("EnumLong"; value: integer),
    generated_record!("EnumShort"; value: integer),
    generated_record!("TestEnumFlags"; value: integer),
    generated_record!("ClassWithNullableEnum"; value: nullable_integer),
    generated_record!("KeyValuePair<int,int>"; key: integer, value: integer),
    generated_record!("Stack"; items: reference),
    generated_record!("Stack<char>"; items: reference),
    generated_record!("System.Char"; value: integer),
    generated_record!("SimpleDerivedClass"; base: integer, derived: integer),
    generated_record!("SimplerStruct"; first: integer, second: integer),
    generated_record!("SimpleStruct"; first: integer, text: reference),
    generated_record!("ComplexStruct"; first: integer, nested: integer, text: reference),
    generated_record!("GenericBox<SimpleStruct>"; value: integer, text: reference),
    generated_record!("Box"; element: reference),
    generated_record!("GenericBox<long>"; element: integer),
    generated_record!("GenericBox<int?>"; element: nullable_integer),
    generated_record!("System.Int32"; value: integer),
    generated_record!("Tuple<string,string,string>"; first: reference, second: reference, third: reference),
    generated_record!("List<int>"; items: reference),
    generated_record!("List<List<int>>"; items: reference),
    generated_record!("List<Box>"; items: reference),
    generated_record!("List<object>"; items: reference),
];

#[derive(Clone, Copy, Default)]
struct GraphMethods {
    stamped: bool,
    write: bool,
    read: bool,
}

impl From<bool> for GraphMethods {
    fn from(stamped: bool) -> Self {
        Self {
            stamped,
            ..Self::default()
        }
    }
}

impl GraphMethods {
    fn clone(
        self,
        graph: &Graph,
        root: ObjectId,
    ) -> (ObjectReader<'static, 'static, Cursor<Vec<u8>>>, ObjectId) {
        clone_using_methods(
            graph,
            root,
            self.stamped,
            None,
            &[],
            self.write.then_some(GRAPH_CODECS),
            self.read.then_some(GRAPH_CODECS),
        )
    }
}

#[test]
fn should_serialize_object() {
    should_serialize_object_with_methods(GraphMethods::default());
}

#[test]
fn should_serialize_object_stamped() {
    should_serialize_object_with_methods(GraphMethods {
        stamped: true,
        ..GraphMethods::default()
    });
}

fn should_serialize_object_with_methods(methods: GraphMethods) {
    let mut graph = Graph::default();
    let object = graph.insert(record("System.Object", vec![]));
    let (reader, copy) = methods.clone(&graph, object);
    assert_eq!(
        reader.graph().get(copy),
        Some(&record("System.Object", vec![]))
    );
}

#[test]
fn should_serialize_simple_class() {
    should_serialize_simple_class_with_methods(GraphMethods::default());
}

#[test]
fn should_serialize_simple_class_stamped() {
    should_serialize_simple_class_with_methods(GraphMethods {
        stamped: true,
        ..GraphMethods::default()
    });
}

fn should_serialize_simple_class_with_methods(methods: GraphMethods) {
    let mut graph = Graph::default();
    let text = graph.insert(string("Magic"));
    let object = graph.insert(record(
        "SimpleClass",
        vec![Value::Integer(0x69), Value::Ref(text)],
    ));
    let (reader, copy) = methods.clone(&graph, object);
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
    should_serialize_cyclic_reference_with_methods(GraphMethods::default());
}

#[test]
fn should_serialize_cyclic_reference_stamped() {
    should_serialize_cyclic_reference_with_methods(GraphMethods {
        stamped: true,
        ..GraphMethods::default()
    });
}

fn should_serialize_cyclic_reference_with_methods(methods: GraphMethods) {
    let mut graph = Graph::default();
    let one = graph.insert(cyclic_record(Value::Null, 0));
    let another = graph.insert(cyclic_record(Value::Ref(one), 1));
    *graph.get_mut(one).unwrap() = cyclic_record(Value::Ref(another), 0);
    let (reader, one_copy) = methods.clone(&graph, one);
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
    should_serialize_reference_to_self_with_methods(GraphMethods::default());
}

#[test]
fn should_serialize_reference_to_self_stamped() {
    should_serialize_reference_to_self_with_methods(GraphMethods {
        stamped: true,
        ..GraphMethods::default()
    });
}

fn should_serialize_reference_to_self_with_methods(methods: GraphMethods) {
    let mut graph = Graph::default();
    let one = graph.insert(cyclic_record(Value::Null, 0));
    *graph.get_mut(one).unwrap() = cyclic_record(Value::Ref(one), 0);
    let (reader, copy) = methods.clone(&graph, one);
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
    should_serialize_null_reference_with_methods(GraphMethods::default());
}

#[test]
fn should_serialize_null_reference_stamped() {
    should_serialize_null_reference_with_methods(GraphMethods {
        stamped: true,
        ..GraphMethods::default()
    });
}

fn should_serialize_null_reference_with_methods(methods: GraphMethods) {
    let mut graph = Graph::default();
    let object = graph.insert(record("SimpleClass", vec![Value::Integer(0), Value::Null]));
    let (reader, copy) = methods.clone(&graph, object);
    assert_eq!(graph.get(object), reader.graph().get(copy));
}

#[test]
fn should_preserve_identity() {
    should_preserve_identity_with_methods(GraphMethods::default());
}

#[test]
fn should_preserve_identity_stamped() {
    should_preserve_identity_with_methods(GraphMethods {
        stamped: true,
        ..GraphMethods::default()
    });
}

fn should_preserve_identity_with_methods(methods: GraphMethods) {
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
    let (reader, copy) = methods.clone(&graph, container);
    let graph = reader.graph();
    let container = fields(graph.get(copy).unwrap());
    let first = fields(graph.get(reference(&container[0])).unwrap());
    let second = fields(graph.get(reference(&container[1])).unwrap());
    assert_eq!(reference(&first[1]), reference(&second[1]));
}

#[test]
fn should_serialize_readonly_field() {
    should_serialize_readonly_field_with_methods(GraphMethods::default());
}

#[test]
fn should_serialize_readonly_field_stamped() {
    should_serialize_readonly_field_with_methods(GraphMethods {
        stamped: true,
        ..GraphMethods::default()
    });
}

fn should_serialize_readonly_field_with_methods(methods: GraphMethods) {
    let mut graph = Graph::default();
    let object = graph.insert(record("ClassWithReadonly", vec![Value::Integer(0xabcd)]));
    let (reader, copy) = methods.clone(&graph, object);
    assert_eq!(
        fields(reader.graph().get(copy).unwrap())[0],
        Value::Integer(0xabcd)
    );
}

#[test]
fn should_serialize_deep_graph() {
    should_serialize_deep_graph_with_methods(GraphMethods::default());
}

#[test]
fn should_serialize_deep_graph_stamped() {
    should_serialize_deep_graph_with_methods(GraphMethods {
        stamped: true,
        ..GraphMethods::default()
    });
}

fn should_serialize_deep_graph_with_methods(methods: GraphMethods) {
    let mut graph = Graph::default();
    let class = "ClassWithReferenceToTheObjectOfTheSameType";
    let root = graph.insert(record(class, vec![Value::Null]));
    let mut current = root;
    for _ in 0..45_000 {
        let next = graph.insert(record(class, vec![Value::Null]));
        *graph.get_mut(current).unwrap() = record(class, vec![Value::Ref(next)]);
        current = next;
    }
    let (reader, mut copy) = methods.clone(&graph, root);
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

const OPEN_STREAM_CODECS: &[Codec] = &[
    generated_record!("System.Object";),
    generated_record!("Tuple<int,int>"; first: integer, second: integer),
    generated_record!("Tuple<int,double>"; first: integer, second: float),
];

#[derive(Clone, Copy, Default)]
struct OpenStreamMethods {
    write: bool,
    read: bool,
}

fn open_stream(
    graph: &Graph,
    roots: &[ObjectId],
    references: References,
    buffered: bool,
    methods: OpenStreamMethods,
) -> ObjectReader<'static, 'static, Cursor<Vec<u8>>> {
    let mut stream = Cursor::new(Vec::new());
    let mut writer =
        ObjectWriter::new(&mut stream, graph, references, buffered).with_type_stamps(STAMPS);
    if methods.write {
        writer = writer.with_generated_records(OPEN_STREAM_CODECS);
    }
    for &root in roots {
        writer.write_object(root).unwrap();
    }
    writer.finish().unwrap();
    stream.seek(SeekFrom::Start(0)).unwrap();
    let reader = ObjectReader::new(stream, references, buffered).with_type_stamps(STAMPS);
    if methods.read {
        reader.with_generated_records(OPEN_STREAM_CODECS)
    } else {
        reader
    }
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

fn basic_open_stream(references: References, methods: OpenStreamMethods) {
    let (graph, roots) = tuples();
    let mut reader = open_stream(&graph, &roots, references, true, methods);
    let objects = roots.map(|_| reader.read_object().unwrap());
    for (expected, actual) in roots.into_iter().zip(objects) {
        assert_eq!(graph.get(expected), reader.graph().get(actual));
    }
    reader.finish().unwrap();
}

#[test]
fn basic_open_stream_do_not_preserve() {
    basic_open_stream(References::PerRoot, OpenStreamMethods::default());
}

#[test]
fn basic_open_stream_preserve() {
    basic_open_stream(References::Preserve, OpenStreamMethods::default());
}

fn should_preserve_references_with_methods(methods: OpenStreamMethods) {
    let (graph, roots) = tuples();
    let mut reader = open_stream(&graph, &roots, References::Preserve, true, methods);
    let [first, duplicate, second] = roots.map(|_| reader.read_object().unwrap());
    assert_eq!(first, duplicate);
    assert_ne!(duplicate, second);
    reader.finish().unwrap();
}

fn shouldnt_preserve_references_with_methods(methods: OpenStreamMethods) {
    let mut graph = Graph::default();
    let object = graph.insert(record("System.Object", vec![]));
    let mut reader = open_stream(
        &graph,
        &[object, object],
        References::PerRoot,
        true,
        methods,
    );
    assert_ne!(reader.read_object().unwrap(), reader.read_object().unwrap());
    reader.finish().unwrap();
}

fn should_deserialize_many_with_methods(methods: OpenStreamMethods) {
    let mut graph = Graph::default();
    let roots: Vec<_> = (1..=10)
        .map(|x| {
            graph.insert(record(
                "Tuple<int,double>",
                vec![Value::Integer(x), Value::Float(x as f64 + 0.5)],
            ))
        })
        .collect();
    let mut reader = open_stream(&graph, &roots, References::Preserve, false, methods);
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
    methods: OpenStreamMethods,
) -> ObjectReader<'static, 'static, Cursor<Vec<u8>>> {
    let mut stream = Cursor::new(Vec::new());
    let mut writer = ObjectWriter::new_weak(&mut stream, true).with_type_stamps(STAMPS);
    if methods.write {
        writer = writer.with_generated_records(OPEN_STREAM_CODECS);
    }
    for &root in roots {
        writer.write_weak_object(graph, root).unwrap();
    }
    writer.finish().unwrap();
    stream.seek(SeekFrom::Start(0)).unwrap();
    let reader = ObjectReader::new(stream, References::Weak, true).with_type_stamps(STAMPS);
    if methods.read {
        reader.with_generated_records(OPEN_STREAM_CODECS)
    } else {
        reader
    }
}

fn basic_open_stream_weak_with_methods(methods: OpenStreamMethods) {
    let (graph, roots) = tuples();
    let mut reader = weak_open_stream(&graph, &roots, methods);
    let objects = roots.map(|_| reader.read_weak_object().unwrap());
    for (expected, actual) in roots.into_iter().zip(objects) {
        assert_eq!(graph.get(expected), actual.get(actual.id()));
    }
    reader.finish().unwrap();
}

fn should_preserve_weak_references_with_methods(methods: OpenStreamMethods) {
    let (graph, roots) = tuples();
    let mut reader = weak_open_stream(&graph, &roots, methods);
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
    let mut reader = weak_open_stream(&graph, &[root, root, root], OpenStreamMethods::default());
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

fn box_with_lazy_scan(methods: impl Into<GraphMethods>) {
    let mut graph = Graph::default();
    let text = graph.insert(string("Something"));
    let root = graph.insert(record("Box", vec![Value::Ref(text)]));
    let (reader, copy) = methods.into().clone(&graph, root);
    let field = &fields(reader.graph().get(copy).unwrap())[0];
    assert_eq!(reader.graph().get(reference(field)), graph.get(text));
}

fn field_with_different_formal_and_actual_type(methods: impl Into<GraphMethods>) {
    let mut graph = Graph::default();
    let inner = graph.insert(record("Box", vec![Value::Null]));
    let root = graph.insert(record("Box", vec![Value::Ref(inner)]));
    let (reader, copy) = methods.into().clone(&graph, root);
    let field = &fields(reader.graph().get(copy).unwrap())[0];
    let Some(Object::Record { class, .. }) = reader.graph().get(reference(field)) else {
        panic!("expected the concrete record type");
    };
    assert_eq!(class, "Box");
}

fn box_with_long(methods: impl Into<GraphMethods>) {
    let mut graph = Graph::default();
    let root = graph.insert(record("GenericBox<long>", vec![Value::Integer(1234)]));
    let (reader, copy) = methods.into().clone(&graph, root);
    assert_eq!(
        fields(reader.graph().get(copy).unwrap())[0],
        fields(graph.get(root).unwrap())[0]
    );
}

fn string_tuple(methods: impl Into<GraphMethods>) {
    let mut graph = Graph::default();
    let contents = ["One", "Two", "Three"];
    let ids = contents.map(|text| Value::Ref(graph.insert(string(text))));
    let root = graph.insert(record("Tuple<string,string,string>", ids.to_vec()));
    let (reader, copy) = methods.into().clone(&graph, root);
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

fn nullable_field(methods: impl Into<GraphMethods>, value: Option<i32>) {
    let mut graph = Graph::default();
    let element = value.map_or(Value::Null, |value| Value::Integer(i64::from(value)));
    let root = graph.insert(record("GenericBox<int?>", vec![element]));
    let (reader, copy) = methods.into().clone(&graph, root);
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

fn boxed_nullable_field(methods: impl Into<GraphMethods>, value: Option<i32>) {
    let mut graph = Graph::default();
    let element = value.map_or(Value::Null, |value| boxed_integer(&mut graph, value));
    let root = graph.insert(record("Box", vec![element.clone()]));
    let (reader, copy) = methods.into().clone(&graph, root);
    let actual = &fields(reader.graph().get(copy).unwrap())[0];
    match element {
        Value::Ref(id) => assert_eq!(reader.graph().get(reference(actual)), graph.get(id)),
        Value::Null => assert_eq!(*actual, Value::Null),
        _ => unreachable!(),
    }
}

fn non_null_nullable(methods: impl Into<GraphMethods>) {
    nullable_field(methods, Some(3));
}
fn null_nullable(methods: impl Into<GraphMethods>) {
    nullable_field(methods, None);
}
// The upstream method names are reversed relative to their inputs.
fn boxed_null_nullable(methods: impl Into<GraphMethods>) {
    boxed_nullable_field(methods, Some(66));
}
fn boxed_non_null_nullable(methods: impl Into<GraphMethods>) {
    boxed_nullable_field(methods, None);
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

fn primitive_list(methods: impl Into<GraphMethods>, range: std::ops::Range<i64>) {
    let mut graph = Graph::default();
    let elements: Vec<_> = range.map(Value::Integer).collect();
    let root = list(&mut graph, "List<int>", elements.clone());
    let (reader, root) = methods.into().clone(&graph, root);
    assert_eq!(list_values(reader.graph(), root, "List<int>"), elements);
}

fn list_of_lists(methods: impl Into<GraphMethods>) {
    let mut graph = Graph::default();
    let inputs = [[1, 2, 3], [97, 98, 99]].map(|values| values.map(Value::Integer));
    let roots = inputs
        .clone()
        .map(|values| Value::Ref(list(&mut graph, "List<int>", values.to_vec())));
    let root = list(&mut graph, "List<List<int>>", roots.to_vec());
    let (reader, copy) = methods.into().clone(&graph, root);
    let restored = list_values(reader.graph(), copy, "List<List<int>>");
    for index in 0..2 {
        assert_eq!(
            list_values(reader.graph(), reference(&restored[index]), "List<int>"),
            inputs[index]
        );
    }
}

fn list_self_identity(methods: impl Into<GraphMethods>) {
    let mut graph = Graph::default();
    let root = list(&mut graph, "List<Box>", vec![]);
    let first = graph.insert(record("Box", vec![Value::Ref(root)]));
    let second = graph.insert(record("Box", vec![Value::Ref(root)]));
    let backing = reference(&fields(graph.get(root).unwrap())[0]);
    *graph.get_mut(backing).unwrap() = Object::Array(vec![Value::Ref(first), Value::Ref(second)]);
    let (reader, copy) = methods.into().clone(&graph, root);
    let graph = reader.graph();
    let items = list_values(graph, copy, "List<Box>");
    let first = reference(&fields(graph.get(reference(&items[0])).unwrap())[0]);
    let second = reference(&fields(graph.get(reference(&items[1])).unwrap())[0]);
    assert_eq!(copy, first);
    assert_eq!(first, second);
}

fn boxed_primitive(methods: impl Into<GraphMethods>) {
    let mut graph = Graph::default();
    let three = boxed_integer(&mut graph, 3);
    let root = graph.insert(record("Box", vec![three]));
    let (reader, copy) = methods.into().clone(&graph, root);
    let element = reference(&fields(reader.graph().get(copy).unwrap())[0]);
    assert_eq!(
        reader.graph().get(element),
        Some(&record("System.Int32", vec![Value::Integer(3)]))
    );
}

fn boxed_identity(methods: impl Into<GraphMethods>) {
    let mut graph = Graph::default();
    let primitive = boxed_integer(&mut graph, 1234);
    let first = graph.insert(record("Box", vec![primitive.clone()]));
    let second = graph.insert(record("Box", vec![primitive]));
    let root = list(
        &mut graph,
        "List<Box>",
        vec![Value::Ref(first), Value::Ref(second)],
    );
    let (reader, copy) = methods.into().clone(&graph, root);
    let graph = reader.graph();
    let items = list_values(graph, copy, "List<Box>");
    let element =
        |index: usize| reference(&fields(graph.get(reference(&items[index])).unwrap())[0]);
    assert_eq!(element(0), element(1));
}

fn list_with_null(methods: impl Into<GraphMethods>) {
    let mut graph = Graph::default();
    let one = graph.insert(string("One"));
    let two = graph.insert(string("Two"));
    let root = list(
        &mut graph,
        "List<object>",
        vec![Value::Ref(one), Value::Null, Value::Ref(two)],
    );
    let (reader, copy) = methods.into().clone(&graph, root);
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

fn enum_values(methods: impl Into<GraphMethods>, class: &str, values: &[i64]) {
    let methods = methods.into();
    for &value in values {
        let mut graph = Graph::default();
        let original = record(class, vec![Value::Integer(value)]);
        let root = graph.insert(original.clone());
        let (reader, copy) = methods.clone(&graph, root);
        assert_eq!(reader.graph().get(copy), Some(&original));
    }
}

fn multiple_enum_values(methods: impl Into<GraphMethods>) {
    let methods = methods.into();
    enum_values(
        methods,
        "EnumLong",
        &[i64::MAX - 12345, i64::MAX - 12344, -1],
    );
    enum_values(methods, "EnumShort", &[6, 7, i64::from(i16::MIN) + 2]);
    enum_values(
        methods,
        "TestEnumFlags",
        &[1, 2, 4, 1 | 2, 1 | 2 | 4, 2 | 4],
    );
}

fn nullable_enum(methods: impl Into<GraphMethods>) {
    let mut graph = Graph::default();
    let root = graph.insert(record("ClassWithNullableEnum", vec![Value::Integer(1)]));
    let (reader, copy) = methods.into().clone(&graph, root);
    assert_eq!(
        fields(reader.graph().get(copy).unwrap())[0],
        Value::Integer(1)
    );
}

fn key_value_pair(methods: impl Into<GraphMethods>) {
    let mut graph = Graph::default();
    let root = graph.insert(record(
        "KeyValuePair<int,int>",
        vec![Value::Integer(12), Value::Integer(34)],
    ));
    let (reader, copy) = methods.into().clone(&graph, root);
    let restored = fields(reader.graph().get(copy).unwrap());
    assert_eq!(restored[0], Value::Integer(12));
    assert_eq!(restored[1], Value::Integer(34));
}

fn character_stack(methods: impl Into<GraphMethods>, boxed: bool) {
    let mut graph = Graph::default();
    let class = if boxed { "Stack" } else { "Stack<char>" };
    let input: Vec<_> = "It works!".encode_utf16().collect();
    let mut elements: Vec<_> = input
        .iter()
        .copied()
        .map(|character| {
            let value = Value::Integer(i64::from(character));
            if boxed {
                Value::Ref(graph.insert(record("System.Char", vec![value])))
            } else {
                value
            }
        })
        .collect();
    elements.reverse();
    let root = list(&mut graph, class, elements);
    let (reader, copy) = methods.into().clone(&graph, root);
    let graph = reader.graph();
    let actual: Vec<_> = list_values(graph, copy, class)
        .iter()
        .map(|value| {
            let value = if boxed {
                let Some(Object::Record { class, fields }) = graph.get(reference(value)) else {
                    panic!("expected a boxed character");
                };
                assert_eq!(class, "System.Char");
                &fields[0]
            } else {
                value
            };
            let Value::Integer(character) = value else {
                panic!("expected a character")
            };
            u16::try_from(*character).unwrap()
        })
        .collect();
    assert_eq!(actual, input.into_iter().rev().collect::<Vec<_>>());
}

stamp_modes!(simple_enum_plain, simple_enum_stamped, |s| enum_values(
    s,
    "SimpleEnum",
    &[1]
));
stamp_modes!(byte_enum_plain, byte_enum_stamped, |s| enum_values(
    s,
    "EnumByte",
    &[200]
));
stamp_modes!(enums_plain, enums_stamped, multiple_enum_values);
stamp_modes!(nullable_enum_plain, nullable_enum_stamped, nullable_enum);
stamp_modes!(key_value_pair_plain, key_value_pair_stamped, key_value_pair);
stamp_modes!(nongeneric_stack_plain, nongeneric_stack_stamped, |s| {
    character_stack(s, true)
});
stamp_modes!(generic_stack_plain, generic_stack_stamped, |s| {
    character_stack(s, false)
});

fn derived_class(methods: impl Into<GraphMethods>) {
    let mut graph = Graph::default();
    let root = graph.insert(record(
        "SimpleDerivedClass",
        vec![Value::Integer(1), Value::Integer(2)],
    ));
    let (reader, copy) = methods.into().clone(&graph, root);
    let Some(Object::Record { class, fields }) = reader.graph().get(copy) else {
        panic!("expected a derived object");
    };
    assert_eq!(class, "SimpleDerivedClass");
    assert_eq!(fields[1], Value::Integer(2));
    assert_eq!(fields[0], Value::Integer(1));
}

fn simpler_struct(methods: impl Into<GraphMethods>) {
    let mut graph = Graph::default();
    let input = record(
        "SimplerStruct",
        vec![Value::Integer(1_234_567), Value::Integer(543)],
    );
    let root = graph.insert(input.clone());
    let (reader, copy) = methods.into().clone(&graph, root);
    assert_eq!(reader.graph().get(copy), Some(&input));
}

fn struct_with_string(methods: impl Into<GraphMethods>, class_name: &str, integers: &[i64]) {
    let mut graph = Graph::default();
    let text = graph.insert(string("allman"));
    let mut values: Vec<_> = integers.iter().copied().map(Value::Integer).collect();
    values.push(Value::Ref(text));
    let root = graph.insert(record(class_name, values));
    let (reader, copy) = methods.into().clone(&graph, root);
    let Some(Object::Record { class, fields }) = reader.graph().get(copy) else {
        panic!("expected typed record");
    };
    assert_eq!(class, class_name);
    let (text, fields) = fields.split_last().unwrap();
    assert_eq!(
        fields,
        integers
            .iter()
            .copied()
            .map(Value::Integer)
            .collect::<Vec<_>>()
    );
    assert_eq!(string_value(reader.graph(), text), "allman");
}

stamp_modes!(derived_class_plain, derived_class_stamped, derived_class);
stamp_modes!(simpler_struct_plain, simpler_struct_stamped, simpler_struct);
stamp_modes!(simple_struct_plain, simple_struct_stamped, |s| {
    struct_with_string(s, "SimpleStruct", &[5])
});
stamp_modes!(complex_struct_plain, complex_struct_stamped, |s| {
    struct_with_string(s, "ComplexStruct", &[6, 5])
});
stamp_modes!(struct_in_class_plain, struct_in_class_stamped, |s| {
    struct_with_string(s, "GenericBox<SimpleStruct>", &[1])
});

fn new_custom_object(graph: &mut Graph) -> Value {
    let inner = graph.insert(record("System.Object", vec![]));
    Value::Ref(graph.insert(record("CustomObject", vec![Value::Ref(inner)])))
}

fn custom_identity(graph: &Graph, value: &Value) -> ObjectId {
    let id = reference(value);
    let Some(Object::Record { class, fields }) = graph.get(id) else {
        panic!("expected CustomObject");
    };
    assert_eq!(class, "CustomObject");
    let identity = reference(&fields[0]);
    assert_eq!(graph.get(identity), Some(&record("System.Object", vec![])));
    identity
}

fn custom_key_dictionary(stamped: bool) {
    let mut graph = Graph::default();
    let entries = ["test1", "test2", "test3"].map(|text| {
        (
            new_custom_object(&mut graph),
            Value::Ref(graph.insert(string(text))),
        )
    });
    let class = "Dictionary<CustomObject,string>";
    let root = dictionary(&mut graph, class, entries);
    let (reader, copy) = deep_clone(&graph, root, stamped);
    let graph = reader.graph();
    let restored: std::collections::HashMap<_, _> = dictionary_entries(graph, copy, class)
        .map(|(key, value)| (custom_identity(graph, key), string_value(graph, value)))
        .collect();
    assert_eq!(restored.len(), 3);
}

fn custom_item_set(stamped: bool) {
    let mut graph = Graph::default();
    let items = (0..3).map(|_| new_custom_object(&mut graph)).collect();
    let class = "HashSet<CustomObject>";
    let root = list(&mut graph, class, items);
    let (reader, copy) = deep_clone(&graph, root, stamped);
    let graph = reader.graph();
    let restored: std::collections::HashSet<_> = list_values(graph, copy, class)
        .iter()
        .map(|value| custom_identity(graph, value))
        .collect();
    assert_eq!(restored.len(), 3);
}

fn two_different_serializations(stamped: bool) {
    fn new_simple(graph: &mut Graph) -> ObjectId {
        graph.insert(record("SimpleClass", vec![Value::Integer(0), Value::Null]))
    }

    let mut graph = Graph::default();
    let first = new_simple(&mut graph);
    let second = new_simple(&mut graph);
    let root = graph.insert(record(
        "SimpleContainer",
        vec![Value::Ref(first), Value::Ref(second)],
    ));
    let (reader, copy) = deep_clone(&graph, root, stamped);
    assert!(reader.graph().get(copy).is_some());

    let mut graph = Graph::default();
    let simple = new_simple(&mut graph);
    let first = graph.insert(record("SimpleContainer", vec![Value::Null, Value::Null]));
    let second = graph.insert(record("SimpleContainer", vec![Value::Null, Value::Null]));
    let root = graph.insert(Object::Array(
        [simple, first, second].map(Value::Ref).to_vec(),
    ));
    let (reader, copy) = deep_clone(&graph, root, stamped);
    assert!(reader.graph().get(copy).is_some());
    let actual = array_values(reader.graph(), copy);
    for (value, expected) in
        actual
            .iter()
            .zip(["SimpleClass", "SimpleContainer", "SimpleContainer"])
    {
        let Some(Object::Record { class, .. }) = reader.graph().get(reference(value)) else {
            panic!("expected an object record");
        };
        assert_eq!(class, expected);
    }
    assert_eq!(actual.len(), 3);
}

stamp_modes!(
    custom_key_dictionary_plain,
    custom_key_dictionary_stamped,
    custom_key_dictionary
);
stamp_modes!(
    custom_item_set_plain,
    custom_item_set_stamped,
    custom_item_set
);
stamp_modes!(
    different_serializations_plain,
    different_serializations_stamped,
    two_different_serializations
);

fn box_with_guid(stamped: bool) {
    let mut graph = Graph::default();
    let id = super::migrant::Random(0x268a_31af_f145_0f51).guid();
    let root = graph.insert(record("GenericBox<Guid>", vec![Value::Guid(id)]));
    let (reader, copy) = deep_clone(&graph, root, stamped);
    assert_eq!(
        fields(reader.graph().get(copy).unwrap())[0],
        Value::Guid(id)
    );
}

fn class_with_guid(stamped: bool) {
    let mut graph = Graph::default();
    let mut random = super::migrant::Random(0x67f9_e71b_25da_1d98);
    let id = random.guid();
    let number = i64::from(id.0[1]) * i64::from(id.0[0]);
    let text: String = std::iter::from_fn(|| loop {
        let character = char::from(random.next(256) as u8);
        if !character.is_control() {
            return Some(character);
        }
    })
    .take((number / 256) as usize)
    .collect();
    let str_id = graph.insert(string(&text));
    let root = graph.insert(record(
        "ClassWithGuid",
        vec![Value::Guid(id), Value::Integer(number), Value::Ref(str_id)],
    ));
    let (reader, copy) = deep_clone(&graph, root, stamped);
    let graph = reader.graph();
    let Some(Object::Record { class, fields }) = graph.get(copy) else {
        panic!("expected ClassWithGuid");
    };
    assert_eq!(class, "ClassWithGuid");
    assert_eq!(fields[0], Value::Guid(id));
    assert_eq!(fields[1], Value::Integer(number));
    assert_eq!(string_value(graph, &fields[2]), text);
}

stamp_modes!(box_with_guid_plain, box_with_guid_stamped, box_with_guid);
stamp_modes!(
    class_with_guid_plain,
    class_with_guid_stamped,
    class_with_guid
);

fn big_integer_inputs() -> [Value; 3] {
    let positive = ((1u128 << 64) + 5).to_le_bytes()[..9].to_vec();

    // Build 3^100 without narrowing through a machine integer.
    let mut magnitude = vec![1u8];
    for _ in 0..100 {
        let mut carry = 0;
        for byte in &mut magnitude {
            let product = u16::from(*byte) * 3 + carry;
            *byte = product as u8;
            carry = product >> 8;
        }
        if carry != 0 {
            magnitude.push(carry as u8);
        }
    }
    if magnitude.last().unwrap() & 0x80 != 0 {
        magnitude.push(0);
    }
    // Negate the signed fixed-width representation, least significant byte first.
    let mut carry = 1u16;
    for byte in &mut magnitude {
        carry += u16::from(!*byte);
        *byte = carry as u8;
        carry >>= 8;
    }
    [
        Value::BigInteger(positive),
        Value::BigInteger(magnitude),
        Value::BigInteger(vec![0]),
    ]
}

fn big_integer_round_trip(stamped: bool) {
    let inputs = big_integer_inputs();
    for value in &inputs {
        let mut graph = Graph::default();
        let root = graph.insert(record("GenericBox<BigInteger>", vec![value.clone()]));
        let (reader, copy) = deep_clone(&graph, root, stamped);
        assert_eq!(fields(reader.graph().get(copy).unwrap())[0], *value);

        let mut graph = Graph::default();
        let boxed = graph.insert(record("System.Numerics.BigInteger", vec![value.clone()]));
        let root = graph.insert(record("Box", vec![Value::Ref(boxed)]));
        let (reader, copy) = deep_clone(&graph, root, stamped);
        let graph = reader.graph();
        let boxed = reference(&fields(graph.get(copy).unwrap())[0]);
        assert_eq!(
            graph.get(boxed),
            Some(&record("System.Numerics.BigInteger", vec![value.clone()]))
        );
    }

    let mut graph = Graph::default();
    let root = graph.insert(Object::Array(inputs.to_vec()));
    let (reader, copy) = deep_clone(&graph, root, stamped);
    assert_eq!(array_values(reader.graph(), copy), inputs);
}

stamp_modes!(
    big_integer_plain,
    big_integer_stamped,
    big_integer_round_trip
);

const TRANSIENTS: Transients<'static> = Transients {
    classes: &["TransientClass"],
    fields: &[(
        "ClassWithTransientField",
        &[(1, FieldInitializer::Default(Value::Integer(0)))],
    )],
};

fn omit_transient_class(stamped: bool) {
    let mut graph = Graph::default();
    let omitted = graph.insert(record("TransientClass", vec![Value::Null]));
    let root = graph.insert(record("Box", vec![Value::Ref(omitted)]));
    let (reader, copy) = deep_clone_transient(&graph, root, stamped, Some(&TRANSIENTS));
    // The C# assertion is successful completion; inspect the omitted value as well.
    assert_eq!(fields(reader.graph().get(copy).unwrap()), [Value::Null]);
}

fn omit_transient_field(stamped: bool) {
    let mut graph = Graph::default();
    let root = graph.insert(record(
        "ClassWithTransientField",
        [147, 256, 850].map(Value::Integer).to_vec(),
    ));
    let (reader, copy) = deep_clone_transient(&graph, root, stamped, Some(&TRANSIENTS));
    let actual = fields(reader.graph().get(copy).unwrap());
    assert_eq!(
        actual,
        [Value::Integer(147), Value::Integer(0), Value::Integer(850)]
    );
}

fn reject_unserializable_root(stamped: bool, transient: bool) {
    let mut graph = Graph::default();
    let root = transient.then(|| graph.insert(record("TransientClass", vec![Value::Null])));
    let mut stream = Cursor::new(Vec::new());
    let mut writer = ObjectWriter::new(&mut stream, &graph, References::Preserve, true)
        .with_transients(&TRANSIENTS);
    if stamped {
        writer = writer.with_type_stamps(STAMPS);
    }
    assert_eq!(
        writer.write_root(root).unwrap_err().kind(),
        std::io::ErrorKind::InvalidInput
    );
    drop(writer);
    assert_eq!(stream.position(), 0);
}

stamp_modes!(
    omit_transient_class_plain,
    omit_transient_class_stamped,
    omit_transient_class
);
stamp_modes!(
    omit_transient_field_plain,
    omit_transient_field_stamped,
    omit_transient_field
);
stamp_modes!(reject_null_root_plain, reject_null_root_stamped, |s| {
    reject_unserializable_root(s, false)
});
stamp_modes!(
    reject_transient_root_plain,
    reject_transient_root_stamped,
    |s| reject_unserializable_root(s, true)
);

fn reject_resource(stamped: bool, class: &str, resource: NativeResource) {
    let mut graph = Graph::default();
    let root = graph.insert(record(class, vec![Value::NativeResource(resource)]));
    let mut stream = Cursor::new(Vec::new());
    let mut writer = ObjectWriter::new(&mut stream, &graph, References::Preserve, true);
    if stamped {
        writer = writer.with_type_stamps(STAMPS);
    }
    let error = writer.write_object(root).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::Unsupported);
    assert_eq!(
        error
            .get_ref()
            .and_then(|error| error.downcast_ref::<NonSerializableResource>()),
        Some(&NonSerializableResource {
            resource,
            path: vec![class.into()]
        })
    );
    drop(writer);
    assert_eq!(stream.position(), 0);
}

stamp_modes!(reject_intptr_plain, reject_intptr_stamped, |s| {
    reject_resource(s, "ClassWithIntPtr", NativeResource::IntPtr(0x666))
});
stamp_modes!(reject_pointer_plain, reject_pointer_stamped, |s| {
    reject_resource(s, "ClassWithPointer", NativeResource::Pointer(0))
});
stamp_modes!(reject_spinlock_plain, reject_spinlock_stamped, |s| {
    reject_resource(s, "ClassWithSpinLock", NativeResource::SpinLock)
});
stamp_modes!(
    reject_thread_local_plain,
    reject_thread_local_stamped,
    |s| reject_resource(s, "ClassWithThreadLocal", NativeResource::ThreadLocal)
);

fn native_resource_path(stamped: bool) {
    let mut graph = Graph::default();
    let pointer = graph.insert(record(
        "ClassWithIntPtr",
        vec![Value::NativeResource(NativeResource::IntPtr(0))],
    ));
    let middle = graph.insert(record("AnotherContainingType", vec![Value::Ref(pointer)]));
    let root = graph.insert(record(
        "GenericBox<AnotherContainingType>",
        vec![Value::Ref(middle)],
    ));
    let mut stream = Cursor::new(Vec::new());
    let mut writer = ObjectWriter::new(&mut stream, &graph, References::Preserve, true);
    if stamped {
        writer = writer.with_type_stamps(STAMPS);
    }
    let error = writer.write_object(root).unwrap_err();
    let resource = error
        .get_ref()
        .and_then(|error| error.downcast_ref::<NonSerializableResource>())
        .unwrap();
    assert_eq!(
        resource.path,
        [
            "GenericBox<AnotherContainingType>",
            "AnotherContainingType",
            "ClassWithIntPtr"
        ]
    );
}

fn decimal_value(value: u32) -> Value {
    Value::Decimal(crate::migrant::primitive::Decimal {
        magnitude: [value, 0, 0],
        negative: false,
        scale: 0,
    })
}

fn issue_60(stamped: bool) {
    let mut graph = Graph::default();
    let inner = graph.insert(record(
        "Issue60InnerClass",
        vec![decimal_value(1), decimal_value(2)],
    ));
    let root = graph.insert(record(
        "Issue60OuterClass",
        vec![decimal_value(0), Value::Ref(inner)],
    ));
    let (reader, copy) = deep_clone(&graph, root, stamped);
    let graph = reader.graph();
    let restored = fields(graph.get(copy).unwrap());
    assert_eq!(restored[0], decimal_value(0));
    assert_eq!(
        fields(graph.get(reference(&restored[1])).unwrap()),
        [decimal_value(1), decimal_value(2)]
    );
}

stamp_modes!(
    native_resource_path_plain,
    native_resource_path_stamped,
    native_resource_path
);
stamp_modes!(issue_60_plain, issue_60_stamped, issue_60);

fn custom_record(graph: &mut Graph, class: &str, payload: Vec<u8>) -> ObjectId {
    let payload = graph.insert(Object::Bytes(payload));
    graph.insert(record(class, vec![Value::Ref(payload)]))
}

fn custom_bytes<'a>(graph: &'a Graph, root: ObjectId, expected_class: &str) -> &'a [u8] {
    let Some(Object::Record { class, fields }) = graph.get(root) else {
        panic!("expected custom record");
    };
    assert_eq!(class, expected_class);
    let Some(Object::Bytes(bytes)) = graph.get(reference(&fields[0])) else {
        panic!("expected custom payload");
    };
    bytes
}

struct SpeciallySerializable(Vec<usize>);

impl SpeciallySerializable {
    fn fill(random: &mut super::migrant::Random, length: usize) -> Self {
        Self((0..length).map(|_| random.next(256) as usize).collect())
    }

    fn data(&self) -> Vec<u8> {
        self.0.iter().map(|&word| word as u8).collect()
    }

    fn store(&self, graph: &mut Graph) -> ObjectId {
        let payload = crate::migrant::custom_payload::save(|writer| {
            writer.signed(self.0.len() as i64)?;
            for &word in &self.0 {
                writer.bytes(&[word as u8])?;
            }
            Ok(())
        })
        .unwrap();
        custom_record(graph, "SpeciallySerializable", payload)
    }

    fn restore(graph: &Graph, root: ObjectId) -> Self {
        let payload = custom_bytes(graph, root, "SpeciallySerializable");
        crate::migrant::custom_payload::load(payload, |reader| {
            let length = usize::try_from(reader.signed()?).map_err(std::io::Error::other)?;
            if length > payload.len() {
                return Err(std::io::ErrorKind::InvalidData.into());
            }
            let words = (0..length)
                .map(|_| reader.byte().map(usize::from))
                .collect::<std::io::Result<_>>()?;
            Ok(Self(words))
        })
        .unwrap()
    }
}

fn specially_serializable(stamped: bool) {
    let original =
        SpeciallySerializable::fill(&mut super::migrant::Random(0x7ebc_5aaa_33a6_a9f2), 100);
    let mut graph = Graph::default();
    let root = original.store(&mut graph);
    let (reader, copy) = deep_clone(&graph, root, stamped);
    assert_eq!(
        original.data(),
        SpeciallySerializable::restore(reader.graph(), copy).data()
    );
}

fn special_in_array(stamped: bool) {
    let mut random = super::migrant::Random(0xe4b7_395e_148f_a5c2);
    let first = SpeciallySerializable::fill(&mut random, 100);
    let last = SpeciallySerializable::fill(&mut random, 100);
    let mut graph = Graph::default();
    let first_id = first.store(&mut graph);
    let text = graph.insert(string("Some string"));
    let boxed = graph.insert(record("Box", vec![Value::Ref(first_id)]));
    let last_id = last.store(&mut graph);
    let root = graph.insert(Object::Array(
        [first_id, text, boxed, last_id].map(Value::Ref).to_vec(),
    ));
    let (reader, copy) = deep_clone(&graph, root, stamped);
    let graph = reader.graph();
    let items = array_values(graph, copy);
    let first_id = reference(&items[0]);
    let boxed = graph.get(reference(&items[2])).unwrap();
    let last_id = reference(&items[3]);
    assert_eq!(first_id, reference(&fields(boxed)[0]));
    assert_ne!(first_id, last_id);
    assert_eq!(
        first.data(),
        SpeciallySerializable::restore(graph, first_id).data()
    );
    assert_eq!(
        last.data(),
        SpeciallySerializable::restore(graph, last_id).data()
    );
}

fn special_in_struct(stamped: bool) {
    let original =
        SpeciallySerializable::fill(&mut super::migrant::Random(0xab60_45e8_39c1_87ef), 100);
    let mut graph = Graph::default();
    let special = original.store(&mut graph);
    let root = graph.insert(record(
        "StructGenericBox<SpeciallySerializable>",
        vec![Value::Ref(special)],
    ));
    let (reader, copy) = deep_clone(&graph, root, stamped);
    let graph = reader.graph();
    let special = reference(&fields(graph.get(copy).unwrap())[0]);
    assert_eq!(
        original.data(),
        SpeciallySerializable::restore(graph, special).data()
    );
}

fn detect_stream_shift(stamped: bool) {
    let mut graph = Graph::default();
    let payload = crate::migrant::custom_payload::save(|writer| {
        for i in 0..3 {
            writer.signed(666 + i)?;
        }
        Ok(())
    })
    .unwrap();
    let root = custom_record(&mut graph, "BadlySerializable", payload);
    let (reader, copy) = deep_clone(&graph, root, stamped);
    let payload = custom_bytes(reader.graph(), copy, "BadlySerializable");
    let result = crate::migrant::custom_payload::load(payload, |reader| {
        reader.signed()?;
        reader.signed()?;
        Ok(())
    });
    assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::InvalidData);
}

stamp_modes!(
    specially_serializable_plain,
    specially_serializable_stamped,
    specially_serializable
);
stamp_modes!(
    special_in_array_plain,
    special_in_array_stamped,
    special_in_array
);
stamp_modes!(
    special_in_struct_plain,
    special_in_struct_stamped,
    special_in_struct
);
stamp_modes!(
    detect_stream_shift_plain,
    detect_stream_shift_stamped,
    detect_stream_shift
);

fn nested_generic_array(stamped: bool) {
    let mut graph = Graph::default();
    let text = graph.insert(string("test"));
    let generic = graph.insert(record("GenericClass<string>", vec![Value::Ref(text)]));
    let array = graph.insert(Object::Array(vec![Value::Ref(generic)]));
    let root = graph.insert(record(
        "GenericClassWithNestedGenericArgument<string>",
        vec![Value::Ref(array)],
    ));
    let (reader, copy) = deep_clone(&graph, root, stamped);
    let graph = reader.graph();
    let restored = graph.get(copy).expect("expected restored object");
    let array = reference(&fields(restored)[0]);
    let item = graph
        .get(reference(&array_values(graph, array)[0]))
        .unwrap();
    assert_eq!(string_value(graph, &fields(item)[0]), "test");
}

fn list_of_interfaces(stamped: bool) {
    let mut graph = Graph::default();
    let variants = ["ImplementingInterfaceOne", "ImplementingInterfaceTwo"];
    let items = (0..100)
        .map(|i| Value::Ref(graph.insert(record(variants[i % 2], vec![]))))
        .collect();
    let entries = list(&mut graph, "List<IInterface>", items);
    let root = graph.insert(record(
        "ClassWithTheListOfInterfaces",
        vec![Value::Ref(entries)],
    ));
    let (reader, copy) = deep_clone(&graph, root, stamped);
    let graph = reader.graph();
    let restored = graph.get(copy).expect("expected restored object");
    let items = list_values(graph, reference(&fields(restored)[0]), "List<IInterface>");
    assert_eq!(items.len(), 100);
    for (i, item) in items.iter().enumerate() {
        assert_eq!(
            graph.get(reference(item)),
            Some(&record(variants[i % 2], vec![]))
        );
    }
}

stamp_modes!(
    nested_generic_array_plain,
    nested_generic_array_stamped,
    nested_generic_array
);
stamp_modes!(
    list_of_interfaces_plain,
    list_of_interfaces_stamped,
    list_of_interfaces
);

fn manual_reset_event(signaled: bool) -> Value {
    Value::NativeResource(NativeResource::ManualResetEvent { signaled })
}

fn nested_struct(condition: bool) -> Value {
    Value::Integer(if condition { 5 } else { 0 })
}

const CONSTRUCTOR_FIELDS: Transients<'static> = Transients {
    classes: &[],
    fields: &[
        (
            "StructWithConstructorAttribute",
            &[(
                0,
                FieldInitializer::Constructor(|| manual_reset_event(false)),
            )],
        ),
        (
            "StructWithConstructorAttributeOnNestedStruct",
            &[(0, FieldInitializer::Constructor(|| nested_struct(true)))],
        ),
        (
            "WithConstructorAttribute",
            &[(
                0,
                FieldInitializer::Constructor(|| manual_reset_event(false)),
            )],
        ),
    ],
};

fn constructor_for_event_field(stamped: bool, class: &str) {
    let mut graph = Graph::default();
    let root = graph.insert(record(class, vec![Value::Null]));
    let (reader, copy) = deep_clone_transient(&graph, root, stamped, Some(&CONSTRUCTOR_FIELDS));
    assert!(matches!(
        fields(reader.graph().get(copy).unwrap())[0],
        Value::NativeResource(NativeResource::ManualResetEvent { .. })
    ));
}

fn constructor_for_nested_field(stamped: bool) {
    let mut graph = Graph::default();
    let root = graph.insert(record(
        "StructWithConstructorAttributeOnNestedStruct",
        vec![Value::Integer(0)],
    ));
    let (reader, copy) = deep_clone_transient(&graph, root, stamped, Some(&CONSTRUCTOR_FIELDS));
    assert_eq!(
        fields(reader.graph().get(copy).unwrap())[0],
        Value::Integer(5)
    );
}

stamp_modes!(
    constructor_in_struct_plain,
    constructor_in_struct_stamped,
    |s| constructor_for_event_field(s, "StructWithConstructorAttribute")
);
stamp_modes!(
    constructor_in_class_plain,
    constructor_in_class_stamped,
    |s| constructor_for_event_field(s, "WithConstructorAttribute")
);
stamp_modes!(
    constructor_nested_plain,
    constructor_nested_stamped,
    constructor_for_nested_field
);

fn generic_late_hook(stamped: bool) {
    const CLASS: &str = "GenericClassWithLatePostDeserializationHook";
    const TRANSIENTS: Transients<'static> = Transients {
        classes: &[],
        fields: &[(CLASS, &[(0, FieldInitializer::Default(Value::Integer(0)))])],
    };
    const HOOKS: &[LateRecordHook<'static>] = &[LateRecordHook {
        class: CLASS,
        apply: |fields| {
            if fields[0] == Value::Integer(0) {
                fields[0] = Value::Integer(147);
            }
            Ok(())
        },
    }];
    let mut graph = Graph::default();
    let root = graph.insert(record(CLASS, vec![Value::Integer(150)]));
    let (reader, copy) = deep_clone_with_hooks(&graph, root, stamped, Some(&TRANSIENTS), HOOKS);
    assert_eq!(
        fields(reader.graph().get(copy).unwrap())[0],
        Value::Integer(147)
    );
}

stamp_modes!(
    generic_late_hook_plain,
    generic_late_hook_stamped,
    generic_late_hook
);

#[test]
fn should_preserve_references() {
    should_preserve_references_with_methods(OpenStreamMethods::default());
}

#[test]
fn shouldnt_preserve_references() {
    shouldnt_preserve_references_with_methods(OpenStreamMethods::default());
}

#[test]
fn should_deserialize_many() {
    should_deserialize_many_with_methods(OpenStreamMethods::default());
}

#[test]
fn basic_open_stream_weak() {
    basic_open_stream_weak_with_methods(OpenStreamMethods::default());
}

#[test]
fn should_preserve_weak_references() {
    should_preserve_weak_references_with_methods(OpenStreamMethods::default());
}

mod open_generated_reader {
    use super::*;
    const METHOD: OpenStreamMethods = OpenStreamMethods {
        write: false,
        read: true,
    };
    #[test]
    fn basic_do_not_preserve() {
        basic_open_stream(References::PerRoot, METHOD);
    }
    #[test]
    fn basic_preserve() {
        basic_open_stream(References::Preserve, METHOD);
    }
    #[test]
    fn should_preserve_references() {
        should_preserve_references_with_methods(METHOD);
    }
    #[test]
    fn shouldnt_preserve_references() {
        shouldnt_preserve_references_with_methods(METHOD);
    }
    #[test]
    fn should_deserialize_many() {
        should_deserialize_many_with_methods(METHOD);
    }
    #[test]
    fn basic_open_stream_weak() {
        basic_open_stream_weak_with_methods(METHOD);
    }
    #[test]
    fn should_preserve_weak_references() {
        should_preserve_weak_references_with_methods(METHOD);
    }
}

mod open_generated_writer {
    use super::*;
    const METHOD: OpenStreamMethods = OpenStreamMethods {
        write: true,
        read: false,
    };
    #[test]
    fn basic_do_not_preserve() {
        basic_open_stream(References::PerRoot, METHOD);
    }
    #[test]
    fn basic_preserve() {
        basic_open_stream(References::Preserve, METHOD);
    }
    #[test]
    fn should_preserve_references() {
        should_preserve_references_with_methods(METHOD);
    }
    #[test]
    fn shouldnt_preserve_references() {
        shouldnt_preserve_references_with_methods(METHOD);
    }
    #[test]
    fn should_deserialize_many() {
        should_deserialize_many_with_methods(METHOD);
    }
    #[test]
    fn basic_open_stream_weak() {
        basic_open_stream_weak_with_methods(METHOD);
    }
    #[test]
    fn should_preserve_weak_references() {
        should_preserve_weak_references_with_methods(METHOD);
    }
}

mod open_generated_both {
    use super::*;
    const METHOD: OpenStreamMethods = OpenStreamMethods {
        write: true,
        read: true,
    };
    #[test]
    fn basic_do_not_preserve() {
        basic_open_stream(References::PerRoot, METHOD);
    }
    #[test]
    fn basic_preserve() {
        basic_open_stream(References::Preserve, METHOD);
    }
    #[test]
    fn should_preserve_references() {
        should_preserve_references_with_methods(METHOD);
    }
    #[test]
    fn shouldnt_preserve_references() {
        shouldnt_preserve_references_with_methods(METHOD);
    }
    #[test]
    fn should_deserialize_many() {
        should_deserialize_many_with_methods(METHOD);
    }
    #[test]
    fn basic_open_stream_weak() {
        basic_open_stream_weak_with_methods(METHOD);
    }
    #[test]
    fn should_preserve_weak_references() {
        should_preserve_weak_references_with_methods(METHOD);
    }
}

macro_rules! configurations {
    ($module:ident, $case:expr) => {
        mod $module {
            use super::*;
            #[test]
            fn reader_plain() {
                ($case)(GraphMethods {
                    stamped: false,
                    write: false,
                    read: true,
                });
            }
            #[test]
            fn reader_stamped() {
                ($case)(GraphMethods {
                    stamped: true,
                    write: false,
                    read: true,
                });
            }
            #[test]
            fn writer_plain() {
                ($case)(GraphMethods {
                    stamped: false,
                    write: true,
                    read: false,
                });
            }
            #[test]
            fn writer_stamped() {
                ($case)(GraphMethods {
                    stamped: true,
                    write: true,
                    read: false,
                });
            }
            #[test]
            fn both_plain() {
                ($case)(GraphMethods {
                    stamped: false,
                    write: true,
                    read: true,
                });
            }
            #[test]
            fn both_stamped() {
                ($case)(GraphMethods {
                    stamped: true,
                    write: true,
                    read: true,
                });
            }
        }
    };
}
mod serial_generated_graph {
    use super::*;

    configurations!(object, should_serialize_object_with_methods);
    configurations!(simple_class, should_serialize_simple_class_with_methods);
    configurations!(
        cyclic_reference,
        should_serialize_cyclic_reference_with_methods
    );
    configurations!(
        reference_to_self,
        should_serialize_reference_to_self_with_methods
    );
    configurations!(null_reference, should_serialize_null_reference_with_methods);
    configurations!(preserve_identity, should_preserve_identity_with_methods);
    configurations!(readonly_field, should_serialize_readonly_field_with_methods);
    configurations!(deep_graph, should_serialize_deep_graph_with_methods);
}

mod serial_generated_values {
    use super::*;
    configurations!(simple_enum, |m| enum_values(m, "SimpleEnum", &[1]));
    configurations!(byte_enum, |m| enum_values(m, "EnumByte", &[200]));
    configurations!(enums, multiple_enum_values);
    configurations!(nullable_enum, nullable_enum);
    configurations!(key_value_pair, key_value_pair);
    configurations!(nongeneric_stack, |m| character_stack(m, true));
    configurations!(generic_stack, |m| character_stack(m, false));
    configurations!(derived_class, derived_class);
    configurations!(simpler_struct, simpler_struct);
    configurations!(simple_struct, |m| struct_with_string(
        m,
        "SimpleStruct",
        &[5]
    ));
    configurations!(complex_struct, |m| struct_with_string(
        m,
        "ComplexStruct",
        &[6, 5]
    ));
}

mod serial_generated_boxes_lists {
    use super::*;
    configurations!(lazy_box, box_with_lazy_scan);
    configurations!(actual_type, field_with_different_formal_and_actual_type);
    configurations!(long_box, box_with_long);
    configurations!(tuple, string_tuple);
    configurations!(non_null_nullable, non_null_nullable);
    configurations!(null_nullable, null_nullable);
    configurations!(boxed_null_nullable, boxed_null_nullable);
    configurations!(boxed_non_null_nullable, boxed_non_null_nullable);
    configurations!(boxed_primitive, boxed_primitive);
    configurations!(boxed_identity, boxed_identity);
    configurations!(struct_in_class, |m| struct_with_string(
        m,
        "GenericBox<SimpleStruct>",
        &[1]
    ));
    configurations!(primitive_list, |m| primitive_list(m, 1..4));
    configurations!(long_primitive_list, |m| primitive_list(m, 0..100_000));
    configurations!(empty_list, |m| primitive_list(m, 0..0));
    configurations!(list_of_lists, list_of_lists);
    configurations!(list_with_null, list_with_null);
    configurations!(list_identity, list_self_identity);
}
