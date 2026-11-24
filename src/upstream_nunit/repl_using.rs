use super::*;

struct Files(PathBuf);

impl Files {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!(
            "penv-nunit-using-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap()
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
