use super::TimeSource;
use crate::time_handle::TimeHandle;
use std::task::Poll;
use std::time::{Duration, Instant};

pub struct MasterTimeSource<const QUANTUM: u64> {
    clock: TimeSource<QUANTUM>,
    advance_immediately: bool,
    wall_start: Option<Instant>,
}

impl<const QUANTUM: u64> MasterTimeSource<QUANTUM> {
    pub fn new(advance_immediately: bool) -> Self {
        assert_ne!(QUANTUM, 0);
        Self {
            clock: TimeSource::default(),
            advance_immediately,
            wall_start: None,
        }
    }

    pub fn sync_points(&self) -> u64 {
        self.clock.sync_points()
    }

    pub fn elapsed(&self) -> u64 {
        self.clock.elapsed()
    }

    /// Run a fixed number of synchronization points, advancing all registered sinks per grant.
    pub fn run(
        &mut self,
        points: u64,
        mut grant: impl FnMut(u64) -> Result<(), &'static str>,
    ) -> Result<(), &'static str> {
        let finish = self
            .sync_points()
            .checked_add(points)
            .ok_or("sync point overflow")?;
        for index in self.sync_points()..finish {
            let boundary = index.checked_mul(QUANTUM).ok_or("virtual time overflow")?;
            if !self.advance_immediately {
                let start = self.wall_start.get_or_insert_with(Instant::now);
                let remaining = Duration::from_nanos(boundary).saturating_sub(start.elapsed());
                if !remaining.is_zero() {
                    std::thread::sleep(remaining);
                }
            }
            if index != 0 {
                grant(boundary)?;
            }
            self.clock.grant_until(boundary);
        }
        Ok(())
    }
}

#[derive(Default)]
pub struct SlaveTimeSource<const QUANTUM: u64> {
    clock: TimeSource<QUANTUM>,
    handle: TimeHandle,
}

impl<const QUANTUM: u64> SlaveTimeSource<QUANTUM> {
    pub fn sync_points(&self) -> u64 {
        self.clock.sync_points()
    }

    pub fn elapsed(&self) -> u64 {
        self.clock.elapsed()
    }

    pub fn disconnect(&mut self) {
        self.handle.dispose();
    }

    pub fn advance_to(
        &mut self,
        boundary: u64,
        mut grant_child: impl FnMut(u64),
    ) -> Result<(), &'static str> {
        if !self.handle.is_ready_for_grant() {
            return Ok(());
        }
        let elapsed = self.handle.elapsed();
        let delta = boundary
            .checked_sub(elapsed)
            .ok_or("parent time moved backwards")?;
        self.handle.grant(delta)?;
        let Poll::Ready(Some(granted)) = self.handle.request()? else {
            return Err("slave did not receive its grant");
        };
        self.clock.grant_until(elapsed + granted);
        grant_child(self.clock.elapsed());
        self.handle.report_continue()?;
        let Poll::Ready(_) = self.handle.wait()? else {
            return Err("slave did not report its grant");
        };
        Ok(())
    }
}
