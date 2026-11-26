use std::num::NonZeroU64;
use std::time::Duration;

const NANOS_PER_SECOND: u128 = 1_000_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompareAboveLimit {
    pub compare: u64,
    pub limit: u64,
}

/// Ascending virtual timer whose compare register is bounded by its configured limit.
pub struct ComparingTimer {
    frequency: NonZeroU64,
    limit: NonZeroU64,
    compare: u64,
    enabled: bool,
    event_enabled: bool,
    value: u64,
    fraction: u128,
}

impl ComparingTimer {
    pub fn new(
        frequency: NonZeroU64,
        limit: NonZeroU64,
        compare: u64,
    ) -> Result<Self, CompareAboveLimit> {
        let mut timer = Self {
            frequency,
            limit,
            compare: 0,
            enabled: false,
            event_enabled: false,
            value: 0,
            fraction: 0,
        };
        timer.set_compare(compare)?;
        Ok(timer)
    }

    pub fn compare(&self) -> u64 {
        self.compare
    }

    pub fn value(&self) -> u64 {
        self.value
    }

    pub fn set_compare(&mut self, compare: u64) -> Result<(), CompareAboveLimit> {
        let limit = self.limit.get();
        if compare > limit {
            return Err(CompareAboveLimit { compare, limit });
        }
        self.compare = compare;
        Ok(())
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    pub fn set_event_enabled(&mut self, enabled: bool) {
        self.event_enabled = enabled;
    }

    /// Advances virtual time and returns the number of compare events generated.
    pub fn advance(&mut self, elapsed: Duration) -> u128 {
        if !self.enabled {
            return 0;
        }

        let frequency = u128::from(self.frequency.get());
        let fractional_ticks = u128::from(elapsed.subsec_nanos()) * frequency + self.fraction;
        let ticks = u128::from(elapsed.as_secs()) * frequency + fractional_ticks / NANOS_PER_SECOND;
        self.fraction = fractional_ticks % NANOS_PER_SECOND;

        let period = u128::from(self.limit.get());
        let start = u128::from(self.value);
        let end = start + ticks;
        self.value = (end % period) as u64;
        if !self.event_enabled {
            return 0;
        }

        // Both 0 and the period endpoint are observed on wrap.
        let compare = u128::from(self.compare);
        let first = if compare > start {
            compare - start
        } else {
            period - (start - compare)
        };
        if ticks < first {
            0
        } else {
            1 + (ticks - first) / period
        }
    }
}
