/// Convert one virtual time step to peripheral clock ticks, carrying its fraction.
pub fn clock_ticks(phase: &mut u64, frequency: u32, step_hz: u32) -> u64 {
    *phase += u64::from(frequency);
    let rate = u64::from(step_hz);
    let ticks = *phase / rate;
    *phase %= rate;
    ticks
}

pub struct LimitAdvance {
    pub remaining: u64,
    pub expirations: u64,
}

impl LimitAdvance {
    pub fn events(&self, enabled: bool) -> u64 {
        if enabled { self.expirations } else { 0 }
    }
}

/// Advance a counter expressed as distance to its terminal boundary. An
/// ascending counter uses limit minus value; a descending counter uses value.
pub fn advance_limit(remaining: u64, period: u64, ticks: u64) -> LimitAdvance {
    if ticks == 0 || remaining == 0 {
        return LimitAdvance {
            remaining,
            expirations: 0,
        };
    }
    if ticks < remaining {
        return LimitAdvance {
            remaining: remaining - ticks,
            expirations: 0,
        };
    }
    if period == 0 {
        return LimitAdvance {
            remaining: 0,
            expirations: 1,
        };
    }
    let after_first = ticks - remaining;
    LimitAdvance {
        remaining: period - after_first % period,
        expirations: 1 + after_first / period,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The fixed upstream mock advances at the quantum used by each case.
    struct Fixture {
        frequency: u32,
        step_hz: u32,
        limit: u64,
        remaining: u64,
        phase: u64,
        ascending: bool,
        event_enabled: bool,
        callbacks: u64,
    }

    impl Fixture {
        fn new(frequency: u32, limit: u64, ascending: bool, step_hz: u32) -> Self {
            Self {
                frequency,
                step_hz,
                limit,
                remaining: limit,
                phase: 0,
                ascending,
                event_enabled: false,
                callbacks: 0,
            }
        }

        fn advance(&mut self, steps: u32) {
            for _ in 0..steps {
                let advance = advance_limit(
                    self.remaining,
                    self.limit,
                    clock_ticks(&mut self.phase, self.frequency, self.step_hz),
                );
                self.remaining = advance.remaining;
                self.callbacks += advance.events(self.event_enabled);
            }
        }

        fn value(&self) -> u64 {
            if self.ascending {
                self.limit - self.remaining
            } else {
                self.remaining
            }
        }

        fn switch_to_descending(&mut self) {
            self.remaining = self.value();
            self.ascending = false;
        }
    }

    #[test]
    fn renode_limit_timer_ascends() {
        let mut timer = Fixture::new(100, 100_000, true, 1);
        let mut previous = 0;
        for _ in 0..100 {
            timer.advance(1);
            assert!(timer.value() > previous);
            previous = timer.value();
        }
    }

    #[test]
    fn renode_limit_timer_descends() {
        let mut timer = Fixture::new(100, 100_000, false, 1);
        let mut previous = timer.limit;
        for _ in 0..100 {
            timer.advance(1);
            assert!(timer.value() < previous);
            previous = timer.value();
        }
    }

    fn assert_stays_within_limit(ascending: bool) {
        let mut timer = Fixture::new(1, 100, ascending, 1);
        timer.advance(99);
        for _ in 0..3 {
            assert!(timer.value() <= 100);
            timer.advance(1);
        }
    }

    #[test]
    fn renode_limit_timer_stays_within_ascending_limit() {
        assert_stays_within_limit(true);
    }

    #[test]
    fn renode_limit_timer_stays_within_descending_limit() {
        assert_stays_within_limit(false);
    }

    fn check_precision(frequency: u32, microseconds: u32) {
        let mut timer = Fixture::new(frequency, 4, false, 1_000_000);
        timer.event_enabled = true;
        timer.advance(microseconds);
        assert_eq!((timer.callbacks, timer.value()), (2, 2));
    }

    #[test]
    fn renode_limit_timer_reports_microsecond_events() {
        check_precision(1_000_000, 10);
    }

    #[test]
    fn renode_limit_timer_reports_multiple_events_within_one_microsecond() {
        check_precision(10_000_000, 1);
    }

    #[test]
    fn renode_limit_timer_preserves_value_on_direction_switch() {
        let mut timer = Fixture::new(100, 100_000, true, 1);
        timer.event_enabled = true;
        timer.advance(2);
        timer.switch_to_descending();
        timer.advance(2);
        assert_ne!(timer.callbacks, 0);
    }

    #[test]
    fn renode_limit_timer_does_not_emit_disabled_alarm() {
        let mut timer = Fixture::new(1, 10, false, 1);
        timer.advance(11);
        assert_eq!(timer.callbacks, 0);
    }

    #[test]
    fn renode_limit_timer_emits_enabled_alarm() {
        let mut timer = Fixture::new(1, 10, false, 1);
        timer.event_enabled = true;
        timer.advance(10);
        assert_ne!(timer.callbacks, 0);
    }
}
