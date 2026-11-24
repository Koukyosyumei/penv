use crate::register_enum::RegisterEnum;

#[test]
fn should_parse_enum_with_simple_value() {
    let source = r#"typedef enum {
                A_32 = 0
            } Registers;
"#;
    let parsed = RegisterEnum::parse(source.as_bytes(), &[]).unwrap();
    assert_eq!(parsed.registers.len(), 1);
    assert_eq!(parsed.groups.len(), 0);
    assert_eq!(parsed.registers[0].name, "A");
    assert_eq!(parsed.registers[0].width, 32);
    assert_eq!(parsed.registers[0].value, 0);
}

#[test]
fn should_parse_enum_with_multiple_values() {
    let source = r#"typedef enum {
                A_32 = 0,
                B_64 = 1,
                C_128 = 2
            } Registers;
"#;
    let parsed = RegisterEnum::parse(source.as_bytes(), &[]).unwrap();
    assert_eq!(parsed.registers.len(), 3);
    assert_eq!(parsed.groups.len(), 0);
    assert_eq!(parsed.registers[0].name, "A");
    assert_eq!(parsed.registers[0].width, 32);
    assert_eq!(parsed.registers[0].value, 0);
    assert_eq!(parsed.registers[1].name, "B");
    assert_eq!(parsed.registers[1].width, 64);
    assert_eq!(parsed.registers[1].value, 1);
    assert_eq!(parsed.registers[2].name, "C");
    assert_eq!(parsed.registers[2].width, 128);
    assert_eq!(parsed.registers[2].value, 2);
}

#[test]
fn should_parse_enum_with_undefined_ifdef() {
    let source = r#"typedef enum {
                A_32 = 0,
                #ifdef XYZ
                B_64 = 1,
                #endif
                C_128 = 2
            } Registers;
"#;
    let parsed = RegisterEnum::parse(source.as_bytes(), &[]).unwrap();
    assert_eq!(parsed.registers.len(), 2);
    assert_eq!(parsed.groups.len(), 0);
    assert_eq!(parsed.registers[0].name, "A");
    assert_eq!(parsed.registers[0].width, 32);
    assert_eq!(parsed.registers[0].value, 0);
    assert_eq!(parsed.registers[1].name, "C");
    assert_eq!(parsed.registers[1].width, 128);
    assert_eq!(parsed.registers[1].value, 2);
}

#[test]
fn should_parse_enum_with_defined_ifdef() {
    let source = r#"typedef enum {
                A_32 = 0,
                #ifdef XYZ
                B_64 = 1,
                #endif
                C_128 = 2
            } Registers;
"#;
    let parsed = RegisterEnum::parse(source.as_bytes(), &["XYZ"]).unwrap();
    assert_eq!(parsed.registers.len(), 3);
    assert_eq!(parsed.groups.len(), 0);
    assert_eq!(parsed.registers[0].name, "A");
    assert_eq!(parsed.registers[0].width, 32);
    assert_eq!(parsed.registers[0].value, 0);
    assert_eq!(parsed.registers[1].name, "B");
    assert_eq!(parsed.registers[1].width, 64);
    assert_eq!(parsed.registers[1].value, 1);
    assert_eq!(parsed.registers[2].name, "C");
    assert_eq!(parsed.registers[2].width, 128);
    assert_eq!(parsed.registers[2].value, 2);
}

#[test]
fn should_parse_enum_with_multiple_values_with_indices() {
    let source = r#"typedef enum {
                A_1_32 = 15,
                A_2_32 = 16,
                A_3_32 = 17
            } Registers;
"#;
    let parsed = RegisterEnum::parse(source.as_bytes(), &[]).unwrap();
    assert_eq!(parsed.registers.len(), 0);
    assert_eq!(parsed.groups.len(), 1);
    assert_eq!(parsed.groups[0].name, "A");
    assert_eq!(parsed.groups[0].width, 32);
    assert_eq!(parsed.groups[0].index_value_map.len(), 3);
    assert_eq!(parsed.groups[0].index_value_map[&1], 15);
    assert_eq!(parsed.groups[0].index_value_map[&2], 16);
    assert_eq!(parsed.groups[0].index_value_map[&3], 17);
}

#[test]
fn should_parse_enum_with_comment_or_empty_lines() {
    let source = r#"typedef enum {

                // Some comment
                A_1_32 = 15,

                A_2_32 = 16,
                //Another comment
                A_3_32 = 17
            } Registers;
"#;
    let parsed = RegisterEnum::parse(source.as_bytes(), &[]).unwrap();
    assert_eq!(parsed.registers.len(), 0);
    assert_eq!(parsed.groups.len(), 1);
}
