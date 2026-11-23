use crate::time_handle::threaded::ThreadedTimeHandle;

/// Coordinates grants and tracks the earliest sink's reported progress.
pub struct GrantGroup<const QUANTUM: u64> {
    elapsed: u64,
    end: u64,
    complete: bool,
}

impl<const QUANTUM: u64> Default for GrantGroup<QUANTUM> {
    fn default() -> Self {
        Self {
            elapsed: 0,
            end: 0,
            complete: true,
        }
    }
}

impl<const QUANTUM: u64> GrantGroup<QUANTUM> {
    pub fn elapsed(&self) -> u64 {
        self.elapsed
    }

    pub fn round_complete(&self) -> bool {
        self.complete
    }

    fn next_boundary(&self) -> Result<u64, &'static str> {
        if !self.round_complete() {
            return Err("previous time quantum is incomplete");
        }
        if QUANTUM == 0 {
            return Err("time quantum must be positive");
        }
        self.end.checked_add(QUANTUM).ok_or("time quantum overflow")
    }

    pub fn begin_round(&mut self) -> Result<(), &'static str> {
        self.end = self.next_boundary()?;
        self.complete = false;
        Ok(())
    }

    pub fn grant(&mut self, sinks: &[ThreadedTimeHandle]) -> Result<(), &'static str> {
        let end = self.next_boundary()?;
        ThreadedTimeHandle::grant_batch(sinks, QUANTUM)?;
        self.end = end;
        self.complete = false;
        Ok(())
    }

    pub fn observe_sinks(&mut self, sinks: &[ThreadedTimeHandle]) {
        let mut progress = self.end;
        let mut all_ready = !sinks.is_empty();
        for sink in sinks {
            let (elapsed, ready) = sink.progress();
            progress = progress.min(elapsed);
            all_ready &= ready;
        }
        if !sinks.is_empty() {
            self.observe_progress(progress, all_ready);
        }
    }

    pub fn observe_child(&mut self, child: &Self) {
        self.observe_progress(child.elapsed, child.complete);
    }

    fn observe_progress(&mut self, progress: u64, ready: bool) {
        self.elapsed = self.elapsed.max(progress.min(self.end));
        self.complete |= ready && progress >= self.end;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time_handle::TimeHandle;
    use std::thread;

    const QUANTUM: u64 = 10;
    const LEFT: bool = false;
    const DONE: bool = true;

    fn group<const N: usize>() -> ([ThreadedTimeHandle; N], GrantGroup<QUANTUM>) {
        (
            std::array::from_fn(|_| ThreadedTimeHandle::default()),
            GrantGroup::default(),
        )
    }

    fn report(handle: &ThreadedTimeHandle, expected: u64, used: u64, done: bool, resumed: bool) {
        assert_eq!(handle.request(), Ok(Some(expected)));
        if done {
            assert_eq!(used, expected);
            handle.update(TimeHandle::report_continue).unwrap();
        } else {
            handle
                .update(|core| core.report_break(expected - used))
                .unwrap();
        }
        assert_eq!(handle.wait(), Ok((done, resumed, used)));
    }

    fn resume_and_report(handle: &ThreadedTimeHandle, expected: u64, used: u64, done: bool) {
        assert_eq!(
            handle.update(TimeHandle::request),
            Ok(std::task::Poll::Pending)
        );
        assert_eq!(handle.update(TimeHandle::unblock), Ok(true));
        report(handle, expected, used, done, true);
    }

    #[test]
    fn renode_time_source_updates_executed_time_after_blocking() {
        let (sinks, mut slave) = group::<1>();
        let mut master = GrantGroup::<QUANTUM>::default();
        slave.grant(&sinks).unwrap();
        master.begin_round().unwrap();
        thread::scope(|scope| {
            scope
                .spawn(|| report(&sinks[0], 10, 5, LEFT, false))
                .join()
                .unwrap()
        });
        slave.observe_sinks(&sinks);
        master.observe_child(&slave);
        assert_eq!((master.elapsed(), slave.elapsed()), (5, 5));
        assert!(!master.round_complete());
        assert!(slave.grant(&sinks).is_err());
        thread::scope(|scope| {
            scope
                .spawn(|| resume_and_report(&sinks[0], 5, 5, DONE))
                .join()
                .unwrap()
        });
        slave.observe_sinks(&sinks);
        master.observe_child(&slave);
        assert_eq!(
            (master.elapsed(), slave.elapsed(), sinks[0].elapsed()),
            (10, 10, 10)
        );
        assert!(master.round_complete());
    }

    #[test]
    fn renode_time_source_handles_two_blocking_sinks() {
        let (sinks, mut master) = group::<2>();
        let mut slave = GrantGroup::<QUANTUM>::default();
        master.grant(&sinks).unwrap();
        slave.begin_round().unwrap();

        thread::scope(|scope| {
            scope.spawn(|| report(&sinks[0], 10, 4, LEFT, false));
            scope.spawn(|| report(&sinks[1], 10, 6, LEFT, false));
        });
        master.observe_sinks(&sinks);
        slave.observe_child(&master);
        assert_eq!((master.elapsed(), slave.elapsed()), (4, 4));

        for (first, second, expected) in [
            ((6, 4, LEFT), (4, 0, LEFT), 6),
            ((2, 0, LEFT), (4, 4, DONE), 8),
        ] {
            thread::scope(|scope| {
                scope.spawn(|| resume_and_report(&sinks[0], first.0, first.1, first.2));
                scope.spawn(|| resume_and_report(&sinks[1], second.0, second.1, second.2));
            });
            master.observe_sinks(&sinks);
            slave.observe_child(&master);
            assert_eq!((master.elapsed(), slave.elapsed()), (expected, expected));
            assert!(master.grant(&sinks).is_err());
            if expected == 8 {
                assert_eq!(
                    sinks[1].update(TimeHandle::request),
                    Ok(std::task::Poll::Pending)
                );
            }
        }

        resume_and_report(&sinks[0], 2, 2, DONE);
        master.observe_sinks(&sinks);
        slave.observe_child(&master);
        assert_eq!((master.elapsed(), slave.elapsed()), (10, 10));
        assert!(master.round_complete());
    }

    #[test]
    fn renode_time_source_handles_blocking_at_end_of_grant() {
        let (sinks, mut slave) = group::<1>();
        let mut master = GrantGroup::<QUANTUM>::default();
        slave.grant(&sinks).unwrap();
        master.begin_round().unwrap();
        report(&sinks[0], 10, 10, LEFT, false);
        slave.observe_sinks(&sinks);
        master.observe_child(&slave);
        assert_eq!((master.elapsed(), slave.elapsed()), (10, 10));
        assert!(slave.grant(&sinks).is_err());
        assert!(master.begin_round().is_err());
        resume_and_report(&sinks[0], 0, 0, DONE);
        slave.observe_sinks(&sinks);
        master.observe_child(&slave);
        slave.grant(&sinks).unwrap();
        master.begin_round().unwrap();
        report(&sinks[0], 10, 10, DONE, false);
        slave.observe_sinks(&sinks);
        master.observe_child(&slave);
        assert_eq!((master.elapsed(), slave.elapsed()), (20, 20));
    }
}
