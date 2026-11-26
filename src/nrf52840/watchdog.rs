use crate::bus::CORTEX_M_STEP_HZ;

pub(super) struct Watchdog {
    remaining: u64,
    phase: u64,
    reload: u32,
    enabled_requests: u8,
    pending_requests: u8,
    config: u32,
    timeout: bool,
    interrupt: bool,
    expiring: bool,
    pub(super) reset_requested: bool,
}

impl Default for Watchdog {
    fn default() -> Self {
        Self {
            remaining: 0,
            phase: 0,
            reload: u32::MAX,
            enabled_requests: 1,
            pending_requests: 1,
            config: 0,
            timeout: false,
            interrupt: false,
            expiring: false,
            reset_requested: false,
        }
    }
}

impl Watchdog {
    #[cfg(test)]
    fn tick(&mut self, sleeping: bool) {
        self.advance(sleeping, 1);
    }

    pub(super) fn advance(&mut self, sleeping: bool, ticks: u64) {
        if self.remaining == 0 || (sleeping && self.config & 1 == 0 && !self.expiring) {
            return;
        }
        self.phase += 32_768 * ticks;
        if self.phase < CORTEX_M_STEP_HZ {
            return;
        }
        self.phase -= CORTEX_M_STEP_HZ;
        self.remaining -= 1;
        if self.remaining == 0 {
            if self.interrupt && !self.expiring {
                self.timeout = true;
                self.expiring = true;
                self.remaining = 2;
            } else {
                self.reset_requested = true;
            }
        }
    }

    pub(super) fn pending(&self) -> bool {
        self.interrupt && self.timeout
    }

    pub(super) fn read(&self, offset: u32) -> Option<u32> {
        Some(match offset {
            0x000 | 0x600..=0x61c => 0,
            0x100 => self.timeout.into(),
            0x304 | 0x308 => self.interrupt.into(),
            0x400 => u32::from(self.remaining != 0),
            0x404 => self.pending_requests.into(),
            0x504 => self.reload,
            0x508 => self.enabled_requests.into(),
            0x50c => self.config,
            _ => return None,
        })
    }

    pub(super) fn write(&mut self, offset: u32, value: u32) -> bool {
        let running = self.remaining != 0;
        match offset {
            0x000 => {
                if !running && value & 1 != 0 {
                    self.remaining = u64::from(self.reload) + 1;
                    self.pending_requests = self.enabled_requests;
                }
            }
            0x100 => self.timeout = value & 1 != 0,
            0x304 => self.interrupt |= value & 1 != 0,
            0x308 => self.interrupt &= value & 1 == 0,
            0x504 => {
                if !running && value >= 15 {
                    self.reload = value;
                }
            }
            0x508 => {
                if !running {
                    self.enabled_requests = value as u8;
                    self.pending_requests = self.enabled_requests;
                }
            }
            0x50c => {
                if !running {
                    self.config = value & 9;
                }
            }
            0x600..=0x61c => {
                if running && !self.expiring && value == 0x6e52_4635 {
                    let request = 1 << ((offset - 0x600) / 4);
                    if self.pending_requests & request != 0 {
                        self.pending_requests &= !request;
                        if self.pending_requests == 0 {
                            self.remaining = u64::from(self.reload) + 1;
                            self.pending_requests = self.enabled_requests;
                        }
                    }
                }
            }
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tick(watchdog: &mut Watchdog, sleeping: bool) {
        let remaining = watchdog.remaining;
        for _ in 0..CORTEX_M_STEP_HZ.div_ceil(32_768) {
            watchdog.tick(sleeping);
            if watchdog.remaining != remaining {
                return;
            }
        }
    }

    #[test]
    fn requires_every_enabled_feed_and_delays_reset_for_timeout_irq() {
        let mut watchdog = Watchdog::default();
        assert!(watchdog.write(0x504, 15));
        assert!(watchdog.write(0x508, 3));
        assert!(watchdog.write(0x000, 1));
        assert!(watchdog.write(0x504, 31));
        assert_eq!(watchdog.read(0x504), Some(15));
        tick(&mut watchdog, true);
        assert_eq!(watchdog.remaining, 16);
        tick(&mut watchdog, false);
        assert_eq!(watchdog.remaining, 15);
        assert!(watchdog.write(0x600, 0x6e52_4635));
        assert!(watchdog.write(0x608, 0x6e52_4635));
        assert!(watchdog.write(0x604, 0));
        assert_eq!(watchdog.read(0x404), Some(2));
        assert_eq!(watchdog.remaining, 15);
        assert!(watchdog.write(0x604, 0x6e52_4635));
        assert_eq!(watchdog.read(0x404), Some(3));
        assert_eq!(watchdog.remaining, 16);

        assert!(watchdog.write(0x304, 1));
        for _ in 0..16 {
            tick(&mut watchdog, false);
        }
        assert!(watchdog.pending());
        assert!(!watchdog.reset_requested);
        assert!(watchdog.write(0x100, 0));
        assert!(watchdog.write(0x600, 0x6e52_4635));
        assert!(watchdog.write(0x604, 0x6e52_4635));
        tick(&mut watchdog, true);
        assert!(!watchdog.reset_requested);
        tick(&mut watchdog, true);
        assert!(watchdog.reset_requested);
    }
}
