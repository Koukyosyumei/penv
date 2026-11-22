#[derive(Default)]
pub struct Gpio {
    output: u32,
    enable: u32,
}

impl Gpio {
    pub fn read(&self, offset: u32) -> u32 {
        match offset {
            4 => self.output & self.enable,
            8 => self.enable,
            _ => 0,
        }
    }

    pub fn write(&mut self, offset: u32, value: u32) {
        match offset {
            4 => self.output = value,
            8 => self.enable = value,
            _ => {}
        }
    }
}

struct Counter {
    control: u32,
    running: bool,
    limit: u64,
    value: u64,
    phase: u64,
}

impl Default for Counter {
    fn default() -> Self {
        Self {
            control: 0,
            running: false,
            limit: u64::from(u16::MAX),
            value: 0,
            phase: 0,
        }
    }
}

#[derive(Default)]
pub struct Timer {
    pub connected: bool,
    prescaler: u32,
    pending: u32,
    mask: u32,
    counters: [Counter; 2],
}

impl Timer {
    pub fn tick(&mut self) -> bool {
        for (index, counter) in self.counters.iter_mut().enumerate() {
            if !counter.running {
                continue;
            }
            let divider = if counter.control & 3 == 2 {
                u64::from(self.prescaler) + 1
            } else {
                1
            };
            counter.phase += 1;
            if counter.phase < divider {
                continue;
            }
            counter.phase = 0;
            counter.value += 1;
            if counter.value >= counter.limit {
                counter.value = 0;
                counter.running = counter.control & (1 << 16) != 0;
                self.pending |= 1 << index;
            }
        }
        self.connected && self.mask & self.pending != 0
    }

    pub fn read(&self, offset: u32) -> u32 {
        let [a, b] = &self.counters;
        match offset {
            0 => self.prescaler,
            0x10 => self.pending,
            0x14 => self.mask,
            0x40 => a.control,
            0x50 => b.control,
            // Renode exposes Timer A's limit at both offsets.
            0x44 | 0x54 => a.limit as u32,
            0x48 => a.value as u32,
            0x58 => b.value as u32,
            _ => 0,
        }
    }

    pub fn write(&mut self, offset: u32, value: u32) {
        match offset {
            0 => {
                self.prescaler = value;
                for counter in &mut self.counters {
                    counter.phase = 0;
                }
            }
            0x10 => self.pending &= !value,
            0x14 => self.mask = value & 3,
            0x40..=0x58 => {
                let counter = &mut self.counters[((offset - 0x40) / 16) as usize];
                match offset & 15 {
                    0 => {
                        counter.control = value & 0x1_0003;
                        counter.running = value & 3 != 0;
                        counter.phase = 0;
                    }
                    4 => counter.limit = u64::from(value) + 1,
                    8 => {
                        counter.value = 0;
                        counter.phase = 0;
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prescaled_periodic_timer_reasserts_after_write_one_to_clear() {
        let mut timer = Timer {
            connected: true,
            ..Timer::default()
        };
        timer.write(0, 1);
        timer.write(0x14, 2);
        timer.write(0x54, 0);
        timer.write(0x50, 0x1_0002);
        assert!(!timer.tick());
        assert!(timer.tick());
        assert_eq!(timer.read(0x10), 2);
        timer.write(0x10, 2);
        assert!(!timer.tick());
        assert!(timer.tick());
    }
}
