use crate::machine::VIRTUAL_STEPS_PER_SECOND;

pub const PENDSV: u8 = 14;
pub const SYSTICK: u8 = 15;
const CSR: u32 = 0xe000_e010;
const RELOAD: u32 = 0xe000_e014;
const CURRENT: u32 = 0xe000_e018;
const CALIBRATION: u32 = 0xe000_e01c;
const ICSR: u32 = 0xe000_ed04;

pub struct SysTick {
    control: u32,
    reload: u32,
    current: u32,
    count_flag: bool,
    pending: bool,
    pendsv_pending: bool,
    frequency: u32,
    phase: u64,
}

impl Default for SysTick {
    fn default() -> Self {
        Self {
            control: 0,
            reload: 0,
            current: 0,
            count_flag: false,
            pending: false,
            pendsv_pending: false,
            frequency: VIRTUAL_STEPS_PER_SECOND,
            phase: 0,
        }
    }
}

impl SysTick {
    pub const RANGES: [(u32, u32); 2] = [(CSR, CALIBRATION + 4), (ICSR, ICSR + 4)];

    pub fn overlaps(start: u64, end: u64) -> bool {
        Self::RANGES
            .into_iter()
            .any(|(low, high)| start < u64::from(high) && end > u64::from(low))
    }

    pub fn reset(&mut self) {
        *self = Self {
            frequency: self.frequency,
            ..Self::default()
        };
    }

    pub fn set_frequency(&mut self, frequency: u32) {
        self.frequency = frequency;
    }

    pub fn tick(&mut self) {
        if self.control & 1 == 0 {
            return;
        }
        let steps_per_second = u64::from(VIRTUAL_STEPS_PER_SECOND);
        self.phase += u64::from(self.frequency);
        let cycles = (self.phase / steps_per_second) as u32;
        self.phase %= steps_per_second;
        if cycles == 0 {
            return;
        }
        if self.current == 0 {
            return;
        }
        if cycles < self.current {
            self.current -= cycles;
            return;
        }
        self.count_flag = true;
        self.pending |= self.control & 2 != 0;
        self.current = if self.reload == 0 {
            0
        } else {
            self.reload - (cycles - self.current) % self.reload
        };
    }

    pub fn pending(&self, exception: u8) -> bool {
        match exception {
            PENDSV => self.pendsv_pending,
            SYSTICK => self.pending,
            _ => false,
        }
    }

    pub fn acknowledge(&mut self, exception: u8) {
        match exception {
            PENDSV => self.pendsv_pending = false,
            SYSTICK => self.pending = false,
            _ => unreachable!(),
        }
    }

    pub fn read(&mut self, address: u32) -> Option<u32> {
        Some(match address {
            CSR => 4 | self.control | (u32::from(std::mem::take(&mut self.count_flag)) << 16),
            RELOAD => self.reload,
            CURRENT => self.current,
            CALIBRATION => 1 << 31, // No separate reference clock.
            ICSR => (u32::from(self.pending) << 26) | (u32::from(self.pendsv_pending) << 28),
            _ => return None,
        })
    }

    pub fn write(&mut self, address: u32, value: u32) -> bool {
        match address {
            CSR => {
                if self.control & 1 == 0 && value & 1 != 0 && self.current == 0 {
                    self.current = self.reload;
                }
                self.control = value & 3; // Processor clock is the only source.
            }
            RELOAD => {
                self.reload = value & 0xff_ffff;
                if self.control & 1 != 0 && self.current == 0 {
                    self.current = self.reload;
                }
            }
            CURRENT => {
                self.current = self.reload;
                self.phase = 0;
                self.count_flag = false;
            }
            CALIBRATION => {}
            ICSR => {
                if value & (1 << 25) != 0 {
                    self.pending = false;
                }
                if value & (1 << 26) != 0 {
                    self.pending = true;
                }
                if value & (1 << 27) != 0 {
                    self.pendsv_pending = false;
                }
                if value & (1 << 28) != 0 {
                    self.pendsv_pending = true;
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

    fn check_upstream_reload_order(reload_before_clear: bool) {
        let mut timer = SysTick::default();
        timer.set_frequency(100);
        timer.write(CSR, 0);
        if reload_before_clear {
            timer.write(RELOAD, 10);
            timer.write(CURRENT, 0);
        } else {
            timer.write(CURRENT, 0);
            timer.write(RELOAD, 10);
        }
        timer.write(CSR, 5);
        assert_eq!(timer.read(CURRENT), Some(10));
        for (milliseconds, expected) in [(90, 1), (10, 10), (10, 9)] {
            for _ in 0..milliseconds * (VIRTUAL_STEPS_PER_SECOND / 1000) {
                timer.tick();
            }
            assert_eq!(timer.read(CURRENT), Some(expected));
        }
    }

    #[test]
    fn upstream_count_down_with_reload_set_before_reset() {
        check_upstream_reload_order(false);
    }

    #[test]
    fn upstream_count_down_with_reload_set_after_reset() {
        check_upstream_reload_order(true);
    }

    #[test]
    fn advances_fractional_clock_and_preserves_reload_phase_after_wrap() {
        let mut timer = SysTick::default();
        timer.set_frequency(2_500_000);
        timer.write(RELOAD, 3);
        timer.write(CSR, 3);
        timer.tick(); // 2 cycles, down to 1.
        assert_eq!(timer.read(CURRENT), Some(1));
        assert!(!timer.pending(SYSTICK));
        timer.tick(); // 3 cycles, one to reach 0 and two after the reload.
        assert_eq!(timer.read(CURRENT), Some(1));
        assert!(timer.pending(SYSTICK));
        assert_eq!(timer.read(CSR), Some(0x10007));
        assert_eq!(timer.read(CSR), Some(7));
    }
}
