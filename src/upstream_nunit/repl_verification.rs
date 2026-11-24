use super::*;

fn assert_error(source: &str, expected: &str) {
    let error = creation::create_objects(source, Path::new("."))
        .err()
        .expect("expected a parsing error");
    assert!(error.contains(&format!("{expected}:")), "{error}");
}

#[test]
fn should_fail_on_empty_entry() {
    assert_error(
        r#"
mock:"#,
        "EmptyEntry",
    );
}

#[test]
fn should_fail_on_not_existing_type() {
    assert_error(
        r#"
someDevice: NotExistingType"#,
        "TypeNotResolved",
    );
}

#[test]
fn should_fail_on_string_mismatch() {
    assert_error(
        r#"
external: Antmicro.Renode.UnitTests.Mocks.MockCPU
    Placeholder: 8
"#,
        "TypeMismatch",
    );
}

#[test]
fn should_fail_on_non_writable_property() {
    assert_error(
        r#"
external: Antmicro.Renode.UnitTests.Mocks.MockCPU
    Model: "abc""#,
        "PropertyNotWritable",
    );
}

#[test]
fn should_fail_on_numeric_enum_mismatch() {
    assert_error(
        r#"
external: Antmicro.Renode.UnitTests.Mocks.MockCPU
    EnumValue: "abc""#,
        "TypeMismatch",
    );
}

#[test]
fn should_fail_on_double_attribute() {
    assert_error(
        r#"
external: Antmicro.Renode.UnitTests.Mocks.MockCPU
    EnumValue: One
    EnumValue: Two"#,
        "PropertyOrCtorNameUsedMoreThanOnce",
    );
}

#[test]
fn should_fail_on_non_existing_property() {
    assert_error(
        r#"
external: Antmicro.Renode.UnitTests.Mocks.MockCPU
    NotExistingProperty: "value""#,
        "PropertyDoesNotExist",
    );
}

#[test]
fn should_fail_on_missing_reference() {
    assert_error(
        r#"
device: Antmicro.Renode.UnitTests.Mocks.MockCPU
    OtherCpu: unknown"#,
        "MissingReference",
    );
}

#[test]
fn should_process_one_object() {
    creation::create_objects(
        r#"
mock: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral"#,
        Path::new("."),
    )
    .unwrap();
}

#[test]
fn should_process_late_attach() {
    creation::create_objects(
        r#"
mock: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral

mock: @sysbus <0x0, +0x100>"#,
        Path::new("."),
    )
    .unwrap();
}

#[test]
fn should_process_without_type_name_in_first_entry() {
    creation::create_objects(
        r#"
mock: @sysbus <0x0, 0x1000>

mock: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral"#,
        Path::new("."),
    )
    .unwrap();
}

#[test]
fn should_fail_if_type_is_specified_second_time() {
    assert_error(
        r#"
mock: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral

mock: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral"#,
        "VariableAlreadyDeclared",
    );
}

#[test]
fn should_fail_on_registering_not_iperipheral() {
    assert_error(
        r#"
external: Antmicro.Renode.UnitTests.Mocks.MockExternal @ sysbus
"#,
        "NoUsableRegisterInterface",
    );
}

#[test]
fn should_check_type_of_areference() {
    assert_error(
        r#"
one: Antmicro.Renode.UnitTests.Mocks.MockCPU
    OtherCpu: two

two: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral"#,
        "TypeMismatch",
    );
}

#[test]
fn should_fail_on_missing_reference_in_registration_point() {
    assert_error(
        r#"
device: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral @ sysbus regPoint"#,
        "MissingReference",
    );
}

#[test]
fn should_fail_on_wrong_reference_registration_point_type() {
    assert_error(
        r#"
regPoint: Antmicro.Renode.UnitTests.Mocks.MockCPU
device: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral @ sysbus regPoint"#,
        "TypeMismatch",
    );
}

#[test]
fn should_fail_on_non_construable_registration_point() {
    assert_error(
        r#"
device: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral @ sysbus "something""#,
        "NoCtorForRegistrationPoint",
    );
}

#[test]
fn should_fail_on_non_existing_inline_type() {
    assert_error(
        r#"
external: Antmicro.Renode.UnitTests.Mocks.MockCPU
    OtherCpu: new NotExistingType"#,
        "TypeNotResolved",
    );
}

#[test]
fn should_fail_on_incorrect_inline_object_type() {
    assert_error(
        r#"
one: Antmicro.Renode.UnitTests.Mocks.MockCPU
    OtherCpu: new Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral"#,
        "TypeMismatch",
    );
}

#[test]
fn should_fail_on_incorrect_property_type_in_nested_inline_object() {
    assert_error(
        r#"
cpu: Antmicro.Renode.UnitTests.Mocks.MockCPU
    OtherCpu: new Antmicro.Renode.UnitTests.Mocks.MockCPU
        OtherCpu: new Antmicro.Renode.UnitTests.Mocks.MockCPU
            EnumValue: "string""#,
        "TypeMismatch",
    );
}

#[test]
fn should_set_property_of_inline_object() {
    let platform = creation::create_objects(
        r#"
cpu: Antmicro.Renode.UnitTests.Mocks.MockCPU @ sysbus
    OtherCpu: new Antmicro.Renode.UnitTests.Mocks.MockCPU
        OtherCpu: new Antmicro.Renode.UnitTests.Mocks.MockCPU
            Placeholder: "something""#,
        Path::new("."),
    )
    .unwrap();
    let cpu = platform.cpu("sysbus.cpu").unwrap();
    let nested = platform
        .other_cpu(platform.other_cpu(cpu).unwrap())
        .unwrap();
    assert_eq!(nested.placeholder.as_deref(), Some("something"));
}

#[test]
fn should_fail_on_wrong_object_registration_point_type() {
    assert_error(
        r#"
device: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral @ sysbus new Antmicro.Renode.UnitTests.Mocks.MockCPU { Placeholder: "abc" }"#,
        "TypeMismatch",
    );
}

#[test]
fn should_register_peripheral_with_many_registration_points() {
    let platform = creation::create_objects(
        r#"
peripheral: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral @{
    sysbus <0x100, +0x10>;
    sysbus <0x200, +0x20>} as "alias"
"#,
        Path::new("."),
    )
    .unwrap();
    let ranges = platform.peripheral_ranges("sysbus.alias").unwrap();
    assert_eq!(ranges[0], (0x100, 0x10));
    assert_eq!(ranges[1], (0x200, 0x20));
    assert_eq!(ranges.len(), 2);
}

#[test]
fn should_handle_alias() {
    let platform = creation::create_objects(
        r#"
cpu: Antmicro.Renode.UnitTests.Mocks.MockCPU @ sysbus as "otherName""#,
        Path::new("."),
    )
    .unwrap();
    assert!(platform.cpu("sysbus.otherName").is_some());
    assert!(platform.cpu("sysbus.cpu").is_none());
}

#[test]
fn should_update_property() {
    let platform = creation::create_objects(
        r#"
cpu: Antmicro.Renode.UnitTests.Mocks.MockCPU @ sysbus
    Placeholder: "one"
    EnumValue: TwoStateEnum.Two

cpu:
    Placeholder: "two""#,
        Path::new("."),
    )
    .unwrap();
    let cpu = platform.cpu("sysbus.cpu").unwrap();
    assert_eq!(cpu.placeholder.as_deref(), Some("two"));
    assert_eq!(cpu.enum_value, creation::TwoState::Two);
}

#[test]
fn should_handle_escaped_ouote_in_string() {
    let platform = creation::create_objects(
        r#"
cpu: Antmicro.Renode.UnitTests.Mocks.MockCPU @ sysbus
    Placeholder: "one with \"escaped\" quote\"""#,
        Path::new("."),
    )
    .unwrap();
    let cpu = platform.cpu("sysbus.cpu").unwrap();
    assert_eq!(
        cpu.placeholder.as_deref(),
        Some("one with \"escaped\" quote\"")
    );
}

#[test]
fn should_handle_none_in_property() {
    let platform = creation::create_objects(
        r#"
cpu: Antmicro.Renode.UnitTests.Mocks.MockCPU @ sysbus
    EnumValue: TwoStateEnum.Two

cpu:
    EnumValue: none"#,
        Path::new("."),
    )
    .unwrap();
    let cpu = platform.cpu("sysbus.cpu").unwrap();
    assert_eq!(cpu.enum_value, creation::TwoState::One);
}

#[test]
fn should_handle_multiline_quoted_string_as_value() {
    let platform = creation::create_objects(
        r#"
cpu: Antmicro.Renode.UnitTests.Mocks.MockCPU @ sysbus
    Placeholder: '''this is
multiline
string'''"#,
        Path::new("."),
    )
    .unwrap();
    let cpu = platform.cpu("sysbus.cpu").unwrap();
    assert_eq!(
        cpu.placeholder.as_deref(),
        Some("this is\nmultiline\nstring")
    );
}

#[test]
fn should_handle_multiple_multiline_quoted_strings() {
    let platform = creation::create_objects(
        r#"
cpu: Antmicro.Renode.UnitTests.Mocks.MockCPU @ sysbus
    Placeholder: '''this is a
multiline
string'''
cpu2: Antmicro.Renode.UnitTests.Mocks.MockCPU @ sysbus
    Placeholder: '''this is another
multilineeee
stringgggg'''"#,
        Path::new("."),
    )
    .unwrap();
    let cpu = platform.cpu("sysbus.cpu").unwrap();
    assert_eq!(
        cpu.placeholder.as_deref(),
        Some("this is a\nmultiline\nstring")
    );
    let cpu = platform.cpu("sysbus.cpu2").unwrap();
    assert_eq!(
        cpu.placeholder.as_deref(),
        Some("this is another\nmultilineeee\nstringgggg")
    );
}

#[test]
fn should_handle_multiline_quoted_string_in_one_line() {
    let platform = creation::create_objects(
        r#"
cpu: Antmicro.Renode.UnitTests.Mocks.MockCPU @ sysbus
    Placeholder: '''this is single line'''"#,
        Path::new("."),
    )
    .unwrap();
    let cpu = platform.cpu("sysbus.cpu").unwrap();
    assert_eq!(cpu.placeholder.as_deref(), Some("this is single line"));
}

#[test]
fn should_handle_escaped_quote_in_multiline_string() {
    let platform = creation::create_objects(
        r#"
cpu: Antmicro.Renode.UnitTests.Mocks.MockCPU @ sysbus
    Placeholder: '''this is \''' single line'''"#,
        Path::new("."),
    )
    .unwrap();
    let cpu = platform.cpu("sysbus.cpu").unwrap();
    assert_eq!(cpu.placeholder.as_deref(), Some("this is ''' single line"));
}

#[test]
fn should_handle_escaped_unescaped_single_quote_char_in_multiline_string() {
    let platform = creation::create_objects(
        r#"
cpu: Antmicro.Renode.UnitTests.Mocks.MockCPU @ sysbus
    Placeholder: ''' this is string with ' and '' '''"#,
        Path::new("."),
    )
    .unwrap();
    let cpu = platform.cpu("sysbus.cpu").unwrap();
    assert_eq!(
        cpu.placeholder.as_deref(),
        Some(" this is string with ' and '' ")
    );
}

#[test]
fn should_not_treat_double_slash_in_multiline_string_as_comment() {
    let platform = creation::create_objects(
        r#"
cpu: Antmicro.Renode.UnitTests.Mocks.MockCPU @ sysbus
    Placeholder: ''' // this is //
// not
a comment // neither is this
or //this
or// this
or this // '''"#,
        Path::new("."),
    )
    .unwrap();
    let cpu = platform.cpu("sysbus.cpu").unwrap();
    assert_eq!(
        cpu.placeholder.as_deref(),
        Some(
            " // this is //\n// not\na comment // neither is this\nor //this\nor// this\nor this // "
        )
    );
}

#[test]
fn should_process_valid_enum() {
    let platform = creation::create_objects(r#"
mockPeripheral: Antmicro.Renode.Tests.UnitTests.Mocks.MockPeripheralWithEnumAttribute @ sysbus <0, 1>
    mockEnum: MockEnum.ValidValue
"#, Path::new(".")).unwrap();
    let values = platform.enum_values("sysbus.mockPeripheral").unwrap();
    assert_eq!(values.0, 1);
}

#[test]
fn should_process_valid_shorthand_enum() {
    let platform = creation::create_objects(r#"
mockPeripheral: Antmicro.Renode.Tests.UnitTests.Mocks.MockPeripheralWithEnumAttribute @ sysbus <0, 1>
    mockEnum: .ValidValue
"#, Path::new(".")).unwrap();
    let values = platform.enum_values("sysbus.mockPeripheral").unwrap();
    assert_eq!(values.0, 1);
}

#[test]
fn should_fail_on_invalid_enum() {
    assert_error(
        r#"
peripheral: Antmicro.Renode.Tests.UnitTests.Mocks.MockPeripheralWithEnumAttribute
    mockEnum: MockEnum.InvalidValue
"#,
        "NoCtor",
    );
}

#[test]
fn should_process_valid_int_as_enum() {
    let platform = creation::create_objects(
        r#"
peripheral: Antmicro.Renode.Tests.UnitTests.Mocks.MockPeripheralWithEnumAttribute @ sysbus <0, 1>
    mockEnum: 1
"#,
        Path::new("."),
    )
    .unwrap();
    let values = platform.enum_values("sysbus.peripheral").unwrap();
    assert_eq!(values.0, 1);
}

#[test]
fn should_fail_on_invalid_int_to_enum_cast() {
    assert_error(
        r#"
peripheral: Antmicro.Renode.Tests.UnitTests.Mocks.MockPeripheralWithEnumAttribute
    mockEnum: 2
"#,
        "NoCtor",
    );
}

#[test]
fn should_fail_on_invalid_int_to_enum_with_attribute_cast() {
    assert_error(
        r#"
peripheral: Antmicro.Renode.Tests.UnitTests.Mocks.MockPeripheralWithEnumAttribute @ sysbus <0, 1>
    mockEnumWithAttribute: MockEnumWithAttribute.InvalidValue
"#,
        "NoCtor",
    );
}

#[test]
fn should_process_cast_of_any_int_to_enum_with_attribute() {
    let platform = creation::create_objects(
        r#"
peripheral: Antmicro.Renode.Tests.UnitTests.Mocks.MockPeripheralWithEnumAttribute @ sysbus <0, 1>
    mockEnumWithAttribute: 2
"#,
        Path::new("."),
    )
    .unwrap();
    let values = platform.enum_values("sysbus.peripheral").unwrap();
    assert_eq!(values.1, 2);
}

#[test]
fn should_fail_on_multiline_quoted_string_in_using() {
    let error = creation::create_objects(
        r#"
using '''this is
multiline
string '''"#,
        Path::new("."),
    )
    .err()
    .expect("expected syntax error");
    assert!(error.contains("SyntaxError:"), "{error}");
}

#[test]
fn should_fail_on_any_string_after_closed_multiline_string() {
    let error = creation::create_objects(
        r#"
cpu: Antmicro.Renode.UnitTests.Mocks.MockCPU @ sysbus
    Placeholder: '''this is
not a single line ''' xx"#,
        Path::new("."),
    )
    .err()
    .expect("expected syntax error");
    assert!(error.contains("SyntaxError:"), "{error}");
    assert_eq!(error.lines().nth(1), Some("At 4:23:"));
}

#[test]
fn should_fail_on_unclosed_multiline_string() {
    let error = creation::create_objects(
        r#"
cpu: Antmicro.Renode.UnitTests.Mocks.MockCPU @ sysbus
    Placeholder: '''this is "#,
        Path::new("."),
    )
    .err()
    .expect("expected syntax error");
    assert!(error.contains("SyntaxError:"), "{error}");
    assert_eq!(error.lines().nth(1), Some("At 3:17:"));
}

#[test]
fn should_fail_on_unclosed_multiple_multiline_quoted_strings() {
    let error = creation::create_objects(
        r#"
cpu: Antmicro.Renode.UnitTests.Mocks.MockCPU @ sysbus
    Placeholder: '''this is
multiline
string'''
cpu2: Antmicro.Renode.UnitTests.Mocks.MockCPU @ sysbus
    Placeholder: '''this is
multiline
string'''
cpu3: Antmicro.Renode.UnitTests.Mocks.MockCPU @ sysbus
    Placeholder: '''this is
multiline
string"#,
        Path::new("."),
    )
    .err()
    .expect("expected syntax error");
    assert!(error.contains("SyntaxError:"), "{error}");
    assert_eq!(error.lines().nth(1), Some("At 11:17:"));
}

#[test]
fn should_fail_on_multiple_multiline_string_signs_in_one_line() {
    let error = creation::create_objects(
        r#"
cpu: Antmicro.Renode.UnitTests.Mocks.MockCPU @ sysbus
    Placeholder: '''this is \''' ''' ''' '''
not a single line '''"#,
        Path::new("."),
    )
    .err()
    .expect("expected syntax error");
    assert!(error.contains("SyntaxError:"), "{error}");
    assert_eq!(error.lines().nth(1), Some("At 3:37:"));
}

#[test]
fn should_handle_empty_numerical_value() {
    let platform = creation::create_objects(r#"
mockPeripheral: Antmicro.Renode.Tests.UnitTests.Mocks.MockPeripheralWithNumericalAttrubute @ sysbus  <0, 1>
    mockInt: empty"#, Path::new(".")).unwrap();
    assert_eq!(
        platform.scalar("sysbus.mockPeripheral").unwrap(),
        &creation::ScalarValue::Integer(0)
    );
}

#[test]
fn should_handle_empty_string_value() {
    let platform = creation::create_objects(r#"
mockPeripheral: Antmicro.Renode.Tests.UnitTests.Mocks.MockPeripheralWithStringAttribute @ sysbus  <0, 1>
    mockString: empty"#, Path::new(".")).unwrap();
    assert_eq!(
        platform.scalar("sysbus.mockPeripheral").unwrap(),
        &creation::ScalarValue::String(None)
    );
}

#[test]
fn should_handle_empty_range_value() {
    let platform = creation::create_objects(r#"
mockPeripheral: Antmicro.Renode.Tests.UnitTests.Mocks.MockPeripheralWithRangeAttribute @ sysbus  <0, 1>
    mockRange: empty"#, Path::new(".")).unwrap();
    assert_eq!(
        platform.scalar("sysbus.mockPeripheral").unwrap(),
        &creation::ScalarValue::Range(0, 0)
    );
}

#[test]
fn should_handle_empty_object_value() {
    let platform = creation::create_objects(r#"
mockPeripheral: Antmicro.Renode.Tests.UnitTests.Mocks.MockPeripheralWithObjectAttribute @ sysbus  <0, 1>
    mockObject: empty"#, Path::new(".")).unwrap();
    assert_eq!(
        platform.scalar("sysbus.mockPeripheral").unwrap(),
        &creation::ScalarValue::Object(None)
    );
}

#[test]
fn should_handle_empty_enum_value() {
    let platform = creation::create_objects(
        r#"
cpu: Antmicro.Renode.UnitTests.Mocks.MockCPU @ sysbus
    EnumValue: empty"#,
        Path::new("."),
    )
    .unwrap();
    assert_eq!(
        platform.cpu("sysbus.cpu").unwrap().enum_value,
        creation::TwoState::One
    );
}

#[test]
fn should_handle_multiple_escaped_multiline_string_signs_in_one_line() {
    let platform = creation::create_objects(
        r#"
cpu: Antmicro.Renode.UnitTests.Mocks.MockCPU @ sysbus
    Placeholder: '''this is \''' \''' \''' \'''
not a single line '''"#,
        Path::new("."),
    )
    .unwrap();
    let cpu = platform.cpu("sysbus.cpu").unwrap();
    assert_eq!(
        cpu.placeholder.as_deref(),
        Some("this is ''' ''' ''' '''\nnot a single line ")
    );
}

#[test]
fn should_handle_empty_list_value() {
    let platform = creation::create_objects(r#"
mockPeripheral: Antmicro.Renode.Tests.UnitTests.Mocks.MockPeripheralWithCollectionAttributes @ sysbus  <0, 1>
    mockIntList: empty"#, Path::new(".")).unwrap();
    let value = platform.collections("sysbus.mockPeripheral").unwrap();
    assert_eq!(value.integers, None);
}

#[test]
fn should_handle_empty_array_value() {
    let platform = creation::create_objects(r#"
mockPeripheral: Antmicro.Renode.Tests.UnitTests.Mocks.MockPeripheralWithCollectionAttributes @ sysbus  <0, 1>
    mockIntArray: empty"#, Path::new(".")).unwrap();
    let value = platform.collections("sysbus.mockPeripheral").unwrap();
    assert_eq!(value.array, None);
}

#[test]
fn should_handle_empty_list_of_integers() {
    let platform = creation::create_objects(r#"
mockPeripheral: Antmicro.Renode.Tests.UnitTests.Mocks.MockPeripheralWithCollectionAttributes @ sysbus  <0, 1>
    mockIntList: []"#, Path::new(".")).unwrap();
    let value = platform.collections("sysbus.mockPeripheral").unwrap();
    assert_eq!(value.integers.as_deref(), Some([].as_slice()));
}

#[test]
fn should_handle_list_of_integers() {
    let platform = creation::create_objects(r#"
mockPeripheral: Antmicro.Renode.Tests.UnitTests.Mocks.MockPeripheralWithCollectionAttributes @ sysbus  <0, 1>
    mockIntList: [2, 1, 37]"#, Path::new(".")).unwrap();
    let value = platform.collections("sysbus.mockPeripheral").unwrap();
    assert_eq!(value.integers.as_deref(), Some([2, 1, 37].as_slice()));
}

#[test]
fn should_handle_array_of_integers() {
    let platform = creation::create_objects(r#"
mockPeripheral: Antmicro.Renode.Tests.UnitTests.Mocks.MockPeripheralWithCollectionAttributes @ sysbus  <0, 1>
    mockIntArray: [1, 2, 34]"#, Path::new(".")).unwrap();
    let value = platform.collections("sysbus.mockPeripheral").unwrap();
    assert_eq!(value.array.as_deref(), Some([1, 2, 34].as_slice()));
}

#[test]
fn should_handle_list_of_strings() {
    let platform = creation::create_objects(r#"
mockPeripheral: Antmicro.Renode.Tests.UnitTests.Mocks.MockPeripheralWithCollectionAttributes @ sysbus  <0, 1>
    mockStringList: ["a", "b", "c"]"#, Path::new(".")).unwrap();
    let value = platform.collections("sysbus.mockPeripheral").unwrap();
    assert_eq!(value.strings.as_ref().unwrap(), &["a", "b", "c"]);
}

#[test]
fn should_fail_on_type_mismatch_in_list() {
    assert_error(
        r#"
mockPeripheral: Antmicro.Renode.Tests.UnitTests.Mocks.MockPeripheralWithCollectionAttributes @ sysbus  <0, 1>
    mockIntList: [1, "b", 3]"#,
        "TypeMismatch",
    );
}

#[test]
fn should_fail_on_type_mismatch_in_array() {
    assert_error(
        r#"
mockPeripheral: Antmicro.Renode.Tests.UnitTests.Mocks.MockPeripheralWithCollectionAttributes @ sysbus  <0, 1>
    mockIntArray: [1, "b", 3]"#,
        "TypeMismatch",
    );
}

#[test]
fn should_replace_list() {
    let platform = creation::create_objects(r#"
mockPeripheral: Antmicro.Renode.Tests.UnitTests.Mocks.MockPeripheralWithCollectionAttributes @ sysbus  <0, 1>
    mockIntList: [1, 2, 3]

mockPeripheral:
    mockIntList: [4, 5]"#, Path::new(".")).unwrap();
    let value = platform.collections("sysbus.mockPeripheral").unwrap();
    assert_eq!(value.integers.as_deref(), Some([4, 5].as_slice()));
}

#[test]
fn should_handle_empty_dict_value() {
    let platform = creation::create_objects(r#"
mockPeripheral: Antmicro.Renode.Tests.UnitTests.Mocks.MockPeripheralWithCollectionAttributes @ sysbus  <0, 1>
    mockIntStringDict: {}"#, Path::new(".")).unwrap();
    let value = platform.collections("sysbus.mockPeripheral").unwrap();
    assert_eq!(value.dictionary.as_ref().unwrap().len(), 0);
}

#[test]
fn should_handle_empty_keyword_dict_value() {
    let platform = creation::create_objects(r#"
mockPeripheral: Antmicro.Renode.Tests.UnitTests.Mocks.MockPeripheralWithCollectionAttributes @ sysbus  <0, 1>
    mockIntStringDict: empty"#, Path::new(".")).unwrap();
    let value = platform.collections("sysbus.mockPeripheral").unwrap();
    assert_eq!(value.dictionary, None);
}

#[test]
fn should_handle_dict_int_string() {
    let platform = creation::create_objects(r#"
mockPeripheral: Antmicro.Renode.Tests.UnitTests.Mocks.MockPeripheralWithCollectionAttributes @ sysbus  <0, 1>
    mockIntStringDict: {0: "zero" ; 1: "one" }"#, Path::new(".")).unwrap();
    let value = platform.collections("sysbus.mockPeripheral").unwrap();
    let dict = value.dictionary.as_ref().unwrap();
    assert_eq!(dict.len(), 2);
    assert_eq!(dict[&0], "zero");
    assert_eq!(dict[&1], "one");
}

#[test]
fn should_handle_list_of_references() {
    let platform = creation::create_objects(r#"
cpu1: Antmicro.Renode.UnitTests.Mocks.MockCPU @ sysbus
cpu2: Antmicro.Renode.UnitTests.Mocks.MockCPU @ sysbus

mockPeripheral: Antmicro.Renode.Tests.UnitTests.Mocks.MockPeripheralWithCollectionAttributes @ sysbus  <0, 1>
    mockCpuList: [cpu1, cpu2]"#, Path::new(".")).unwrap();
    let value = platform.collections("sysbus.mockPeripheral").unwrap();
    let expected = [
        platform.cpu_id("sysbus.cpu1").unwrap(),
        platform.cpu_id("sysbus.cpu2").unwrap(),
    ];
    assert_eq!(value.cpu_list.as_deref(), Some(expected.as_slice()));
}

#[test]
fn should_handle_list_of_unordered_references() {
    let platform = creation::create_objects(r#"
cpu1: Antmicro.Renode.UnitTests.Mocks.MockCPU @ sysbus

mockPeripheral: Antmicro.Renode.Tests.UnitTests.Mocks.MockPeripheralWithCollectionAttributes @ sysbus  <0, 1>
    mockCpuList: [cpu1, cpu2]

cpu2: Antmicro.Renode.UnitTests.Mocks.MockCPU @ sysbus
    "#, Path::new(".")).unwrap();
    let value = platform.collections("sysbus.mockPeripheral").unwrap();
    let expected = [
        platform.cpu_id("sysbus.cpu1").unwrap(),
        platform.cpu_id("sysbus.cpu2").unwrap(),
    ];
    assert_eq!(value.cpu_list.as_deref(), Some(expected.as_slice()));
}

#[test]
fn should_handle_dict_of_unordered_references() {
    let platform = creation::create_objects(r#"
cpu1: Antmicro.Renode.UnitTests.Mocks.MockCPU @ sysbus

mockPeripheral: Antmicro.Renode.Tests.UnitTests.Mocks.MockPeripheralWithCollectionAttributes @ sysbus  <0, 1>
    mockIntCpuDict: {1: cpu1; 2: cpu2}

cpu2: Antmicro.Renode.UnitTests.Mocks.MockCPU @ sysbus
    "#, Path::new(".")).unwrap();
    let value = platform.collections("sysbus.mockPeripheral").unwrap();
    let cpu1 = platform.cpu_id("sysbus.cpu1").unwrap();
    let cpu2 = platform.cpu_id("sysbus.cpu2").unwrap();
    let dict = value.int_cpu.as_ref().unwrap();
    assert_eq!(dict.len(), 2);
    assert_eq!(dict[&1], cpu1);
    assert_eq!(dict[&2], cpu2);
}

#[test]
fn should_handle_dict_of_unordered_references_as_keys() {
    let platform = creation::create_objects(r#"
cpu1: Antmicro.Renode.UnitTests.Mocks.MockCPU @ sysbus

mockPeripheral: Antmicro.Renode.Tests.UnitTests.Mocks.MockPeripheralWithCollectionAttributes @ sysbus  <0, 1>
    mockCpuIntDict: {cpu1: 1; cpu2: 2}

cpu2: Antmicro.Renode.UnitTests.Mocks.MockCPU @ sysbus
    "#, Path::new(".")).unwrap();
    let value = platform.collections("sysbus.mockPeripheral").unwrap();
    let cpu1 = platform.cpu_id("sysbus.cpu1").unwrap();
    let cpu2 = platform.cpu_id("sysbus.cpu2").unwrap();
    let dict = value.cpu_int.as_ref().unwrap();
    assert_eq!(dict.len(), 2);
    assert_eq!(dict[&cpu1], 1);
    assert_eq!(dict[&cpu2], 2);
}

#[test]
fn should_fail_on_empty_keyword_as_type() {
    assert_error(
        r#"
mockPeripheral: empty @ sysbus <0, 1>
    value: 0"#,
        "SyntaxError",
    );
}

#[test]
fn should_fail_on_empty_keyword_as_registration_destination() {
    assert_error(
        r#"
mockPeripheral:  Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral @ empty <0, 1>
    value: 0"#,
        "SyntaxError",
    );
}

#[test]
fn should_fail_on_empty_keyword_as_parameter_name() {
    assert_error(
        r#"
mockPeripheral:  Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral @ sysbus <0, 1>
    empty: 0"#,
        "SyntaxError",
    );
}

#[test]
fn should_fail_on_empty_keyword_as_using_parameter() {
    assert_error(r#"using empty"#, "SyntaxError");
}

#[test]
fn should_handle_empty_bool_value() {
    let platform = creation::create_objects(r#"
mockPeripheral: Antmicro.Renode.Tests.UnitTests.Mocks.MockPeripheralWithBoolAttribute @ sysbus  <0, 1>
    mockBool: empty"#, Path::new(".")).unwrap();
    assert_eq!(
        platform.scalar("sysbus.mockPeripheral"),
        Some(&creation::ScalarValue::Boolean(false))
    );
}

#[test]
fn should_handle_empty_reference_attribute() {
    let platform = creation::create_objects(r#"
mockPeripheral: Antmicro.Renode.Tests.UnitTests.Mocks.MockPeripheralUsingReferenceAttribute @ sysbus <0, 1>
    mockReference: empty"#, Path::new(".")).unwrap();
    assert_eq!(
        platform.scalar("sysbus.mockPeripheral"),
        Some(&creation::ScalarValue::PeripheralReference(None))
    );
}

#[test]
fn should_handle_none_in_ctor_param() {
    let platform = creation::create_objects(
        r#"
peripheral: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral @ sysbus <0, 1>
    value: 1

peripheral:
    value: none"#,
        Path::new("."),
    )
    .unwrap();
    assert_eq!(
        platform.empty_peripheral_counter("sysbus.peripheral"),
        Some(1)
    );
}

#[test]
fn should_fail_on_alias_without_registration_info() {
    assert_error(
        r#"
peripheral: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral as "alias""#,
        "AliasWithoutRegistration",
    );
}

#[test]
fn should_fail_on_alias_with_none_registration_info() {
    assert_error(
        r#"
peripheral: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral @ none as "alias""#,
        "AliasWithNoneRegistration",
    );
}

#[test]
fn should_fail_with_constructor_attributes_with_non_creating_entry() {
    assert_error(
        r#"
sysbus:
    x: 5"#,
        "CtorAttributesInNonCreatingEntry",
    );
}

#[test]
fn should_fail_on_non_existing_constructor() {
    assert_error(
        r#"
p: Antmicro.Renode.UnitTests.Mocks.MockPeripheralWithProtectedConstructor @ sysbus 0x0
"#,
        "NoCtor",
    );
}

#[test]
fn should_validate_object_value_in_ctor_parameter() {
    assert_error(
        r#"
device: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral
    ctorParam: new NoSuchType"#,
        "TypeNotResolved",
    );
}

#[test]
fn should_fail_on_no_available_ctor_in_entry() {
    assert_error(
        r#"
device: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral
    ctorParam: 3"#,
        "NoCtor",
    );
}

#[test]
fn should_fail_on_ambiguous_ctor_in_entry() {
    assert_error(
        r#"
device: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral
    value: 3"#,
        "AmbiguousCtor",
    );
}

#[test]
fn should_fail_on_no_available_ctor_in_object_value() {
    assert_error(
        r#"
peripheral: Antmicro.Renode.UnitTests.Mocks.MockPeripheralWithDependency
    other: new Antmicro.Renode.UnitTests.Mocks.MockPeripheralWithDependency
        other: new Antmicro.Renode.UnitTests.Mocks.MockPeripheralWithDependency
            x: 7"#,
        "NoCtor",
    );
}

#[test]
fn should_fail_on_ambiguous_ctor_in_object_value() {
    assert_error(
        r#"
peripheral: Antmicro.Renode.UnitTests.Mocks.MockPeripheralWithDependency
    other: new Antmicro.Renode.UnitTests.Mocks.MockPeripheralWithDependency
        other: new Antmicro.Renode.UnitTests.Mocks.MockPeripheralWithDependency
            other: new Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral
                value: 4"#,
        "AmbiguousCtor",
    );
}

#[test]
fn should_find_cyclic_dependency() {
    assert_error(
        r#"
peri1: Antmicro.Renode.UnitTests.Mocks.MockPeripheralWithDependency
    other: new Antmicro.Renode.UnitTests.Mocks.MockPeripheralWithDependency
        other: peri2

peri3: Antmicro.Renode.UnitTests.Mocks.MockPeripheralWithDependency
    other: peri4

peri4: Antmicro.Renode.UnitTests.Mocks.MockPeripheralWithDependency
    other: peri1

peri2: Antmicro.Renode.UnitTests.Mocks.MockPeripheralWithDependency
    other: peri3
"#,
        "CreationOrderCycle",
    );
}

#[test]
fn should_find_cyclic_reference_to_itself() {
    assert_error(
        r#"
peri: Antmicro.Renode.UnitTests.Mocks.MockPeripheralWithDependency
    other: peri"#,
        "CreationOrderCycle",
    );
}

#[test]
fn should_fail_on_non_existing_reference_in_ctor_attribute() {
    assert_error(
        r#"
peripheral: Antmicro.Renode.UnitTests.Mocks.MockPeripheralWithDependency
    other: nonExististing"#,
        "MissingReference",
    );
}

#[test]
fn should_find_cyclic_reference_between_files() {
    let result = super::upstream_using_tests::create_platform(
        r#"
using "A"
peri2: Antmicro.Renode.UnitTests.Mocks.MockPeripheralWithDependency
    other: peri1

peri3:
    other: peri2"#,
        &[(
            "A",
            "
peri1: Antmicro.Renode.UnitTests.Mocks.MockPeripheralWithDependency
    other: peri3
peri3: Antmicro.Renode.UnitTests.Mocks.MockPeripheralWithDependency
",
        )],
    );
    let error = result.err().expect("expected constructor dependency cycle");
    assert!(error.starts_with("CreationOrderCycle:"), "{error}");
}

#[test]
fn should_update_registration_point() {
    let platform = creation::create_objects(r#"
register1: Antmicro.Renode.UnitTests.Mocks.NullRegister @ sysbus <0, 100>
register2: Antmicro.Renode.UnitTests.Mocks.NullRegister @ sysbus new Antmicro.Renode.Peripherals.Bus.BusRangeRegistration { range: <100, +100> }
cpu: Antmicro.Renode.UnitTests.Mocks.MockCPU @ register1
cpu: @register2"#, Path::new(".")).unwrap();
    assert!(platform.cpu("sysbus.register2.cpu").is_some());
    assert!(platform.cpu("sysbus.register1.cpu").is_none());
}

#[test]
fn should_cancel_registration() {
    let platform = creation::create_objects(
        r#"
register: Antmicro.Renode.UnitTests.Mocks.NullRegister @ sysbus <0, 100>
cpu: Antmicro.Renode.UnitTests.Mocks.MockCPU @ register
cpu: @none"#,
        Path::new("."),
    )
    .unwrap();
    assert!(platform.cpu("sysbus.register.cpu").is_none());
}

#[test]
fn should_handle_registration_in_reverse_order() {
    let platform = creation::create_objects(
        r#"
cpu: Antmicro.Renode.UnitTests.Mocks.MockCPU @ register
register: Antmicro.Renode.UnitTests.Mocks.NullRegister @ sysbus <0, 100>
"#,
        Path::new("."),
    )
    .unwrap();
    assert!(platform.cpu("sysbus.register.cpu").is_some());
}

#[test]
fn should_process_repeated_registration() {
    let platform = creation::create_objects(
        r#"
mockRegister1: Antmicro.Renode.UnitTests.Mocks.MockRegister @ sysbus 0x0
cpu: Antmicro.Renode.UnitTests.Mocks.MockCPU @ {
    mockRegister1;
    mockRegister2
}
mockRegister2: Antmicro.Renode.UnitTests.Mocks.MockRegister @ sysbus 0x100
"#,
        Path::new("."),
    )
    .unwrap();
    assert!(platform.cpu("sysbus.mockRegister1.cpu").is_some());
    assert!(platform.cpu("sysbus.mockRegister2.cpu").is_some());
}

#[test]
fn should_fail_registration_on_unregistered_parent() {
    assert_error(
        r#"
unregisteredParent: Antmicro.Renode.UnitTests.Mocks.MockRegister

cpu: Antmicro.Renode.UnitTests.Mocks.MockCPU @ unregisteredParent
"#,
        "RegistrationException",
    );
}

#[test]
fn should_fail_on_ambiguous_constructor_for_registration_point() {
    assert_error(
        r#"
register: Antmicro.Renode.UnitTests.Mocks.AmbiguousRegister
device: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral @ register 1"#,
        "AmbiguousCtorForRegistrationPoint",
    );
}

#[test]
fn should_fail_on_ambiguous_registration_point() {
    assert_error(
        r#"
register: Antmicro.Renode.UnitTests.Mocks.AmbiguousRegister
regPoint: Antmicro.Renode.UnitTests.Mocks.MockRegistrationPoint
peripheral: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral @ register regPoint"#,
        "AmbiguousRegistrationPointType",
    );
}

#[test]
fn should_fail_on_ambiguous_registree() {
    assert_error(
        r#"
register: Antmicro.Renode.UnitTests.Mocks.AmbiguousRegister
peripheral: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral @ register 2.0"#,
        "AmbiguousRegistree",
    );
}

#[test]
fn should_fail_on_non_peripheral_register() {
    assert_error(
        r#"
emptyIterestingType: Antmicro.Renode.UnitTests.Mocks.EmptyInterestingType
cpu: Antmicro.Renode.UnitTests.Mocks.MockCPU @ emptyIterestingType
"#,
        "CastException",
    );
}

#[test]
fn should_catch_exception_on_registration() {
    assert_error(
        r#"
peripheral: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral @ {
    sysbus <0x100, +0x100>;
    sysbus <0x150, +0x100>
}"#,
        "RegistrationException",
    );
}

#[test]
fn should_handle_name_conflict() {
    assert_error(
        r#"
p1: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral @ sysbus <0x100, +0x50>
p2: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral @ sysbus <0x200, +0x50> as "p1"
"#,
        "NameSettingException",
    );
}

#[test]
fn should_reject_previous_incompatible_registration_point() {
    assert_error(
        r#"
mock: @ sysbus <0, 1>
mock: Antmicro.Renode.UnitTests.Mocks.MockCPU"#,
        "NoCtorForRegistrationPoint",
    );
}

#[test]
fn should_accept_previous_registration_point() {
    creation::create_objects(
        r#"
mock: @ sysbus
mock: Antmicro.Renode.UnitTests.Mocks.MockCPU"#,
        Path::new("."),
    )
    .unwrap();
}

#[test]
fn should_catch_exception_on_entry_construction() {
    assert_error(
        r#"
peripheral: Antmicro.Renode.UnitTests.Mocks.MockPeripheralWithDependency
    throwException: true"#,
        "ConstructionException",
    );
}

#[test]
fn should_catch_exception_on_object_value_construction() {
    assert_error(
        r#"
peripheral: Antmicro.Renode.UnitTests.Mocks.MockPeripheralWithDependency
    other: new Antmicro.Renode.UnitTests.Mocks.MockPeripheralWithDependency
        throwException: true"#,
        "ConstructionException",
    );
}

#[test]
fn should_catch_exception_on_property_setting() {
    assert_error(
        r#"
peripheral: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral
    ThrowingProperty: 1"#,
        "PropertySettingException",
    );
}

#[derive(Default)]
struct RecordingInit {
    reject: bool,
    executed: Vec<(String, Vec<String>)>,
}

impl creation::InitHandler for RecordingInit {
    fn validate(&mut self, _: &str, _: &str) -> Result<(), String> {
        if self.reject {
            Err("Invalid init section".into())
        } else {
            Ok(())
        }
    }

    fn execute(&mut self, name: &str, commands: &[&str]) {
        self.executed.push((
            name.into(),
            commands.iter().map(|&command| command.into()).collect(),
        ));
    }
}

fn assert_init(source: &str, object: &str, commands: &[&str]) {
    let mut handler = RecordingInit::default();
    creation::create_with_init(source, Path::new("."), &mut handler).unwrap();
    assert_eq!(
        handler.executed,
        [(
            object.into(),
            commands.iter().map(|&command| command.into()).collect()
        )]
    );
}

#[test]
fn should_replace_init() {
    assert_init(
        r#"
peri: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral @ sysbus <0, 1>
    init:
        Increment 1

peri:
    init:
        Increment 2
        Increment 3"#,
        "peri",
        &["Increment 2", "Increment 3"],
    );
}

#[test]
fn should_add_init() {
    assert_init(
        r#"
peri: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral
    init:
        Increment 1

peri:
    init add:
        Increment 2
        Increment 3"#,
        "peri",
        &["Increment 1", "Increment 2", "Increment 3"],
    );
}

#[test]
fn should_update_sysbus_init() {
    assert_init(
        r#"
sysbus:
    init:
        WriteByte 0 1"#,
        "sysbus",
        &["WriteByte 0 1"],
    );
}

#[test]
fn should_process_entry_dependant_on_sysbus() {
    creation::create_objects(
        r#"
peripheral: Antmicro.Renode.UnitTests.Mocks.MockPeripheralWithDependency
    other: sysbus"#,
        Path::new("."),
    )
    .unwrap();
}

#[test]
fn should_process_entry_dependant_on_sysbus_with_update_entry() {
    creation::create_objects(
        r#"
sysbus:
    init:
        Method

peripheral: Antmicro.Renode.UnitTests.Mocks.MockPeripheralWithDependency
    other: sysbus"#,
        Path::new("."),
    )
    .unwrap();
}
