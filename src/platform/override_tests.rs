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
