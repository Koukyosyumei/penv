use crate::svd::Svd;

use std::path::Path;

const PREFIX: &str = r#"<?xml version="1.0" encoding="utf-8" standalone="no"?>
            <device>
                <description>Test description</description>
                <name>Test name</name>
                <size>32</size>
                <access>read-write</access>
                <resetValue>0xA5A5A5A5</resetValue>
"#;

const POSTFIX: &str = r#"
            </device>
"#;

fn parse_file(content: &str) -> Result<Svd, String> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "penv-svd-{}-{}.svd",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&path, content).unwrap();
    let result = Svd::open(&path);
    std::fs::remove_file(path).unwrap();
    result
}

fn parse_infix(infix: &str) -> Result<Svd, String> {
    parse_file(&format!("{PREFIX}{infix}{POSTFIX}"))
}

#[test]

fn should_throw_on_nonexisting_file() {
    assert!(Svd::open(Path::new("invalid").join("path.svd")).is_err());
}

#[test]

fn should_throw_on_empty_file() {
    assert!(parse_file(r#""#).is_err());
}

#[test]

fn should_throw_on_invalid_xml() {
    assert!(parse_file(r#"Lorem ipsum..."#).is_err());
}

#[test]

fn should_throw_on_invalid_svd() {
    assert!(parse_file(
        r#"<?xml version="1.0" encoding="utf-8" standalone="no"?>
                    <invalidTag>
                    </invalidTag>
"#
    )
    .is_err());
}

#[test]

fn should_throw_on_device_with_no_mandatory_fields() {
    assert!(parse_file(
        r#"<?xml version="1.0" encoding="utf-8" standalone="no"?>
                    <device>
                    </device>
"#
    )
    .is_err());
}

#[test]

fn should_throw_on_device_with_no_peripherals_tag() {
    assert!(parse_infix(r#""#).is_err());
}

#[test]

fn should_throw_on_device_with_not_every_mandatory_field() {
    for item in ["descripion", "name"] {
        assert!(parse_file(&format!(
            r#"<?xml version="1.0" encoding="utf-8" standalone="no"?>
                        <device>
                            <{item}>value</{item}>
                        </device>"#
        ))
        .is_err());
    }
}

#[test]

fn should_throw_on_cpu_without_endianness() {
    assert!(parse_file(
        r#"<?xml version="1.0" encoding="utf-8" standalone="no"?>
                    <device>
                        <description>Test description</description>
                        <name>Test name</name>
                        <cpu>
                        </cpu>
                    </device>"#
    )
    .is_err());
}

#[test]

fn should_throw_on_peripheral_with_not_every_mandatory_field_name() {
    let tag = "name";

    assert!(parse_infix(&format!(
        r#"
                    <peripherals>
                        <peripheral>
                            <{tag}>0x1000</{tag}>
                        </peripheral>
                    </peripherals>
"#
    ))
    .is_err());
}

#[test]

fn should_throw_on_peripheral_with_not_every_mandatory_field_base_address() {
    let tag = "baseAddress";

    assert!(parse_infix(&format!(
        r#"
                    <peripherals>
                        <peripheral>
                            <{tag}>0x1000</{tag}>
                        </peripheral>
                    </peripherals>
"#
    ))
    .is_err());
}

#[test]

fn should_throw_on_cluster_without_mandatory_fields() {
    assert!(parse_infix(
        r#"
                    <peripherals>
                        <peripheral>
                            <name>Peripheral1</name>
                            <baseAddress>0x1000</baseAddress>
                            <registers>
                                <cluster>
                                    <!--addressOffset and name missing-->
                                    <register>
                                        <name>REG1</name>
                                        <addressOffset>0x0</addressOffset>
                                        <resetValue>0</resetValue>
                                    </register>
                                </cluster>
                            </registers>
                        </peripheral>
                    </peripherals>
"#
    )
    .is_err());
}

#[test]

fn should_throw_on_register_with_not_every_mandatory_field_name() {
    let tag = "name";

    assert!(parse_infix(&format!(
        r#"
                    <peripherals>
                        <peripheral>
                            <name>Peripheral1</name>
                            <baseAddress>0x1000</baseAddress>
                            <registers>
                                <cluster>
                                    <addressOffset>0</addressOffset>
                                    <register>
                                        <{tag}>0x1000</{tag}>
                                    </register>
                                </cluster>
                            </registers>
                        </peripheral>
                    </peripherals>
"#
    ))
    .is_err());
}

#[test]

fn should_throw_on_register_with_not_every_mandatory_field_address_offset() {
    let tag = "addressOffset";

    assert!(parse_infix(&format!(
        r#"
                    <peripherals>
                        <peripheral>
                            <name>Peripheral1</name>
                            <baseAddress>0x1000</baseAddress>
                            <registers>
                                <cluster>
                                    <addressOffset>0</addressOffset>
                                    <register>
                                        <{tag}>0x1000</{tag}>
                                    </register>
                                </cluster>
                            </registers>
                        </peripheral>
                    </peripherals>
"#
    ))
    .is_err());
}

#[test]

fn should_throw_on_device_with_invalid_size() {
    assert!(parse_file(
        r#"<?xml version="1.0" encoding="utf-8" standalone="no"?>
                    <device>
                        <description>Test description</description>
                        <name>Test name</name>
                        <size>invalidValue</size>
                    </device>"#
    )
    .is_err());
}

#[test]

fn should_throw_on_device_with_invalid_reset_value() {
    assert!(parse_file(
        r#"<?xml version="1.0" encoding="utf-8" standalone="no"?>
                    <device>
                        <description>Test description</description>
                        <name>Test name</name>
                        <resetValue>invalidValue</resetValue>
                        <peripherals>
                        </peripherals>
                    </device>"#
    )
    .is_err());
}

#[test]

fn should_throw_on_device_with_invalid_access() {
    assert!(parse_file(
        r#"<?xml version="1.0" encoding="utf-8" standalone="no"?>
                    <device>
                        <description>Test description</description>
                        <name>Test name</name>
                        <access>invalidValue</access>
                    </device>"#
    )
    .is_err());
}

#[test]

fn should_throw_on_cpu_with_invalid_endianness() {
    assert!(parse_file(
        r#"<?xml version="1.0" encoding="utf-8" standalone="no"?>
                    <device>
                        <description>Test description</description>
                        <name>Test name</name>
                        <cpu>
                            <endian>invalidValue</endian>
                        </cpu>
                        <peripherals>
                        </peripherals>
                    </device>"#
    )
    .is_err());
}

#[test]

fn should_throw_on_register_without_determined_size_and_reset_value() {
    assert!(parse_file(
        r#"<?xml version="1.0" encoding="utf-8" standalone="no"?>
                <device>
                    <description>Test description</description>
                    <name>Test name</name>
                    <access>read-write</access>
                    <peripherals>
                        <peripheral>
                            <name>name</name>
                            <baseAddress>0</baseAddress>
                            <registers>
                                <register>
                                    <name>REG1</name>
                                    <addressOffset>0</addressOffset>
                                </register>
                            </registers>
                        </peripheral>
                    </peripherals>
                </device>"#
    )
    .is_err());
}
