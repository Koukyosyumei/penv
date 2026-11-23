use super::*;

fn assert_override(source: &str, imports: &[(&str, &str)], expected: &str) {
    let directory = std::env::temp_dir().join(format!(
        "penv-overrides-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap()
    ));
    fs::create_dir_all(&directory).unwrap();
    for (name, content) in imports {
        fs::write(directory.join(name), content).unwrap();
    }
    let mut entries = Vec::new();
    let parsed = parse_content(
        Path::new("<platform string>"),
        &directory,
        source,
        &directory,
        &mut HashSet::new(),
        &mut entries,
    );
    fs::remove_dir_all(directory).unwrap();
    parsed.unwrap();
    let declarations = merge_declarations(entries).unwrap();
    let cpu = declarations
        .iter()
        .find(|entry| entry.name == "cpu")
        .unwrap();
    assert_eq!(cpu.model, "Antmicro.Renode.UnitTests.Mocks.MockCPU");
    assert!(matches!(
        cpu.registration.as_deref(),
        Some("sysbus" | "sysbus 0x0")
    ));
    let props: HashMap<_, _> = keys(cpu, &["Placeholder", "EnumValue"])
        .unwrap()
        .into_iter()
        .collect();
    assert_eq!(
        quoted_string(props["Placeholder"]).as_deref(),
        Some(expected)
    );
    if imports.is_empty() {
        assert_eq!(props["EnumValue"], "TwoStateEnum.Two");
    }
}

#[test]
fn renode_override_property() {
    assert_override(
        "\ncpu: Antmicro.Renode.UnitTests.Mocks.MockCPU @ sysbus\n    Placeholder: \"one\"\n    EnumValue: TwoStateEnum.Two\n\ncpu:\n    Placeholder: \"two\"",
        &[],
        "two",
    );
}

#[test]
fn renode_override_0() {
    assert_override(
        "\nusing \"A\"\nusing \"B\"\ncpu: Antmicro.Renode.UnitTests.Mocks.MockCPU @ sysbus 0x0\n    Placeholder: \"set by source first\"\n\ncpu:\n    Placeholder: \"set by source second\"\n",
        &[
            ("A", "\ncpu:\n    Placeholder: \"set by A\"\n"),
            ("B", "\nusing \"C\"\ncpu:\n    Placeholder: \"set by B\"\n"),
            ("C", "\ncpu:\n    Placeholder: \"set by C\"\n"),
        ],
        "set by source second",
    );
}

#[test]
fn renode_override_1() {
    assert_override(
        "\nusing \"A\"\nusing \"B\"\ncpu: Antmicro.Renode.UnitTests.Mocks.MockCPU @ sysbus\n",
        &[
            (
                "A",
                "\ncpu:\n    Placeholder: \"set by A first\"\ncpu:\n    Placeholder: \"set by A second\"\n",
            ),
            (
                "B",
                "\nusing \"C\"\ncpu:\n    Placeholder: \"set by B first\"\ncpu:\n    Placeholder: \"set by B second\"\n",
            ),
            (
                "C",
                "\ncpu:\n    Placeholder: \"set by C first\"\ncpu:\n    Placeholder: \"set by C second\"\n",
            ),
        ],
        "set by B second",
    );
}

#[test]
fn renode_override_2() {
    assert_override(
        "\nusing \"A\"\ncpu: Antmicro.Renode.UnitTests.Mocks.MockCPU @ sysbus 0x0\n",
        &[
            ("A", "\nusing \"B\"\ncpu:\n    Placeholder: \"set by A\"\n"),
            ("B", "\ncpu:\n    Placeholder: \"set by B\"\n"),
        ],
        "set by A",
    );
}

#[test]
fn renode_override_3() {
    assert_override(
        "\nusing \"A\"\nusing \"B\"\nusing \"C\"\ncpu: Antmicro.Renode.UnitTests.Mocks.MockCPU @ sysbus\n",
        &[
            (
                "A",
                "\ncpu:\n    Placeholder: \"set by A first\"\ncpu:\n    Placeholder: \"set by A second\"\n",
            ),
            (
                "B",
                "\ncpu:\n    Placeholder: \"set by B first\"\ncpu:\n    Placeholder: \"set by B second\"\n",
            ),
            ("C", "\nusing \"A\";\n"),
        ],
        "set by A second",
    );
}

#[test]
fn renode_override_4() {
    assert_override(
        "\nusing \"A\"\nusing \"B\"\ncpu: Antmicro.Renode.UnitTests.Mocks.MockCPU @ sysbus\n",
        &[
            (
                "A",
                "\ncpu:\n    Placeholder: \"set by A first\"\ncpu:\n    Placeholder: \"set by A second\"\n",
            ),
            (
                "B",
                "\nusing \"A\"\ncpu:\n    Placeholder: \"set by B first\"\ncpu:\n    Placeholder: \"set by B second\"\n",
            ),
        ],
        "set by B second",
    );
}

fn mock_registrations(source: &str) -> HashSet<String> {
    let mut entries = Vec::new();
    let path = Path::new("<platform string>");
    parse_content(
        path,
        Path::new("."),
        source,
        Path::new("."),
        &mut HashSet::new(),
        &mut entries,
    )
    .unwrap();
    let entries = merge_declarations(entries).unwrap();

    // Model the fixed upstream NullRegister and MockCPU fixtures. Named
    // registration parents are instantiated before registering the CPU.
    let mut names = HashSet::new();
    for parent in entries
        .iter()
        .filter(|entry| entry.model == "Antmicro.Renode.UnitTests.Mocks.NullRegister")
    {
        let expected = if parent.name == "register2" {
            (100, 100)
        } else {
            (0, 100)
        };
        assert_eq!(parent.bus_range().unwrap(), expected);
        names.insert(format!("sysbus.{}", parent.name));
    }
    let cpu = entries.iter().find(|entry| entry.name == "cpu").unwrap();
    assert_eq!(cpu.model, "Antmicro.Renode.UnitTests.Mocks.MockCPU");
    let target = cpu.registration.as_deref().unwrap();
    if target != "none" {
        let path = format!("sysbus.{target}");
        assert!(names.contains(&path));
        names.insert(format!("{path}.{}", cpu.name));
    }
    names
}

#[test]
fn renode_registration_updates_target() {
    let names = mock_registrations(
        "\nregister1: Antmicro.Renode.UnitTests.Mocks.NullRegister @ sysbus <0, 100>\nregister2: Antmicro.Renode.UnitTests.Mocks.NullRegister @ sysbus new Antmicro.Renode.Peripherals.Bus.BusRangeRegistration { range: <100, +100> }\ncpu: Antmicro.Renode.UnitTests.Mocks.MockCPU @ register1\ncpu: @register2",
    );
    assert!(names.contains("sysbus.register2.cpu"));
    assert!(!names.contains("sysbus.register1.cpu"));
}

#[test]
fn renode_registration_cancels_target() {
    let names = mock_registrations(
        "\nregister: Antmicro.Renode.UnitTests.Mocks.NullRegister @ sysbus <0, 100>\ncpu: Antmicro.Renode.UnitTests.Mocks.MockCPU @ register\ncpu: @none",
    );
    assert!(!names.contains("sysbus.register.cpu"));
}

#[test]
fn renode_registration_updates_production_bus_mapping() {
    let source = r#"
mem: Memory.MappedMemory @ sysbus 0 { size: 0x1000 }
nvic: IRQControllers.NVIC @ sysbus 0xe000e000 { -> cpu@0 }
cpu: CPU.CortexM @ sysbus { cpuType: "cortex-m4"; nvic: nvic }
moved: Mocks.DoubleWordPeripheral @ sysbus <0x1000, +0x100> { value: 0x666 }
moved: @sysbus <0x2000, +0x100>
gone: Mocks.DoubleWordPeripheral @ sysbus <0x3000, +0x100> { value: 0x667 }
gone: @none
"#;
    let path = Path::new("<platform string>");
    let mut entries = Vec::new();
    parse_content(
        path,
        Path::new("."),
        source,
        Path::new("."),
        &mut HashSet::new(),
        &mut entries,
    )
    .unwrap();
    let mut machine = load_entries(path, &merge_declarations(entries).unwrap(), false).unwrap();
    assert_eq!(machine.bus.read::<4>(0x2000).unwrap(), 0x666);
    for address in [0x1000, 0x3000] {
        assert!(machine.bus.read::<4>(address).is_err());
    }
}

#[test]
fn renode_alias_sets_registered_name() {
    let source = r#"
cpu: Antmicro.Renode.UnitTests.Mocks.MockCPU @ sysbus as "otherName""#;
    let path = Path::new("<platform string>");
    let mut entries = Vec::new();
    parse_content(
        path,
        Path::new("."),
        source,
        Path::new("."),
        &mut HashSet::new(),
        &mut entries,
    )
    .unwrap();
    let entries = merge_declarations(entries).unwrap();
    let [cpu] = entries.as_slice() else {
        panic!("expected the mock CPU")
    };
    assert_eq!(cpu.model, "Antmicro.Renode.UnitTests.Mocks.MockCPU");
    assert_eq!(cpu.name, "cpu");
    assert_eq!(cpu.registration.as_deref(), Some("sysbus"));
    let names = HashSet::from([format!("sysbus.{}", cpu.registered_name())]);
    assert!(names.contains("sysbus.otherName"));
    assert!(!names.contains("sysbus.cpu"));
}

#[test]
fn renode_alias_requires_registration() {
    let source = r#"
peripheral: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral as "alias""#;
    let error = check_string(source, None).unwrap_err();
    assert!(error.contains("AliasWithoutRegistration:"));
}

#[test]
fn renode_alias_rejects_none_registration() {
    let source = r#"
peripheral: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral @ none as "alias""#;
    let error = check_string(source, None).unwrap_err();
    assert!(error.contains("AliasWithNoneRegistration:"));
}

#[test]
fn renode_attribute_duplicate_property() {
    let source = "\nexternal: Antmicro.Renode.UnitTests.Mocks.MockCPU\n    EnumValue: One\n    EnumValue: Two";
    let error = check_string(source, None).unwrap_err();
    assert!(error.contains("PropertyOrCtorNameUsedMoreThanOnce:"));
}

#[test]
fn renode_attribute_duplicate_init() {
    let source = "\ndevice: Antmicro.Renode.UnitTests.Mocks.MockCPU\n    init:\n        DoSomething\n    init add:\n        DoSomethingElse";
    let error = check_string(source, None).unwrap_err();
    assert!(error.contains("MoreThanOneInitAttribute:"));
}

#[test]
fn renode_attribute_constructor_on_existing_sysbus() {
    let error = check_string("\nsysbus:\n    x: 5", None).unwrap_err();
    assert!(error.contains("CtorAttributesInNonCreatingEntry:"));
}

#[test]
fn renode_reference_missing_irq_target() {
    let source = "\nsender: Antmicro.Renode.UnitTests.Mocks.MockIrqSender\n    IRQ -> receiver@0";
    let error = check_string(source, None).unwrap_err();
    assert!(error.contains("IrqDestinationDoesNotExist:"));
}

#[test]
fn renode_reference_missing_registration_point() {
    let source = "\ndevice: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral @ sysbus regPoint";
    let error = check_string(source, None).unwrap_err();
    assert!(error.contains("MissingReference:"));
}

#[test]
fn renode_irq_validation_wrong_arity() {
    let error = check_string("\nsender: Antmicro.Renode.UnitTests.Mocks.MockIrqSender\n    [Irq] -> receiver@[0,1]\nreceiver: Antmicro.Renode.UnitTests.Mocks.MockReceiver", None).unwrap_err();
    assert!(error.contains("WrongIrqArity:"));
}

#[test]
fn renode_irq_validation_overlapping_numbered_source() {
    let error = check_string("\nreceiver: Antmicro.Renode.UnitTests.Mocks.MockReceiver\nsender: Antmicro.Renode.UnitTests.Mocks.MockGPIOByNumberConnectorPeripheral\n    gpios: 2\n    2 -> receiver@6\n    [0, 1-3, 7] -> receiver@[5-9]", None).unwrap_err();
    assert!(error.contains("IrqSourceUsedMoreThanOnce:"));
}

#[test]
fn renode_irq_validation_duplicate_default_source() {
    let error = check_string("\nreceiver: Antmicro.Renode.UnitTests.Mocks.MockReceiver\nsender: Antmicro.Renode.UnitTests.Mocks.MockIrqSender\n    -> receiver@0\n    -> receiver @1", None).unwrap_err();
    assert!(error.contains("IrqSourceUsedMoreThanOnce:"));
}

#[test]
fn renode_irq_model_unknown_name() {
    let error = check_string("\nsender: Antmicro.Renode.UnitTests.Mocks.MockIrqSender\n    Something -> receiver@0\nreceiver: Antmicro.Renode.UnitTests.Mocks.MockReceiver", None).unwrap_err();
    assert!(error.contains("IrqSourceDoesNotExist:"));
}

#[test]
fn renode_irq_model_numbered_on_named_model() {
    let error = check_string("\nsender: Antmicro.Renode.UnitTests.Mocks.MockIrqSender\n    [0-1] -> receiver@[0-1]\nreceiver: Antmicro.Renode.UnitTests.Mocks.MockReceiver", None).unwrap_err();
    assert!(error.contains("IrqSourceIsNotNumberedGpioOutput:"));
}

#[test]
fn renode_irq_model_ambiguous_default() {
    let error = check_string("\nsender: Antmicro.Renode.UnitTests.Mocks.MockIrqSenderWithTwoInterrupts\n    -> receiver@0\n\nreceiver: Antmicro.Renode.UnitTests.Mocks.MockReceiver", None).unwrap_err();
    assert!(error.contains("AmbiguousDefaultIrqSource:"));
}

#[test]
fn renode_irq_model_no_default() {
    let error = check_string("\ncpu: Antmicro.Renode.UnitTests.Mocks.MockCPU\n    -> receiver@0\nreceiver: Antmicro.Renode.UnitTests.Mocks.MockReceiver", None).unwrap_err();
    assert!(error.contains("IrqSourceDoesNotExist:"));
}

#[test]
fn renode_irq_model_same_named_and_default() {
    let error = check_string("\nreceiver: Antmicro.Renode.UnitTests.Mocks.MockReceiver\nsender: Antmicro.Renode.UnitTests.Mocks.MockIrqSender\n    -> receiver@0\n    Irq -> receiver @1", None).unwrap_err();
    assert!(error.contains("IrqSourceUsedMoreThanOnce:"));
}

#[test]
fn renode_irq_capabilities_rejects_non_receiver() {
    let source = "\ncpu: Antmicro.Renode.UnitTests.Mocks.MockCPU\nsender: Antmicro.Renode.UnitTests.Mocks.MockIrqSender\n    Irq -> cpu@0";
    let error = check_string(source, None).unwrap_err();
    assert!(error.contains("IrqDestinationIsNotIrqReceiver:"));
}

#[test]
fn renode_irq_capabilities_rejects_missing_source_pin() {
    let source = "\nreceiver: Antmicro.Renode.UnitTests.Mocks.MockReceiver\nsender: Antmicro.Renode.UnitTests.Mocks.MockGPIOByNumberConnectorPeripheral\n    gpios: 2\n    2 -> receiver@1";
    let error = check_string(source, None).unwrap_err();
    assert!(error.contains("IrqSourcePinDoesNotExist:"));
}

fn assert_empty_peripheral_mapping(source: &str, expected: (u32, u32)) {
    let mut entries = Vec::new();
    let path = Path::new("<platform string>");
    parse_content(
        path,
        Path::new("."),
        source,
        Path::new("."),
        &mut HashSet::new(),
        &mut entries,
    )
    .unwrap();
    let entries = merge_declarations(entries).unwrap();
    let [mock] = entries.as_slice() else {
        panic!("expected one mock peripheral")
    };
    assert_eq!(mock.name, "mock");
    assert_eq!(
        mock.model,
        "Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral"
    );
    assert_eq!(mock.bus_range().unwrap(), expected);
    let mut machine = build_entries(path, &entries, false, true).unwrap();
    assert_eq!(machine.bus.read::<1>(expected.0).unwrap(), 0);
    assert_eq!(machine.bus.read::<4>(expected.0).unwrap(), 0);
    assert!(machine.bus.read::<1>(expected.0 + expected.1).is_err());
}

#[test]
fn renode_declaration_late_attach() {
    assert_empty_peripheral_mapping(
        "\nmock: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral\n\nmock: @sysbus <0x0, +0x100>",
        (0, 0x100),
    );
}

#[test]
fn renode_declaration_registration_before_type() {
    assert_empty_peripheral_mapping(
        "\nmock: @sysbus <0x0, 0x1000>\n\nmock: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral",
        (0, 0x1000),
    );
}

#[test]
fn renode_declaration_duplicate_type() {
    let source = "\nmock: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral\n\nmock: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral";
    assert!(
        check_string(source, None)
            .unwrap_err()
            .contains("VariableAlreadyDeclared:")
    );
}

#[test]
fn renode_declaration_empty_entry() {
    assert!(
        check_string("\nmock:", None)
            .unwrap_err()
            .contains("EmptyEntry:")
    );
}

#[test]
fn renode_declaration_constructs_unregistered_empty_peripheral() {
    check_string(
        "\nmock: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral",
        None,
    )
    .unwrap();
}

#[test]
fn renode_empty_ctor_missing_inline_type() {
    let error = check_string(
        "\ndevice: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral\n    ctorParam: new NoSuchType",
        None,
    )
    .unwrap_err();
    assert!(error.contains("TypeNotResolved:"));
}

#[test]
fn renode_empty_ctor_no_match() {
    let error = check_string(
        "\ndevice: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral\n    ctorParam: 3",
        None,
    )
    .unwrap_err();
    assert!(error.contains("NoCtor:"));
}

#[test]
fn renode_empty_ctor_ambiguous() {
    let error = check_string(
        "\ndevice: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral\n    value: 3",
        None,
    )
    .unwrap_err();
    assert!(error.contains("AmbiguousCtor:"));
}

#[test]
fn renode_creation_type_missing_type() {
    let error = check_string("\nsomeDevice: NotExistingType", None).unwrap_err();
    assert!(error.contains("TypeNotResolved:"));
}

#[test]
fn renode_creation_type_external_registration() {
    let error = check_string(
        "\nexternal: Antmicro.Renode.UnitTests.Mocks.MockExternal @ sysbus\n",
        None,
    )
    .unwrap_err();
    assert!(error.contains("NoUsableRegisterInterface:"));
}

#[test]
fn renode_creation_type_protected_constructor() {
    let error = check_string("\np: Antmicro.Renode.UnitTests.Mocks.MockPeripheralWithProtectedConstructor @ sysbus 0x0\n", None).unwrap_err();
    assert!(error.contains("NoCtor:"));
}

#[test]
fn renode_registration_validation_reference_type() {
    let error = check_string("\nregPoint: Antmicro.Renode.UnitTests.Mocks.MockCPU\ndevice: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral @ sysbus regPoint", None).unwrap_err();
    assert!(error.contains("TypeMismatch:"));
}

#[test]
fn renode_registration_validation_inline_type() {
    let error = check_string("\ndevice: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral @ sysbus new Antmicro.Renode.UnitTests.Mocks.MockCPU { Placeholder: \"abc\" }", None).unwrap_err();
    assert!(error.contains("TypeMismatch:"));
}

#[test]
fn renode_registration_validation_non_peripheral_parent() {
    let error = check_string("\nemptyIterestingType: Antmicro.Renode.UnitTests.Mocks.EmptyInterestingType\ncpu: Antmicro.Renode.UnitTests.Mocks.MockCPU @ emptyIterestingType\n", None).unwrap_err();
    assert!(error.contains("CastException:"));
}

#[test]
fn renode_registration_resolution_no_constructor() {
    let error = check_string(
        "\ndevice: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral @ sysbus \"something\"",
        None,
    )
    .unwrap_err();
    assert!(error.contains("NoCtorForRegistrationPoint:"));
}

#[test]
fn renode_registration_resolution_ambiguous_constructor() {
    let error = check_string("\nregister: Antmicro.Renode.UnitTests.Mocks.AmbiguousRegister\ndevice: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral @ register 1", None).unwrap_err();
    assert!(error.contains("AmbiguousCtorForRegistrationPoint:"));
}

#[test]
fn renode_registration_resolution_ambiguous_point_type() {
    let error = check_string("\nregister: Antmicro.Renode.UnitTests.Mocks.AmbiguousRegister\nregPoint: Antmicro.Renode.UnitTests.Mocks.MockRegistrationPoint\nperipheral: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral @ register regPoint", None).unwrap_err();
    assert!(error.contains("AmbiguousRegistrationPointType:"));
}

#[test]
fn renode_registration_resolution_ambiguous_registree() {
    let error = check_string("\nregister: Antmicro.Renode.UnitTests.Mocks.AmbiguousRegister\nperipheral: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral @ register 2.0", None).unwrap_err();
    assert!(error.contains("AmbiguousRegistree:"));
}

#[test]
fn renode_property_fail_on_string_mismatch() {
    let error = check_string(
        "\nexternal: Antmicro.Renode.UnitTests.Mocks.MockCPU\n    Placeholder: 8\n",
        None,
    )
    .unwrap_err();
    assert!(error.contains("TypeMismatch:"));
}

#[test]
fn renode_property_fail_on_non_writable_property() {
    let error = check_string(
        "\nexternal: Antmicro.Renode.UnitTests.Mocks.MockCPU\n    Model: \"abc\"",
        None,
    )
    .unwrap_err();
    assert!(error.contains("PropertyNotWritable:"));
}

#[test]
fn renode_property_fail_on_numeric_enum_mismatch() {
    let error = check_string(
        "\nexternal: Antmicro.Renode.UnitTests.Mocks.MockCPU\n    EnumValue: \"abc\"",
        None,
    )
    .unwrap_err();
    assert!(error.contains("TypeMismatch:"));
}

#[test]
fn renode_property_fail_on_non_existing_inline_type() {
    let error = check_string(
        "\nexternal: Antmicro.Renode.UnitTests.Mocks.MockCPU\n    OtherCpu: new NotExistingType",
        None,
    )
    .unwrap_err();
    assert!(error.contains("TypeNotResolved:"));
}

#[test]
fn renode_property_fail_on_non_existing_property() {
    let error = check_string(
        "\nexternal: Antmicro.Renode.UnitTests.Mocks.MockCPU\n    NotExistingProperty: \"value\"",
        None,
    )
    .unwrap_err();
    assert!(error.contains("PropertyDoesNotExist:"));
}

#[test]
fn renode_property_check_type_of_a_reference() {
    let error = check_string("\none: Antmicro.Renode.UnitTests.Mocks.MockCPU\n    OtherCpu: two\n\ntwo: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral", None).unwrap_err();
    assert!(error.contains("TypeMismatch:"));
}

#[test]
fn renode_property_fail_on_incorrect_inline_object_type() {
    let error = check_string("\none: Antmicro.Renode.UnitTests.Mocks.MockCPU\n    OtherCpu: new Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral", None).unwrap_err();
    assert!(error.contains("TypeMismatch:"));
}

#[test]
fn renode_property_fail_on_missing_reference() {
    let error = check_string(
        "\ndevice: Antmicro.Renode.UnitTests.Mocks.MockCPU\n    OtherCpu: unknown",
        None,
    )
    .unwrap_err();
    assert!(error.contains("MissingReference:"));
}

#[test]
fn renode_nested_inline_property_type() {
    let error = check_string("\ncpu: Antmicro.Renode.UnitTests.Mocks.MockCPU\n    OtherCpu: new Antmicro.Renode.UnitTests.Mocks.MockCPU\n        OtherCpu: new Antmicro.Renode.UnitTests.Mocks.MockCPU\n            EnumValue: \"string\"", None).unwrap_err();
    assert!(error.contains("TypeMismatch:"), "{error}");
}

#[test]
fn renode_nested_inline_no_constructor() {
    let error = check_string("\nperipheral: Antmicro.Renode.UnitTests.Mocks.MockPeripheralWithDependency\n    other: new Antmicro.Renode.UnitTests.Mocks.MockPeripheralWithDependency\n        other: new Antmicro.Renode.UnitTests.Mocks.MockPeripheralWithDependency\n            x: 7", None).unwrap_err();
    assert!(error.contains("NoCtor:"), "{error}");
}

#[test]
fn renode_nested_inline_ambiguous_constructor() {
    let error = check_string("\nperipheral: Antmicro.Renode.UnitTests.Mocks.MockPeripheralWithDependency\n    other: new Antmicro.Renode.UnitTests.Mocks.MockPeripheralWithDependency\n        other: new Antmicro.Renode.UnitTests.Mocks.MockPeripheralWithDependency\n            other: new Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral\n                value: 4", None).unwrap_err();
    assert!(error.contains("AmbiguousCtor:"), "{error}");
}

fn check_reference_imports(source: &str, files: &[(&str, &str)]) -> Result<(), String> {
    let directory = std::env::temp_dir().join(format!(
        "penv-reference-imports-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap()
    ));
    fs::create_dir_all(&directory).unwrap();
    for &(name, content) in files {
        fs::write(directory.join(name), content).unwrap();
    }
    let result = check_string(source, Some(&directory));
    fs::remove_dir_all(directory).unwrap();
    result
}

#[test]
fn renode_using_reference_direct() {
    check_reference_imports(
        "\nusing \"A\"\ncpu2: Antmicro.Renode.UnitTests.Mocks.MockCPU\n    OtherCpu: cpu1",
        &[("A", "\ncpu1: Antmicro.Renode.UnitTests.Mocks.MockCPU")],
    )
    .unwrap();
}

#[test]
fn renode_using_reference_private() {
    assert!(
        check_reference_imports(
            "\nusing \"A\"\ncpu2: Antmicro.Renode.UnitTests.Mocks.MockCPU\n    OtherCpu: cpu1",
            &[("A", "\nlocal cpu1: Antmicro.Renode.UnitTests.Mocks.MockCPU")]
        )
        .unwrap_err()
        .contains("MissingReference:")
    );
}

#[test]
fn renode_using_reference_nested() {
    check_reference_imports(
        "\nusing \"A\"\ncpu3: Antmicro.Renode.UnitTests.Mocks.MockCPU\n    OtherCpu: cpu2",
        &[
            (
                "A",
                "\nusing \"B\"\ncpu2: Antmicro.Renode.UnitTests.Mocks.MockCPU\n    OtherCpu: cpu1",
            ),
            ("B", "\ncpu1: Antmicro.Renode.UnitTests.Mocks.MockCPU"),
        ],
    )
    .unwrap();
}

#[test]
fn renode_using_reference_reverse() {
    check_reference_imports(
        "\nusing \"A\"\notherCpu: Antmicro.Renode.UnitTests.Mocks.MockCPU",
        &[(
            "A",
            "\ncpu: Antmicro.Renode.UnitTests.Mocks.MockCPU\n    OtherCpu: otherCpu",
        )],
    )
    .unwrap();
}

fn assert_using_syntax(source: &str, expected: &[&str]) {
    let mut comments = ReplComments::default();
    let mut paths = Vec::new();
    for line in source.lines() {
        let line = comments.line(line).unwrap();
        if !line.trim().is_empty() {
            paths.push(parse_using_statement(line.trim()).unwrap().unwrap());
        }
    }
    assert_eq!(paths, expected);
    let files: Vec<_> = expected.iter().map(|&path| (path, "")).collect();
    check_reference_imports(source, &files).unwrap();
}

#[test]
fn renode_using_parser_empty() {
    assert_using_syntax("", &[]);
}

#[test]
fn renode_using_parser_1() {
    assert_using_syntax(
        "\nusing \"file.pl8\"\nusing \"other_file.pl8\"",
        &["file.pl8", "other_file.pl8"],
    );
}

#[test]
fn renode_using_parser_2() {
    assert_using_syntax(
        "\nusing \"file.pl8\";\nusing \"other_file.pl8\"",
        &["file.pl8", "other_file.pl8"],
    );
}

#[test]
fn renode_using_parser_3() {
    assert_using_syntax("\nusing \"file.pl8\"", &["file.pl8"]);
}

#[test]
fn renode_using_parser_4() {
    assert_using_syntax("\nusing \"file.pl8\";", &["file.pl8"]);
}

#[test]
fn renode_using_parser_5() {
    assert_using_syntax(
        "\nusing \"first.pl8\";\nusing \"second.pl8\";\nusing \"third.pl8\";",
        &["first.pl8", "second.pl8", "third.pl8"],
    );
}

#[test]
fn renode_using_parser_accepts_tab_after_keyword() {
    assert_using_syntax("using\t\"file.pl8\";", &["file.pl8"]);
}

fn registration_syntax(source: &str) -> Result<Entry, String> {
    let mut entries = Vec::new();
    parse_content(
        Path::new("<platform string>"),
        Path::new("."),
        source,
        Path::new("."),
        &mut HashSet::new(),
        &mut entries,
    )?;
    assert_eq!(entries.len(), 1);
    let entry = entries.pop().unwrap();
    assert_eq!(entry.name, "uart");
    Ok(entry)
}

#[test]
fn renode_registration_syntax_parse_simple_entry() {
    let entry = registration_syntax("\nuart: UART").unwrap();
    assert_eq!(entry.model, "UART");
}

#[test]
fn renode_registration_syntax_parse_entry_with_simple_registration_info() {
    let entry = registration_syntax("\nuart: @ sysbus").unwrap();
    assert!(entry.model.is_empty());
    assert_eq!(entry.registration.as_deref(), Some("sysbus"));
}

#[test]
fn renode_registration_syntax_parse_entry_with_range_registration_point() {
    let entry = registration_syntax("\nuart: @ sysbus <0x2000A000, +0x1000>").unwrap();
    assert!(entry.model.is_empty());
    assert_eq!(entry.bus_range().unwrap(), (0x2000a000, 0x1000));
}

#[test]
fn renode_registration_syntax_parse_entry_with_string_registration_point() {
    let entry = registration_syntax("\nuart: @ sysbus \"something\"").unwrap();
    assert!(entry.model.is_empty());
    assert_eq!(
        quoted_string(
            entry
                .registration
                .as_deref()
                .unwrap()
                .strip_prefix("sysbus ")
                .unwrap()
        )
        .as_deref(),
        Some("something")
    );
}

#[test]
fn renode_registration_syntax_parse_single_registration_point_in_braces() {
    let entry = registration_syntax("\nuart: @{ sysbus 0x100 }").unwrap();
    assert!(entry.model.is_empty());
    assert_eq!(entry.address().unwrap(), 0x100);
}

#[test]
fn renode_registration_syntax_parse_single_registration_point_in_braces_with_semicolon() {
    let entry = registration_syntax("\nuart: @{ sysbus 0x100; }").unwrap();
    assert!(entry.model.is_empty());
    assert_eq!(entry.address().unwrap(), 0x100);
}

#[test]
fn renode_registration_syntax_parse_entry_with_many_registration_points() {
    let entry = registration_syntax("\nuart: @{ sysbus 0x100;\n         sysbus 0x200}").unwrap();
    assert!(entry.model.is_empty());
    assert_eq!(
        registration_items(entry.registration.as_deref().unwrap()).unwrap(),
        ["sysbus 0x100", "sysbus 0x200"]
    );
}

#[test]
fn renode_registration_syntax_parse_entry_with_many_registration_points_with_semicolons() {
    let entry = registration_syntax("\nuart: @{ sysbus 0x100;\n         sysbus 0x200;}").unwrap();
    assert!(entry.model.is_empty());
    assert_eq!(
        registration_items(entry.registration.as_deref().unwrap()).unwrap(),
        ["sysbus 0x100", "sysbus 0x200"]
    );
}

#[test]
fn renode_registration_syntax_fail_on_non_identifier_registration_point_in_braces() {
    assert!(registration_syntax("\nuart: @{ _nonIdentifier }").is_err());
}

#[test]
fn renode_registration_syntax_fail_on_non_identifier_registration_point_in_braces_with_semicolon() {
    assert!(registration_syntax("\nuart: @{ _nonIdentifier; }").is_err());
}

#[test]
fn renode_registration_syntax_fail_on_non_identifier_registration_point_in_braces_with_leading_semicolon()
 {
    assert!(registration_syntax("\nuart: @{ ;_nonIdentifier }").is_err());
}

#[test]
fn renode_registration_syntax_fail_on_multiple_registration_points_with_non_identifier_in_braces_with_semicolons()
 {
    assert!(
        registration_syntax(
            "\nuart: @{\n    sysbus 0x100;\n    sysbus 0x200;\n    _nonIdentifier\n}"
        )
        .is_err()
    );
}

#[test]
fn renode_attribute_syntax_parse_entry_with_many_attributes_with_semicolons() {
    let entry = attribute_syntax(
        "\nother: Display @ sysbus <0x0, +0x100> { refreshMode: Automatic; -> ic@3; }\n",
    )
    .unwrap();
    assert_eq!(entry.body.len(), 2);
}

#[test]
fn renode_attribute_syntax_parse_entry_with_one_attribute() {
    let entry = attribute_syntax("\nuart:\n    baudRate: BaudRate.B9600\n").unwrap();
    assert!(entry.model.is_empty());
    assert_eq!(entry.name, "uart");
    assert_eq!(entry.body.len(), 1);
    let (key, value) = entry.body[0].1.split_once(':').unwrap();
    assert_eq!(key, "baudRate");
    let (kind, value) = enum_token(value.trim()).unwrap();
    assert_eq!(value, "B9600");
    assert_eq!(kind, Some("BaudRate"));
}

#[test]
fn renode_attribute_syntax_parse_entry_with_no_attrbutes() {
    let entry = attribute_syntax("\nuart: {}").unwrap();
    assert!(entry.body.is_empty());
}

#[test]
fn renode_attribute_syntax_fail_on_entry_with_no_attrbutes_and_semicolon() {
    assert!(attribute_syntax("\nuart: {;}").is_err());
}

#[test]
fn renode_attribute_syntax_fail_on_non_identifier_in_braces() {
    assert!(attribute_syntax("\nuart: {_nonIdentifier}").is_err());
}

#[test]
fn renode_attribute_syntax_fail_on_non_identifier_in_braces_and_semicolon() {
    assert!(attribute_syntax("\nuart: {;_nonIdentifier}").is_err());
}

#[test]
fn renode_attribute_syntax_parse_entry_with_one_attribute_in_braces() {
    let entry = attribute_syntax("\nuart: {baudRate: BaudRate.B9600}\n").unwrap();
    assert!(entry.model.is_empty());
    assert_eq!(entry.name, "uart");
    assert_eq!(entry.body.len(), 1);
    let (key, value) = entry.body[0].1.split_once(':').unwrap();
    assert_eq!(key, "baudRate");
    let (kind, value) = enum_token(value.trim()).unwrap();
    assert_eq!(value, "B9600");
    assert_eq!(kind, Some("BaudRate"));
}

#[test]
fn renode_attribute_syntax_parse_entry_with_one_attribute_in_braces_with_semicolon() {
    let entry = attribute_syntax("\nuart: {baudRate: BaudRate.B9600;}\n").unwrap();
    assert!(entry.model.is_empty());
    assert_eq!(entry.name, "uart");
    assert_eq!(entry.body.len(), 1);
    let (key, value) = entry.body[0].1.split_once(':').unwrap();
    assert_eq!(key, "baudRate");
    let (kind, value) = enum_token(value.trim()).unwrap();
    assert_eq!(value, "B9600");
    assert_eq!(kind, Some("BaudRate"));
}

#[test]
fn renode_attribute_syntax_fail_on_entry_with_one_attribute_in_braces_with_multiple_semicolons() {
    assert!(attribute_syntax("\nuart: {baudRate: BaudRate.B9600;;}\n").is_err());
}

#[test]
fn renode_attribute_syntax_fail_on_non_identifier_with_with_multiple_semicolons() {
    assert!(attribute_syntax("\nuart: {_nonIdentifier;;}\n").is_err());
}

#[test]
fn renode_attribute_syntax_fail_on_entry_with_empty_statement_and_one_attribute_in_braces_with_semicolon()
 {
    assert!(attribute_syntax("\nuart: {; baudRate: BaudRate.B9600;}\n").is_err());
}

#[test]
fn renode_attribute_syntax_parse_entry_with_shorthand_enum_value() {
    let entry = attribute_syntax("\nuart:\n    baudRate: .B9600\n").unwrap();
    assert_eq!(entry.body.len(), 1);
    let (key, value) = entry.body[0].1.split_once(':').unwrap();
    assert_eq!(key, "baudRate");
    let (kind, value) = enum_token(value.trim()).unwrap();
    assert_eq!(value, "B9600");
    assert_eq!(kind, None);
}

#[test]
fn renode_attribute_syntax_parse_entry_with_two_attributes() {
    let entry =
        attribute_syntax("\nuart:\n    friendlyName: \"some name\"\n    size: 0x1000\n").unwrap();
    assert_eq!(entry.body.len(), 2);
}

#[test]
fn renode_attribute_syntax_parse_entry_with_two_attributes_in_braces_and_one_semicolon() {
    let entry =
        attribute_syntax("\nuart: {\n    friendlyName: \"some name\";\n    size: 0x1000\n}")
            .unwrap();
    assert_eq!(entry.body.len(), 2);
}

#[test]
fn renode_attribute_syntax_fail_on_entry_with_attribute_and_non_identifier_in_braces() {
    assert!(
        attribute_syntax("\nuart: {\n    friendlyName: \"some name\";\n    _nonIdentifier\n}")
            .is_err()
    );
}

#[test]
fn renode_attribute_syntax_parse_entry_with_two_attributes_in_braces_and_all_semicolons() {
    let entry =
        attribute_syntax("\nuart: {\n    friendlyName: \"some name\";\n    size: 0x1000;\n}")
            .unwrap();
    assert_eq!(entry.body.len(), 2);
}

#[test]
fn renode_attribute_syntax_fail_on_entry_with_two_attributes_in_braces_with_trailing_semicolon() {
    assert!(
        attribute_syntax("\nuart: {\n    friendlyName: \"some name\";\n    size: 0x1000;\n};")
            .is_err()
    );
}

#[test]
fn renode_attribute_syntax_fail_on_entry_with_two_attributes_and_non_identifier_in_braces_and_semicolons()
 {
    assert!(
        attribute_syntax(
            "\nuart: {\n    friendlyName: \"some name\";\n    size: 0x1000;\n    _nonIdentifier\n}"
        )
        .is_err()
    );
}

fn attribute_syntax(source: &str) -> Result<Entry, String> {
    let mut entries = Vec::new();
    let path = Path::new("<platform string>");
    parse_content(
        path,
        Path::new("."),
        source,
        Path::new("."),
        &mut HashSet::new(),
        &mut entries,
    )?;
    assert_eq!(entries.len(), 1);
    Ok(entries.pop().unwrap())
}
