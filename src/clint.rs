pub const SOFTWARE: u32 = 1 << 3;
pub const TIMER: u32 = 1 << 7;

struct Target {
    compare: u64,
    software: bool,
}

pub struct Clint {
    pub base: u32,
    time: u64,
    clock: Option<StepClock>,
    targets: Vec<Target>,
}

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

    pub fn new(base: u32, target_count: usize) -> Self {
        Self {
            base,
            time: 0,
            clock: None,
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

    pub fn tick(&mut self, hart: usize) -> u32 {
        let ticks = self.clock.as_mut().map_or(1, StepClock::advance);
        self.time = self.time.wrapping_add(ticks);
        self.targets.get(hart).map_or(0, |target| {
            (u32::from(target.software) * SOFTWARE)
                | (u32::from(self.time >= target.compare) * TIMER)
        })
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
