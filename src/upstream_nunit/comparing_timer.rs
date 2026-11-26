use crate::comparing_timer::{CompareAboveLimit, ComparingTimer};
use std::num::NonZeroU64;

#[test]
fn should_throw_on_compare_higher_than_limit() {
    let mut timer = ComparingTimer::new(
        NonZeroU64::new(10).unwrap(),
        NonZeroU64::new(20).unwrap(),
        5,
    )
    .unwrap();
    assert_eq!(
        timer.set_compare(30),
        Err(CompareAboveLimit {
            compare: 30,
            limit: 20
        })
    );
    assert_eq!(timer.compare(), 5);
}
