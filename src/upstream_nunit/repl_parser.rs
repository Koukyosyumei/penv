use super::*;

fn description(source: &str) -> Result<Vec<Entry>, String> {
    let path = Path::new("<NUnit REPL>");
    let mut entries = Vec::new();
    parse_text(source, path, &mut entries, |_, _| {
        Err("unexpected using".into())
    })?;
    Ok(entries)
}

fn entry(source: &str) -> Entry {
    let mut entries = description(source).unwrap();
    assert_eq!(entries.len(), 1);
    entries.pop().unwrap()
}

fn property(entry: &Entry) -> (&str, &str) {
    let [(_, property)] = entry.body.as_slice() else {
        panic!("expected one property");
    };
    let (name, value) = property.split_once(':').unwrap();
    (name.trim(), value.trim())
}

#[test]
fn should_parse_single_registration_point_in_braces() {
    const SOURCE: &str = r#"
uart: @{ sysbus 0x100 }"#;
    assert!(description(SOURCE).is_ok());
}

#[test]
fn should_parse_single_registration_point_in_braces_with_semicolon() {
    const SOURCE: &str = r#"
uart: @{ sysbus 0x100; }"#;
    assert!(description(SOURCE).is_ok());
}

#[test]
fn should_parse_entry_with_many_registration_points_with_semicolons() {
    const SOURCE: &str = r#"
uart: @{ sysbus 0x100;
         sysbus 0x200;}"#;
    assert!(description(SOURCE).is_ok());
}

#[test]
fn should_fail_on_non_identifier_registration_point_in_braces() {
    const SOURCE: &str = r#"
uart: @{ _nonIdentifier }"#;
    assert!(description(SOURCE).is_err());
}

#[test]
fn should_fail_on_non_identifier_registration_point_in_braces_with_semicolon() {
    const SOURCE: &str = r#"
uart: @{ _nonIdentifier; }"#;
    assert!(description(SOURCE).is_err());
}

#[test]
fn should_fail_on_non_identifier_registration_point_in_braces_with_leading_semicolon() {
    const SOURCE: &str = r#"
uart: @{ ;_nonIdentifier }"#;
    assert!(description(SOURCE).is_err());
}

#[test]
fn should_fail_on_multiple_registration_points_with_non_identifier_in_braces_with_semicolons() {
    const SOURCE: &str = r#"
uart: @{
    sysbus 0x100;
    sysbus 0x200;
    _nonIdentifier
}"#;
    assert!(description(SOURCE).is_err());
}

#[test]
fn should_parse_entry_with_many_attributes_with_semicolons() {
    const SOURCE: &str = r#"
other: Display @ sysbus <0x0, +0x100> { refreshMode: Automatic; -> ic@3; }
"#;
    assert!(description(SOURCE).is_ok());
}

#[test]
fn should_parse_entry_with_no_attrbutes() {
    const SOURCE: &str = r#"
uart: {}"#;
    assert!(description(SOURCE).is_ok());
}

#[test]
fn should_fail_on_entry_with_no_attrbutes_and_semicolon() {
    const SOURCE: &str = r#"
uart: {;}"#;
    assert!(description(SOURCE).is_err());
}

#[test]
fn should_fail_on_non_identifier_in_braces() {
    const SOURCE: &str = r#"
uart: {_nonIdentifier}"#;
    assert!(description(SOURCE).is_err());
}

#[test]
fn should_fail_on_non_identifier_in_braces_and_semicolon() {
    const SOURCE: &str = r#"
uart: {;_nonIdentifier}"#;
    assert!(description(SOURCE).is_err());
}

#[test]
fn should_fail_on_entry_with_one_attribute_in_braces_with_multiple_semicolons() {
    const SOURCE: &str = r#"
uart: {baudRate: BaudRate.B9600;;}
"#;
    assert!(description(SOURCE).is_err());
}

#[test]
fn should_fail_on_non_identifier_with_with_multiple_semicolons() {
    const SOURCE: &str = r#"
uart: {_nonIdentifier;;}
"#;
    assert!(description(SOURCE).is_err());
}

#[test]
fn should_fail_on_entry_with_empty_statement_and_one_attribute_in_braces_with_semicolon() {
    const SOURCE: &str = r#"
uart: {; baudRate: BaudRate.B9600;}
"#;
    assert!(description(SOURCE).is_err());
}

#[test]
fn should_parse_entry_with_two_attributes() {
    const SOURCE: &str = r#"
uart:
    friendlyName: "some name"
    size: 0x1000
"#;
    assert_eq!(entry(SOURCE).body.len(), 2);
}

#[test]
fn should_parse_entry_with_two_attributes_in_braces_and_one_semicolon() {
    const SOURCE: &str = r#"
uart: {
    friendlyName: "some name";
    size: 0x1000
}"#;
    assert_eq!(entry(SOURCE).body.len(), 2);
}

#[test]
fn should_fail_on_entry_with_attribute_and_non_identifier_in_braces() {
    const SOURCE: &str = r#"
uart: {
    friendlyName: "some name";
    _nonIdentifier
}"#;
    assert!(description(SOURCE).is_err());
}

#[test]
fn should_parse_entry_with_two_attributes_in_braces_and_all_semicolons() {
    const SOURCE: &str = r#"
uart: {
    friendlyName: "some name";
    size: 0x1000;
}"#;
    assert_eq!(entry(SOURCE).body.len(), 2);
}

#[test]
fn should_fail_on_entry_with_two_attributes_in_braces_with_trailing_semicolon() {
    const SOURCE: &str = r#"
uart: {
    friendlyName: "some name";
    size: 0x1000;
};"#;
    assert!(description(SOURCE).is_err());
}

#[test]
fn should_fail_on_entry_with_two_attributes_and_non_identifier_in_braces_and_semicolons() {
    const SOURCE: &str = r#"
uart: {
    friendlyName: "some name";
    size: 0x1000;
    _nonIdentifier
}"#;
    assert!(description(SOURCE).is_err());
}

#[test]
fn should_parse_two_entries() {
    const SOURCE: &str = r#"
uart1: UART @ sysbus <0x1000, +0x100>
    someProperty: someValue

uart2: UART @ sysbus <0x2000, +0x100>
    otherProperty: "otherValue"
"#;
    assert_eq!(description(SOURCE).unwrap().len(), 2);
}

#[test]
fn should_parse_example() {
    const SOURCE: &str = r#"
screen: Display@graphicsCard1
    resolution: new Point { x: 5; y: 6 }
    refreshMode: Automatic
    model: "SuperDisplay"
    -> ic@3
    init:
        DrawCircle 20 20
        DrawCircle 30 40

screen:
    init:
        DrawRect 40 50 1

screen:
    init:
        base init
        DrawRect 40 50 1

other: Display @ sysbus <0x0, +0x100> { refreshMode: Automatic; -> ic@3 }
"#;
    assert!(description(SOURCE).is_ok());
}

#[test]
fn should_parse_two_short_entries() {
    const SOURCE: &str = r#"
uart: UART

uart: @sysbus <0x100, +0x100>
"#;
    assert_eq!(description(SOURCE).unwrap().len(), 2);
}

#[test]
fn should_not_parse_dict_with_missing_closing_brace() {
    const SOURCE: &str = r#"
device: Something
    dict: {"key0": 0; "key1": 1"#;
    let error = description(SOURCE).unwrap_err();
    assert!(
        error.to_lowercase().contains(r#"unexpected end of input"#),
        "{error}"
    );
}

#[test]
fn should_not_parse_dict_with_comma() {
    const SOURCE: &str = r#"
device: Something
    dict: {"key0": 0, "key1": 1}"#;
    let error = description(SOURCE).unwrap_err();
    assert!(
        error.to_lowercase().contains(r#"unexpected ','"#),
        "{error}"
    );
}

#[test]
fn should_not_parse_dict_with_missing_separator() {
    const SOURCE: &str = r#"
device: Something
    dict: {"key0": 0 "key1": 1}"#;
    let error = description(SOURCE).unwrap_err();
    assert!(
        error.to_lowercase().contains(r#"unexpected '"'"#),
        "{error}"
    );
}

#[test]
fn should_not_parse_dict_with_missing_colon() {
    const SOURCE: &str = r#"
device: Something
    dict: {"key0" 0, "key1": 1}"#;
    let error = description(SOURCE).unwrap_err();
    assert!(
        error.to_lowercase().contains(r#"unexpected '0'"#),
        "{error}"
    );
}

#[test]
fn should_parse_empty_file() {
    assert!(description("").is_ok());
}

#[test]
fn should_parse_simple_entry() {
    let item = entry("\nuart: UART");
    assert_eq!(item.name, "uart");
    assert_eq!(item.model, "UART");
}

fn registration(source: &str, expected: Option<RegistrationValue<'_>>) {
    let item = entry(source);
    assert_eq!(item.name, "uart");
    assert!(item.model.is_empty());
    assert_eq!(
        registration_point(item.registration.as_ref().unwrap()),
        Ok(("sysbus", expected))
    );
}

#[test]
fn should_parse_entry_with_simple_registration_info() {
    registration("\nuart: @ sysbus", None);
}

#[test]
fn should_parse_entry_with_range_registration_point() {
    registration(
        "\nuart: @ sysbus <0x2000A000, +0x1000>",
        Some(RegistrationValue::Range {
            base: 0x2000a000,
            size: 0x1000,
        }),
    );
}

#[test]
fn should_parse_entry_with_string_registration_point() {
    registration(
        "\nuart: @ sysbus \"something\"",
        Some(RegistrationValue::String("something".into())),
    );
}

#[test]
fn should_parse_entry_with_many_registration_points() {
    let item = entry("\nuart: @{ sysbus 0x100;\n         sysbus 0x200}");
    assert_eq!(item.name, "uart");
    assert!(item.model.is_empty());
    let raw = item.registration.unwrap();
    let points = registration_points(&raw[1..raw.len() - 1]).unwrap();
    assert_eq!(
        points[0],
        ("sysbus", Some(RegistrationValue::Other("0x100")))
    );
    assert_eq!(
        points[1],
        ("sysbus", Some(RegistrationValue::Other("0x200")))
    );
}

fn baud_rate(source: &str, expected_type: Option<&str>, assert_null_model: bool) {
    let item = entry(source);
    assert_eq!(item.name, "uart");
    if assert_null_model {
        assert!(item.model.is_empty());
    }
    let (name, value) = property(&item);
    assert_eq!(name, "baudRate");
    assert_eq!(
        symbolic_value(value),
        Some(SymbolicValue::Enum(expected_type, "B9600"))
    );
}

#[test]
fn should_parse_entry_with_one_attribute() {
    baud_rate(
        "\nuart:\n    baudRate: BaudRate.B9600\n",
        Some("BaudRate"),
        true,
    );
}

#[test]
fn should_parse_entry_with_one_attribute_in_braces() {
    baud_rate(
        "\nuart: {baudRate: BaudRate.B9600}\n",
        Some("BaudRate"),
        true,
    );
}

#[test]
fn should_parse_entry_with_one_attribute_in_braces_with_semicolon() {
    baud_rate(
        "\nuart: {baudRate: BaudRate.B9600;}\n",
        Some("BaudRate"),
        true,
    );
}

#[test]
fn should_parse_entry_with_shorthand_enum_value() {
    baud_rate("\nuart:\n    baudRate: .B9600\n", None, false);
}

#[test]
fn should_parse_entry_with_all_attributes() {
    let item = entry(
        r#"
uart:
    model: "ABC666"
    numberOfBits: 32
    sleepTime: 0.01
    friendUart: otherUart
    range: <0x1000, +0x100>
"#,
    );
    let attributes: HashMap<_, _> = item
        .body
        .iter()
        .map(|(_, text)| {
            let (key, value) = text.split_once(':').unwrap();
            (key, value.trim())
        })
        .collect();
    assert_eq!(attributes.len(), 5);
    assert_eq!(quoted_value(attributes["model"]).unwrap(), "ABC666");
    for (key, expected) in [("numberOfBits", "32"), ("sleepTime", "0.01")] {
        assert_eq!(
            scalar_prefix(attributes[key]).unwrap().0,
            ListItem::Number(expected.into())
        );
    }
    assert_eq!(
        symbolic_value(attributes["friendUart"]),
        Some(SymbolicValue::Reference("otherUart"))
    );
    assert_eq!(range_value(attributes["range"]), Ok((0x1000, 0x100)));
}

fn init_lines(source: &str, expected: &[&str]) {
    let item = entry(source);
    let [(_, header), commands @ ..] = item.body.as_slice() else {
        panic!("expected an init section");
    };
    assert_eq!(header, "init:");
    assert!(
        commands
            .iter()
            .map(|(_, line)| line.as_str())
            .eq(expected.iter().copied())
    );
}

#[test]
fn should_parse_init_attribute_with_one_line() {
    init_lines(
        "\nuart:\n    init:\n        DoSomething 3\n",
        &["DoSomething 3"],
    );
}

#[test]
fn should_parse_init_attribute_with_more_lines() {
    init_lines(
        "\nuart:\n    init:\n        Method1 a b\n        Method2 true\n        Method3",
        &["Method1 a b", "Method2 true", "Method3"],
    );
}

#[test]
fn should_parse_init_attribute_with_semicolon_in_quotes() {
    init_lines(
        "\nuart:\n    init:\n        CallMethod \"string ; with semicolon\"\n",
        &["CallMethod \"string ; with semicolon\""],
    );
}

#[test]
fn should_parse_object_value() {
    let item =
        entry("\ndisplay: Display\n    resolution: new Point\n        x: 640\n        y: 480\n");
    let (_, value) = property(&item);
    let (object, remainder) = object_prefix(value).unwrap();
    assert!(remainder.trim().is_empty());
    assert_eq!(object.model, "Point");
    assert_eq!(object.fields.len(), 2);
    let fields: HashMap<_, _> = object.fields.into_iter().collect();
    assert_eq!(
        scalar_prefix(fields["x"]).unwrap().0,
        ListItem::Number("640".into())
    );
    assert_eq!(
        scalar_prefix(fields["y"]).unwrap().0,
        ListItem::Number("480".into())
    );
}

#[test]
fn should_parse_local_and_non_local_entry() {
    let entries = description("\nperipheral: SomeHub\nlocal other: Other").unwrap();
    assert!(!entries[0].local);
    assert!(entries[1].local);
}

#[test]
fn should_parse_bool() {
    let item = entry(
        "
device: Something @ somewhere
    BoolProp: true
    BoolProp2: false
    BoolProp3: True
    BoolProp4: False
    BoolProp5: tRue
    BoolProp6: FALSE
",
    );
    let values: Vec<_> = item
        .body
        .iter()
        .map(|(_, text)| symbolic_value(text.split_once(':').unwrap().1.trim()).unwrap())
        .collect();
    let booleans: Vec<_> = values
        .iter()
        .filter_map(|value| match value {
            SymbolicValue::Boolean(value) => Some(*value),
            _ => None,
        })
        .collect();
    let references: Vec<_> = values
        .iter()
        .filter_map(|value| match value {
            SymbolicValue::Reference(value) => Some(*value),
            _ => None,
        })
        .collect();
    assert_eq!(booleans, [true, false, true, false]);
    assert_eq!(references, ["tRue", "FALSE"]);
}

#[test]
fn should_parse_empty_list() {
    let item = entry("\ndevice: Something\n    list: []");
    assert!(list_value(property(&item).1).unwrap().is_empty());
}

#[test]
fn should_parse_list_of_numbers_with_trailing_comma() {
    let item = entry("\ndevice: Something\n    list: [1, 0x10, 3,]");
    let items = list_value(property(&item).1).unwrap();
    assert_eq!(
        items,
        ["1", "0x10", "3"].map(|number| ListItem::Number(number.into()))
    );
}

#[test]
fn should_parse_nested_lists() {
    let item = entry("\ndevice: Something\n    eye: [[1, 0, 0], [0, 1, 0], [0, 0, 1]]");
    let items = list_value(property(&item).1).unwrap();
    assert_eq!(items.len(), 3);
    assert!(items.iter().all(|item| matches!(item, ListItem::List(_))));
    let ListItem::List(first) = &items[0] else {
        unreachable!()
    };
    assert_eq!(
        first.as_slice(),
        ["1", "0", "0"].map(|number| ListItem::Number(number.into()))
    );
}

#[test]
fn should_parse_list_with_mixed_simple_values() {
    let item = entry("\ndevice: Something\n    list: [1, \"hello\", true, other, <0, +10>]");
    assert_eq!(
        list_value(property(&item).1).unwrap(),
        [
            ListItem::Number("1".into()),
            ListItem::String("hello".into()),
            ListItem::Symbol(SymbolicValue::Boolean(true)),
            ListItem::Symbol(SymbolicValue::Reference("other")),
            ListItem::Range(0, 10),
        ]
    );
}

#[test]
fn should_parse_list_with_inline_object() {
    let item = entry(
        "\ndisplay: Display\n    resolutions: [new Point { x: 640; y: 480 }, new Point { x: 5120; y: 2160 }]\n",
    );
    let objects = list_value(property(&item).1).unwrap();
    assert_eq!(objects.len(), 2);
    for (object, (x, y)) in objects.into_iter().zip([("640", "480"), ("5120", "2160")]) {
        let ListItem::Object(object) = object else {
            panic!("expected an inline object")
        };
        let fields: HashMap<_, _> = object.fields.into_iter().collect();
        assert_eq!(
            scalar_prefix(fields["x"]).unwrap().0,
            ListItem::Number(x.into())
        );
        assert_eq!(
            scalar_prefix(fields["y"]).unwrap().0,
            ListItem::Number(y.into())
        );
    }
}

#[test]
fn should_parse_with_mixed_line_endings() {
    assert!(
        description(
            "peripheral: SomeHub\nlocal first_other: Other\r\nlocal second_other: Other\r\n"
        )
        .is_ok()
    );
}

#[test]
fn should_parse_string_with_escaped_backslashes() {
    let item = entry(
        r#"
uart:
    string: "\\escaped backslash: \\, two in a row: \\\\, before quote: \\\", and one at the end: \\"
"#,
    );
    assert_eq!(item.name, "uart");
    assert!(item.model.is_empty());
    let (name, value) = property(&item);
    assert_eq!(name, "string");
    assert_eq!(
        quoted_value(value).unwrap(),
        r#"\escaped backslash: \, two in a row: \\, before quote: \", and one at the end: \"#
    );
}

#[test]
fn should_parse_empty_dict() {
    let item = entry("\ndevice: Something\n    dict: {}");
    assert!(dictionary_value(property(&item).1).unwrap().is_empty());
}

fn check_dict(source: &str) {
    let item = entry(source);
    let pairs = dictionary_value(property(&item).1).unwrap();
    assert_eq!(pairs.len(), 2);
    for (index, (key, value)) in pairs.iter().enumerate() {
        assert_eq!(quoted_value(key).unwrap(), format!("key{index}"));
        assert_eq!(
            scalar_prefix(value).unwrap().0,
            ListItem::Number(index.to_string().into())
        );
    }
}

#[test]
fn should_parse_simple_dict() {
    check_dict("\ndevice: Something\n    dict: {\"key0\": 0; \"key1\": 1}");
}

#[test]
fn should_parse_dict_with_trailing_separator() {
    check_dict("\ndevice: Something\n    dict: {\"key0\": 0; \"key1\": 1;}");
}

fn description_with_usings(source: &str) -> (Vec<String>, Vec<Entry>) {
    let mut usings = Vec::new();
    let mut entries = Vec::new();
    parse_text(
        source,
        Path::new("<NUnit REPL>"),
        &mut entries,
        |path, _| {
            usings.push(path.to_owned());
            Ok(())
        },
    )
    .unwrap();
    (usings, entries)
}

fn check_usings(source: &str, expected: &[&str]) {
    let (mut paths, _) = description_with_usings(source);
    paths.sort_unstable();
    assert_eq!(paths, expected);
}

#[test]
fn should_parse_using_entry() {
    check_usings(
        "\nusing \"file.pl8\"\nusing \"other_file.pl8\"",
        &["file.pl8", "other_file.pl8"],
    );
}

#[test]
fn should_parse_using_entry2() {
    check_usings(
        "\nusing \"file.pl8\";\nusing \"other_file.pl8\";",
        &["file.pl8", "other_file.pl8"],
    );
}

#[test]
fn should_parse_using_entry3() {
    check_usings("\nusing \"file.pl8\"", &["file.pl8"]);
}

#[test]
fn should_parse_using_entry4() {
    check_usings("\nusing \"file.pl8\";", &["file.pl8"]);
}

#[test]
fn should_parse_using_entry5() {
    check_usings(
        "\nusing \"first.pl8\";\nusing \"second.pl8\";\nusing \"third.pl8\";",
        &["first.pl8", "second.pl8", "third.pl8"],
    );
}

#[test]
fn should_parse_using_and_entry() {
    let (paths, entries) = description_with_usings("\nusing \"other_file\"\n\ndevice: SomeDevice");
    assert_eq!(paths[0], "other_file");
    assert_eq!(entries[0].name, "device");
    assert_eq!(entries[0].model, "SomeDevice");
}
