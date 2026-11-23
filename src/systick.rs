use crate::bus::CORTEX_M_STEP_HZ;

pub const PENDSV: u8 = 14;
pub const SYSTICK: u8 = 15;
const CSR: u32 = 0xe000_e010;
const RELOAD: u32 = 0xe000_e014;
const CURRENT: u32 = 0xe000_e018;
const CALIBRATION: u32 = 0xe000_e01c;
const ICSR: u32 = 0xe000_ed04;

#[derive(Default)]
pub struct SysTick {
    control: u32,
    reload: u32,
    current: u32,
    count_flag: bool,
    pending: bool,
    pendsv_pending: bool,
    frequency: Option<u32>,
    phase: u64,
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
        self.frequency = Some(frequency);
        self.phase = 0;
    }

    pub fn tick(&mut self) {
        if self.control & 1 == 0 {
            return;
        }
        if let Some(frequency) = self.frequency {
            self.phase += u64::from(frequency);
            while self.phase >= CORTEX_M_STEP_HZ {
                self.phase -= CORTEX_M_STEP_HZ;
                self.cycle();
            }
        } else {
            self.cycle();
        }
    }

    fn cycle(&mut self) {
        if self.current == 0 {
            self.current = self.reload;
        } else {
            self.current -= 1;
            if self.current == 0 {
                self.count_flag = true;
                self.pending |= self.control & 2 != 0;
            }
        }
    }

    pub fn pending_exceptions(&self) -> u64 {
        (u64::from(self.pending) << SYSTICK) | (u64::from(self.pendsv_pending) << PENDSV)
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
            CSR => self.control = value & 3, // Processor clock is the only source.
            RELOAD => self.reload = value & 0xff_ffff,
            CURRENT => {
                self.current = 0;
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

    #[test]
    fn renode_systick_counter_stops_and_resumes_via_enable() {
        // arm-cortex-m-signals.robot: adapt the stop/resume observation to CSR.ENABLE.
        let mut timer = SysTick::default();
        timer.set_frequency((CORTEX_M_STEP_HZ / 2) as u32);
        assert!(timer.write(RELOAD, 10));
        assert!(timer.write(CSR, 5));
        for _ in 0..3 {
            timer.tick();
        }
        assert_eq!(timer.read(CURRENT), Some(10));
        assert!(timer.write(CSR, 4));
        for _ in 0..8 {
            timer.tick();
        }
        assert_eq!(timer.read(CURRENT), Some(10));
        assert!(timer.write(CSR, 5));
        timer.tick();
        assert_eq!(timer.read(CURRENT), Some(9));
    }

    // arm-cortex-m-interrupts.robot, the two SysTick reload/CVR write orders.
    #[test]
    fn renode_systick_counts_and_reloads_for_both_cvr_write_orders() {
        for writes in [[CURRENT, RELOAD], [RELOAD, CURRENT]] {
            let mut timer = SysTick::default();
            // Scale the upstream 100 Hz setup to four execution steps per tick.
            timer.set_frequency((CORTEX_M_STEP_HZ / 4) as u32);
            assert!(timer.write(CSR, 0));
            for address in writes {
                assert!(timer.write(address, if address == RELOAD { 10 } else { 0 }));
            }
            assert_eq!(timer.read(CURRENT), Some(0));
            assert!(timer.write(CSR, 5));
            for _ in 0..3 {
                timer.tick();
            }
            assert_eq!(timer.read(CURRENT), Some(0));
            timer.tick();
            assert_eq!(timer.read(CURRENT), Some(10));

            for _ in 0..36 {
                timer.tick();
            }
            assert_eq!(timer.read(CURRENT), Some(1));
            assert_eq!(timer.read(CSR).unwrap() & (1 << 16), 0);

            for _ in 0..4 {
                timer.tick();
            }
            assert_eq!(timer.read(CURRENT), Some(0));
            assert_ne!(timer.read(CSR).unwrap() & (1 << 16), 0);
            assert_eq!(timer.read(CSR).unwrap() & (1 << 16), 0);
            assert!(!timer.pending(SYSTICK));

            for (steps, expected) in [(4, 10), (4, 9)] {
                for _ in 0..steps {
                    timer.tick();
                }
                assert_eq!(timer.read(CURRENT), Some(expected));
            }
        }
    }
}
