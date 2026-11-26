#[derive(Clone)]
pub struct SyncAction<T> {
    timestamp: u64,
    action: T,
}

impl<T> SyncAction<T> {
    pub fn new(timestamp: u64, action: T) -> Self {
        Self { timestamp, action }
    }

    pub fn take_due(slot: &mut Option<Self>, boundary: u64) -> Option<T> {
        if slot
            .as_ref()
            .is_some_and(|event| event.timestamp <= boundary)
        {
            return slot.take().map(|event| event.action);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::SyncAction;
    use crate::time_source::TimeSource;

    #[derive(Default)]
    struct TimeSourceFixture {
        master: TimeSource<10>,
        slave: TimeSource<10>,
        requested: bool,
        calls: Vec<u64>,
        action: Option<SyncAction<()>>,
    }

    impl TimeSourceFixture {
        fn run(&mut self, rounds: u64, request: impl Fn(u64, u64) -> Option<u64>) {
            for _ in 0..rounds {
                let round = self.master.sync_points();
                self.master.grant_until(round * 10);
                // Round zero is the initial synchronization with no sink grant.
                if round != 0 {
                    self.slave.grant_until(self.master.elapsed());
                    let boundary = self.slave.elapsed();
                    if let Some(timestamp) = request(round, boundary) {
                        self.requested = true;
                        self.action = Some(SyncAction::new(timestamp, ()));
                    }
                    if SyncAction::take_due(&mut self.action, boundary).is_some() {
                        self.calls.push(boundary);
                    }
                }
            }
        }
    }

    #[test]
    fn renode_time_source_should_execute_delayed_action_in_nearest_synced_state_exactly_once() {
        let mut clock = TimeSourceFixture::default();
        let request = |round, boundary| (round == 1).then_some(boundary);
        clock.run(1, request);
        assert!(clock.calls.is_empty());
        assert!(!clock.requested);
        clock.run(1, request);
        assert!(clock.requested);
        assert_eq!(clock.calls, [10]);
        clock.run(1, request);
        assert_eq!(clock.calls, [10]);
    }

    #[test]
    fn renode_time_source_should_execute_delayed_action_added_later_in_nearest_synced_state() {
        let mut clock = TimeSourceFixture::default();
        clock.run(11, |round, boundary| (round == 10).then_some(boundary));
        assert_eq!(clock.calls, [100]);
    }

    #[test]
    fn renode_time_source_should_execute_delayed_action_in_synced_state_exactly_once() {
        let mut clock = TimeSourceFixture::default();
        let request = |round, _| (round == 1).then_some(30);
        clock.run(3, request);
        assert!(clock.calls.is_empty());
        clock.run(1, request);
        assert_eq!(clock.calls, [30]);
        clock.run(1, request);
        assert_eq!(clock.calls, [30]);
    }

    #[test]
    fn renode_time_source_should_execute_non_aligned_delayed_action_in_synced_state_exactly_once() {
        let mut clock = TimeSourceFixture::default();
        let request = |round, _| (round == 1).then_some(31);
        clock.run(4, request);
        assert!(clock.calls.is_empty());
        clock.run(1, request);
        assert_eq!(clock.calls, [40]);
        clock.run(1, request);
        assert_eq!(clock.calls, [40]);
    }
}
