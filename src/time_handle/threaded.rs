use std::{
    sync::{Condvar, Mutex},
    task::Poll,
};

use super::TimeHandle;

/// A blocking adapter for producers and consumers on different host threads.
#[derive(Default)]
pub struct ThreadedTimeHandle {
    state: Mutex<TimeHandle>,
    changed: Condvar,
}

impl ThreadedTimeHandle {
    pub fn update<T>(&self, operation: impl FnOnce(&mut TimeHandle) -> T) -> T {
        let mut state = self.state.lock().unwrap();
        let result = operation(&mut state);
        self.changed.notify_all();
        result
    }

    pub fn request(&self) -> Result<Option<u64>, &'static str> {
        self.poll_until_ready(TimeHandle::request, || {})
    }

    pub fn wait(&self) -> Result<(bool, bool, u64), &'static str> {
        self.poll_until_ready(TimeHandle::wait, || {})
    }

    pub fn set_enabled(&self, enabled: bool) -> Result<(), &'static str> {
        self.poll_until_ready(|state| Ok(state.set_enabled(enabled)), || {})
    }

    fn poll_until_ready<T>(
        &self,
        mut operation: impl FnMut(&mut TimeHandle) -> Result<Poll<T>, &'static str>,
        mut before_wait: impl FnMut(),
    ) -> Result<T, &'static str> {
        let mut state = self.state.lock().unwrap();
        loop {
            match operation(&mut state)? {
                Poll::Ready(result) => {
                    self.changed.notify_all();
                    return Ok(result);
                }
                Poll::Pending => {
                    before_wait();
                    state = self.changed.wait(state).unwrap();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::{Arc, mpsc},
        thread,
        time::Duration,
    };

    const INTERVAL: u64 = 1000;
    const TIMEOUT: Duration = Duration::from_secs(3);

    struct Pending<T> {
        result: mpsc::Receiver<T>,
        worker: thread::JoinHandle<()>,
    }

    impl<T> Pending<T> {
        fn finish(self) -> T {
            let value = self
                .result
                .recv_timeout(TIMEOUT)
                .expect("worker did not wake");
            self.worker.join().unwrap();
            value
        }
    }

    fn blocked<T: Send + 'static>(
        handle: &Arc<ThreadedTimeHandle>,
        operation: impl Fn(&mut TimeHandle) -> Result<Poll<T>, &'static str> + Send + 'static,
    ) -> Pending<T> {
        let (waiting_tx, waiting_rx) = mpsc::sync_channel(1);
        let (result_tx, result) = mpsc::channel();
        let thread_handle = Arc::clone(handle);
        let worker = thread::spawn(move || {
            let mut ready = Some(waiting_tx);
            let value = thread_handle
                .poll_until_ready(operation, || {
                    if let Some(ready) = ready.take() {
                        ready.send(()).unwrap();
                    }
                })
                .unwrap();
            result_tx.send(value).unwrap();
        });
        waiting_rx
            .recv_timeout(TIMEOUT)
            .expect("worker never reached the wait");
        // The observer signals while holding the handle lock. Acquiring it here
        // confirms that the worker released it inside the condition-variable wait.
        handle.update(|_| assert!(matches!(result.try_recv(), Err(mpsc::TryRecvError::Empty))));
        Pending { result, worker }
    }

    fn request_grant(handle: &Arc<ThreadedTimeHandle>, request_first: bool) {
        if request_first {
            let request = blocked(handle, TimeHandle::request);
            handle.update(|core| core.grant(INTERVAL)).unwrap();
            assert_eq!(request.finish(), Some(INTERVAL));
        } else {
            handle.update(|core| core.grant(INTERVAL)).unwrap();
            assert_eq!(handle.request(), Ok(Some(INTERVAL)));
        }
    }

    fn wait_for_report(request_first: bool, interrupted: bool) {
        let handle = Arc::new(ThreadedTimeHandle::default());
        wait_for_report_on(&handle, request_first, interrupted);
    }

    fn wait_for_report_on(
        handle: &Arc<ThreadedTimeHandle>,
        request_first: bool,
        interrupted: bool,
    ) {
        request_grant(handle, request_first);
        let wait = blocked(handle, TimeHandle::wait);
        if interrupted {
            handle.update(|core| core.report_break(0)).unwrap();
        } else {
            handle.update(TimeHandle::report_continue).unwrap();
        }
        assert_eq!(wait.finish(), (!interrupted, false, INTERVAL));
    }

    #[test]
    fn renode_threaded_time_handle_request_wakes_on_grant() {
        request_grant(&Arc::new(ThreadedTimeHandle::default()), true);
    }

    #[test]
    fn renode_threaded_time_handle_wait_blocks_request_first() {
        wait_for_report(true, false);
    }

    #[test]
    fn renode_threaded_time_handle_wait_blocks_grant_first() {
        wait_for_report(false, false);
    }

    #[test]
    fn renode_threaded_time_handle_continue_wakes_wait_request_first() {
        wait_for_report(true, false);
    }

    #[test]
    fn renode_threaded_time_handle_continue_wakes_wait_grant_first() {
        wait_for_report(false, false);
    }

    #[test]
    fn renode_threaded_time_handle_break_wakes_wait_request_first() {
        wait_for_report(true, true);
    }

    #[test]
    fn renode_threaded_time_handle_break_wakes_wait_grant_first() {
        wait_for_report(false, true);
    }
    #[test]
    fn renode_threaded_time_handle_wait_after_continue_is_immediate() {
        let handle = Arc::new(ThreadedTimeHandle::default());
        request_grant(&handle, true);
        handle.update(TimeHandle::report_continue).unwrap();
        assert_eq!(handle.wait(), Ok((true, false, INTERVAL)));
    }

    #[test]
    fn renode_threaded_time_handle_wait_after_break_is_immediate() {
        let handle = Arc::new(ThreadedTimeHandle::default());
        request_grant(&handle, true);
        handle.update(|core| core.report_break(0)).unwrap();
        assert_eq!(handle.wait(), Ok((false, false, INTERVAL)));
    }

    #[test]
    fn renode_threaded_time_handle_disabling_sink_wakes_request() {
        let handle = Arc::new(ThreadedTimeHandle::default());
        let request = blocked(&handle, TimeHandle::request);
        handle.set_enabled(false).unwrap();
        assert_eq!(request.finish(), None);
    }

    #[test]
    fn renode_threaded_time_handle_deactivating_source_wakes_request() {
        let handle = Arc::new(ThreadedTimeHandle::default());
        let request = blocked(&handle, TimeHandle::request);
        handle.update(|core| core.set_source_active(false));
        assert_eq!(request.finish(), None);
    }

    #[test]
    fn renode_threaded_time_handle_reactivation_allows_a_later_grant() {
        let handle = Arc::new(ThreadedTimeHandle::default());
        handle.update(|core| core.set_source_active(false));
        handle.update(|core| core.set_source_active(true));
        request_grant(&handle, true);
    }

    #[test]
    fn renode_threaded_time_handle_request_observes_source_activity_changes() {
        let handle = Arc::new(ThreadedTimeHandle::default());
        let request = blocked(&handle, TimeHandle::request);
        handle.update(|core| core.set_source_active(false));
        assert_eq!(request.finish(), None);
        assert_eq!(handle.request(), Ok(None));
        handle.update(|core| core.set_source_active(true));
        wait_for_report_on(&handle, true, false);
    }

    #[test]
    fn renode_threaded_time_handle_wait_on_disabled_sink_is_immediate() {
        let handle = Arc::new(ThreadedTimeHandle::default());
        let request = blocked(&handle, TimeHandle::request);
        handle.set_enabled(false).unwrap();
        assert_eq!(request.finish(), None);
        handle.update(|core| core.grant(INTERVAL)).unwrap();
        assert_eq!(handle.wait(), Ok((true, false, INTERVAL)));
    }

    fn break_before_unblock() -> Arc<ThreadedTimeHandle> {
        let handle = Arc::new(ThreadedTimeHandle::default());
        request_grant(&handle, true);
        handle.update(|core| core.report_break(0)).unwrap();
        assert_eq!(handle.wait(), Ok((false, false, INTERVAL)));
        handle
    }

    #[test]
    fn renode_threaded_time_handle_request_blocks_after_break() {
        let handle = break_before_unblock();
        let request = blocked(&handle, TimeHandle::request);
        handle.update(TimeHandle::dispose);
        assert_eq!(request.finish(), None);
    }

    #[test]
    fn renode_threaded_time_handle_unblock_wakes_request_with_zero_ticks() {
        let handle = break_before_unblock();
        let request = blocked(&handle, TimeHandle::request);
        assert_eq!(handle.update(TimeHandle::unblock), Ok(true));
        assert_eq!(request.finish(), Some(0));
    }

    fn report_after_unblock(interrupted: bool) {
        let handle = Arc::new(ThreadedTimeHandle::default());
        handle.update(|core| core.grant(INTERVAL)).unwrap();
        assert_eq!(handle.wait(), Ok((false, false, 0)));
        let request = blocked(&handle, TimeHandle::request);
        assert_eq!(handle.update(TimeHandle::unblock), Ok(true));
        assert_eq!(request.finish(), Some(INTERVAL));
        let wait = blocked(&handle, TimeHandle::wait);
        if interrupted {
            handle.update(|core| core.report_break(0)).unwrap();
        } else {
            handle.update(TimeHandle::report_continue).unwrap();
        }
        assert_eq!(wait.finish(), (!interrupted, true, INTERVAL));
    }

    #[test]
    fn renode_threaded_time_handle_continue_wakes_wait_after_unblock() {
        report_after_unblock(false);
    }

    #[test]
    fn renode_threaded_time_handle_break_wakes_wait_after_unblock() {
        report_after_unblock(true);
    }

    #[test]
    fn renode_threaded_time_handle_latch_defers_reenable_until_unlatch() {
        let handle = Arc::new(ThreadedTimeHandle::default());
        let request = blocked(&handle, TimeHandle::request);
        handle.update(|core| core.set_source_active(false));
        assert_eq!(request.finish(), None);
        handle.update(|core| core.set_source_active(true));
        handle.set_enabled(false).unwrap();
        handle.update(TimeHandle::latch);
        handle.update(|core| core.grant(INTERVAL)).unwrap();
        let enable = blocked(&handle, |core| Ok(core.set_enabled(true)));
        assert_eq!(handle.wait(), Ok((true, false, INTERVAL)));
        assert_eq!(handle.request(), Ok(None));
        handle.update(TimeHandle::unlatch).unwrap();
        enable.finish();
    }
}
