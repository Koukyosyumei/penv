pub(crate) struct Agt {
    pub(crate) base: u32,
    clocks: [u32; 3],
    routes: [Option<usize>; 3],
    count: u16,
    reload: u16,
    compare: [u16; 2],
    mode: u8,
    divider: u8,
    compare_enable: u8,
    flags: u8,
    running: bool,
    phase: u64,
}

impl Agt {
    pub(crate) const SIZE: u32 = 0x100;

    pub(crate) fn new(base: u32, clocks: [u32; 3], routes: [Option<usize>; 3]) -> Self {
        Self {
            base,
            clocks,
            routes,
            count: u16::MAX,
            reload: u16::MAX,
            compare: [u16::MAX; 2],
            mode: 0,
            divider: 0,
            compare_enable: 0,
            flags: 0,
            running: false,
            phase: 0,
        }
    }

    pub(crate) fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    pub(crate) fn running(&self) -> bool {
        self.running
    }

    pub(crate) fn reset(&mut self) {
        *self = Self::new(self.base, self.clocks, self.routes);
    }

    pub(crate) fn tick(&mut self, step_hz: u32, mut pulse: impl FnMut(usize)) {
        if !self.running {
            return;
        }
        let [peripheral, low_speed, sub] = self.clocks;
        let (frequency, divisor) = match self.mode >> 4 {
            1 => (peripheral, 8),
            3 => (peripheral, 2),
            4 => (low_speed, 1 << self.divider),
            7 => (sub, 1 << self.divider),
            _ => (peripheral, 1),
        };
        let denominator = u64::from(step_hz) * divisor;
        self.phase += u64::from(frequency);
        let ticks = self.phase / denominator;
        self.phase %= denominator;
        if ticks == 0 {
            return;
        }

        let period = u64::from(self.reload).max(1);
        let distance = u64::from(self.count).max(1);
        for (channel, &compare) in self.compare.iter().enumerate() {
            let before_wrap = compare < self.count && ticks >= u64::from(self.count - compare);
            let after_wrap = ticks > distance
                && compare < self.reload
                && ticks - distance >= u64::from(self.reload - compare);
            if self.compare_enable & (1 << (channel * 4)) != 0 && (before_wrap || after_wrap) {
                self.flags |= 1 << (6 + channel);
                if let Some(event) = self.routes[channel + 1] {
                    pulse(event);
                }
            }
        }
        if ticks >= distance {
            self.flags |= 0x20;
            self.count = self
                .reload
                .saturating_sub(((ticks - distance) % period) as u16);
            if let Some(event) = self.routes[0] {
                pulse(event);
            }
        } else {
            self.count -= ticks as u16;
        }
    }

    pub(crate) fn read(&self, address: u32, width: usize) -> u32 {
        match (width, address - self.base) {
            (2, 0) => self.count.into(),
            (2, 2) => self.compare[0].into(),
            (2, 4) => self.compare[1].into(),
            (1, 8) => u32::from(self.flags | if self.running { 3 } else { 0 }),
            (1, 9) => self.mode.into(),
            (1, 10) => self.divider.into(),
            (1, 14) => self.compare_enable.into(),
            _ => 0,
        }
    }

    pub(crate) fn write(&mut self, address: u32, width: usize, value: u32) {
        let byte = value as u8;
        match (width, address - self.base) {
            (2, 0) => {
                self.reload = value as u16;
                self.count = self.reload;
            }
            (2, 2) => self.compare[0] = value as u16,
            (2, 4) => self.compare[1] = value as u16,
            (1, 8) => {
                self.flags &= byte;
                if byte & 4 != 0 {
                    self.running = false;
                    self.count = u16::MAX;
                } else {
                    let running = byte & 1 != 0;
                    if running && !self.running {
                        self.count = self.reload;
                        self.phase = 0;
                    }
                    self.running = running;
                }
            }
            (1, 9) if matches!(byte & 7, 0 | 1) && matches!((byte >> 4) & 7, 0 | 1 | 3 | 4 | 7) => {
                self.mode = byte & 0x77;
                self.phase = 0;
            }
            (1, 10) => {
                self.divider = byte & 7;
                self.phase = 0;
            }
            (1, 14) => self.compare_enable = byte & 0x11,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clocks_compare_and_underflow_and_clears_status() {
        let mut timer = Agt::new(0, [8, 4, 2], [Some(70), Some(71), None]);
        timer.write(0, 2, 3);
        timer.write(2, 2, 2);
        timer.write(14, 1, 1);
        timer.write(8, 1, 1);
        let mut events = Vec::new();
        for _ in 0..3 {
            timer.tick(8, |event| events.push(event));
        }
        assert_eq!(events, [71, 70]);
        assert_eq!(timer.read(8, 1), 0x63);
        timer.write(8, 1, 0x21);
        assert_eq!(timer.read(8, 1), 0x23);
        timer.write(8, 1, 4);
        assert_eq!(timer.read(0, 2), 0xffff);
    }
}
