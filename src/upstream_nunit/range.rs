use crate::address_range::{AddressRange as Range, MinimalRanges, RangeError};

fn range(start: u64, size: u64) -> Range {
    Range::by(start, size).unwrap()
}

#[test]
fn create_range_with_extensions() {
    let expected = Range {
        start: 0x1000,
        end: 0x10ff,
    };
    assert_eq!(Range::by(0x1000, 0x100), Ok(expected));
    assert_eq!(Range::to(0x1000, 0x10ff), Ok(expected));
    let one_byte = Range { start: 0, end: 0 };
    assert_eq!(Range::by(0, 1), Ok(one_byte));
    assert_eq!(Range::to(0, 0), Ok(one_byte));
}

#[test]
fn should_handle_creating_range_from_zero_to_ulong_max_value() {
    assert_eq!(Range::to(0, u64::MAX), Err(RangeError::FullAddressSpace));
}

#[test]
fn should_handle_creating_range_with_invalid_end_address() {
    assert_eq!(Range::to(0x1000, 0x100), Err(RangeError::InvalidEnd));
}

#[test]
fn should_handle_creating_range_with_invalid_size() {
    // The Rust start/size constructor is also the equivalent of Renode's By extension.
    assert_eq!(
        Range::by(0x1000, u64::MAX),
        Err(RangeError::InvalidSize(u64::MAX))
    );
}

#[test]
fn should_intersect_range() {
    let first = range(0x1000, 0x200);
    let second = range(0x1100, 0x300);
    let expected = Some(range(0x1100, 0x100));
    assert_eq!(first.intersect(second), expected);
    assert_eq!(second.intersect(first), expected);
}

#[test]
fn should_intersect_ranges_with_one_common_address() {
    assert_eq!(
        range(0x1000, 0x200).intersect(range(0x11ff, 0x300)),
        Some(range(0x11ff, 1))
    );
}

#[test]
fn should_shift_range() {
    assert_eq!(
        range(0x2200, 0x200).shift_by(0x300),
        Ok(range(0x2500, 0x200))
    );
}

#[test]
fn should_contain_first_address() {
    assert!(range(0x1600, 0xa00).contains(0x1600));
}

#[test]
fn should_contain_last_address() {
    assert!(range(0x1600, 0xa00).contains(0x1fff));
}

#[test]
fn should_not_contain_nearby_addresses() {
    let region = range(0x600, 0x100);
    assert!(!region.contains(0x5ff));
    assert!(!region.contains(0x700));
}

#[test]
fn should_contain_itself() {
    let region = range(0x1000, 0x1200);
    assert!(region.contains_range(region));
}

fn check_subtraction(subtracted: Range, expected: &[Range]) {
    let mut actual: Vec<_> = range(0x1600, 0xa00).subtract(subtracted).collect();
    actual.sort_unstable();
    assert_eq!(actual, expected);
}

#[test]
fn subtract_no_intersection() {
    check_subtraction(range(0x1000, 0x200), &[range(0x1600, 0xa00)]);
}

#[test]
fn subtract_containing() {
    check_subtraction(range(0x1000, 0x1200), &[]);
}

#[test]
fn subtract_itself() {
    check_subtraction(range(0x1600, 0xa00), &[]);
}

#[test]
fn subtract_left() {
    check_subtraction(range(0x1000, 0x800), &[range(0x1800, 0x800)]);
}

#[test]
fn subtract_right() {
    check_subtraction(range(0x1800, 0x800), &[range(0x1600, 0x200)]);
}

#[test]
fn subtract_contained() {
    check_subtraction(
        range(0x1800, 0x200),
        &[range(0x1600, 0x200), range(0x1a00, 0x600)],
    );
}

#[test]
fn minimal_ranges_collection_add() {
    let mut ranges = MinimalRanges::default();
    assert_eq!(ranges.ranges().len(), 0);
    ranges.add(range(0x1000, 0x400)).unwrap();
    ranges.add(range(0x1800, 0x800)).unwrap();
    assert_eq!(ranges.ranges().len(), 2);
    ranges.add(range(0, 0x1000)).unwrap();
    assert_eq!(ranges.ranges().len(), 2);
    ranges.add(range(0x2000, 0x400)).unwrap();
    assert_eq!(ranges.ranges().len(), 2);
    ranges.add(range(0x1000, 0x1000)).unwrap();
    assert_eq!(ranges.ranges().len(), 1);
    assert_eq!(ranges.ranges(), &[range(0, 0x2400)]);
}

#[test]
fn minimal_ranges_collection_remove_outside() {
    let mut ranges = MinimalRanges::default();
    let original = range(0x1000, 0x1000);
    ranges.add(original).unwrap();
    assert!(!ranges.remove(range(0, 0x1000)));
    assert!(!ranges.remove(range(0x2000, 0x1000)));
    assert!(!ranges.remove(range(0x8000, 0x1000)));
    assert_eq!(ranges.ranges(), &[original]);
}

#[test]
fn minimal_ranges_collection_remove_subrange() {
    let mut ranges = MinimalRanges::default();
    let untouched = range(0x8000, 0x1000);
    ranges.add(range(0x1000, 0x1000)).unwrap();
    ranges.add(untouched).unwrap();
    assert!(ranges.remove(range(0x1400, 0x800)));
    assert_eq!(
        ranges.ranges(),
        &[range(0x1000, 0x400), range(0x1c00, 0x400), untouched]
    );
}
