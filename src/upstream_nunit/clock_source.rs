use crate::clock_source::{ClockEntry, ClockSource};
use std::{
    cell::{Cell, RefCell},
    num::NonZeroU64,
    time::Duration,
};

fn nonzero(value: u64) -> NonZeroU64 {
    NonZeroU64::new(value).unwrap()
}

fn advance(clock: &mut ClockSource<'_>, seconds: u64) {
    clock.advance(Duration::from_secs(seconds)).unwrap();
}

#[test]
fn should_tick_with_one_handler() {
    let count = Cell::new(0);
    let mut clock = ClockSource::default();
    clock.add(nonzero(2), nonzero(1), 0, |_| count.set(count.get() + 1));
    for (seconds, expected) in [(1, 0), (1, 1), (1, 1), (2, 2)] {
        advance(&mut clock, seconds);
        assert_eq!(count.get(), expected);
    }
}

#[test]
fn should_tick_with_two_handlers() {
    let first = Cell::new(0);
    let second = Cell::new(0);
    let mut clock = ClockSource::default();
    clock.add(nonzero(2), nonzero(1), 0, |_| first.set(first.get() + 1));
    clock.add(nonzero(5), nonzero(1), 0, |_| second.set(second.get() + 1));
    for (seconds, expected) in [(2, (1, 0)), (2, (2, 0)), (1, (2, 1))] {
        advance(&mut clock, seconds);
        assert_eq!((first.get(), second.get()), expected);
    }
}

#[test]
fn should_have_handlers_in_sync() {
    let values = RefCell::new(Vec::new());
    let mut clock = ClockSource::default();
    let first = clock.add(nonzero(10_000), nonzero(10), 0, |_| {});
    let observations = &values;
    clock.add(nonzero(10), nonzero(1), 0, move |clock| {
        observations.borrow_mut().push(clock.entry(first).value);
    });
    advance(&mut clock, 9);
    advance(&mut clock, 8);
    advance(&mut clock, 20);
    assert_eq!(*values.borrow(), [100, 200, 300]);
}

#[test]
fn should_observe_shorter_period_clock_after_add() {
    let first_count = Cell::new(0);
    let second_count = Cell::new(0);
    let mut clock = ClockSource::default();
    let first = clock.add(nonzero(1000), nonzero(1), 0, |_| {
        first_count.set(first_count.get() + 1)
    });
    advance(&mut clock, 500);
    let second = clock.add(nonzero(100), nonzero(1), 0, |_| {
        second_count.set(second_count.get() + 1)
    });
    assert_eq!(
        clock.entry(first),
        ClockEntry {
            period: 1000,
            value: 500
        }
    );
    assert_eq!(
        clock.entry(second),
        ClockEntry {
            period: 100,
            value: 0
        }
    );

    advance(&mut clock, 50);
    assert_eq!((first_count.get(), second_count.get()), (0, 0));
    assert_eq!(
        clock.entry(first),
        ClockEntry {
            period: 1000,
            value: 550
        }
    );
    assert_eq!(
        clock.entry(second),
        ClockEntry {
            period: 100,
            value: 50
        }
    );

    advance(&mut clock, 50);
    assert_eq!((first_count.get(), second_count.get()), (0, 1));
    assert_eq!(
        clock.entry(first),
        ClockEntry {
            period: 1000,
            value: 600
        }
    );
    assert_eq!(
        clock.entry(second),
        ClockEntry {
            period: 100,
            value: 0
        }
    );
}
