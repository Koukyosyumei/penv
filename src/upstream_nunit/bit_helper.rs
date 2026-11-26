use crate::bit_field::{mask, masked_value, reverse_by_byte, set_masked_value};

#[test]
fn should_reverse_bits_by_byte() {
    assert_eq!(0_u8.reverse_bits(), 0);
    assert_eq!(reverse_by_byte(0), 0);
    assert_eq!(0xb2_u8.reverse_bits(), 0x4d);
    assert_eq!(reverse_by_byte(0x58d4_3cb2), 0x1a2b_3c4d);
}

#[test]
fn should_reverse_bits_by_word() {
    // Whole-word reversal swaps the two reversed halfwords. Rotate them
    // back into their original positions.
    let reverse_by_word = |value: u32| value.reverse_bits().rotate_left(16);
    assert_eq!(0_u16.reverse_bits(), 0);
    assert_eq!(reverse_by_word(0), 0);
    assert_eq!(0xb23c_u16.reverse_bits(), 0x3c4d);
    assert_eq!(reverse_by_word(0xd458_b23c), 0x1a2b_3c4d);
}

#[test]
fn should_reverse_double_word_bits() {
    assert_eq!(0_u32.reverse_bits(), 0);
    assert_eq!(0xb23c_d458_u32.reverse_bits(), 0x1a2b_3c4d);
}

#[test]
fn should_reverse_quadruple_word_bits() {
    assert_eq!(0_u64.reverse_bits(), 0);
    assert_eq!(
        0xd15e_f67a_b23c_d458_u64.reverse_bits(),
        0x1a2b_3c4d_5e6f_7a8b
    );
}

#[test]
fn should_calculate_double_word_mask() {
    assert_eq!(mask(0, 0), 0);
    assert_eq!(mask(32, 0), 0xffff_ffff);
    assert_eq!(mask(29, 3), 0xffff_fff8);
    assert_eq!(mask(30, 1), 0x7fff_fffe);
}

#[test]
fn should_calculate_quad_word_mask() {
    assert_eq!(mask(0, 0), 0);
    assert_eq!(mask(64, 0), 0xffff_ffff_ffff_ffff);
    assert_eq!(mask(61, 3), 0xffff_ffff_ffff_fff8);
    assert_eq!(mask(62, 1), 0x7fff_ffff_ffff_fffe);
}

#[test]
fn should_get_double_word_masked_value() {
    assert_eq!(masked_value(0, 0, 0), 0);
    assert_eq!(masked_value(0x1234, 32, 0), 0);
    assert_eq!(masked_value(0x1234, 0, 13), 0x1234);
    assert_eq!(masked_value(0xffff_ffff, 0, 32), 0xffff_ffff);
    assert_eq!(masked_value(0x38, 4, 1), 0x10);
}

#[test]
fn should_get_quad_word_masked_value() {
    assert_eq!(masked_value(0, 0, 0), 0);
    assert_eq!(masked_value(0x1234, 32, 0), 0);
    assert_eq!(masked_value(0x1234, 0, 13), 0x1234);
    assert_eq!(
        masked_value(0xff_ffff_ffff_ffff, 0, 64),
        0xff_ffff_ffff_ffff
    );
    assert_eq!(masked_value(0x38, 4, 1), 0x10);
}

fn check_set_masked_value(width: u32, full: u64, last: u64) {
    let mut register = 0;
    for (value, offset, size, expected) in [
        (0, 0, 0, 0),
        (0x1234, 32, 0, 0),
        (0x1234, 0, 13, 0x1234),
        (full, 0, width, full),
        (0x28, 4, 1, last),
    ] {
        register = set_masked_value(register, value, offset, size);
        assert_eq!(register, expected);
    }
}

#[test]
fn should_set_double_word_masked_value() {
    check_set_masked_value(32, 0xffff_ffff, 0xffff_ffef);
}

#[test]
fn should_set_quad_word_masked_value() {
    check_set_masked_value(64, 0xfff_ffff_ffff_ffff, 0xfff_ffff_ffff_ffef);
}
