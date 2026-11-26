use crate::bus::CORTEX_M_STEP_HZ;

pub(super) const VALRDY: u32 = 0x100;
const STEPS_PER_BYTE: u64 = CORTEX_M_STEP_HZ / 1_000_000 * 64;

pub(super) struct Rng {
    steps_left: Option<u64>,
    ready: bool,
    interrupt: bool,
    stop_on_ready: bool,
    correction: bool,
    pub(super) raised: u32,
    value: u8,
    state: u32,
}

impl Default for Rng {
    fn default() -> Self {
        Self {
            steps_left: None,
            ready: false,
            interrupt: false,
            stop_on_ready: false,
            correction: false,
            raised: 0,
            value: 0,
            state: 0x6d2b_79f5,
        }
    }
}

impl Rng {
    pub(super) fn pending(&self) -> bool {
        self.ready && self.interrupt
    }

    #[cfg(test)]
    fn tick(&mut self) {
        self.advance(1);
    }

    pub(super) fn advance(&mut self, mut ticks: u64) {
        while let Some(remaining) = self.steps_left {
            if ticks < remaining {
                self.steps_left = Some(remaining - ticks);
                break;
            }
            ticks -= remaining;
            self.generate();
        }
    }

    fn generate(&mut self) {
        // A fixed seed gives deterministic simulated input, including after system reset.
        self.state ^= self.state << 13;
        self.state ^= self.state >> 17;
        self.state ^= self.state << 5;
        self.value = (self.state >> 24) as u8;
        self.ready = true;
        self.raised |= 1;
        self.steps_left = if self.stop_on_ready {
            None
        } else {
            Some(self.period())
        };
    }

    fn period(&self) -> u64 {
        STEPS_PER_BYTE * if self.correction { 2 } else { 1 }
    }

    pub(super) fn read(&self, offset: u32) -> Option<u32> {
        Some(match offset {
            0x000 | 0x004 => 0,
            VALRDY => self.ready.into(),
            0x200 => self.stop_on_ready.into(),
            0x304 | 0x308 => self.interrupt.into(),
            0x504 => self.correction.into(),
            0x508 => self.value.into(),
            _ => return None,
        })
    }

    pub(super) fn write(&mut self, offset: u32, value: u32) -> bool {
        let bit = value & 1 != 0;
        match offset {
            0x000 => {
                if bit && self.steps_left.is_none() {
                    self.steps_left = Some(self.period());
                }
            }
            0x004 => {
                if bit {
                    self.steps_left = None;
                }
            }
            VALRDY => self.ready = bit,
            0x200 => self.stop_on_ready = bit,
            0x304 => self.interrupt |= bit,
            0x308 => self.interrupt &= !bit,
            0x504 => self.correction = bit,
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_is_latched_on_shortcut_stop_and_reset_repeats_stream() {
        let mut rng = Rng::default();
        assert!(rng.write(0x504, 1));
        assert!(rng.write(0x200, 1));
        assert!(rng.write(0x304, 1));
        assert!(rng.write(0, 1));
        for _ in 0..STEPS_PER_BYTE * 2 - 1 {
            rng.tick();
        }
        assert!(!rng.pending());
        rng.tick();
        assert!(rng.pending());
        assert_eq!(rng.steps_left, None);
        let value = rng.read(0x508);
        assert!(rng.write(0x308, 1));
        assert!(!rng.pending());
        assert_eq!(rng.read(VALRDY), Some(1));
        assert!(rng.write(VALRDY, 0));
        for _ in 0..STEPS_PER_BYTE * 2 {
            rng.tick();
        }
        assert_eq!(rng.read(VALRDY), Some(0));
        assert_eq!(rng.read(0x508), value);

        rng = Rng::default();
        assert!(rng.write(0, 1));
        for _ in 0..STEPS_PER_BYTE {
            rng.tick();
        }
        assert_eq!(rng.read(0x508), value);
    }
}
