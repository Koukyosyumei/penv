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
