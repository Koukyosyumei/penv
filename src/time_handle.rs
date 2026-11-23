use std::task::Poll;

#[derive(Default)]
enum Grant {
    #[default]
    Idle,
    Offered(u64),
    Running(u64),
    Reported(u64),
    Interrupted {
        used: u64,
        left: u64,
    },
    Blocked(u64),
    ResumeOffered(u64),
}

pub struct TimeHandle {
    grant: Grant,
    waiting_to_unblock: bool,
    recently_unblocked: bool,
    enabled: bool,
    source_active: bool,
    disposed: bool,
}

impl Default for TimeHandle {
    fn default() -> Self {
        Self {
            grant: Grant::Idle,
            waiting_to_unblock: false,
            recently_unblocked: false,
            enabled: true,
            source_active: true,
            disposed: false,
        }
    }
}

impl TimeHandle {
    pub fn is_ready_for_grant(&self) -> bool {
        !self.disposed && matches!(self.grant, Grant::Idle)
    }

    pub fn dispose(&mut self) {
        self.disposed = true;
        self.set_source_active(false);
        self.set_enabled(false);
        self.grant = Grant::Idle;
    }

    pub fn set_source_active(&mut self, active: bool) {
        self.source_active = active;
        if !active {
            self.waiting_to_unblock = false;
        }
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.waiting_to_unblock = false;
            self.recently_unblocked = false;
            match self.grant {
                Grant::Blocked(left)
                | Grant::ResumeOffered(left)
                | Grant::Interrupted { left, .. } => {
                    self.grant = Grant::Reported(left);
                }
                _ => {}
            }
        }
    }

    pub fn grant(&mut self, ticks: u64) -> Result<(), &'static str> {
        if !self.is_ready_for_grant() {
            return Err("time handle already has a grant");
        }
        self.grant = if self.enabled {
            Grant::Offered(ticks)
        } else {
            Grant::Reported(ticks)
        };
        Ok(())
    }

    pub fn request(&mut self) -> Result<Poll<Option<u64>>, &'static str> {
        if self.disposed || !self.enabled {
            return Ok(Poll::Ready(None));
        }
        match self.grant {
            Grant::Idle if !self.source_active => Ok(Poll::Ready(None)),
            Grant::Idle => Ok(Poll::Pending),
            Grant::Blocked(_) | Grant::Interrupted { .. } if !self.source_active => {
                Ok(Poll::Ready(None))
            }
            Grant::Blocked(_) | Grant::Interrupted { .. } => {
                self.waiting_to_unblock = true;
                Ok(Poll::Pending)
            }
            Grant::ResumeOffered(ticks) => {
                self.recently_unblocked = true;
                self.grant = Grant::Running(ticks);
                Ok(Poll::Ready(Some(ticks)))
            }
            Grant::Offered(ticks) => {
                self.grant = Grant::Running(ticks);
                Ok(Poll::Ready(Some(ticks)))
            }
            _ => Err("time handle already has a request"),
        }
    }

    pub fn report_continue(&mut self) -> Result<(), &'static str> {
        let Grant::Running(ticks) = self.grant else {
            return Err("no requested time grant to report");
        };
        self.grant = Grant::Reported(ticks);
        Ok(())
    }

    pub fn report_break(&mut self, time_left: u64) -> Result<(), &'static str> {
        let Grant::Running(ticks) = self.grant else {
            return Err("no requested time grant to report");
        };
        let used = ticks
            .checked_sub(time_left)
            .ok_or("remaining time exceeds the grant")?;
        self.grant = Grant::Interrupted {
            used,
            left: time_left,
        };
        Ok(())
    }

    pub fn unblock(&mut self) -> Result<bool, &'static str> {
        let Grant::Blocked(left) = self.grant else {
            return Err("time handle is not blocked");
        };
        if !std::mem::take(&mut self.waiting_to_unblock) {
            return Ok(false);
        }
        self.grant = Grant::ResumeOffered(left);
        Ok(true)
    }

    // The result carries completion, whether the sink resumed, and consumed ticks.
    pub fn wait(&mut self) -> Result<Poll<(bool, bool, u64)>, &'static str> {
        let (done, ticks) = match self.grant {
            Grant::Idle => return Err("no time grant to wait for"),
            Grant::Offered(ticks) => {
                self.grant = Grant::Blocked(ticks);
                (false, 0)
            }
            Grant::Blocked(_) => (false, 0),
            Grant::Running(_) | Grant::ResumeOffered(_) => return Ok(Poll::Pending),
            Grant::Interrupted { used, left } => {
                self.grant = Grant::Blocked(left);
                (false, used)
            }
            Grant::Reported(ticks) => {
                self.grant = Grant::Idle;
                (true, ticks)
            }
        };
        Ok(Poll::Ready((
            done,
            std::mem::take(&mut self.recently_unblocked),
            ticks,
        )))
    }

    pub fn run_grant<T>(
        ticks: u64,
        run: impl FnOnce(u64) -> Result<(T, u64), String>,
    ) -> Result<(T, u64), String> {
        let mut handle = Self::default();
        handle.grant(ticks)?;
        let Poll::Ready(Some(granted)) = handle.request()? else {
            return Err("time grant has no consumer".into());
        };
        let (result, used) = run(granted)?;
        let remaining = granted.checked_sub(used).ok_or("time grant was exceeded")?;
        if remaining == 0 {
            handle.report_continue()?;
        } else {
            handle.report_break(remaining)?;
        }
        let Poll::Ready((_, _, accounted)) = handle.wait()? else {
            return Err("time grant was not reported".into());
        };
        Ok((result, accounted))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INTERVAL: u64 = 1000;

    #[test]
    fn renode_time_handle_wait_without_request_returns_without_progress() {
        let mut handle = TimeHandle::default();
        handle.grant(INTERVAL).unwrap();
        assert_eq!(handle.wait(), Ok(Poll::Ready((false, false, 0))));
        assert_eq!(handle.request(), Ok(Poll::Pending));
    }

    #[test]
    fn renode_time_handle_request_waits_for_grant() {
        let mut handle = TimeHandle::default();
        assert_eq!(handle.request(), Ok(Poll::Pending));
        handle.grant(INTERVAL).unwrap();
        assert_eq!(handle.request(), Ok(Poll::Ready(Some(INTERVAL))));
    }

    #[test]
    fn renode_time_handle_rejects_double_grant() {
        let mut handle = TimeHandle::default();
        handle.grant(INTERVAL).unwrap();
        assert!(handle.grant(INTERVAL).is_err());
        assert_eq!(handle.request(), Ok(Poll::Ready(Some(INTERVAL))));
    }

    #[test]
    fn renode_time_handle_rejects_wait_before_grant() {
        assert!(TimeHandle::default().wait().is_err());
    }

    #[test]
    fn renode_time_handle_rejects_repeated_wait_after_continue() {
        let mut handle = running();
        handle.report_continue().unwrap();
        assert_eq!(handle.wait(), Ok(Poll::Ready((true, false, INTERVAL))));
        assert!(handle.wait().is_err());
    }

    #[test]
    fn renode_time_handle_rejects_double_request() {
        let mut handle = running();
        assert!(handle.request().is_err());
    }

    #[test]
    fn renode_time_handle_rejects_continue_before_request() {
        assert!(TimeHandle::default().report_continue().is_err());
    }

    #[test]
    fn renode_time_handle_rejects_break_before_request() {
        assert!(TimeHandle::default().report_break(0).is_err());
    }

    #[test]
    fn renode_time_handle_rejects_double_continue() {
        let mut handle = running();
        handle.report_continue().unwrap();
        assert!(handle.report_continue().is_err());
    }

    #[test]
    fn renode_time_handle_rejects_double_break() {
        let mut handle = running();
        handle.report_break(0).unwrap();
        assert!(handle.report_break(0).is_err());
    }

    #[test]
    fn renode_time_handle_rejects_continue_after_break() {
        let mut handle = running();
        handle.report_break(0).unwrap();
        assert!(handle.report_continue().is_err());
    }

    #[test]
    fn renode_time_handle_rejects_break_after_continue() {
        let mut handle = running();
        handle.report_continue().unwrap();
        assert!(handle.report_break(0).is_err());
    }

    #[test]
    fn renode_time_handle_rejects_grant_after_break() {
        let mut handle = running();
        handle.report_break(0).unwrap();
        handle.wait().unwrap();
        assert!(handle.grant(INTERVAL).is_err());
    }

    #[test]
    fn renode_time_handle_second_wait_after_break_returns_zero() {
        let mut handle = running();
        handle.report_break(0).unwrap();
        assert_eq!(handle.wait(), Ok(Poll::Ready((false, false, INTERVAL))));
        assert_eq!(handle.wait(), Ok(Poll::Ready((false, false, 0))));
        assert_eq!(handle.request(), Ok(Poll::Pending));
    }

    #[test]
    fn renode_time_handle_wait_returns_after_break_grant_first() {
        let mut handle = running();
        handle.report_break(0).unwrap();
        assert_eq!(handle.wait(), Ok(Poll::Ready((false, false, INTERVAL))));
    }

    #[test]
    fn renode_time_handle_wait_returns_after_break_request_first() {
        let mut handle = TimeHandle::default();
        assert_eq!(handle.request(), Ok(Poll::Pending));
        handle.grant(INTERVAL).unwrap();
        assert_eq!(handle.request(), Ok(Poll::Ready(Some(INTERVAL))));
        handle.report_break(0).unwrap();
        assert_eq!(handle.wait(), Ok(Poll::Ready((false, false, INTERVAL))));
    }

    #[test]
    fn renode_time_handle_request_after_break_waits_for_unblock() {
        let mut handle = broken_after_pending_request();
        assert_eq!(handle.request(), Ok(Poll::Pending));
    }

    #[test]
    fn renode_time_handle_unblocks_zero_remaining_grant() {
        let mut handle = broken_after_pending_request();
        assert_eq!(handle.unblock(), Ok(false));
        assert_eq!(handle.request(), Ok(Poll::Pending));
        assert_eq!(handle.unblock(), Ok(true));
        assert_eq!(handle.request(), Ok(Poll::Ready(Some(0))));
    }

    #[test]
    fn renode_time_handle_continue_after_unblock_reports_resumed_grant() {
        finish_after_unblock(false);
    }

    #[test]
    fn renode_time_handle_break_after_unblock_reports_resumed_grant() {
        finish_after_unblock(true);
    }

    fn finish_after_unblock(break_again: bool) {
        let mut handle = TimeHandle::default();
        handle.grant(INTERVAL).unwrap();
        assert_eq!(handle.wait(), Ok(Poll::Ready((false, false, 0))));
        assert_eq!(handle.request(), Ok(Poll::Pending));
        assert_eq!(handle.unblock(), Ok(true));
        assert_eq!(handle.request(), Ok(Poll::Ready(Some(INTERVAL))));
        assert_eq!(handle.wait(), Ok(Poll::Pending));
        if break_again {
            handle.report_break(0).unwrap();
        } else {
            handle.report_continue().unwrap();
        }
        assert_eq!(
            handle.wait(),
            Ok(Poll::Ready((!break_again, true, INTERVAL)))
        );
    }

    #[test]
    fn renode_time_handle_unblocks_only_the_unused_interval() {
        let mut handle = running();
        handle.report_break(250).unwrap();
        assert_eq!(handle.wait(), Ok(Poll::Ready((false, false, 750))));
        assert_eq!(handle.request(), Ok(Poll::Pending));
        assert_eq!(handle.unblock(), Ok(true));
        assert_eq!(handle.request(), Ok(Poll::Ready(Some(250))));
        handle.report_continue().unwrap();
        assert_eq!(handle.wait(), Ok(Poll::Ready((true, true, 250))));
    }

    fn broken_after_pending_request() -> TimeHandle {
        let mut handle = TimeHandle::default();
        assert_eq!(handle.request(), Ok(Poll::Pending));
        handle.grant(INTERVAL).unwrap();
        assert_eq!(handle.request(), Ok(Poll::Ready(Some(INTERVAL))));
        handle.report_break(0).unwrap();
        assert_eq!(handle.wait(), Ok(Poll::Ready((false, false, INTERVAL))));
        handle
    }

    #[test]
    fn renode_time_handle_disposal_disables_grant_readiness() {
        let mut handle = TimeHandle::default();
        assert!(handle.is_ready_for_grant());
        handle.dispose();
        assert!(!handle.is_ready_for_grant());
    }

    #[test]
    fn renode_time_handle_rejects_grant_after_dispose() {
        let mut handle = TimeHandle::default();
        handle.dispose();
        assert!(handle.grant(INTERVAL).is_err());
    }

    #[test]
    fn renode_time_handle_request_returns_without_grant_when_disabled() {
        let mut handle = TimeHandle::default();
        handle.set_enabled(false);
        assert_eq!(handle.request(), Ok(Poll::Ready(None)));
    }

    #[test]
    fn renode_time_handle_disabling_sink_resolves_pending_request() {
        let mut handle = TimeHandle::default();
        assert_eq!(handle.request(), Ok(Poll::Pending));
        handle.set_enabled(false);
        assert_eq!(handle.request(), Ok(Poll::Ready(None)));
    }

    #[test]
    fn renode_time_handle_deactivating_source_resolves_pending_request() {
        let mut handle = TimeHandle::default();
        assert_eq!(handle.request(), Ok(Poll::Pending));
        handle.set_source_active(false);
        assert_eq!(handle.request(), Ok(Poll::Ready(None)));
    }

    #[test]
    fn renode_time_handle_request_responds_to_source_state_transitions() {
        let mut handle = TimeHandle::default();
        assert_eq!(handle.request(), Ok(Poll::Pending));
        handle.set_source_active(false);
        assert_eq!(handle.request(), Ok(Poll::Ready(None)));
        assert_eq!(handle.request(), Ok(Poll::Ready(None)));
        handle.set_source_active(true);
        assert_eq!(handle.request(), Ok(Poll::Pending));
        handle.grant(INTERVAL).unwrap();
        assert_eq!(handle.request(), Ok(Poll::Ready(Some(INTERVAL))));
        assert_eq!(handle.wait(), Ok(Poll::Pending));
    }

    #[test]
    fn renode_time_handle_grant_fulfills_request_after_source_reactivation() {
        let mut handle = TimeHandle::default();
        handle.set_source_active(false);
        handle.set_source_active(true);
        assert_eq!(handle.request(), Ok(Poll::Pending));
        handle.grant(INTERVAL).unwrap();
        assert_eq!(handle.request(), Ok(Poll::Ready(Some(INTERVAL))));
    }

    #[test]
    fn renode_time_handle_request_returns_immediately_after_grant_on_reactivated_source() {
        let mut handle = TimeHandle::default();
        handle.set_source_active(false);
        handle.set_source_active(true);
        handle.grant(INTERVAL).unwrap();
        assert_eq!(handle.request(), Ok(Poll::Ready(Some(INTERVAL))));
    }

    #[test]
    fn renode_time_handle_wait_on_disabled_handle_reports_grant_used() {
        let mut handle = TimeHandle::default();
        assert_eq!(handle.request(), Ok(Poll::Pending));
        handle.set_enabled(false);
        assert_eq!(handle.request(), Ok(Poll::Ready(None)));
        handle.grant(INTERVAL).unwrap();
        assert_eq!(handle.wait(), Ok(Poll::Ready((true, false, INTERVAL))));
    }

    #[test]
    fn renode_time_handle_reenabled_sink_waits_without_consuming_unrequested_grant() {
        let mut handle = TimeHandle::default();
        handle.set_enabled(false);
        handle.set_enabled(true);
        handle.grant(INTERVAL).unwrap();
        assert_eq!(handle.wait(), Ok(Poll::Ready((false, false, 0))));
    }

    #[test]
    fn renode_time_handle_disabling_blocked_sink_completes_the_remaining_grant() {
        let mut handle = running();
        handle.report_break(500).unwrap();
        assert_eq!(handle.wait(), Ok(Poll::Ready((false, false, 500))));
        handle.set_enabled(false);
        assert_eq!(handle.wait(), Ok(Poll::Ready((true, false, 500))));
        handle.grant(INTERVAL).unwrap();
        assert_eq!(handle.wait(), Ok(Poll::Ready((true, false, INTERVAL))));
        handle.set_enabled(true);
        handle.grant(INTERVAL).unwrap();
        assert_eq!(handle.request(), Ok(Poll::Ready(Some(INTERVAL))));
        handle.report_continue().unwrap();
        assert_eq!(handle.wait(), Ok(Poll::Ready((true, false, INTERVAL))));
    }

    fn running() -> TimeHandle {
        let mut handle = TimeHandle::default();
        handle.grant(INTERVAL).unwrap();
        assert_eq!(handle.request(), Ok(Poll::Ready(Some(INTERVAL))));
        handle
    }

    fn wait_for_report(request_first: bool, report_first: bool) {
        let mut handle = TimeHandle::default();
        if request_first {
            assert_eq!(handle.request(), Ok(Poll::Pending));
        }
        handle.grant(INTERVAL).unwrap();
        assert_eq!(handle.request(), Ok(Poll::Ready(Some(INTERVAL))));
        if !report_first {
            assert_eq!(handle.wait(), Ok(Poll::Pending));
        }
        handle.report_continue().unwrap();
        assert_eq!(handle.wait(), Ok(Poll::Ready((true, false, INTERVAL))));
    }

    #[test]
    fn renode_time_handle_continue_finishes_wait_request_first() {
        wait_for_report(true, false);
    }

    #[test]
    fn renode_time_handle_continue_finishes_wait_grant_first() {
        wait_for_report(false, false);
    }

    #[test]
    fn renode_time_handle_wait_returns_after_continue_grant_first() {
        wait_for_report(false, true);
    }

    #[test]
    fn renode_time_handle_wait_returns_after_continue_request_first() {
        wait_for_report(true, true);
    }

    #[test]
    fn renode_time_handle_wait_for_report_grant_first() {
        let mut handle = TimeHandle::default();
        handle.grant(INTERVAL).unwrap();
        assert_eq!(handle.request(), Ok(Poll::Ready(Some(INTERVAL))));
        assert_eq!(handle.wait(), Ok(Poll::Pending));
    }

    #[test]
    fn renode_time_handle_wait_for_report_request_first() {
        let mut handle = TimeHandle::default();
        assert_eq!(handle.request(), Ok(Poll::Pending));
        handle.grant(INTERVAL).unwrap();
        assert_eq!(handle.request(), Ok(Poll::Ready(Some(INTERVAL))));
        assert_eq!(handle.wait(), Ok(Poll::Pending));
    }
}
