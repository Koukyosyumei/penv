use std::{cell::RefCell, num::NonZeroU64, time::Duration};

const NANOS_PER_SECOND: u128 = 1_000_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClockId(usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClockEntry {
    pub period: u64,
    pub value: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClockOverflow;

type Handler<'a> = Box<dyn FnMut(&ClockSource<'a>) + 'a>;

struct Registration<'a> {
    origin: u64,
    initial_value: u64,
    period: NonZeroU64,
    frequency: NonZeroU64,
    next_ticks: u128,
    handler: RefCell<Handler<'a>>,
}

impl Registration<'_> {
    fn deadline(&self) -> u128 {
        let scaled = self.next_ticks * NANOS_PER_SECOND;
        u128::from(self.origin) + scaled.div_ceil(u128::from(self.frequency.get()))
    }
}

/// A periodic virtual clock with independently registered tick rates.
#[derive(Default)]
pub struct ClockSource<'a> {
    now: u64,
    entries: Vec<Registration<'a>>,
}

impl<'a> ClockSource<'a> {
    pub fn add(
        &mut self,
        period: NonZeroU64,
        frequency: NonZeroU64,
        initial_value: u64,
        handler: impl FnMut(&Self) + 'a,
    ) -> ClockId {
        let initial_value = initial_value % period;
        let id = ClockId(self.entries.len());
        self.entries.push(Registration {
            origin: self.now,
            initial_value,
            period,
            frequency,
            next_ticks: u128::from(period.get() - initial_value),
            handler: RefCell::new(Box::new(handler)),
        });
        id
    }

    pub fn entry(&self, id: ClockId) -> ClockEntry {
        let registration = &self.entries[id.0];
        let elapsed = u128::from(self.now - registration.origin);
        let ticks = elapsed * u128::from(registration.frequency.get()) / NANOS_PER_SECOND;
        let period = registration.period.get();
        ClockEntry {
            period,
            value: ((ticks + u128::from(registration.initial_value)) % u128::from(period)) as u64,
        }
    }

    pub fn advance(&mut self, duration: Duration) -> Result<(), ClockOverflow> {
        let delta = u64::try_from(duration.as_nanos()).map_err(|_| ClockOverflow)?;
        let target = self.now.checked_add(delta).ok_or(ClockOverflow)?;
        while let Some((deadline, index)) = self
            .entries
            .iter()
            .enumerate()
            .map(|(index, entry)| (entry.deadline(), index))
            .min()
            .filter(|&(deadline, _)| deadline <= u128::from(target))
        {
            self.now = deadline as u64;
            let entry = &mut self.entries[index];
            entry.next_ticks += u128::from(entry.period.get());
            (self.entries[index].handler.borrow_mut())(self);
        }
        self.now = target;
        Ok(())
    }
}
