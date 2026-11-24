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
