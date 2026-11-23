use std::task::Poll;

#[derive(Default)]
enum Grant {
    #[default]
    Idle,
    Offered(u64),
    Running(u64),
    Reported(u64),
    Blocked,
}

#[derive(Default)]
pub struct TimeHandle {
    grant: Grant,
}

impl TimeHandle {
    pub fn grant(&mut self, ticks: u64) -> Result<(), &'static str> {
        if !matches!(self.grant, Grant::Idle) {
            return Err("time handle already has a grant");
        }
        self.grant = Grant::Offered(ticks);
        Ok(())
    }

    pub fn request(&mut self) -> Result<Poll<u64>, &'static str> {
        match self.grant {
            Grant::Idle | Grant::Blocked => Ok(Poll::Pending),
            Grant::Offered(ticks) => {
                self.grant = Grant::Running(ticks);
                Ok(Poll::Ready(ticks))
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

    pub fn wait(&mut self) -> Result<Poll<(bool, u64)>, &'static str> {
        match self.grant {
            Grant::Idle => Err("no time grant to wait for"),
            Grant::Offered(_) => {
                self.grant = Grant::Blocked;
                Ok(Poll::Ready((false, 0)))
            }
            Grant::Blocked => Ok(Poll::Ready((false, 0))),
            Grant::Running(_) => Ok(Poll::Pending),
            Grant::Reported(ticks) => {
                self.grant = Grant::Idle;
                Ok(Poll::Ready((true, ticks)))
            }
        }
    }

    pub fn run_grant<T>(
        &mut self,
        ticks: u64,
        run: impl FnOnce(u64) -> Result<T, String>,
    ) -> Result<T, String> {
        self.grant(ticks)?;
        let Poll::Ready(granted) = self.request()? else {
            return Err("time grant has no consumer".into());
        };
        let result = run(granted);
        self.report_continue()?;
        let _ = self.wait()?;
        result
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
        assert_eq!(handle.wait(), Ok(Poll::Ready((false, 0))));
        assert_eq!(handle.request(), Ok(Poll::Pending));
    }

    #[test]
    fn renode_time_handle_request_waits_for_grant() {
        let mut handle = TimeHandle::default();
        assert_eq!(handle.request(), Ok(Poll::Pending));
        handle.grant(INTERVAL).unwrap();
        assert_eq!(handle.request(), Ok(Poll::Ready(INTERVAL)));
    }

    fn wait_for_report(request_first: bool, report_first: bool) {
        let mut handle = TimeHandle::default();
        if request_first {
            assert_eq!(handle.request(), Ok(Poll::Pending));
        }
        handle.grant(INTERVAL).unwrap();
        assert_eq!(handle.request(), Ok(Poll::Ready(INTERVAL)));
        if !report_first {
            assert_eq!(handle.wait(), Ok(Poll::Pending));
        }
        handle.report_continue().unwrap();
        assert_eq!(handle.wait(), Ok(Poll::Ready((true, INTERVAL))));
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
        assert_eq!(handle.request(), Ok(Poll::Ready(INTERVAL)));
        assert_eq!(handle.wait(), Ok(Poll::Pending));
    }

    #[test]
    fn renode_time_handle_wait_for_report_request_first() {
        let mut handle = TimeHandle::default();
        assert_eq!(handle.request(), Ok(Poll::Pending));
        handle.grant(INTERVAL).unwrap();
        assert_eq!(handle.request(), Ok(Poll::Ready(INTERVAL)));
        assert_eq!(handle.wait(), Ok(Poll::Pending));
    }
}
