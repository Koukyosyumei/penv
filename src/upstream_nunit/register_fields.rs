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

use crate::register_fields::{FieldId, ReadMode as R, WriteMode as W};
const RESET: u64 = 0x3780;
const READ_MASK: u64 = 0b0000_1111_1110_0111__1111_1111_1111_1111;

fn complex_register() -> (Register<'static>, [FieldId; 21]) {
    let mut reg = Register::new(64, RESET);
    let fields = [
        (0, 2, R::Value, W::Value),
        (2, 1, R::Value, W::Value),
        (3, 4, R::Value, W::Value),
        (7, 2, R::Value, W::Value),
        (9, 1, R::Value, W::Value),
        (10, 4, R::Value, W::Value),
        (15, 1, R::Value, W::OneToggle),
        (16, 1, R::Value, W::SetBits),
        (17, 1, R::Value, W::OneClear),
        (18, 1, R::Value, W::ZeroClear),
        (19, 1, R::Clear, W::Value),
        (20, 1, R::None, W::Value),
        (21, 1, R::Value, W::None),
        (22, 3, R::Value, W::Value),
        (25, 1, R::Value, W::Value),
        (26, 2, R::Value, W::Value),
        (28, 1, R::None, W::ZeroSet),
        (29, 1, R::None, W::ZeroToggle),
        (30, 1, R::Set, W::None),
        (31, 1, R::Value, W::Clear),
    ]
    .map(|(shift, width, read, write)| {
        let id = reg.define(shift, width).unwrap();
        reg.modes(id, read, write);
        id
    });
    let top = reg.define(32, 1).unwrap();
    reg.modes(top, R::Value, W::Set);
    (
        reg,
        std::array::from_fn(|index| fields.get(index).copied().unwrap_or(top)),
    )
}

macro_rules! field_case {
    ($name:ident, |$reg:ident, $fields:ident| $body:block) => {
        #[test]
        fn $name() {
            let (mut $reg, $fields) = complex_register();
            $body
        }
    };
}

field_case!(should_read_bool_field, |reg, f| {
    reg.write(1 << 2);
    assert_eq!(reg.get(f[1]), 1);
});
field_case!(should_read_enum_field, |reg, f| {
    reg.write(3);
    assert_eq!(reg.get(f[0]), 3);
});
field_case!(should_read_value_field, |reg, f| {
    reg.write(88);
    assert_eq!(reg.get(f[2]), 11);
});
field_case!(should_write_bool_field, |reg, f| {
    reg.set(f[1], 1).unwrap();
    assert_eq!(reg.read(), (1 << 2) | RESET);
});
field_case!(should_write_enum_field, |reg, f| {
    reg.set(f[0], 3).unwrap();
    assert_eq!(reg.read(), 3 | RESET);
});
field_case!(should_write_value_field, |reg, f| {
    reg.set(f[2], 11).unwrap();
    assert_eq!(reg.read(), 88 | RESET);
});
field_case!(should_reset_complex_register, |reg, _f| {
    reg.reset();
    assert_eq!(reg.read(), 0x3780);
});
field_case!(should_not_write_unwritable_field, |reg, f| {
    reg.write(1 << 21);
    assert_eq!(reg.get(f[12]), 0);
});
field_case!(should_not_read_unreadable_field, |reg, f| {
    reg.set(f[11], 1).unwrap();
    assert_eq!(reg.read(), RESET);
});
field_case!(should_write_zero_to_clear, |reg, f| {
    reg.set(f[9], 1).unwrap();
    assert_eq!(reg.read(), (1 << 18) | RESET);
    reg.write(0);
    assert_eq!(reg.get(f[9]), 0);
    assert_eq!(reg.read() & READ_MASK, 0);
});
field_case!(should_write_one_to_clear, |reg, f| {
    reg.set(f[8], 1).unwrap();
    assert_eq!(reg.read() & READ_MASK, (1 << 17) | RESET);
    reg.write(1 << 17);
    assert_eq!(reg.get(f[9]), 0);
    assert_eq!(reg.read() & READ_MASK, 0);
});
field_case!(should_write_to_clear, |reg, f| {
    reg.set(f[19], 1).unwrap();
    assert_eq!(reg.read(), (1 << 31) | RESET);
    reg.write(1 << 31);
    assert_eq!(reg.get(f[19]), 0);
    reg.set(f[19], 1).unwrap();
    reg.write(0);
    assert_eq!(reg.get(f[19]), 0);
});
field_case!(should_read_to_clear, |reg, f| {
    reg.set(f[10], 1).unwrap();
    assert_eq!(reg.read(), (1 << 19) | RESET);
    assert_eq!(reg.get(f[10]), 0);
});
field_case!(should_write_zero_to_set, |reg, f| {
    reg.set(f[16], 0).unwrap();
    for (value, expected) in [(1 << 28, 0), (0, 1), (0, 1), (1 << 28, 1)] {
        reg.write(value);
        assert_eq!(reg.get(f[16]), expected);
    }
});
field_case!(should_write_zero_to_toggle, |reg, f| {
    reg.set(f[17], 0).unwrap();
    for (value, expected) in [(1 << 29, 0), (0, 1), (1 << 29, 1), (0, 0)] {
        reg.write(value);
        assert_eq!(reg.get(f[17]), expected);
    }
});
field_case!(should_read_to_set, |reg, f| {
    reg.set(f[18], 0).unwrap();
    assert_eq!(reg.read(), !(1 << 30) & RESET);
    assert_eq!(reg.get(f[18]), 1);
});
field_case!(should_work_with_undefined_enum_value, |reg, f| {
    reg.write(2);
    assert_eq!(reg.get(f[0]), 2);
});
field_case!(should_toggle_field, |reg, f| {
    for expected in [1, 0, 1] {
        reg.write(1 << 15);
        assert_eq!(reg.get(f[6]), expected);
    }
});
field_case!(should_set_field, |reg, f| {
    reg.write(1 << 16);
    assert_eq!(reg.get(f[7]), 1);
    reg.write(0);
    assert_eq!(reg.get(f[7]), 1);
});
field_case!(should_write_to_set, |reg, f| {
    reg.reset();
    assert_eq!(reg.read(), RESET);
    reg.set(f[20], 0).unwrap();
    reg.write(1 << 32);
    assert_eq!(reg.get(f[20]), 1);
    reg.set(f[20], 0).unwrap();
    reg.write(0);
    assert_eq!(reg.get(f[20]), 1);
});
