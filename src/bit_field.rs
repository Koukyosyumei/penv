pub(crate) fn mask(width: u32, position: u32) -> u64 {
    assert!(position <= u64::BITS && width <= u64::BITS - position);
    match width {
        0 => 0,
        64 => u64::MAX,
        _ => ((1 << width) - 1) << position,
    }
}

pub(crate) fn masked_value(register: u64, position: u32, width: u32) -> u64 {
    register & mask(width, position)
}

pub(crate) fn set_masked_value(register: u64, value: u64, position: u32, width: u32) -> u64 {
    let mask = mask(width, position);
    (register & !mask) | (value.unbounded_shl(position) & mask)
}

pub(crate) fn reverse_by_byte(value: u32) -> u32 {
    value.reverse_bits().swap_bytes()
}
