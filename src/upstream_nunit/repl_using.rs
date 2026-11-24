use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

pub(super) struct Files(pub(super) PathBuf);

impl Files {
    pub(super) fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let directory = std::env::temp_dir().join(format!(
            "penv-nunit-using-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&directory).unwrap();
        Self(directory)
    }
}

impl Drop for Files {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn using_error(source: &str, includes: &[(&str, &str)], diagnostic: &str) {
    let files = Files::new();
    for (name, content) in includes {
        fs::write(files.0.join(name), content).unwrap();
    }
    let error = parse_inline(source, &files.0).unwrap_err();
    assert!(error.contains(diagnostic), "{error}");
}

#[test]
fn should_fail_on_duplicate_using_entry() {
    using_error(
        "\nusing \"A\"\nusing \"B\"\nusing \"A\"\ncpu: Antmicro.Renode.UnitTests.Mocks.MockCPU @ sysbus\n",
        &[
            (
                "A",
                "\ncpu:\n    Placeholder: \"set by A first\"\ncpu:\n    Placeholder: \"set by A second\"\n",
            ),
            (
                "B",
                "\ncpu:\n    Placeholder: \"set by B first\"\ncpu:\n    Placeholder: \"set by B second\"\n",
            ),
        ],
        "Duplicate using entry.",
    );
}

#[test]
fn should_fail_on_recurring_usings() {
    using_error(
        "\nusing \"B\"\np3: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral",
        &[
            (
                "A",
                "\nusing \"B\"\np2: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral",
            ),
            (
                "B",
                "\nusing \"A\"\np: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral",
            ),
        ],
        "There is a cycle in using file dependency.",
    );
}

#[test]
fn should_fail_on_directly_recurring_usings() {
    using_error(
        "\nusing \"A\"\np2: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral",
        &[(
            "A",
            "\nusing \"A\"\np: Antmicro.Renode.UnitTests.Mocks.EmptyPeripheral",
        )],
        "There is a cycle in using file dependency.",
    );
}

#[test]
fn should_fail_on_non_existing_using_file() {
    using_error("\nusing \"A\"", &[], "Using 'A' does not exist");
}

fn resolved(path: &str, includer: &str) -> PathBuf {
    resolve_using(path, Path::new(includer).parent().unwrap(), Path::new(".")).unwrap()
}

#[test]
#[cfg_attr(not(windows), ignore = "Windows path semantics")]
fn should_handle_absolute_path_windows() {
    for path in [r"C:\tmp\platform.repl", "C:/tmp/platform.repl"] {
        assert_eq!(
            resolved(path, r"C:\tmp\includer.repl").as_os_str(),
            r"C:\tmp\platform.repl"
        );
    }
}

#[test]
#[cfg_attr(not(windows), ignore = "Windows path semantics")]
fn should_handle_relative_path_in_same_directory_windows() {
    for path in [r".\platform.repl", "./platform.repl"] {
        assert_eq!(
            resolved(path, r"C:\tmp\includer.repl").as_os_str(),
            r"C:\tmp\platform.repl"
        );
    }
}

#[test]
#[cfg_attr(not(windows), ignore = "Windows path semantics")]
fn should_handle_relative_path_in_parent_directory_windows() {
    for path in [r"..\abc\platform.repl", "../abc/platform.repl"] {
        assert_eq!(
            resolved(path, r"C:\tmp\includer.repl").as_os_str(),
            r"C:\abc\platform.repl"
        );
    }
}

#[test]
#[cfg_attr(windows, ignore = "Unix path semantics")]
fn should_handle_absolute_path_unix() {
    assert_eq!(
        resolved("/tmp/platform.repl", "/tmp/includer.repl"),
        Path::new("/tmp/platform.repl")
    );
}

#[test]
#[cfg_attr(windows, ignore = "Unix path semantics")]
fn should_handle_relative_path_in_same_directory_unix() {
    assert_eq!(
        resolved("./platform.repl", "/tmp/includer.repl"),
        Path::new("/tmp/platform.repl")
    );
}

#[test]
#[cfg_attr(windows, ignore = "Unix path semantics")]
fn should_handle_relative_path_in_parent_directory_unix() {
    assert_eq!(
        resolved("../abc/platform.repl", "/tmp/includer.repl"),
        Path::new("/abc/platform.repl")
    );
}

#[test]
#[cfg_attr(windows, ignore = "Unix path semantics")]
fn backslash_should_not_be_path_separator_on_unix() {
    assert_eq!(
        resolved(r"./plat\form.repl", "/tmp/includer.repl"),
        Path::new(r"/tmp/plat\form.repl")
    );
}

pub(super) fn create_platform(
    source: &str,
    includes: &[(&str, &str)],
) -> Result<creation::CreatedPlatform, String> {
    let files = Files::new();
    for (name, content) in includes {
        fs::write(files.0.join(name), content).unwrap();
    }
    creation::create_objects(source, &files.0)
}

fn assert_override(source: &str, imports: &[(&str, &str)], expected: &str) {
    let machine = create_platform(source, imports).unwrap();
    let cpu = machine.cpu("sysbus.cpu").unwrap();
    assert_eq!(cpu.placeholder.as_deref(), Some(expected));
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

#[test]
fn renode_using_reference_direct() {
    create_platform(
        "\nusing \"A\"\ncpu2: Antmicro.Renode.UnitTests.Mocks.MockCPU\n    OtherCpu: cpu1",
        &[("A", "\ncpu1: Antmicro.Renode.UnitTests.Mocks.MockCPU")],
    )
    .unwrap();
}

#[test]
fn renode_using_reference_private() {
    assert!(
        create_platform(
            "\nusing \"A\"\ncpu2: Antmicro.Renode.UnitTests.Mocks.MockCPU\n    OtherCpu: cpu1",
            &[("A", "\nlocal cpu1: Antmicro.Renode.UnitTests.Mocks.MockCPU")]
        )
        .err()
        .unwrap()
        .contains("MissingReference:")
    );
}

#[test]
fn renode_using_reference_nested() {
    create_platform(
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
    create_platform(
        "\nusing \"A\"\notherCpu: Antmicro.Renode.UnitTests.Mocks.MockCPU",
        &[(
            "A",
            "\ncpu: Antmicro.Renode.UnitTests.Mocks.MockCPU\n    OtherCpu: otherCpu",
        )],
    )
    .unwrap();
}
