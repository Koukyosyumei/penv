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
