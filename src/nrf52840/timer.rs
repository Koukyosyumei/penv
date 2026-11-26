use crate::bus::CORTEX_M_STEP_HZ;

pub(super) struct Timer {
    channels: usize,
    running: bool,
    phase: u64,
    counter: u32,
    mode: u32,
    bitmode: u32,
    prescaler: u32,
    shorts: u32,
    compare: [u32; 6],
    events: u32,
    interrupts: u32,
    pub(super) raised: u32,
}

impl Timer {
    pub(super) fn new(channels: usize) -> Self {
        Self {
            channels,
            running: false,
            phase: 0,
            counter: 0,
            mode: 0,
            bitmode: 0,
            prescaler: 4,
            shorts: 0,
            compare: [0; 6],
            events: 0,
            interrupts: 0,
            raised: 0,
        }
    }

    pub(super) fn reset(&mut self) {
        *self = Self::new(self.channels);
    }

    fn channel(&self, offset: u32, first: u32) -> Option<usize> {
        let relative = offset.checked_sub(first)?;
        let index = (relative / 4) as usize;
        (relative & 3 == 0 && index < self.channels).then_some(index)
    }

    pub(super) fn event(&self, offset: u32) -> bool {
        self.channel(offset, 0x140).is_some()
    }

    pub(super) fn task(&self, offset: u32) -> bool {
        matches!(offset, 0x000 | 0x004 | 0x008 | 0x00c | 0x010)
            || self.channel(offset, 0x040).is_some()
    }

    pub(super) fn pending(&self) -> bool {
        self.events & self.interrupts != 0
    }

    #[cfg(test)]
    fn tick(&mut self) {
        self.advance(1);
    }

    pub(super) fn advance(&mut self, ticks: u64) {
        if !self.running || self.mode != 0 {
            return;
        }
        self.phase += 16_000_000 * ticks;
        let period = CORTEX_M_STEP_HZ << self.prescaler;
        while self.running && self.phase >= period {
            self.phase -= period;
            self.increment();
        }
    }

    fn mask(&self) -> u32 {
        [0xffff, 0xff, 0x00ff_ffff, u32::MAX][self.bitmode as usize]
    }

    fn clear(&mut self) {
        self.counter = 0;
        self.phase = 0;
    }

    fn increment(&mut self) {
        let mask = self.mask();
        self.counter = self.counter.wrapping_add(1) & mask;
        let mut matched = 0;
        for (index, value) in self.compare[..self.channels].iter().enumerate() {
            if value & mask == self.counter {
                matched |= 1 << index;
            }
        }

        // Capture all simultaneous matches before applying the local shortcuts.
        self.events |= matched << 16;
        self.raised |= matched << 16;
        if self.shorts & matched != 0 {
            self.counter = 0;
        }
        if self.shorts & (matched << 8) != 0 {
            self.running = false;
        }
    }

    pub(super) fn read(&self, offset: u32) -> Option<u32> {
        Some(match offset {
            offset if self.task(offset) => 0,
            0x200 => self.shorts,
            0x304 | 0x308 => self.interrupts,
            0x504 => self.mode,
            0x508 => self.bitmode,
            0x510 => self.prescaler,
            _ => {
                if let Some(index) = self.channel(offset, 0x140) {
                    (self.events >> (16 + index)) & 1
                } else {
                    self.compare[self.channel(offset, 0x540)?]
                }
            }
        })
    }

    pub(super) fn write(&mut self, offset: u32, value: u32) -> bool {
        let channels = (1 << self.channels) - 1;
        match offset {
            0x000 if value & 1 != 0 => self.running = true,
            0x004 if value & 1 != 0 => self.running = false,
            0x008 if value & 1 != 0 && self.running && self.mode != 0 => self.increment(),
            0x00c if value & 1 != 0 => self.clear(),
            0x010 if value & 1 != 0 => {
                self.running = false;
                self.clear();
            }
            0x200 => self.shorts = value & (channels | (channels << 8)),
            0x304 => self.interrupts |= value & (channels << 16),
            0x308 => self.interrupts &= !value,
            0x504 if !self.running && value <= 2 => self.mode = value,
            0x508 if !self.running => {
                self.bitmode = value & 3;
                self.counter &= self.mask();
            }
            0x510 if !self.running => {
                self.prescaler = value & 15;
                self.phase = 0;
            }
            offset if self.task(offset) => {
                if value & 1 != 0
                    && let Some(index) = self.channel(offset, 0x040)
                {
                    self.compare[index] = self.counter;
                }
            }
            _ => {
                if let Some(index) = self.channel(offset, 0x540) {
                    self.compare[index] = value;
                } else if let Some(index) = self.channel(offset, 0x140) {
                    super::set_event(&mut self.events, 1 << (16 + index), value);
                } else {
                    return false;
                }
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prescaled_compare_repeats_with_clear_shortcut() {
        let mut timer = Timer::new(4);
        assert_eq!(timer.read(0x510), Some(4));
        assert!(timer.write(0x510, 1));
        assert!(timer.write(0x540, 2));
        assert!(timer.write(0x200, 1));
        assert!(timer.write(0x304, 1 << 16));
        assert!(timer.write(0, 1));

        for _ in 0..24 {
            timer.tick();
        }
        assert!(!timer.pending());
        timer.tick();
        assert!(timer.pending());
        assert_eq!(timer.counter, 0);
        assert_eq!(timer.raised, 1 << 16);
        assert!(timer.write(0x140, 0));
        for _ in 0..25 {
            timer.tick();
        }
        assert!(timer.pending());
        assert!(timer.write(0x308, 1 << 16));
        assert!(!timer.pending());
        assert_eq!(timer.read(0x140), Some(1));
    }

    #[test]
    fn counter_wraps_and_latches_all_matches_before_stop() {
        let mut timer = Timer::new(6);
        assert!(timer.write(0x504, 1));
        assert!(timer.write(0x508, 1));
        assert!(timer.write(0x200, 1 << (8 + 5)));
        assert!(timer.write(0x008, 1));
        assert_eq!(timer.counter, 0);
        assert!(timer.write(0, 1));
        for _ in 0..255 {
            timer.write(0x008, 1);
        }
        assert_eq!(timer.counter, 255);
        assert_eq!(timer.read(0x154), Some(0));
        assert!(timer.write(0x008, 1));
        assert_eq!(timer.raised, 0x3f << 16);
        assert_eq!(timer.read(0x154), Some(1));
        assert!(timer.write(0x008, 1));
        assert_eq!(timer.counter, 0);
        assert!(!timer.write(0x558, 1));
        timer.reset();
        assert_eq!(timer.read(0x154), Some(0));
        assert_eq!(timer.read(0x510), Some(4));
    }
}
