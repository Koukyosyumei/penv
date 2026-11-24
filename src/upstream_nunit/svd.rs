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

fn read_infix(infix: &str, big: bool) -> Svd {
    let prefix = if big { BIG_PREFIX } else { PREFIX };
    parse_file(&format!("{prefix}{infix}{POSTFIX}")).unwrap()
}

const BIG_PREFIX: &str = r#"<?xml version="1.0" encoding="utf-8" standalone="no"?>
            <device>
                <description>Test description</description>
                <name>Test name</name>
                <size>32</size>
                <access>read-write</access>
                <resetValue>0xA5A5A5A5</resetValue>
                <cpu>
                    <endian>big</endian>
                </cpu>
"#;
#[test]
fn should_read_value_from_register() {
    let value = 0xdead_beefu32;
    let device = read_infix(
        &format!(
            r#"
                <peripherals>
                    <peripheral>
                        <name>Peripheral1</name>
                        <baseAddress>0x1000</baseAddress>
                        <registers>
                            <register>
                                <name>REG1</name>
                                <addressOffset>0x0</addressOffset>
                                <resetValue>{value}</resetValue>
                            </register>
                        </registers>
                    </peripheral>
                </peripherals>
"#
        ),
        false,
    );
    assert_eq!(device.read(0x1000, 4), Some(u64::from(value)));
}
#[test]
fn should_read_value_from_register_in_big_endian() {
    let value = 0xdead_beefu32;
    let device = read_infix(
        &format!(
            r#"
                <peripherals>
                    <peripheral>
                        <name>Peripheral1</name>
                        <baseAddress>0x1000</baseAddress>
                        <registers>
                            <register>
                                <name>REG1</name>
                                <addressOffset>0x0</addressOffset>
                                <resetValue>{value}</resetValue>
                            </register>
                        </registers>
                    </peripheral>
                </peripherals>
"#
        ),
        true,
    );
    assert_eq!(device.read(0x1000, 4), Some(u64::from(value.swap_bytes())));
}
#[test]
fn should_handle_different_access_permissions_write_only() {
    let access = "write-only";
    let device = read_infix(
        &format!(
            r#"
                <peripherals>
                    <peripheral>
                        <name>Peripheral1</name>
                        <baseAddress>0x1000</baseAddress>
                        <registers>
                            <register>
                                <name>REG1</name>
                                <addressOffset>0x0</addressOffset>
                                <resetValue>0x01234567</resetValue>
                                <access>{access}</access>
                            </register>
                        </registers>
                    </peripheral>
                </peripherals>
"#
        ),
        false,
    );
    assert_eq!(device.read(0x1000, 4), Some(0));
}
#[test]
fn should_handle_different_access_permissions_read_only() {
    let access = "read-only";
    let device = read_infix(
        &format!(
            r#"
                <peripherals>
                    <peripheral>
                        <name>Peripheral1</name>
                        <baseAddress>0x1000</baseAddress>
                        <registers>
                            <register>
                                <name>REG1</name>
                                <addressOffset>0x0</addressOffset>
                                <resetValue>0x01234567</resetValue>
                                <access>{access}</access>
                            </register>
                        </registers>
                    </peripheral>
                </peripherals>
"#
        ),
        false,
    );
    assert_eq!(device.read(0x1000, 4), Some(0x01234567));
}
#[test]
fn should_handle_different_access_permissions_read_write() {
    let access = "read-write";
    let device = read_infix(
        &format!(
            r#"
                <peripherals>
                    <peripheral>
                        <name>Peripheral1</name>
                        <baseAddress>0x1000</baseAddress>
                        <registers>
                            <register>
                                <name>REG1</name>
                                <addressOffset>0x0</addressOffset>
                                <resetValue>0x01234567</resetValue>
                                <access>{access}</access>
                            </register>
                        </registers>
                    </peripheral>
                </peripherals>
"#
        ),
        false,
    );
    assert_eq!(device.read(0x1000, 4), Some(0x01234567));
}
#[test]
fn should_handle_different_access_permissions_read_writeonce() {
    let access = "read-writeOnce";
    let device = read_infix(
        &format!(
            r#"
                <peripherals>
                    <peripheral>
                        <name>Peripheral1</name>
                        <baseAddress>0x1000</baseAddress>
                        <registers>
                            <register>
                                <name>REG1</name>
                                <addressOffset>0x0</addressOffset>
                                <resetValue>0x01234567</resetValue>
                                <access>{access}</access>
                            </register>
                        </registers>
                    </peripheral>
                </peripherals>
"#
        ),
        false,
    );
    assert_eq!(device.read(0x1000, 4), Some(0x01234567));
}
#[test]
fn should_handle_different_access_permissions_writeonce() {
    let access = "writeOnce";
    let device = read_infix(
        &format!(
            r#"
                <peripherals>
                    <peripheral>
                        <name>Peripheral1</name>
                        <baseAddress>0x1000</baseAddress>
                        <registers>
                            <register>
                                <name>REG1</name>
                                <addressOffset>0x0</addressOffset>
                                <resetValue>0x01234567</resetValue>
                                <access>{access}</access>
                            </register>
                        </registers>
                    </peripheral>
                </peripherals>
"#
        ),
        false,
    );
    assert_eq!(device.read(0x1000, 4), Some(0));
}
#[test]
fn should_read_from_registers_of_different_sizes() {
    let value = 0xdead_beefu32;
    for i in 1..32 {
        let device = read_infix(
            &format!(
                r#"
                    <peripherals>
                        <peripheral>
                            <name>Peripheral1</name>
                            <baseAddress>0x1000</baseAddress>
                            <registers>
                                <register>
                                   <name>REG1</name>
                                    <addressOffset>0x0</addressOffset>
                                    <size>{i}</size>
                                    <resetValue>{value}</resetValue>
                                    <access>read-write</access>
                                </register>
                            </registers>
                        </peripheral>
                    </peripherals>
"#
            ),
            false,
        );
        let expected = u64::from(value) & ((1u64 << i) - 1);
        assert_eq!(device.read(0x1000, 4), Some(expected));
    }
}
#[test]
fn should_read_from_unaligned_offset_in_one_peripheral() {
    let bytes = [11u8, 22, 33, 44, 55, 66, 77, 88];
    let first = u32::from_le_bytes(bytes[..4].try_into().unwrap());
    let last = i16::from_le_bytes(bytes[6..].try_into().unwrap());
    let fifth = bytes[4];
    let sixth = bytes[5];
    let device = read_infix(
        &format!(
            r#"
                <peripherals>
                    <peripheral>
                        <access>read-write</access>
                        <name>Peripheral1</name>
                        <baseAddress>0x1000</baseAddress>
                        <registers>
                            <register>
                                <name>REG1</name>
                                <addressOffset>0x0</addressOffset>
                                <resetValue>{first}</resetValue>
                                <size>32</size>
                            </register>
                            <register>
                                <name>REG2</name>
                                <addressOffset>0x4</addressOffset>
                                <resetValue>{fifth}</resetValue>
                                <size>8</size>
                            </register>
                            <register>
                                <name>REG3</name>
                                <addressOffset>0x5</addressOffset>
                                <resetValue>{sixth}</resetValue>
                                <size>8</size>
                            </register>
                            <register>
                                <name>REG4</name>
                                <addressOffset>0x6</addressOffset>
                                <resetValue>{last}</resetValue>
                                <size>16</size>
                            </register>
                        </registers>
                    </peripheral>
                </peripherals>
"#
        ),
        false,
    );
    let expected_bytes = bytes;
    for offset in -3isize..8 {
        let expected = std::array::from_fn(|index| {
            let position = offset + index as isize;
            usize::try_from(position)
                .ok()
                .and_then(|index| expected_bytes.get(index))
                .copied()
                .unwrap_or(0)
        });
        assert_eq!(
            device.read((0x1000 + offset) as u64, 4),
            Some(u32::from_le_bytes(expected).into())
        );
    }
}
#[test]
fn should_read_from_unaligned_offset_in_many_peripherals() {
    let bytes = [11u8, 22, 33, 44, 55, 66, 77, 88];
    let first = u32::from_le_bytes(bytes[..4].try_into().unwrap());
    let last = i16::from_le_bytes(bytes[6..].try_into().unwrap());
    let fifth = bytes[4];
    let sixth = bytes[5];
    let device = read_infix(
        &format!(
            r#"
                <peripherals>

                    <peripheral>
                        <access>read-write</access>
                        <name>Peripheral1</name>
                        <baseAddress>0x1000</baseAddress>
                        <registers>
                            <register>
                                <name>REG1</name>
                                <addressOffset>0x0</addressOffset>
                                <resetValue>{first}</resetValue>
                                <size>32</size>
                            </register>
                        </registers>
                    </peripheral>

                    <peripheral>
                        <access>read-write</access>
                        <name>Peripheral2</name>
                        <baseAddress>0x1004</baseAddress>
                        <registers>
                            <register>
                                <name>REG2</name>
                                <addressOffset>0</addressOffset>
                                <resetValue>{fifth}</resetValue>
                                <size>8</size>
                            </register>
                            <register>
                                <name>REG3</name>
                                <addressOffset>1</addressOffset>
                                <resetValue>{sixth}</resetValue>
                                <size>8</size>
                            </register>
                        </registers>
                    </peripheral>

                    <peripheral>
                        <access>read-write</access>
                        <name>Peripheral3</name>
                        <baseAddress>0x1006</baseAddress>
                        <registers>
                            <register>
                                <name>REG4</name>
                                <addressOffset>0</addressOffset>
                                <resetValue>{last}</resetValue>
                                <size>16</size>
                            </register>
                        </registers>
                    </peripheral>
                </peripherals>
"#
        ),
        false,
    );
    let expected_bytes = bytes;
    for offset in -3isize..8 {
        let expected = std::array::from_fn(|index| {
            let position = offset + index as isize;
            usize::try_from(position)
                .ok()
                .and_then(|index| expected_bytes.get(index))
                .copied()
                .unwrap_or(0)
        });
        assert_eq!(
            device.read((0x1000 + offset) as u64, 4),
            Some(u32::from_le_bytes(expected).into())
        );
    }
}
#[test]
fn should_read_from_unaligned_offset_in_one_peripheral_in_big_endian() {
    let bytes = [11u8, 22, 33, 44, 55, 66, 77, 88];
    let first = u32::from_le_bytes(bytes[..4].try_into().unwrap());
    let last = i16::from_le_bytes(bytes[6..].try_into().unwrap());
    let fifth = bytes[4];
    let sixth = bytes[5];
    let device = read_infix(
        &format!(
            r#"
                <peripherals>
                    <peripheral>
                        <access>read-write</access>
                        <name>Peripheral1</name>
                        <baseAddress>0x1000</baseAddress>
                        <registers>
                            <register>
                                <name>REG1</name>
                                <addressOffset>0x0</addressOffset>
                                <resetValue>{first}</resetValue>
                                <size>32</size>
                            </register>
                            <register>
                                <name>REG2</name>
                                <addressOffset>0x4</addressOffset>
                                <resetValue>{fifth}</resetValue>
                                <size>8</size>
                            </register>
                            <register>
                                <name>REG3</name>
                                <addressOffset>0x5</addressOffset>
                                <resetValue>{sixth}</resetValue>
                                <size>8</size>
                            </register>
                            <register>
                                <name>REG4</name>
                                <addressOffset>0x6</addressOffset>
                                <resetValue>{last}</resetValue>
                                <size>16</size>
                            </register>
                        </registers>
                    </peripheral>
                </peripherals>
"#
        ),
        true,
    );
    let mut expected_bytes = bytes;
    expected_bytes[..4].reverse();
    expected_bytes[6..].reverse();
    for offset in -3isize..8 {
        let expected = std::array::from_fn(|index| {
            let position = offset + index as isize;
            usize::try_from(position)
                .ok()
                .and_then(|index| expected_bytes.get(index))
                .copied()
                .unwrap_or(0)
        });
        assert_eq!(
            device.read((0x1000 + offset) as u64, 4),
            Some(u32::from_le_bytes(expected).into())
        );
    }
}
#[test]
fn should_read_from_unaligned_offset_in_one_peripheral_with_write_only_register() {
    let bytes = [11u8, 22, 33, 44, 55, 66, 77, 88];
    let first = u32::from_le_bytes(bytes[..4].try_into().unwrap());
    let last = i16::from_le_bytes(bytes[6..].try_into().unwrap());
    let fifth = bytes[4];
    let sixth = bytes[5];
    let device = read_infix(
        &format!(
            r#"
                <peripherals>
                    <peripheral>
                        <access>read-write</access>
                        <name>Peripheral1</name>
                        <baseAddress>0x1000</baseAddress>
                        <registers>
                            <register>
                                <name>REG1</name>
                                <addressOffset>0x0</addressOffset>
                                <resetValue>{first}</resetValue>
                                <size>32</size>
                            </register>
                            <register>
                                <name>REG2</name>
                                <addressOffset>0x4</addressOffset>
                                <resetValue>{fifth}</resetValue>
                                <access>write-only</access>
                                <size>8</size>
                            </register>
                            <register>
                                <name>REG3</name>
                                <addressOffset>0x5</addressOffset>
                                <resetValue>{sixth}</resetValue>
                                <access>write-only</access>
                                <size>8</size>
                            </register>
                            <register>
                                <name>REG4</name>
                                <addressOffset>0x6</addressOffset>
                                <resetValue>{last}</resetValue>
                                <size>16</size>
                            </register>
                        </registers>
                    </peripheral>
                </peripherals>
"#
        ),
        true,
    );
    let mut expected_bytes = bytes;
    expected_bytes[..4].reverse();
    expected_bytes[6..].reverse();
    expected_bytes[4..6].fill(0);
    for offset in -3isize..8 {
        let expected = std::array::from_fn(|index| {
            let position = offset + index as isize;
            usize::try_from(position)
                .ok()
                .and_then(|index| expected_bytes.get(index))
                .copied()
                .unwrap_or(0)
        });
        assert_eq!(
            device.read((0x1000 + offset) as u64, 4),
            Some(u32::from_le_bytes(expected).into())
        );
    }
}
#[test]
fn should_concatenate_address_offsets() {
    let base = 0x10000;
    let [offset0, offset1, offset2] = [0x1000, 0x100, 0x10];
    let device = read_infix(
        &format!(
            r#"
                <peripherals>
                    <peripheral>
                        <name>Peripheral1</name>
                        <baseAddress>{base}</baseAddress>
                        <access>read-write</access>
                        <registers>
                            <cluster>
                                <name>C0</name>
                                <addressOffset>{offset0}</addressOffset>
                                <cluster>
                                    <name>C1</name>
                                    <addressOffset>{offset1}</addressOffset>
                                    <register>
                                        <name>REG1</name>
                                        <addressOffset>{offset2}</addressOffset>
                                        <resetValue>0x1000</resetValue>
                                    </register>
                                </cluster>
                            </cluster>
                        </registers>
                    </peripheral>
                </peripherals>
"#
        ),
        false,
    );
    assert_eq!(device.read(0x11110, 4), Some(0x1000));
}
#[test]
fn should_inherit_settings_from_parent_peripheral() {
    let device = read_infix(
        r#"
                <peripherals>
                    <peripheral>
                        <name>Peripheral1</name>
                        <baseAddress>0x1000</baseAddress>
                        <size>11</size>
                        <resetValue>0xA5445E63</resetValue>
                        <access>read-writeOnce</access>
                        <registers>
                            <register>
                                <name>REG1</name>
                                <addressOffset>0x0</addressOffset>
                            </register>
                            <register>
                                <name>REG2</name>
                                <addressOffset>0x10</addressOffset>
                                <size>27</size>
                            </register>
                            <register>
                                <name>REG3</name>
                                <addressOffset>0x20</addressOffset>
                                <resetValue>0x12345678</resetValue>
                                <size>27</size>
                            </register>
                        </registers>
                    </peripheral>
                </peripherals>
"#,
        false,
    );
    assert_eq!(device.read(0x1000, 4), Some(0x00000663));
    assert_eq!(device.read(0x1010, 4), Some(0x05445e63));
    assert_eq!(device.read(0x1020, 4), Some(0x02345678));
}
#[test]
fn should_inherit_settings_from_parent_cluster() {
    let device = read_infix(
        r#"
                <peripherals>
                    <peripheral>
                        <name>Peripheral1</name>
                        <baseAddress>0x1000</baseAddress>
                        <size>32</size>
                        <resetValue>0xFFFFFFFF</resetValue>
                        <access>read-writeOnce</access>
                        <registers>
                            <cluster>
                                <name>Cluster1</name>
                                <addressOffset>0x100</addressOffset>
                                <resetValue>0xA5445E63</resetValue>
                                <size>11</size>
                                <register>
                                    <name>REG1</name>
                                    <addressOffset>0x0</addressOffset>
                                </register>
                                <register>
                                    <name>REG2</name>
                                    <addressOffset>0x10</addressOffset>
                                    <size>27</size>
                                </register>
                                <register>
                                    <name>REG3</name>
                                    <addressOffset>0x20</addressOffset>
                                    <resetValue>0x12345678</resetValue>
                                    <size>27</size>
                                </register>
                            </cluster>
                        </registers>
                    </peripheral>
                </peripherals>
"#,
        false,
    );
    assert_eq!(device.read(0x1100, 4), Some(0x00000663));
    assert_eq!(device.read(0x1110, 4), Some(0x05445e63));
    assert_eq!(device.read(0x1120, 4), Some(0x02345678));
}
#[test]
fn should_inherit_settings_from_parent_cluster_and_peripheral() {
    let device = read_infix(
        r#"
                <peripherals>
                    <peripheral>
                        <name>Peripheral1</name>
                        <baseAddress>0x1000</baseAddress>
                        <size>11</size>
                        <resetValue>0xFFFFFFFF</resetValue>
                        <access>read-writeOnce</access>
                        <registers>
                            <register>
                                <name>REG1</name>
                                <addressOffset>0x0</addressOffset>
                            </register>
                            <cluster>
                                <name>Cluster1</name>
                                <addressOffset>0x100</addressOffset>
                                <resetValue>0xA5445E63</resetValue>
                                <size>27</size>
                                <register>
                                    <name>REG2</name>
                                    <addressOffset>0x10</addressOffset>
                                </register>
                                <register>
                                    <name>REG3</name>
                                    <addressOffset>0x20</addressOffset>
                                    <resetValue>0x12345678</resetValue>
                                </register>
                            </cluster>
                        </registers>
                    </peripheral>
                </peripherals>
"#,
        false,
    );
    assert_eq!(device.read(0x1000, 4), Some(0x000007ff));
    assert_eq!(device.read(0x1110, 4), Some(0x05445e63));
    assert_eq!(device.read(0x1120, 4), Some(0x02345678));
}
#[test]
fn should_throw_on_deriving_from_nonexisting_register() {
    assert!(parse_infix(
        r#"
                    <peripherals>
                        <peripheral>
                            <name>Peripheral1</name>
                            <baseAddress>0x1000</baseAddress>
                            <access>read-write</access>
                            <registers>
                                <register>
                                    <name>REG1</name>
                                    <addressOffset>0x0</addressOffset>
                                    <size>32</size>
                                    <resetValue>0xF2468ACE</resetValue>
                                </register>
                                <register derivedFrom="invalidObject">
                                    <name>REG2</name>
                                    <addressOffset>0x10</addressOffset>
                                </register>
                            </registers>
                        </peripheral>
                    </peripherals>
"#
    )
    .is_err());
}
#[test]
fn should_derive_from_register_in_the_same_scope() {
    let device = read_infix(
        r#"
                <peripherals>
                    <peripheral>
                        <name>Peripheral1</name>
                        <baseAddress>0x1000</baseAddress>
                        <access>read-write</access>
                        <registers>
                            <register>
                                <name>REG1</name>
                                <addressOffset>0x0</addressOffset>
                                <size>32</size>
                                <resetValue>0xF2468ACE</resetValue>
                            </register>
                            <register derivedFrom="REG1">
                                <name>REG2</name>
                                <addressOffset>0x10</addressOffset>
                            </register>
                        </registers>
                    </peripheral>
                </peripherals>
"#,
        false,
    );
    assert_eq!(device.read(0x1010, 4), Some(0xf2468ace));
}
#[test]
fn should_derive_from_register_in_different_cluster() {
    let device = read_infix(
        r#"
                <peripherals>
                    <peripheral>
                        <name>Peripheral1</name>
                        <baseAddress>0x1000</baseAddress>
                        <access>read-write</access>
                        <registers>
                            <cluster>
                                <name>C0</name>
                                <addressOffset>0x0</addressOffset>
                                <register>
                                    <name>REG1</name>
                                    <addressOffset>0x0</addressOffset>
                                    <size>32</size>
                                    <resetValue>0xF2468ACE</resetValue>
                                </register>
                            </cluster>
                            <register derivedFrom="Peripheral1.C0.REG1">
                                <name>REG2</name>
                                <addressOffset>0x10</addressOffset>
                            </register>
                        </registers>
                    </peripheral>
                </peripherals>
"#,
        false,
    );
    assert_eq!(device.read(0x1010, 4), Some(0xf2468ace));
}
#[test]
fn should_derive_from_register_in_different_peripheral() {
    let device = read_infix(
        r#"
                <peripherals>
                    <peripheral>
                        <name>Peripheral1</name>
                        <baseAddress>0x1000</baseAddress>
                        <access>read-write</access>
                        <registers>
                            <cluster>
                                <name>C0</name>
                                <size>32</size>
                                <addressOffset>0x0</addressOffset>
                                <register>
                                    <name>REG1</name>
                                    <addressOffset>0x0</addressOffset>
                                    <resetValue>0xF2468ACE</resetValue>
                                </register>
                            </cluster>
                        </registers>
                    </peripheral>
                    <peripheral>
                        <name>Peripheral2</name>
                        <baseAddress>0x2000</baseAddress>
                        <access>read-write</access>
                        <registers>
                            <register derivedFrom="Peripheral1.C0.REG1">
                                <name>REG2</name>
                                <addressOffset>0x10</addressOffset>
                            </register>
                        </registers>
                    </peripheral>
                </peripherals>
"#,
        false,
    );
    assert_eq!(device.read(0x2010, 4), Some(0xf2468ace));
}
