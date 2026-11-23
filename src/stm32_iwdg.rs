pub struct Iwdg {
    pub base: u32,
    frequency: u32,
    window_option: bool,
    default_prescaler: u8,
    prescaler: u8,
    reload: u16,
    window: u16,
    remaining: u16,
    phase: u64,
    running: bool,
    unlocked: bool,
    reset_requested: bool,
}

impl Iwdg {
    pub const MODEL: &str = "Timers.STM32_IndependentWatchdog";
    pub const SIZE: u32 = 0x400;

    pub fn new(base: u32, frequency: u32, window_option: bool, default_prescaler: u8) -> Self {
        Self {
            base,
            frequency,
            window_option,
            default_prescaler,
            prescaler: default_prescaler,
            reload: 0xfff,
            window: 0xfff,
            remaining: 0x1000,
            phase: 0,
            running: false,
            unlocked: false,
            reset_requested: false,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new(
            self.base,
            self.frequency,
            self.window_option,
            self.default_prescaler,
        );
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    pub fn read(&self, address: u32) -> u32 {
        match address - self.base {
            4 => self.prescaler.into(),
            8 => self.reload.into(),
            0x10 if self.window_option => self.window.into(),
            _ => 0,
        }
    }

    pub fn write(&mut self, address: u32, value: u32) {
        match address - self.base {
            0 => {
                self.unlocked = value as u16 == 0x5555;
                match value as u16 {
                    0xcccc => self.running = true,
                    0xaaaa
                        if self.window_option
                            && self.running
                            && self.remaining.saturating_sub(1) > self.window =>
                    {
                        self.reset_requested = true;
                    }
                    0xaaaa => self.feed(),
                    _ => {}
                }
            }
            4 if self.unlocked => {
                self.prescaler = (value & 7) as u8;
                self.phase = 0;
            }
            8 if self.unlocked => self.reload = (value & 0xfff) as u16,
            0x10 if self.unlocked && self.window_option => {
                self.window = (value & 0xfff) as u16;
                self.feed();
            }
            _ => {}
        }
    }

    fn feed(&mut self) {
        self.remaining = self.reload + 1;
        self.phase = 0;
    }

    pub fn tick(&mut self, step_hz: u32) -> bool {
        if self.reset_requested {
            return true;
        }
        if !self.running {
            return false;
        }
        let divisor = u64::from(step_hz) << (2 + self.prescaler.min(6));
        self.phase += u64::from(self.frequency);
        let ticks = (self.phase / divisor) as u16;
        self.phase %= divisor;
        self.remaining = self.remaining.saturating_sub(ticks);
        self.remaining == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::{Machine, Stop};

    // Mirrors the SRAM-retaining reset requirement in Renode's STM32L072 watchdog test.
    #[test]
    fn feed_delays_reset_then_reenters_the_guest_with_sram_intact() {
        const BASE: u32 = 0x5800_4800;
        let mut machine = Machine::without_default_ram();
        machine.bus.map_ram(0, 0x400).unwrap();
        machine.bus.map_nvic(0xf0, None).unwrap();
        machine
            .bus
            .map_iwdg(Iwdg::new(BASE, 4_000_000, true, 0))
            .unwrap();
        machine.primary.configure_cortex_m(Some(0), false);
        let write = |machine: &mut Machine, address, value| {
            machine
                .bus
                .write(address, 4, value, &mut std::io::sink())
                .unwrap();
        };
        write(&mut machine, 0, 0x400);
        write(&mut machine, 4, 0x101);
        // Spin first, then replace the reset entry with BKPT for the second boot.
        machine
            .bus
            .memory(0x100, 2)
            .unwrap()
            .copy_from_slice(&[0xfe, 0xe7]);
        machine.validate_entry().unwrap();
        write(&mut machine, 0x200, 0x1234_abcd);
        machine.primary.registers[1] = 9;
        write(&mut machine, BASE + 8, 0);
        assert_eq!(machine.bus.read::<4>(BASE + 8).unwrap(), 0xfff);
        for (offset, value) in [(0, 0x5555), (8, 3), (0, 0xaaaa), (0, 0xcccc)] {
            write(&mut machine, BASE + offset, value);
        }
        let mut output = std::io::sink();
        assert_eq!(machine.run(3, &mut output).unwrap().0, Stop::Limit);
        write(&mut machine, BASE, 0xaaaa);
        assert_eq!(machine.run(3, &mut output).unwrap().0, Stop::Limit);
        assert_eq!(machine.primary.registers[1], 9);
        machine
            .bus
            .memory(0x100, 2)
            .unwrap()
            .copy_from_slice(&[0, 0xbe]);
        assert_eq!(machine.run(1, &mut output).unwrap().0, Stop::Ebreak);
        assert_eq!(machine.primary.registers[1], 0);
        assert_eq!(machine.primary.registers[13], 0x400);
        assert_eq!(machine.bus.read_ram::<4>(0x200).unwrap(), 0x1234_abcd);
        assert_eq!(machine.bus.read::<4>(BASE + 8).unwrap(), 0xfff);
    }

    #[test]
    fn refresh_before_the_window_requests_reset() {
        let mut watchdog = Iwdg::new(0, 4_000_000, true, 0);
        watchdog.write(0, 0x5555);
        watchdog.write(8, 10);
        watchdog.write(0x10, 5);
        watchdog.write(0, 0xcccc);
        assert!(!watchdog.tick(crate::machine::VIRTUAL_STEPS_PER_SECOND));
        watchdog.write(0, 0xaaaa);
        assert!(watchdog.tick(crate::machine::VIRTUAL_STEPS_PER_SECOND));
    }
}
