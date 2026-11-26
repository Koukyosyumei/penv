use std::{fs, path::PathBuf};

use crate::monitor::managed_extensions::{Compiler, Metadata};

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src/upstream_nunit/fixtures/csharp")
        .join(name)
}

#[test]
fn should_not_throw_on_empty_file() {
    let mut compiler = Compiler::new().unwrap();
    let source = fixture_path("Empty.cs");
    assert!(fs::read(&source).unwrap().is_empty());
    let assembly = compiler.compile(&[source]).unwrap();
    assert!(compiler.scan(&assembly).is_ok());
}

#[test]
fn should_compile_cs_file() {
    let mut compiler = Compiler::new().unwrap();
    let before = Metadata::default();
    assert!(!before.methods.contains("GetMockString"));
    let assembly = compiler
        .compile(&[fixture_path("MockExtension.cs")])
        .unwrap();
    let after = compiler.scan(&assembly).unwrap();
    assert!(!after.methods.is_empty());
    assert!(after.methods.contains("GetMockString"));
    assert!(
        after
            .types
            .contains("Antmicro.Renode.Utilities.MockExtension")
    );
}

#[test]
fn should_handle_new_extensions_from_loaded_assembly() {
    use crate::{machine::Machine, monitor::MonitorBuiltins};

    let mut monitor = MonitorBuiltins::default();
    let mut machine = Machine::without_default_ram();
    let path = fixture_path("MockExtension.cs");
    let mut contents = String::new();
    for command in [
        "emulation GetSeed".to_owned(),
        format!("i @{}", path.to_string_lossy().replace(' ', r"\ ")),
        "emulation GetMockString".to_owned(),
    ] {
        let reply = monitor
            .execute(&mut machine, &command)
            .unwrap_or_else(|| panic!("unrecognized monitor command {command}"))
            .unwrap_or_else(|error| panic!("{command}: {error}"));
        contents.push_str(&reply);
    }
    assert!(contents.contains("this is an extension"));
}
