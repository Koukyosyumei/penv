use std::time::Duration;

// TimeHandle stores virtual time as u64 nanosecond ticks and uses
// checked_add and checked_sub when accounting for grants and progress.
fn ticks(microseconds: u64) -> u64 {
    Duration::from_micros(microseconds)
        .as_nanos()
        .try_into()
        .unwrap()
}

#[test]
fn should_return_correct_result_from_substraction() {
    let lower = ticks(1242);
    let higher = ticks(1243);
    assert_eq!(higher.checked_sub(lower), Some(ticks(1)));
}

#[test]
fn should_not_allow_overflow_on_substraction() {
    let lower = ticks(1242);
    let higher = ticks(1243);
    assert_eq!(lower.checked_sub(higher), None);
}

#[test]
fn should_not_allow_overflow_on_addition() {
    let value = ticks(1);
    assert_eq!(u64::MAX.checked_add(value), None);
}
