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

use crate::time_handle::TimeHandle;
use std::{
    sync::{Arc, mpsc},
    task::Poll,
};

#[derive(Clone, Copy)]
pub enum SinkOwner {
    Parent,
    Child,
}

enum SinkEvent {
    Request(usize),
    Report(usize),
}

pub struct DispatchSink {
    index: usize,
    handles: Arc<[ThreadedTimeHandle]>,
    events: mpsc::Sender<SinkEvent>,
}

impl DispatchSink {
    fn handle(&self) -> &ThreadedTimeHandle {
        &self.handles[self.index]
    }

    pub fn request(&self) -> Result<Option<u64>, &'static str> {
        match self.handle().update(TimeHandle::request)? {
            Poll::Ready(grant) => Ok(grant),
            Poll::Pending => {
                self.events
                    .send(SinkEvent::Request(self.index))
                    .map_err(|_| "dispatcher stopped")?;
                self.handle().request()
            }
        }
    }

    pub fn complete(&self) -> Result<(), &'static str> {
        self.handle().update(TimeHandle::report_continue)?;
        self.send_report()
    }

    pub fn interrupt(&self, remaining: u64) -> Result<(), &'static str> {
        self.handle()
            .update(|handle| handle.report_break(remaining))?;
        self.send_report()
    }

    fn send_report(&self) -> Result<(), &'static str> {
        self.events
            .send(SinkEvent::Report(self.index))
            .map_err(|_| "dispatcher stopped")
    }
}

pub struct GrantDispatcher<const QUANTUM: u64> {
    handles: Arc<[ThreadedTimeHandle]>,
    events: mpsc::Receiver<SinkEvent>,
}

impl<const QUANTUM: u64> GrantDispatcher<QUANTUM> {
    pub fn new(count: usize) -> (Self, Vec<DispatchSink>) {
        let handles: Arc<[_]> = (0..count).map(|_| ThreadedTimeHandle::default()).collect();
        let (send, events) = mpsc::channel();
        let sinks = (0..count)
            .map(|index| DispatchSink {
                index,
                handles: Arc::clone(&handles),
                events: send.clone(),
            })
            .collect();
        (Self { handles, events }, sinks)
    }

    pub fn run_round(
        &mut self,
        parent: &mut GrantGroup<QUANTUM>,
        child: &mut GrantGroup<QUANTUM>,
        owner: SinkOwner,
        mut observe: impl FnMut((u64, u64, bool)),
    ) -> Result<(), &'static str> {
        // Validate both clocks before publishing any grant.
        parent.next_boundary()?;
        child.next_boundary()?;
        let (driving, follower) = match owner {
            SinkOwner::Parent => (parent, child),
            SinkOwner::Child => (child, parent),
        };
        driving.grant(&self.handles)?;
        follower.begin_round()?;
        let mut previous = (driving.elapsed(), driving.elapsed(), false);
        while !driving.round_complete() {
            match self
                .events
                .recv()
                .map_err(|_| "time sinks disconnected before reporting")?
            {
                SinkEvent::Request(index) => {
                    // A request can be queued before the initial grant. It only
                    // requires a dispatch action if a previous report blocked it.
                    let _ = self.handles[index].update(TimeHandle::unblock);
                }
                SinkEvent::Report(index) => {
                    self.handles[index].wait()?;
                    driving.observe_sinks(&self.handles);
                    follower.observe_child(driving);
                    let snapshot = (
                        driving.elapsed(),
                        follower.elapsed(),
                        driving.round_complete(),
                    );
                    if snapshot != previous {
                        previous = snapshot;
                        observe(snapshot);
                    }
                }
            }
        }
        Ok(())
    }
}

impl<const QUANTUM: u64> Drop for GrantDispatcher<QUANTUM> {
    fn drop(&mut self) {
        for handle in &*self.handles {
            handle.update(TimeHandle::dispose);
        }
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

#[cfg(test)]
mod dispatch_tests {
    use super::*;
    use std::{sync::mpsc, thread, time::Duration};

    fn run_rounds(
        traces: &[&[(u64, u64, bool)]],
        owner: SinkOwner,
        expected: &[(u64, bool)],
        rounds: usize,
    ) {
        let (mut dispatch, sinks) = GrantDispatcher::<10>::new(traces.len());
        let (observed, progress) = mpsc::channel();
        let (mut parent, mut child) = (GrantGroup::default(), GrantGroup::default());
        thread::scope(|scope| {
            let mut workers = Vec::new();
            for (sink, trace) in sinks.into_iter().zip(traces) {
                let (command, commands) = mpsc::channel();
                workers.push(command);
                scope.spawn(move || {
                    for &(offered, used, done) in *trace {
                        commands.recv_timeout(Duration::from_secs(3)).unwrap();
                        assert_eq!(sink.request().unwrap(), Some(offered));
                        if done {
                            assert_eq!(offered, used);
                            sink.complete().unwrap();
                        } else {
                            sink.interrupt(offered - used).unwrap();
                        }
                    }
                });
            }
            scope.spawn(move || {
                for _ in 0..rounds {
                    dispatch
                        .run_round(&mut parent, &mut child, owner, |snapshot| {
                            observed.send(snapshot).unwrap();
                        })
                        .unwrap();
                }
            });
            let mut previous = 0;
            for (index, &(target, complete)) in expected.iter().enumerate() {
                for (sender, trace) in workers.iter().zip(traces) {
                    if index < trace.len() {
                        sender.send(()).unwrap();
                    }
                }
                loop {
                    let snapshot = progress.recv_timeout(Duration::from_secs(3)).unwrap();
                    assert_eq!(snapshot.0, snapshot.1);
                    assert!((previous..=target).contains(&snapshot.0), "{snapshot:?}");
                    if snapshot == (target, target, complete) {
                        break;
                    }
                }
                previous = target;
            }
        });
    }

    #[test]
    fn renode_time_source_dispatches_updates_after_blocking() {
        run_rounds(
            &[&[(10, 5, false), (5, 5, true)]],
            SinkOwner::Child,
            &[(5, false), (10, true)],
            1,
        );
    }

    #[test]
    fn renode_time_source_dispatches_two_blocking_sinks() {
        run_rounds(
            &[
                &[(10, 4, false), (6, 4, false), (2, 0, false), (2, 2, true)],
                &[(10, 6, false), (4, 0, false), (4, 4, true)],
            ],
            SinkOwner::Parent,
            &[(4, false), (6, false), (8, false), (10, true)],
            1,
        );
    }

    #[test]
    fn renode_time_source_dispatches_blocking_at_end_of_grant() {
        run_rounds(
            &[&[(10, 10, false), (0, 0, true), (10, 10, true)]],
            SinkOwner::Child,
            &[(10, false), (10, true), (20, true)],
            2,
        );
    }
}
