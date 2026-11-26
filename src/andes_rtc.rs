pub(crate) struct AndesRtc {
    pub base: u32,
    pub irq: Option<(usize, u32)>,
    counter: u32,
    alarm: u32,
    control: u32,
    status: u32,
    phase: u64,
    half_second: bool,
}

impl AndesRtc {
    pub const SIZE: u32 = 0x1000;
    const TIME_MASK: u32 = 0x1_ffff;
    const INTERRUPT_MASK: u32 = 0xfc;
    const STATUS_WRITE_DONE: u32 = 1 << 16;

    pub(crate) fn reset(&mut self) {
        *self = Self::new(self.base, self.irq);
    }

    pub fn new(base: u32, irq: Option<(usize, u32)>) -> Self {
        Self {
            base,
            irq,
            counter: 0,
            alarm: 0,
            control: 0,
            status: 0,
            phase: 0,
            half_second: false,
        }
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    pub fn interrupt(&self) -> bool {
        self.control & self.status & Self::INTERRUPT_MASK != 0
    }

    pub fn read(&self, address: u32) -> u32 {
        match address - self.base {
            0 => 0x0301_1000,
            0x10 => self.counter,
            0x14 => self.alarm,
            0x18 => self.control,
            0x1c => self.status | Self::STATUS_WRITE_DONE,
            _ => 0,
        }
    }

    pub fn write(&mut self, address: u32, value: u32) {
        match address - self.base {
            0x10 => self.counter = value,
            0x14 => self.alarm = value & Self::TIME_MASK,
            0x18 => self.control = value & 0xff,
            0x1c => self.status &= !(value & Self::INTERRUPT_MASK),
            _ => {}
        }
    }

    pub(crate) fn can_advance(&self) -> bool {
        self.control & 1 != 0
    }

    pub fn tick(&mut self, rate: u32) {
        if self.control & 1 == 0 {
            return;
        }
        self.phase += 2;
        let rate = u64::from(rate);
        while self.phase >= rate {
            self.phase -= rate;
            self.half_second = !self.half_second;
            self.status |= 1 << 7;
            if !self.half_second {
                self.advance_second();
            }
        }
    }

    fn advance_second(&mut self) {
        self.status |= 1 << 6;
        for (shift, mask, max, rollover) in [
            (0, 63, 59, 1 << 5),
            (6, 63, 59, 1 << 4),
            (12, 31, 23, 1 << 3),
            (17, 0x7fff, 0x7fff, 0),
        ] {
            if (self.counter >> shift) & mask < max {
                self.counter += 1 << shift;
                break;
            }
            self.counter &= !(mask << shift);
            self.status |= rollover;
        }
        if self.counter & Self::TIME_MASK == self.alarm {
            self.status |= 1 << 2;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::AndesRtc;

    #[test]
    fn rtc_rolls_over_midnight_and_latches_the_alarm_until_cleared() {
        let mut rtc = AndesRtc::new(0, None);
        rtc.write(0x10, (23 << 12) | (59 << 6) | 59);
        rtc.write(0x18, 1 | (1 << 2));
        rtc.tick(4);
        rtc.tick(4);
        assert_eq!(rtc.read(0x1c), 0x1_0080);
        assert!(!rtc.interrupt());
        rtc.tick(4);
        rtc.tick(4);
        assert_eq!(rtc.read(0x10), 1 << 17);
        assert_eq!(rtc.read(0x1c), 0x1_00fc);
        assert!(rtc.interrupt());
        rtc.write(0x1c, 1 << 2);
        assert_eq!(rtc.read(0x1c), 0x1_00f8);
        assert!(!rtc.interrupt());
    }
}
