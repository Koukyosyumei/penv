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

type ExpectedIrq<'a> = (&'a [IrqEndpoint<'a>], &'a [IrqDestination<'a>]);

fn assert_irq_syntax(source: &str, expected: &[ExpectedIrq<'_>]) {
    let entry = attribute_syntax(source).unwrap();
    assert_eq!(entry.body.len(), expected.len());
    for ((_, text), (sources, destinations)) in entry.body.iter().zip(expected) {
        let (source, destination) = text.split_once("->").unwrap();
        let actual = if source.trim().is_empty() {
            None
        } else {
            Some(irq_endpoints(source, 256).unwrap())
        };
        assert_eq!(actual.as_deref(), (!sources.is_empty()).then_some(*sources));
        assert_eq!(irq_destinations(destination).unwrap(), *destinations);
    }
}

fn irq_destination<'a>(name: &'a str, local: Option<u32>, groups: &[&[u8]]) -> IrqDestination<'a> {
    IrqDestination {
        name,
        local,
        groups: groups.iter().map(|group| group.to_vec()).collect(),
    }
}

#[test]
fn renode_irq_syntax_simple() {
    use IrqEndpoint::{Named, Number};
    assert_irq_syntax(
        "\nuart:\n    1 -> pic@2\n    IRQ -> pic@3\n    -> pic@4",
        &[
            (&[Number(1)], &[irq_destination("pic", None, &[&[2]])]),
            (&[Named("IRQ")], &[irq_destination("pic", None, &[&[3]])]),
            (&[], &[irq_destination("pic", None, &[&[4]])]),
        ],
    );
}

#[test]
fn renode_irq_syntax_simple_multiplexed() {
    use IrqEndpoint::{Named, Number};
    assert_irq_syntax(
        "\nuart:\n    1 -> pic@2 | pic1@3\n    -> pic2@4 | pic3@5\n    IRQ -> pic4@6 | pic5@7 | pic6@8 | pic7@9",
        &[
            (
                &[Number(1)],
                &[
                    irq_destination("pic", None, &[&[2]]),
                    irq_destination("pic1", None, &[&[3]]),
                ],
            ),
            (
                &[],
                &[
                    irq_destination("pic2", None, &[&[4]]),
                    irq_destination("pic3", None, &[&[5]]),
                ],
            ),
            (
                &[Named("IRQ")],
                &[
                    irq_destination("pic4", None, &[&[6]]),
                    irq_destination("pic5", None, &[&[7]]),
                    irq_destination("pic6", None, &[&[8]]),
                    irq_destination("pic7", None, &[&[9]]),
                ],
            ),
        ],
    );
}

#[test]
fn renode_irq_syntax_multiplexed_groups() {
    use IrqEndpoint::{Named, Number};
    assert_irq_syntax(
        "\nuart:\n    [1-2, IRQ] -> pic0@[4-6] | pic1@[7-8, 9] | pic2@[10, 11, 12]",
        &[(
            &[Number(1), Number(2), Named("IRQ")],
            &[
                irq_destination("pic0", None, &[&[4, 5, 6]]),
                irq_destination("pic1", None, &[&[7, 8], &[9]]),
                irq_destination("pic2", None, &[&[10], &[11], &[12]]),
            ],
        )],
    );
}

#[test]
fn renode_irq_syntax_multi_irq() {
    use IrqEndpoint::{Named, Number};
    assert_irq_syntax(
        "\nuart:\n    [1-3, IRQ] -> pic @ [2-5]",
        &[(
            &[Number(1), Number(2), Number(3), Named("IRQ")],
            &[irq_destination("pic", None, &[&[2, 3, 4, 5]])],
        )],
    );
}

#[test]
fn renode_irq_syntax_local_receiver() {
    assert_irq_syntax(
        "\nuart:\n    1 -> something#3@2",
        &[(
            &[IrqEndpoint::Number(1)],
            &[irq_destination("something", Some(3), &[&[2]])],
        )],
    );
}

fn assert_init_syntax(source: &str, expected: &[&str]) {
    let entry = attribute_syntax(source).unwrap();
    let [(_, attribute)] = entry.body.as_slice() else {
        panic!("expected one init attribute")
    };
    let (phase, append, commands) = script_commands(attribute).unwrap();
    assert_eq!(phase, ScriptPhase::Init);
    assert!(!append);
    assert_eq!(commands, expected);
}

#[test]
fn renode_init_syntax_one_line() {
    assert_init_syntax(
        "\nuart:\n    init:\n        DoSomething 3\n",
        &["DoSomething 3"],
    );
}

#[test]
fn renode_init_syntax_multiple_lines() {
    assert_init_syntax(
        "\nuart:\n    init:\n        Method1 a b\n        Method2 true\n        Method3",
        &["Method1 a b", "Method2 true", "Method3"],
    );
}

#[test]
fn renode_init_syntax_quoted_semicolon() {
    assert_init_syntax(
        "\nuart:\n    init:\n        CallMethod \"string ; with semicolon\"\n",
        &["CallMethod \"string ; with semicolon\""],
    );
}

fn typed_attributes(entry: &Entry) -> Vec<(&str, ScalarValue<'_>)> {
    entry
        .body
        .iter()
        .map(|(_, text)| {
            let (key, value) = text.split_once(':').unwrap();
            (key, scalar_value(value).unwrap())
        })
        .collect()
}

#[test]
fn renode_typed_scalar_attribute_values() {
    use ScalarValue::*;
    let entry = attribute_syntax("\nuart:\n    model: \"ABC666\"\n    numberOfBits: 32\n    sleepTime: 0.01\n    friendUart: otherUart\n    range: <0x1000, +0x100>\n").unwrap();
    assert_eq!(
        typed_attributes(&entry),
        [
            ("model", Text("ABC666".into())),
            ("numberOfBits", Number("32".into())),
            ("sleepTime", Number("0.01".into())),
            ("friendUart", Reference("otherUart")),
            ("range", Range(0x1000, 0x100)),
        ]
    );
}

#[test]
fn renode_typed_scalar_boolean_case() {
    use ScalarValue::*;
    let entry = attribute_syntax("\ndevice: Something @ somewhere\n    BoolProp: true\n    BoolProp2: false\n    BoolProp3: True\n    BoolProp4: False\n    BoolProp5: tRue\n    BoolProp6: FALSE\n").unwrap();
    let actual: Vec<_> = typed_attributes(&entry)
        .into_iter()
        .map(|(_, value)| value)
        .collect();
    assert_eq!(
        actual,
        [
            Boolean(true),
            Boolean(false),
            Boolean(true),
            Boolean(false),
            Reference("tRue"),
            Reference("FALSE")
        ]
    );
}

#[test]
fn renode_typed_scalar_escaped_backslashes() {
    let entry = attribute_syntax("\nuart:\n    string: \"\\\\escaped backslash: \\\\, two in a row: \\\\\\\\, before quote: \\\\\\\", and one at the end: \\\\\"\n").unwrap();
    assert_eq!(entry.name, "uart");
    assert!(entry.model.is_empty());
    assert_eq!(typed_attributes(&entry), [("string", ScalarValue::Text("\\escaped backslash: \\, two in a row: \\\\, before quote: \\\", and one at the end: \\".into()))]);
}

#[test]
fn renode_list_value_empty() {
    let entry = attribute_syntax("\ndevice: Something\n    list: []").unwrap();
    assert_eq!(
        typed_attributes(&entry),
        [("list", ScalarValue::List(vec![]))]
    );
}

#[test]
fn renode_list_value_trailing_comma() {
    use ScalarValue::*;
    let entry = attribute_syntax("\ndevice: Something\n    list: [1, 0x10, 3,]").unwrap();
    assert_eq!(
        typed_attributes(&entry),
        [(
            "list",
            List(vec![
                Number("1".into()),
                Number("0x10".into()),
                Number("3".into())
            ])
        )]
    );
}

#[test]
fn renode_list_value_nested() {
    use ScalarValue::*;
    let entry = attribute_syntax("\ndevice: Something\n    eye: [[1, 0, 0], [0, 1, 0], [0, 0, 1]]")
        .unwrap();
    let rows = [["1", "0", "0"], ["0", "1", "0"], ["0", "0", "1"]];
    let expected = rows.map(|row| List(row.into_iter().map(|v| Number(v.into())).collect()));
    assert_eq!(typed_attributes(&entry), [("eye", List(expected.into()))]);
}

#[test]
fn renode_list_value_mixed_scalars() {
    use ScalarValue::*;
    let entry =
        attribute_syntax("\ndevice: Something\n    list: [1, \"hello\", true, other, <0, +10>]")
            .unwrap();
    assert_eq!(
        typed_attributes(&entry),
        [(
            "list",
            List(vec![
                Number("1".into()),
                Text("hello".into()),
                Boolean(true),
                Reference("other"),
                Range(0, 10),
            ])
        )]
    );
}

#[test]
fn renode_list_value_inline_objects() {
    use ScalarValue::*;
    let entry = attribute_syntax("\ndisplay: Display\n    resolutions: [new Point { x: 640; y: 480 }, new Point { x: 5120; y: 2160 }]\n").unwrap();
    let point = |x: &str, y: &str| {
        Object(
            "Point",
            vec![("x", Number(x.into())), ("y", Number(y.into()))],
        )
    };
    assert_eq!(
        typed_attributes(&entry),
        [(
            "resolutions",
            List(vec![point("640", "480"), point("5120", "2160")])
        )]
    );
}

fn dictionary_syntax(source: &str) -> Result<ScalarValue<'_>, String> {
    // The grammar assertion operates on the attribute value after parsing the entry.
    let (_, value) = source
        .lines()
        .find_map(|line| line.trim().split_once("dict:"))
        .unwrap();
    attribute_syntax(source)?;
    parse_dictionary(value.trim(), 0)
}

#[test]
fn renode_dict_empty() {
    assert_eq!(
        dictionary_syntax("\ndevice: Something\n    dict: {}").unwrap(),
        ScalarValue::Dictionary(vec![])
    );
}

fn assert_dictionary_pair(source: &str) {
    use ScalarValue::*;
    assert_eq!(
        dictionary_syntax(source).unwrap(),
        Dictionary(vec![
            (Text("key0".into()), Number("0".into())),
            (Text("key1".into()), Number("1".into())),
        ])
    );
}

#[test]
fn renode_dict_simple() {
    assert_dictionary_pair("\ndevice: Something\n    dict: {\"key0\": 0; \"key1\": 1}");
}

#[test]
fn renode_dict_trailing_separator() {
    assert_dictionary_pair("\ndevice: Something\n    dict: {\"key0\": 0; \"key1\": 1;}");
}

#[test]
fn renode_dict_missing_closing_brace() {
    assert!(
        dictionary_syntax("\ndevice: Something\n    dict: {\"key0\": 0; \"key1\": 1")
            .unwrap_err()
            .contains("Unexpected end of input")
    );
}

#[test]
fn renode_dict_comma() {
    assert!(
        dictionary_syntax("\ndevice: Something\n    dict: {\"key0\": 0, \"key1\": 1}")
            .unwrap_err()
            .contains("unexpected ','")
    );
}

#[test]
fn renode_dict_missing_separator() {
    assert!(
        dictionary_syntax("\ndevice: Something\n    dict: {\"key0\": 0 \"key1\": 1}")
            .unwrap_err()
            .contains("unexpected '\"'")
    );
}

#[test]
fn renode_dict_missing_colon() {
    assert!(
        dictionary_syntax("\ndevice: Something\n    dict: {\"key0\" 0, \"key1\": 1}")
            .unwrap_err()
            .contains("unexpected '0'")
    );
}

fn description_syntax(source: &str, root: &Path) -> Result<Vec<Entry>, String> {
    let mut entries = Vec::new();
    parse_content(
        Path::new("<platform string>"),
        root,
        source,
        root,
        &mut HashSet::new(),
        &mut entries,
    )?;
    Ok(entries)
}

#[test]
fn renode_description_parse_object_value() {
    let entries = description_syntax(
        "\ndisplay: Display\n    resolution: new Point\n        x: 640\n        y: 480\n",
        Path::new("."),
    )
    .unwrap();
    assert_eq!(
        typed_attributes(&entries[0]),
        [(
            "resolution",
            ScalarValue::Object(
                "Point",
                vec![
                    ("x", ScalarValue::Number("640".into())),
                    ("y", ScalarValue::Number("480".into()))
                ]
            )
        )]
    );
}

#[test]
fn renode_description_parse_two_entries() {
    let entries = description_syntax("\nuart1: UART @ sysbus <0x1000, +0x100>\n    someProperty: someValue\n\nuart2: UART @ sysbus <0x2000, +0x100>\n    otherProperty: \"otherValue\"\n", Path::new(".")).unwrap();
    assert_eq!(entries.len(), 2);
}

#[test]
fn renode_description_parse_local_and_non_local_entry() {
    let entries =
        description_syntax("\nperipheral: SomeHub\nlocal other: Other", Path::new(".")).unwrap();
    assert_eq!(
        entries.iter().map(|entry| entry.local).collect::<Vec<_>>(),
        [false, true]
    );
}

#[test]
fn renode_description_parse_example() {
    let entries = description_syntax("\nscreen: Display@graphicsCard1\n    resolution: new Point { x: 5; y: 6 }\n    refreshMode: Automatic\n    model: \"SuperDisplay\"\n    -> ic@3\n    init:\n        DrawCircle 20 20\n        DrawCircle 30 40\n\nscreen:\n    init:\n        DrawRect 40 50 1\n\nscreen:\n    init:\n        base init\n        DrawRect 40 50 1\n\nother: Display @ sysbus <0x0, +0x100> { refreshMode: Automatic; -> ic@3 }\n", Path::new(".")).unwrap();
    assert_eq!(entries.len(), 4);
}

#[test]
fn renode_description_parse_two_short_entries() {
    let entries = description_syntax(
        "\nuart: UART\n\nuart: @sysbus <0x100, +0x100>\n",
        Path::new("."),
    )
    .unwrap();
    assert_eq!(entries.len(), 2);
}

#[test]
fn renode_description_parse_using_and_entry() {
    let source = "\nusing \"other_file\"\n\ndevice: SomeDevice";
    let directory =
        std::env::temp_dir().join(format!("penv-using-and-entry-{}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    fs::write(directory.join("other_file"), "").unwrap();
    let parsed = description_syntax(source, &directory);
    fs::remove_dir_all(directory).unwrap();
    assert_eq!(
        parse_using_statement(source.lines().nth(1).unwrap())
            .unwrap()
            .as_deref(),
        Some("other_file")
    );
    let entries = parsed.unwrap();
    assert_eq!(entries[0].name, "device");
    assert_eq!(entries[0].model, "SomeDevice");
}

#[test]
fn renode_description_mixed_line_endings() {
    let entries = description_syntax(
        "peripheral: SomeHub\nlocal first_other: Other\r\nlocal second_other: Other\r\n",
        Path::new("."),
    )
    .unwrap();
    assert_eq!(entries.len(), 3);
}

#[test]
fn renode_empty_keyword_type() {
    let error = check_string(
        "\nmockPeripheral: empty @ sysbus <0, 1>\n    value: 0",
        None,
    )
    .unwrap_err();
    assert!(error.contains("SyntaxError:"), "{error}");
}

#[test]
fn renode_empty_keyword_registration_destination() {
    let error = check_string("\nmockPeripheral:  Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral @ empty <0, 1>\n    value: 0", None).unwrap_err();
    assert!(error.contains("SyntaxError:"), "{error}");
}

#[test]
fn renode_empty_keyword_parameter_name() {
    let error = check_string("\nmockPeripheral:  Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral @ sysbus <0, 1>\n    empty: 0", None).unwrap_err();
    assert!(error.contains("SyntaxError:"), "{error}");
}

#[test]
fn renode_empty_keyword_using_parameter() {
    let error = check_string("using empty", None).unwrap_err();
    assert!(error.contains("SyntaxError:"), "{error}");
}

fn assert_dispatched_script(source: &str, expected_phase: ScriptPhase, expected: &[&str]) {
    let entries = merge_declarations(description_syntax(source, Path::new(".")).unwrap()).unwrap();
    let mut calls = Vec::new();
    build_entries_with_scripts(
        Path::new("<platform string>"),
        &entries,
        false,
        true,
        |entry, phase, commands, _| {
            assert_eq!(phase, expected_phase);
            assert_eq!(entry.name, entries[0].name);
            calls.push(
                commands
                    .iter()
                    .map(|command| (*command).to_owned())
                    .collect::<Vec<_>>(),
            );
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(calls, [expected]);
}

#[test]
fn renode_init_merge_replace() {
    assert_dispatched_script(
        "\nperi: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral @ sysbus <0, 1>\n    init:\n        Increment 1\n\nperi:\n    init:\n        Increment 2\n        Increment 3",
        ScriptPhase::Init,
        &["Increment 2", "Increment 3"],
    );
}

#[test]
fn renode_init_merge_append() {
    assert_dispatched_script(
        "\nperi: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral\n    init:\n        Increment 1\n\nperi:\n    init add:\n        Increment 2\n        Increment 3",
        ScriptPhase::Init,
        &["Increment 1", "Increment 2", "Increment 3"],
    );
}

#[test]
fn renode_init_merge_sysbus_applies_final_section() {
    let source = r#"
mem: Memory.MappedMemory @ sysbus 0 { size: 0x1000 }
nvic: IRQControllers.NVIC @ sysbus 0xe000e000 { -> cpu@0 }
cpu: CPU.CortexM @ sysbus { cpuType: "cortex-m4"; nvic: nvic }
sysbus:
    init:
        Tag <0x1000, 0x10ff> "old" 0x11
sysbus:
    init:
        UnhandledAccessBehaviour Report
        Tag <0x1000, 0x10ff> "new" 0x22
sysbus:
    init add:
        Tag <0x2000, 0x20ff> "extra" 0x33
"#;
    let entries = merge_declarations(description_syntax(source, Path::new(".")).unwrap()).unwrap();
    let mut machine = load_entries(Path::new("<platform string>"), &entries, false).unwrap();
    assert_eq!(machine.bus.read::<4>(0x1000).unwrap(), 0x22);
    assert_eq!(machine.bus.read::<4>(0x2000).unwrap(), 0x33);
}

#[test]
fn renode_init_merge_sysbus_dispatch() {
    assert_dispatched_script(
        "\nsysbus:\n    init:\n        WriteByte 0 1",
        ScriptPhase::Init,
        &["WriteByte 0 1"],
    );
}

#[test]
fn renode_reset_dispatch_replace() {
    assert_dispatched_script(
        "\nperi: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral @ sysbus <0, 1>\n    reset:\n        Increment 1\n\nperi:\n    reset:\n        Increment 2\n        Increment 3",
        ScriptPhase::Reset,
        &["Increment 2", "Increment 3"],
    );
}

#[test]
fn renode_reset_dispatch_append() {
    assert_dispatched_script(
        "\nperi: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral\n    reset:\n        Increment 1\n\nperi:\n    reset add:\n        Increment 2\n        Increment 3",
        ScriptPhase::Reset,
        &["Increment 1", "Increment 2", "Increment 3"],
    );
}

#[test]
fn renode_preinit_dispatch_sysbus() {
    assert_dispatched_script(
        "\nsysbus:\n    preinit:\n        echo 'a'",
        ScriptPhase::Preinit,
        &["echo 'a'"],
    );
}

#[test]
fn renode_preinit_dispatch_replace() {
    assert_dispatched_script(
        "\nperi: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral @ sysbus <0, 1>\n    preinit:\n        echo 'test 1'\n\nperi:\n    preinit:\n        echo 'test 2'\n        echo 'test 3'",
        ScriptPhase::Preinit,
        &["echo 'test 2'", "echo 'test 3'"],
    );
}

#[test]
fn renode_preinit_dispatch_append() {
    assert_dispatched_script(
        "\nperi: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral\n    preinit:\n        echo 'test 1'\n\nperi:\n    preinit add:\n        echo 'test 2'\n        echo 'test 3'",
        ScriptPhase::Preinit,
        &["echo 'test 1'", "echo 'test 2'", "echo 'test 3'"],
    );
}

#[test]
fn renode_preinit_dispatch_order() {
    let source = "\nperi: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral\n    preinit:\n        echo '3 peri preinit'\n\nsysbus:\n    preinit:\n        echo '1 sysbus preinit'\n\nperi2: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral\n    preinit:\n        echo '5 peri2 preinit'\n\nperi:\n    preinit add:\n        echo '4 peri preinit add'\n\nsysbus:\n    preinit add:\n        echo '2 sysbus preinit add'";
    let path = Path::new("<platform string>");
    let entries = merge_declarations(description_syntax(source, Path::new(".")).unwrap()).unwrap();
    let mut calls = Vec::new();
    build_entries_with_scripts(path, &entries, false, true, |entry, phase, commands, _| {
        assert_eq!(phase, ScriptPhase::Preinit);
        calls.push((
            entry.name.clone(),
            commands
                .iter()
                .map(|command| (*command).to_owned())
                .collect::<Vec<_>>(),
        ));
        Ok(())
    })
    .unwrap();
    let calls: Vec<_> = calls
        .iter()
        .map(|(name, commands)| {
            (
                name.as_str(),
                commands.iter().map(String::as_str).collect::<Vec<_>>(),
            )
        })
        .collect();
    assert_eq!(
        calls,
        [
            (
                "sysbus",
                vec!["echo '1 sysbus preinit'", "echo '2 sysbus preinit add'"]
            ),
            (
                "peri",
                vec!["echo '3 peri preinit'", "echo '4 peri preinit add'"]
            ),
            ("peri2", vec!["echo '5 peri2 preinit'"]),
        ]
    );
}
