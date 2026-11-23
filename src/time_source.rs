/// A serial clock domain with an initial sync point and complete quantum grants.
#[derive(Default)]
pub struct TimeSource<const QUANTUM: u64> {
    sync_points: u64,
}

impl<const QUANTUM: u64> TimeSource<QUANTUM> {
    pub fn sync_points(&self) -> u64 {
        self.sync_points
    }

    fn rounds(&self) -> u64 {
        self.sync_points().saturating_sub(1)
    }

    pub fn elapsed(&self) -> u64 {
        self.rounds() * QUANTUM
    }

    pub fn grant_until(&mut self, end: u64) {
        // The elapsed portion of a grant is rounded down. The remainder is
        // still available when the parent supplies a later boundary.
        let boundaries = (end / QUANTUM).saturating_add(1);
        self.sync_points = self.sync_points.max(boundaries);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture<const MASTER: u64, const SLAVE: u64> {
        master: TimeSource<MASTER>,
        slave: TimeSource<SLAVE>,
        leaf: TimeSource<SLAVE>,
        connected: bool,
    }

    impl<const MASTER: u64, const SLAVE: u64> Fixture<MASTER, SLAVE> {
        fn new() -> Self {
            Self {
                master: TimeSource::default(),
                slave: TimeSource::default(),
                leaf: TimeSource::default(),
                connected: true,
            }
        }

        fn run(&mut self, rounds: u64) {
            for round in self.master.sync_points()..self.master.sync_points() + rounds {
                self.master.grant_until(round * MASTER);
                if round != 0 && self.connected {
                    self.slave.grant_until(self.master.elapsed());
                    self.leaf.grant_until(self.slave.elapsed());
                }
            }
        }

        fn check(&self, expected: (u64, u64, u64, u64, u64, u64)) {
            assert_eq!(
                (
                    self.master.sync_points(),
                    self.slave.sync_points(),
                    self.leaf.rounds(),
                    self.master.elapsed(),
                    self.slave.elapsed(),
                    self.leaf.elapsed(),
                ),
                expected,
            );
        }
    }

    #[test]
    fn renode_time_source_should_handle_multiple_sinks() {
        const DEFAULT_QUANTUM: u64 = 100_000; // Renode: 100 microseconds in nanosecond ticks.
        let mut master = TimeSource::<DEFAULT_QUANTUM>::default();
        let mut sinks =
            std::array::from_fn::<_, 5, _>(|_| TimeSource::<DEFAULT_QUANTUM>::default());
        for round in 0..4 {
            master.grant_until(round * DEFAULT_QUANTUM);
            if round != 0 {
                for sink in &mut sinks {
                    sink.grant_until(master.elapsed());
                }
            }
        }
        assert_eq!(master.sync_points(), 4);
        for sink in sinks {
            assert_eq!(sink.elapsed(), 3 * DEFAULT_QUANTUM);
            assert_eq!(sink.rounds(), 3);
        }
    }

    #[test]
    fn renode_time_source_should_handle_slave_time_source_with_same_quantum() {
        let mut fixture = Fixture::<100, 100>::new();
        fixture.run(4);
        fixture.check((4, 4, 3, 300, 300, 300));
    }

    #[test]
    fn renode_time_source_should_handle_slave_time_source_with_different_quantum() {
        let mut fixture = Fixture::<100, 10>::new();
        fixture.run(4);
        fixture.check((4, 31, 30, 300, 300, 300));
    }

    #[test]
    fn renode_time_source_should_handle_slave_time_source_with_not_aligned_quantum() {
        let mut fixture = Fixture::<10, 3>::new();
        for expected in [
            (1, 0, 0, 0, 0, 0),
            (2, 4, 3, 10, 9, 9),
            (3, 7, 6, 20, 18, 18),
            (4, 11, 10, 30, 30, 30),
            (5, 14, 13, 40, 39, 39),
            (6, 17, 16, 50, 48, 48),
            (7, 21, 20, 60, 60, 60),
        ] {
            fixture.run(1);
            fixture.check(expected);
        }
    }

    #[test]
    fn renode_time_source_should_not_tick_disconnected_slave_time_source() {
        let mut fixture = Fixture::<10, 10>::new();
        fixture.run(2);
        fixture.check((2, 2, 1, 10, 10, 10));
        fixture.connected = false;
        fixture.run(1);
        fixture.check((3, 2, 1, 20, 10, 10));
    }
}
