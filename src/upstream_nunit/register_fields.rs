use std::cell::Cell;

use crate::register_fields::{FieldError, Register};

#[test]
fn should_not_accept_out_of_bounds_values() {
    let mut register = Register::new(64, 0x3780);
    let two_bit_enum = register.define(0, 2).unwrap();
    register.define(2, 1).unwrap();
    let four_bit_value = register.define(3, 4).unwrap();
    assert_eq!(
        register.set(two_bit_enum, 1 << 2),
        Err(FieldError::ValueOutOfRange)
    );
    assert_eq!(
        register.set(four_bit_value, 1 << 4),
        Err(FieldError::ValueOutOfRange)
    );
}

#[test]
fn should_not_accept_negative_fields() {
    let mut register = Register::new(32, 0);
    assert!(matches!(
        register.define(0, -1),
        Err(FieldError::InvalidRange)
    ));
    assert!(matches!(
        register.define(0, -1),
        Err(FieldError::InvalidRange)
    ));
}

#[test]
fn should_not_exceed_register_size() {
    for (width, position) in [(64, 63), (32, 31), (16, 15), (8, 7)] {
        let mut register = Register::new(width, 0);
        assert!(matches!(
            register.define(position, 2),
            Err(FieldError::InvalidRange)
        ));
    }
}

#[test]
fn should_not_allow_intersecting_fields() {
    let mut register = Register::new(64, 0);
    register.define(1, 5).unwrap();
    assert!(matches!(register.define(0, 2), Err(FieldError::Overlap)));
}

#[test]
fn should_write_fields_with_max_length() {
    let mut register = Register::new(32, 0);
    register.define(0, 32).unwrap();
    register.write(u32::MAX.into());
    assert_eq!(register.read(), u64::from(u32::MAX));
}

#[repr(u64)]
enum SixtyFourBitEnum {
    A = 0x0123456789123456,
    B = 0xffffffffffffffff,
}

#[test]
fn should_read_64_bit_wide_enum() {
    let mut register = Register::new(64, 0);
    let enumeration = register.define(0, 64).unwrap();
    register.write(u64::MAX);
    assert_eq!(register.get(enumeration), SixtyFourBitEnum::B as u64);
}

#[test]
fn should_write_64_bit_wide_enum() {
    let mut register = Register::new(64, 0);
    let enumeration = register.define(0, 64).unwrap();
    register
        .set(enumeration, SixtyFourBitEnum::A as u64)
        .unwrap();
    assert_eq!(register.read(), SixtyFourBitEnum::A as u64);
}

#[test]
fn should_handle_64_bit_wide_registers_properly() {
    let test = Cell::new(0);
    let mut register = Register::new(64, 0);
    let value = register.define(0, 64).unwrap();
    register.on_write(value, |_, new| test.set(new));
    register.write(0xDEADBEEF01234567);
    assert_eq!(test.get(), 0xDEADBEEF01234567);
}

#[test]
fn should_handle_32_bit_wide_registers_properly() {
    let test = Cell::new(0);
    let mut register = Register::new(32, 0);
    let value = register.define(0, 32).unwrap();
    register.on_write(value, |_, new| test.set(new as u32));
    register.write(0xDEADBEEF);
    assert_eq!(test.get(), 0xDEADBEEF);
}
