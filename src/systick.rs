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
    nmi_pending: bool,
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
            nmi_pending: false,
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

    pub fn tick(&mut self, step_hz: u32) {
        if self.control & 1 == 0 {
            return;
        }
        let steps_per_second = u64::from(step_hz);
        self.phase += u64::from(self.frequency);
        let cycles = (self.phase / steps_per_second) as u32;
        self.phase %= steps_per_second;
        if cycles == 0 {
            return;
        }
        let until_zero = self.current;
        if until_zero == 0 {
            return;
        }
        if cycles < until_zero {
            self.current = until_zero - cycles;
        } else {
            self.count_flag = true;
            self.pending |= self.control & 2 != 0;
            self.current = if self.reload == 0 {
                0
            } else {
                self.reload - (cycles - until_zero) % self.reload
            };
        }
    }

    pub fn pending_mask(&self) -> u8 {
        u8::from(self.pending) | (u8::from(self.pendsv_pending) << 1)
    }

    pub fn pending(&self, exception: u8) -> bool {
        match exception {
            2 => self.nmi_pending,
            PENDSV => self.pendsv_pending,
            SYSTICK => self.pending,
            _ => false,
        }
    }

    pub fn acknowledge(&mut self, exception: u8) {
        match exception {
            2 => self.nmi_pending = false,
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
            ICSR => {
                (u32::from(self.pending) << 26)
                    | (u32::from(self.pendsv_pending) << 28)
                    | (u32::from(self.nmi_pending) << 31)
            }
            _ => return None,
        })
    }

    pub fn write(&mut self, address: u32, value: u32) -> bool {
        match address {
            CSR => {
                if value & 1 != 0 && self.control & 1 == 0 && self.current == 0 {
                    self.current = self.reload;
                }
                self.control = value & 3; // Processor clock is the only source.
            }
            RELOAD => {
                let old_reload = self.reload;
                self.reload = value & 0xff_ffff;
                if old_reload == 0 && self.current == 0 && self.control & 1 != 0 {
                    self.current = self.reload;
                }
            }
            CURRENT => {
                self.current = self.reload;
                self.count_flag = false;
            }
            CALIBRATION => {}
            ICSR => {
                self.nmi_pending |= value & (1 << 31) != 0;
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

    #[test]
    fn renode_systick_counts_down_for_either_reload_and_current_write_order() {
        for registers in [[CURRENT, RELOAD], [RELOAD, CURRENT]] {
            let mut timer = SysTick::default();
            timer.set_frequency(100);
            for address in registers {
                timer.write(address, if address == RELOAD { 10 } else { 0 });
            }
            timer.write(CSR, 5);
            assert_eq!(timer.read(CURRENT), Some(10));
            // Model each 10 ms interval in a single virtual update.
            for _ in 0..9 {
                timer.tick(100);
            }
            assert_eq!(timer.read(CURRENT), Some(1));
            timer.tick(100);
            assert_eq!(timer.read(CURRENT), Some(10));
            assert!(!timer.pending(SYSTICK));
            timer.tick(100);
            assert_eq!(timer.read(CURRENT), Some(9));
        }
    }

    #[test]
    fn advances_fractional_clock_and_preserves_reload_phase_after_wrap() {
        let mut timer = SysTick::default();
        timer.set_frequency(2_500_000);
        timer.write(RELOAD, 3);
        timer.write(CSR, 3);
        timer.tick(crate::machine::VIRTUAL_STEPS_PER_SECOND); // Two cycles, down to one.
        assert_eq!(timer.read(CURRENT), Some(1));
        assert!(!timer.pending(SYSTICK));
        timer.tick(crate::machine::VIRTUAL_STEPS_PER_SECOND); // Three cycles, one to wrap and two to count down.
        assert_eq!(timer.read(CURRENT), Some(1));
        assert!(timer.pending(SYSTICK));
        assert_eq!(timer.read(CSR), Some(0x10007));
        assert_eq!(timer.read(CSR), Some(7));
    }
}
