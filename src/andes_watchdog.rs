pub(crate) struct AndesWatchdog {
    pub base: u32,
    pub name: String,
    pub irq: Option<(usize, u32)>,
    frequency: u32,
    phase: u64,
    control: u32,
    unlocked: bool,
    elapsed: bool,
    interrupt_ticks: u64,
    reset_ticks: u64,
    reset_armed: bool,
}

impl AndesWatchdog {
    pub const SIZE: u32 = 0x100;
    const ENABLE: u32 = 1;
    const INTERRUPT: u32 = 1 << 2;
    const RESET: u32 = 1 << 3;
    const UNLOCK: u16 = 0x5aa5;
    const RESTART: u16 = 0xcafe;

    pub(crate) fn reset(&mut self) {
        self.phase = 0;
        self.control = 0;
        self.unlocked = false;
        self.elapsed = false;
        self.interrupt_ticks = 0;
        self.reset_ticks = 0;
        self.reset_armed = false;
    }

    pub fn new(base: u32, name: String, frequency: u32, irq: Option<(usize, u32)>) -> Self {
        Self {
            base,
            name,
            frequency,
            irq,
            phase: 0,
            control: 0,
            unlocked: false,
            elapsed: false,
            interrupt_ticks: 0,
            reset_ticks: 0,
            reset_armed: false,
        }
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    pub fn interrupt(&self) -> bool {
        self.elapsed && self.control & Self::INTERRUPT != 0
    }

    pub fn read(&self, address: u32) -> u32 {
        match address - self.base {
            0 => 0x0300_2012,
            0x10 => self.control,
            0x1c => u32::from(self.elapsed),
            _ => 0,
        }
    }

    pub fn write(&mut self, address: u32, value: u32) {
        let offset = address - self.base;
        if matches!(offset, 0x10 | 0x14) && !self.unlocked {
            let register = if offset == 0x10 { "Control" } else { "Restart" };
            crate::monitor_log::record(
                2,
                &format!(
                    "{}: Ignoring write of value {value} to write-protected register {register} at offset {offset}",
                    self.name,
                ),
            );
            return;
        }
        match offset {
            0x10 => {
                let previous = self.control;
                self.control = value & 0x7fd;
                if self.control != previous {
                    self.update();
                }
            }
            0x14 if value as u16 == Self::RESTART => {
                self.reset_armed = false;
                self.reset_ticks = 0;
                self.interrupt_ticks = 0;
                self.log("Restarted watchdog");
            }
            0x18 => {
                self.unlocked = value as u16 == Self::UNLOCK;
                return;
            }
            0x1c if value & 1 != 0 => {
                self.elapsed = false;
                self.interrupt_ticks = 0;
                self.update();
            }
            _ => {}
        }
        self.unlocked = false;
    }

    fn log(&self, message: &str) {
        crate::monitor_log::record(0, &format!("{}: {message}", self.name));
    }

    fn update(&mut self) {
        self.reset_armed = self.elapsed
            && self.control & (Self::ENABLE | Self::RESET) == (Self::ENABLE | Self::RESET);
        let irq = if self.interrupt() { "True" } else { "False" };
        self.log(&format!("IRQ: {irq}"));
        let interrupt_timer = if !self.elapsed && self.control & Self::ENABLE != 0 {
            "True"
        } else {
            "False"
        };
        let reset_timer = if self.reset_armed { "True" } else { "False" };
        self.log(&format!("interruptTimer enabled: {interrupt_timer}"));
        self.log(&format!("resetTimer enabled: {reset_timer}"));
    }

    pub(crate) fn set_frequency(&mut self, frequency: u32) {
        self.frequency = frequency;
    }

    pub(crate) fn can_advance(&self) -> bool {
        self.control & Self::ENABLE != 0 && (!self.elapsed || self.reset_armed)
    }

    pub fn tick(&mut self, rate: u32) -> bool {
        if self.control & Self::ENABLE == 0 || (self.elapsed && !self.reset_armed) {
            return false;
        }
        self.phase += u64::from(self.frequency);
        let mut ticks = self.phase / u64::from(rate);
        self.phase %= u64::from(rate);
        if !self.elapsed {
            let limit = 1 << (((self.control >> 4) & 0xf) + 6);
            let until_interrupt = limit - self.interrupt_ticks.min(limit);
            if ticks < until_interrupt {
                self.interrupt_ticks += ticks;
                return false;
            }
            ticks -= until_interrupt;
            self.interrupt_ticks = 0;
            self.elapsed = true;
            self.log("Interrupt timer elapsed");
            self.update();
        }
        if self.reset_armed {
            let limit = 1 << (((self.control >> 8) & 7) + 7);
            self.reset_ticks += ticks;
            if self.reset_ticks >= limit {
                self.reset_armed = false;
                self.reset_ticks = 0;
                self.log("Reset timer elapsed");
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::AndesWatchdog;

    #[test]
    fn protection_restart_and_expiry_follow_separate_interrupt_and_reset_intervals() {
        let mut watchdog = AndesWatchdog::new(0, "watchdog".into(), 1, None);
        watchdog.write(0x10, 0xd);
        assert_eq!(watchdog.read(0x10), 0);
        watchdog.write(0x18, 0x5aa5);
        watchdog.write(0x10, 0xd);
        watchdog.write(0x10, 0);
        assert_eq!(watchdog.read(0x10), 0xd);
        for _ in 0..40 {
            assert!(!watchdog.tick(1));
        }
        watchdog.write(0x18, 0x5aa5);
        watchdog.write(0x14, 0xcafe);
        for _ in 0..63 {
            assert!(!watchdog.tick(1));
        }
        assert!(!watchdog.interrupt());
        assert!(!watchdog.tick(1));
        assert!(watchdog.interrupt());
        for _ in 0..127 {
            assert!(!watchdog.tick(1));
        }
        assert!(watchdog.tick(1));
        assert!(!watchdog.tick(1));
        watchdog.write(0x1c, 1);
        assert!(!watchdog.interrupt());
    }
}
