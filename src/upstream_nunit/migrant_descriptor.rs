use crate::migrant::type_descriptor::{
    Assembly, Differences, Field, TypeDescriptor, TypeIdentity, VersionTolerance,
};

fn identity(name: &str, assembly: &str, version: [u32; 4], guid: u8) -> TypeIdentity {
    TypeIdentity {
        name: name.into(),
        module: assembly.into(),
        module_guid: [guid; 16],
        assembly: Assembly {
            name: assembly.into(),
            version,
            culture: String::new(),
            public_key_token: Vec::new(),
        },
    }
}

fn class(name: &str, base: Option<TypeDescriptor>) -> TypeDescriptor {
    TypeDescriptor {
        identity: identity(name, "TestAssembly", [0; 4], 0),
        base: base.map(Box::new),
        fields: Vec::new(),
    }
}

fn managed_int() -> TypeIdentity {
    identity("System.Int32", "System.Private.CoreLib", [10, 0, 0, 0], 10)
}

fn field(name: &str, field_type: TypeIdentity, transient: bool) -> Field {
    Field {
        name: name.into(),
        field_type,
        transient,
    }
}

fn with_field(mut owner: TypeDescriptor, field: Field) -> TypeDescriptor {
    owner.fields.push(field);
    owner
}

fn a(base: Option<TypeDescriptor>, has_field: bool) -> TypeDescriptor {
    let owner = class("A", base);
    if has_field {
        with_field(owner, field("a", managed_int(), false))
    } else {
        owner
    }
}

fn base(has_field: bool) -> TypeDescriptor {
    let owner = class("Base", None);
    if has_field {
        with_field(owner, field("a", managed_int(), false))
    } else {
        owner
    }
}

fn compare(
    before: TypeDescriptor,
    after: TypeDescriptor,
    added: &[&str],
    removed: &[&str],
    changed: &[&str],
) {
    let differences = after.compare_with(&before, VersionTolerance::Exact);
    assert_eq!(differences.added, added);
    assert_eq!(differences.removed, removed);
    assert_eq!(differences.changed, changed);
}

#[test]
fn should_find_no_differences() {
    let class_c = class("C", Some(class("B", Some(class("A", None)))));
    assert!(
        class_c
            .compare_with(&class_c, VersionTolerance::Exact)
            .is_empty()
    );
}

#[test]
fn should_detect_field_insertion_simple() {
    compare(a(None, false), a(None, true), &["A:a"], &[], &[]);
}

#[test]
fn should_not_detect_insertion_of_transient_field() {
    let before = a(None, false);
    let after = with_field(a(None, false), field("a", managed_int(), true));
    assert!(
        after
            .compare_with(&before, VersionTolerance::Exact)
            .is_empty()
    );
}

#[test]
fn should_detect_insertion_of_overriding_field() {
    compare(
        a(Some(base(true)), false),
        a(Some(base(true)), true),
        &["A:a"],
        &[],
        &[],
    );
}

#[test]
fn should_detect_field_removal_simple() {
    compare(a(None, true), a(None, false), &[], &["A:a"], &[]);
}

#[test]
fn should_not_detect_removal_of_transient_field() {
    let before = with_field(a(None, false), field("a", managed_int(), true));
    let after = a(None, false);
    assert!(
        after
            .compare_with(&before, VersionTolerance::Exact)
            .is_empty()
    );
}

#[test]
fn should_detect_removal_of_overriding_field() {
    compare(
        a(Some(base(true)), true),
        a(Some(base(true)), false),
        &[],
        &["A:a"],
        &[],
    );
}

#[test]
fn should_handle_field_move_down_simple() {
    compare(
        a(Some(base(false)), true),
        a(Some(base(true)), false),
        &["Base:a"],
        &["A:a"],
        &[],
    );
}

#[test]
fn should_handle_field_move_up_simple() {
    compare(
        a(Some(base(true)), false),
        a(Some(base(false)), true),
        &["A:a"],
        &["Base:a"],
        &[],
    );
}

#[test]
fn should_detect_field_type_change() {
    let long = identity("System.Int64", "System.Private.CoreLib", [10, 0, 0, 0], 10);
    compare(
        a(None, true),
        with_field(a(None, false), field("a", long, false)),
        &[],
        &[],
        &["A:a"],
    );
}

#[test]
fn should_handle_assembly_version_change() {
    let describe = |minor, guid| {
        let c = TypeDescriptor {
            identity: identity("C", "TestAssembly", [1, minor, 0, 0], guid),
            base: None,
            fields: vec![field("c", managed_int(), false)],
        };
        let mut a = with_field(class("A", None), field("a", c.identity, false));
        a.identity.assembly.version = [1, minor, 0, 0];
        a.identity.module_guid = [guid; 16];
        a
    };
    let previous = describe(0, 1);
    let current = describe(1, 2);
    assert_eq!(
        current.compare_with(&previous, VersionTolerance::AllowAssemblyVersionChange),
        Differences::default()
    );
}
