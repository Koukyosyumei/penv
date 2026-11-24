use crate::time_source::{
    TimeSource,
    paced::{MasterTimeSource, SlaveTimeSource},
};
use std::time::{Duration, Instant};

#[test]
fn should_not_sleep_on_advance_immediately() {
    const SLAVES: usize = 5;
    const ROUNDS: u64 = 3;
    const QUANTUM: u64 = 1_000_000_000;
    let mut master = MasterTimeSource::<QUANTUM>::new(true);
    let mut sinks = std::array::from_fn::<_, SLAVES, _>(|_| TimeSource::<QUANTUM>::default());
    let stopwatch = Instant::now();

    master
        .run(ROUNDS + 1, |boundary| {
            for sink in &mut sinks {
                sink.grant_until(boundary);
            }
            Ok(())
        })
        .unwrap();

    assert!(stopwatch.elapsed() < Duration::from_secs(ROUNDS));
}

#[test]
fn should_not_tick_disconnected_slave_time_source() {
    let mut master = MasterTimeSource::<10>::new(true);
    let mut slave = SlaveTimeSource::<10>::default();
    let mut sink = TimeSource::<10>::default();

    master
        .run(2, |boundary| {
            slave.advance_to(boundary, |end| sink.grant_until(end))
        })
        .unwrap();
    assert_eq!(master.sync_points(), 2);
    assert_eq!(slave.sync_points(), 2);
    assert_eq!(sink.sync_points().saturating_sub(1), 1);
    assert_eq!(master.elapsed(), 10);
    assert_eq!(slave.elapsed(), 10);
    assert_eq!(sink.elapsed(), 10);

    slave.disconnect();
    master
        .run(1, |boundary| {
            slave.advance_to(boundary, |end| sink.grant_until(end))
        })
        .unwrap();
    assert_eq!(master.sync_points(), 3);
    assert_eq!(slave.sync_points(), 2);
    assert_eq!(sink.sync_points().saturating_sub(1), 1);
    assert_eq!(master.elapsed(), 20);
    assert_eq!(slave.elapsed(), 10);
    assert_eq!(sink.elapsed(), 10);
}
