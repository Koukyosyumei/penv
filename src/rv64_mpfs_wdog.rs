// PolarFire watchdog reset defaults and state transitions.
const REFRESH_KEY: u32 = 0xdead_c0de;
const RESET_KEY: u32 = 0xdead;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Forbidden,
    Permitted,
    Triggered,
}

#[derive(Default)]
pub struct Output {
    pub plic: usize,
    pub harts: Vec<(usize, u32)>,
}

pub struct Watchdog {
    pub base: u32,
    pub outputs: [Output; 2],
    frequency: u64,
    steps_per_second: u64,
    ticks: u64,
    deadline: Option<u64>,
    phase: Phase,
    control: u32,
    status: u32,
    time: u32,
    permit: u32,
    trigger: u32,
    reset: bool,
}

impl Watchdog {
    pub const SIZE: u64 = 0x1000;

    pub fn new(base: u32, frequency: u64, steps_per_second: u64, outputs: [Output; 2]) -> Self {
        Self {
            base,
            outputs,
            frequency,
            steps_per_second,
            ticks: 0,
            deadline: None,
            phase: Phase::Forbidden,
            control: 2,
            status: 0,
            time: 30_000,
            permit: 15_000,
            trigger: 1_000,
            reset: false,
        }
    }

    pub fn advance(&mut self, steps: u64) -> bool {
        self.ticks = (u128::from(steps) * u128::from(self.frequency)
            / u128::from(self.steps_per_second)) as u64;
        while let Some(deadline) = self.deadline.filter(|&deadline| deadline <= self.ticks) {
            match self.phase {
                Phase::Forbidden => self.begin(Phase::Permitted, deadline),
                Phase::Permitted => self.begin(Phase::Triggered, deadline),
                Phase::Triggered => {
                    self.deadline = None;
                    self.reset = true;
                }
            }
        }
        self.reset
    }

    fn begin(&mut self, phase: Phase, now: u64) {
        self.phase = phase;
        let period = match phase {
            Phase::Forbidden => {
                self.status &= !3;
                self.time.saturating_sub(self.permit)
            }
            Phase::Permitted => {
                self.status = (self.status & !2) | 1;
                self.permit.saturating_sub(self.trigger)
            }
            Phase::Triggered => {
                self.status |= 2;
                self.trigger
            }
        };
        self.deadline = Some(now.saturating_add(u64::from(period)));
    }

    pub fn levels(&self) -> [bool; 2] {
        [self.control & self.status & 1 != 0, self.status & 2 != 0]
    }

    fn offset(&self, address: u32, width: usize) -> Option<u32> {
        let offset = address.checked_sub(self.base)?;
        (width == 4 && offset.is_multiple_of(4) && u64::from(offset) < Self::SIZE).then_some(offset)
    }

    pub fn read(&self, address: u32, width: usize) -> Option<u32> {
        Some(match self.offset(address, width)? {
            0 => {
                self.deadline
                    .map_or(0, |deadline| deadline.saturating_sub(self.ticks) as u32)
                    + match self.phase {
                        Phase::Forbidden => self.permit,
                        Phase::Permitted => self.trigger,
                        Phase::Triggered => 0,
                    }
            }
            4 => self.control,
            8 => {
                self.status
                    | (u32::from(self.phase == Phase::Forbidden && self.control & 0x10 != 0) << 2)
                    | (u32::from(self.phase == Phase::Triggered) << 3)
            }
            0xc => self.time,
            0x10 => self.permit,
            0x14 => self.trigger,
            _ => 0,
        })
    }

    pub fn write(&mut self, address: u32, width: usize, value: u32) -> Option<()> {
        let offset = self.offset(address, width)?;
        match offset {
            0 if self.deadline.is_none() => self.begin(Phase::Forbidden, self.ticks),
            0 if self.phase == Phase::Forbidden && self.control & 0x10 != 0 => {
                self.begin(Phase::Triggered, self.ticks);
            }
            0 if self.phase != Phase::Triggered && value == REFRESH_KEY => {
                self.begin(Phase::Forbidden, self.ticks);
            }
            4 => self.control = 2 | (value & 0x11),
            8 => self.status &= !(value & 3),
            0xc => self.time = value & 0xff_ffff,
            0x10 => self.permit = value & 0xff_ffff,
            0x14 => self.trigger = value & 0xfff,
            0x18 if self.phase == Phase::Triggered && value & 0xffff == RESET_KEY => {
                self.reset = true;
            }
            0x18 => self.begin(Phase::Triggered, self.ticks),
            _ => {}
        }
        if matches!(offset, 0 | 4 | 8 | 0xc | 0x18) {
            self.status |= 1 << 4;
        }
        Some(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watchdog_reports_windows_triggers_and_resets() {
        let mut watchdog = Watchdog::new(0x2000_1000, 1, 1, Default::default());
        let base = watchdog.base;
        assert_eq!(watchdog.read(base + 0xc, 4), Some(30_000));
        watchdog.write(base + 0xc, 4, 10).unwrap();
        watchdog.write(base + 0x10, 4, 5).unwrap();
        watchdog.write(base + 0x14, 4, 2).unwrap();
        watchdog.write(base + 4, 4, 0x11).unwrap();
        watchdog.write(base, 4, REFRESH_KEY).unwrap();
        assert_eq!(watchdog.read(base + 8, 4), Some(0x14));
        assert!(!watchdog.advance(5));
        assert_eq!(watchdog.levels(), [true, false]);
        assert!(!watchdog.advance(8));
        assert_eq!(watchdog.levels(), [true, true]);
        assert!(watchdog.advance(10));
    }
}
