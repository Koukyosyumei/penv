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
