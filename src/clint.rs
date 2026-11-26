pub const SOFTWARE: u32 = 1 << 3;
pub const TIMER: u32 = 1 << 7;

#[derive(Clone)]
struct Target {
    compare: u64,
    software: bool,
}

#[derive(Clone)]
pub struct Clint {
    pub base: u32,
    time: u64,
    frequency: u32,
    steps_per_second: u32,
    fraction: u64,
    clock: Option<StepClock>,
    interrupts_connected: bool,
    targets: Vec<Target>,
}

#[derive(Clone)]
struct StepClock {
    whole: u64,
    remainder: u64,
    divisor: u64,
    fraction: u64,
}

impl StepClock {
    fn advance(&mut self) -> u64 {
        let carry = self.fraction >= self.divisor - self.remainder;
        if carry {
            self.fraction -= self.divisor - self.remainder;
        } else {
            self.fraction += self.remainder;
        }
        self.whole + u64::from(carry)
    }
}

impl Clint {
    pub const SIZE: u64 = 0x1_0000;

    pub const STEP_HZ: u32 = 100_000_000;

    pub fn new(base: u32, target_count: usize, frequency: u32) -> Self {
        Self {
            base,
            time: 0,
            frequency,
            steps_per_second: Self::STEP_HZ,
            fraction: 0,
            clock: None,
            interrupts_connected: true,
            targets: (0..target_count)
                .map(|_| Target {
                    compare: u64::MAX,
                    software: false,
                })
                .collect(),
        }
    }

    pub fn contains(&self, address: u32) -> bool {
        u64::from(address.wrapping_sub(self.base)) < Self::SIZE
    }

    pub fn set_step_rate(&mut self, frequency: u64, steps_per_second: u64) {
        self.clock = Some(StepClock {
            whole: frequency / steps_per_second,
            remainder: frequency % steps_per_second,
            divisor: steps_per_second,
            fraction: 0,
        });
    }

    pub(crate) fn set_scheduler_rate(&mut self, rate: u32) {
        self.fraction = (u128::from(self.fraction) * u128::from(rate)
            / u128::from(self.steps_per_second)) as u64;
        self.steps_per_second = rate;
    }

    pub fn disconnect_interrupts(&mut self) {
        self.interrupts_connected = false;
    }

    pub fn tick(&mut self, hart: usize) -> u32 {
        if let Some(clock) = &mut self.clock {
            self.time = self.time.wrapping_add(clock.advance());
        } else {
            let scaled = self.fraction + u64::from(self.frequency);
            let step_hz = u64::from(self.steps_per_second);
            self.time = self.time.wrapping_add(scaled / step_hz);
            self.fraction = scaled % step_hz;
        }
        if !self.interrupts_connected {
            return 0;
        }
        self.targets.get(hart).map_or(0, |target| {
            (u32::from(target.software) * SOFTWARE)
                | (u32::from(self.time >= target.compare) * TIMER)
        })
    }

    pub(crate) fn save_snapshot(&self, output: &mut impl std::io::Write) -> std::io::Result<()> {
        use crate::snapshot::write_words;
        write_words(output, &[
            u64::from(self.base), u64::from(self.frequency), self.targets.len() as u64,
            self.time, u64::from(self.steps_per_second), self.fraction,
            u64::from(self.interrupts_connected), u64::from(self.clock.is_some()),
        ])?;
        if let Some(clock) = &self.clock {
            write_words(output, &[clock.whole, clock.remainder, clock.divisor, clock.fraction])?;
        }
        for target in &self.targets {
            write_words(output, &[target.compare, u64::from(target.software)])?;
        }
        Ok(())
    }

    pub(crate) fn load_snapshot(&mut self, input: &mut impl std::io::Read) -> std::io::Result<()> {
        use crate::snapshot::{invalid, read_bool, read_words};
        let [base, frequency, count, time, rate, fraction, connected, custom_clock] = read_words(input)?;
        if base != u64::from(self.base) || frequency != u64::from(self.frequency)
            || count != self.targets.len() as u64 || rate == 0 || rate > u64::from(u32::MAX)
            || fraction >= rate
        {
            return Err(invalid("invalid CLINT snapshot configuration"));
        }
        let clock = if read_bool(custom_clock)? {
            let [whole, remainder, divisor, fraction] = read_words(input)?;
            if remainder >= divisor || fraction >= divisor
                || self.clock.as_ref().is_none_or(|clock| {
                    (clock.whole, clock.remainder, clock.divisor) != (whole, remainder, divisor)
                })
            {
                return Err(invalid("invalid CLINT snapshot clock"));
            }
            Some(StepClock { whole, remainder, divisor, fraction })
        } else if self.clock.is_none() {
            None
        } else {
            return Err(invalid("CLINT snapshot clock differs from the platform"));
        };
        let connected = read_bool(connected)?;
        for target in &mut self.targets {
            let [compare, software] = read_words(input)?;
            target.compare = compare;
            target.software = read_bool(software)?;
        }
        self.time = time;
        self.steps_per_second = rate as u32;
        self.fraction = fraction;
        self.interrupts_connected = connected;
        self.clock = clock;
        Ok(())
    }

    pub fn time(&self) -> u64 {
        self.time
    }

    pub fn read(&self, address: u32) -> u32 {
        match address - self.base {
            offset @ 0..=0x3fff => self
                .targets
                .get((offset / 4) as usize)
                .map_or(0, |target| u32::from(target.software)),
            offset @ 0x4000..=0x7fff => {
                let offset = offset - 0x4000;
                self.targets
                    .get((offset / 8) as usize)
                    .map_or(0, |target| (target.compare >> ((offset & 4) * 8)) as u32)
            }
            0xbff8 => self.time as u32,
            0xbffc => (self.time >> 32) as u32,
            _ => 0,
        }
    }

    pub fn write(&mut self, address: u32, value: u32) {
        match address - self.base {
            offset @ 0..=0x3fff => {
                if let Some(target) = self.targets.get_mut((offset / 4) as usize) {
                    target.software = value & 1 != 0;
                }
            }
            offset @ 0x4000..=0x7fff => {
                let offset = offset - 0x4000;
                if let Some(target) = self.targets.get_mut((offset / 8) as usize) {
                    set_half(&mut target.compare, value, (offset & 4) * 8);
                }
            }
            0xbff8 => set_half(&mut self.time, value, 0),
            0xbffc => set_half(&mut self.time, value, 32),
            _ => {}
        }
    }
}

fn set_half(register: &mut u64, value: u32, shift: u32) {
    *register = (*register & !(u64::from(u32::MAX) << shift)) | (u64::from(value) << shift);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_restores_pending_software_interrupt_and_fractional_tick() {
        let mut clint = Clint::new(0, 1, 10);
        clint.set_step_rate(2, 5);
        clint.write(0, 1);
        clint.write(0x4000, 1);
        clint.write(0x4004, 0);
        assert_eq!(clint.tick(0), SOFTWARE);
        assert_eq!(clint.tick(0), SOFTWARE);
        let mut snapshot = Vec::new();
        clint.save_snapshot(&mut snapshot).unwrap();

        let mut restored = Clint::new(0, 1, 10);
        restored.set_step_rate(2, 5);
        restored.load_snapshot(&mut snapshot.as_slice()).unwrap();
        assert_eq!(restored.time(), 0);
        assert_eq!(restored.tick(0), SOFTWARE | TIMER);
        assert_eq!(restored.time(), 1);
    }

    #[test]
    fn compare_interrupt_uses_the_configured_fractional_clock() {
        let mut clint = Clint::new(0, 1, 10_000_000);
        clint.write(0x4000, 1);
        clint.write(0x4004, 0);
        for _ in 0..9 {
            assert_eq!(clint.tick(0), 0);
        }
        assert_eq!(clint.time(), 0);
        assert_eq!(clint.tick(0), TIMER);
        assert_eq!(clint.time(), 1);
        clint.write(0x4000, 2);
        assert_eq!(clint.tick(0), 0);
    }

    #[test]
    fn compare_low_word_rollover_waits_for_the_programmed_high_word() {
        // Adapt the two RV32 mtimecmp stores in Renode timer-reconfiguration.robot.
        let mut clint = Clint::new(0, 1, Clint::STEP_HZ);
        clint.write(0xbff8, u32::MAX - 1);
        clint.write(0x4000, u32::MAX);
        clint.write(0x4004, 1);
        clint.write(0x4000, 1);
        assert_eq!(clint.tick(0), 0);
        assert_eq!(clint.read(0xbff8), u32::MAX);
        assert_eq!(clint.tick(0), 0);
        assert_eq!(clint.read(0xbff8), 0);
        assert_eq!(clint.read(0xbffc), 1);
        assert_eq!(clint.tick(0), TIMER);
    }
}
