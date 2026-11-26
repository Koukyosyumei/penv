#[cfg(test)]
use crate::machine::VIRTUAL_STEPS_PER_SECOND;

use crate::bus::CORTEX_M_STEP_HZ;

pub const NMI: u8 = 2;
pub const PENDSV: u8 = 14;
pub const SYSTICK: u8 = 15;
const CSR: u32 = 0xe000_e010;
const RELOAD: u32 = 0xe000_e014;
const CURRENT: u32 = 0xe000_e018;
const CALIBRATION: u32 = 0xe000_e01c;
const RENODE_DEFAULT_FREQUENCY: u32 = 50 * 0x80_0000;
pub const ICSR: u32 = 0xe000_ed04;

pub struct SysTick {
    control: u32,
    reload: u32,
    current: u32,
    count_flag: bool,
    pending: bool,
    pendsv_pending: bool,
    nmi_pending: bool,
    frequency: Option<u32>,
    steps_per_second: u64,
    phase: u64,
    clock_gated: bool,
    current_initialized: bool,
    counting: bool,
    zero_state_reload: bool,
    noisy: bool,
}

impl Default for SysTick {
    fn default() -> Self {
        Self {
            control: 0,
            reload: 0,
            current: 0xff_ffff,
            count_flag: false,
            pending: false,
            pendsv_pending: false,
            nmi_pending: false,
            frequency: None,
            steps_per_second: CORTEX_M_STEP_HZ,
            phase: 0,
            clock_gated: false,
            current_initialized: false,
            counting: false,
            zero_state_reload: false,
            noisy: false,
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
            steps_per_second: self.steps_per_second,
            clock_gated: self.clock_gated,
            zero_state_reload: self.zero_state_reload,
            current: if self.zero_state_reload { 0 } else { 0xff_ffff },
            noisy: self.noisy,
            ..Self::default()
        };
    }

    pub fn set_frequency(&mut self, frequency: u32) {
        self.frequency = Some(frequency);
        self.phase = 0;
    }

    pub fn set_step_rate(&mut self, steps_per_second: u32) {
        self.steps_per_second = u64::from(steps_per_second);
    }

    pub fn set_zero_state_reload(&mut self) {
        self.zero_state_reload = true;
        self.current = 0;
    }

    pub fn set_noisy(&mut self) {
        self.noisy = true;
    }

    pub fn set_clocked(&mut self, clocked: bool) {
        self.clock_gated = !clocked;
    }

    pub fn tick(&mut self) {
        if self.clock_gated || self.control & 1 == 0 || (!self.counting && !self.zero_state_reload)
        {
            return;
        }
        let cycles = if let Some(frequency) = self.frequency {
            self.phase += u64::from(frequency);
            let cycles = self.phase / self.steps_per_second;
            self.phase %= self.steps_per_second;
            cycles as u32
        } else {
            1
        };
        if self.zero_state_reload {
            self.tick_zero_state_reload(cycles);
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
        if self.reload == 0 {
            self.current = 0;
            self.counting = false;
            self.phase = 0;
        } else {
            self.current = self.reload - ((cycles - self.current) % self.reload);
        }
    }

    fn tick_zero_state_reload(&mut self, cycles: u32) {
        let to_zero = if self.current == 0 {
            self.reload + 1
        } else {
            self.current
        };
        if cycles < to_zero {
            if cycles != 0 {
                self.current = to_zero - cycles;
            }
            return;
        }
        if self.current != 0 || self.reload != 0 {
            self.count_flag = true;
            self.pending |= self.control & 2 != 0;
        }
        self.current =
            (self.reload + 1 - (cycles - to_zero) % (self.reload + 1)) % (self.reload + 1);
    }

    pub fn pending_exceptions(&self) -> u64 {
        (u64::from(self.nmi_pending) << NMI)
            | (u64::from(self.pending) << SYSTICK)
            | (u64::from(self.pendsv_pending) << PENDSV)
    }

    pub fn pending(&self, exception: u8) -> bool {
        match exception {
            NMI => self.nmi_pending,
            PENDSV => self.pendsv_pending,
            SYSTICK => self.pending,
            _ => false,
        }
    }

    pub fn acknowledge(&mut self, exception: u8) {
        match exception {
            NMI => self.nmi_pending = false,
            PENDSV => self.pendsv_pending = false,
            SYSTICK => self.pending = false,
            _ => unreachable!(),
        }
    }

    pub fn read(&mut self, address: u32) -> Option<u32> {
        Some(match address {
            CSR => 4 | self.control | (u32::from(std::mem::take(&mut self.count_flag)) << 16),
            RELOAD => self.reload,
            CURRENT if self.zero_state_reload && !self.current_initialized => 0xff_ffff,
            CURRENT => self.current,
            CALIBRATION => {
                let frequency = self.frequency.unwrap_or(RENODE_DEFAULT_FREQUENCY);
                ((frequency / 100) & 0xff_ffff)
                    | (u32::from(frequency % 100 != 0) << 30)
                    | (1 << 31)
            }
            ICSR => {
                (u32::from(self.pending) << 26)
                    | (u32::from(self.pendsv_pending) << 28)
                    | (u32::from(self.nmi_pending) << 31)
            }
            _ => return None,
        })
    }

    pub fn enabling_with_zero_reload(&self, address: u32, value: u32) -> bool {
        address == CSR && value & 1 != 0 && self.control & 1 == 0 && self.reload == 0
    }

    pub fn write(&mut self, address: u32, value: u32) -> bool {
        match address {
            CSR if self.zero_state_reload => self.control = value & 3,
            RELOAD if self.zero_state_reload => self.reload = value & 0xff_ffff,
            CURRENT if self.zero_state_reload => {
                self.current = 0;
                self.current_initialized = true;
                self.count_flag = false;
            }
            CSR => {
                // Renode starts a zero-valued counter at RELOAD when enabled.
                let enabled = value & 1 != 0;
                if (self.control & 1 != 0) != enabled {
                    self.counting = enabled && self.reload != 0;
                    if self.noisy && enabled && !self.counting {
                        eprintln!(
                            "Systick_NS enabled but it won't be started as long as the reload value is zero"
                        );
                    }
                    if self.counting && self.current == 0 {
                        self.current = self.reload;
                    }
                }
                self.control = value & 3; // Processor clock is the only source.
            }
            RELOAD => {
                let reload = value & 0xff_ffff;
                if self.control & 1 != 0 && self.reload == 0 && reload != 0 && !self.counting {
                    self.current = reload;
                    self.counting = true;
                }
                self.reload = reload;
            }
            CURRENT => {
                self.current = self.reload;
                self.count_flag = false;
                if self.reload == 0 {
                    self.counting = false;
                }
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
                self.nmi_pending |= value & (1 << 31) != 0;
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
    fn calibration_reflects_the_configured_frequency() {
        let mut timer = SysTick::default();
        assert_eq!(timer.read(CALIBRATION), Some(0x8040_0000));
        timer.set_frequency(72_000_000);
        assert_eq!(timer.read(CALIBRATION), Some(0x800a_fc80));
        timer.set_frequency(72_000_001);
        assert_eq!(timer.read(CALIBRATION), Some(0xc00a_fc80));
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
    fn renode_systick_external_clock_gate_survives_reset() {
        // arm-cortex-m-signals.robot: SysTick portion of both Clocked cases.
        let mut timer = SysTick::default();
        timer.set_frequency((CORTEX_M_STEP_HZ / 2) as u32);
        assert!(timer.write(RELOAD, 10));
        assert!(timer.write(CURRENT, 0));
        assert!(timer.write(CSR, 5));
        timer.tick();
        timer.tick();
        assert_eq!(timer.read(CURRENT), Some(9));
        timer.set_clocked(false);
        for _ in 0..8 {
            timer.tick();
        }
        assert_eq!(timer.read(CURRENT), Some(9));
        assert_eq!(timer.read(CSR), Some(5));

        timer.reset();
        assert_eq!(timer.read(CURRENT), Some(0x00ff_ffff));
        assert_eq!(timer.read(CSR), Some(4));
        assert!(timer.write(RELOAD, 10));
        assert!(timer.write(CURRENT, 0));
        assert!(timer.write(CSR, 5));
        for _ in 0..8 {
            timer.tick();
        }
        assert_eq!(timer.read(CURRENT), Some(10));
        timer.set_clocked(true);
        timer.tick();
        assert_eq!(timer.read(CURRENT), Some(10));
        timer.tick();
        assert_eq!(timer.read(CURRENT), Some(9));
    }

    #[test]
    fn renode_systick_counter_stops_and_resumes_via_enable() {
        // arm-cortex-m-signals.robot: adapt the stop/resume observation to CSR.ENABLE.
        let mut timer = SysTick::default();
        timer.set_frequency((CORTEX_M_STEP_HZ / 2) as u32);
        assert!(timer.write(RELOAD, 10));
        assert!(timer.write(CURRENT, 0));
        assert!(timer.write(CSR, 5));
        for _ in 0..3 {
            timer.tick();
        }
        assert_eq!(timer.read(CURRENT), Some(9));
        assert!(timer.write(CSR, 4));
        for _ in 0..8 {
            timer.tick();
        }
        assert_eq!(timer.read(CURRENT), Some(9));
        assert!(timer.write(CSR, 5));
        timer.tick();
        assert_eq!(timer.read(CURRENT), Some(8));
    }

    // arm-cortex-m-interrupts.robot, the two SysTick reload/CVR write orders.
    #[test]
    fn renode_systick_counts_and_reloads_for_both_cvr_write_orders() {
        for writes in [[CURRENT, RELOAD], [RELOAD, CURRENT]] {
            let mut timer = SysTick::default();
            const FREQUENCY: u32 = 100;
            const STEPS_PER_TICK: u64 = CORTEX_M_STEP_HZ / FREQUENCY as u64;
            timer.set_frequency(FREQUENCY);
            assert!(timer.write(CSR, 0));
            for address in writes {
                assert!(timer.write(address, if address == RELOAD { 10 } else { 0 }));
            }
            assert!(timer.write(CSR, 5));
            assert_eq!(timer.read(CURRENT), Some(10));

            for _ in 0..9 * STEPS_PER_TICK {
                timer.tick();
            }
            assert_eq!(timer.read(CURRENT), Some(1));
            assert_eq!(timer.read(CSR).unwrap() & (1 << 16), 0);

            for _ in 0..STEPS_PER_TICK {
                timer.tick();
            }
            assert_eq!(timer.read(CURRENT), Some(10));
            assert_ne!(timer.read(CSR).unwrap() & (1 << 16), 0);
            assert_eq!(timer.read(CSR).unwrap() & (1 << 16), 0);
            assert!(!timer.pending(SYSTICK));

            for _ in 0..STEPS_PER_TICK {
                timer.tick();
            }
            assert_eq!(timer.read(CURRENT), Some(9));
        }
    }

    #[test]
    fn renode_systick_current_write_reloads_without_pend_and_zero_reload_stops() {
        let mut timer = SysTick::default();
        assert!(timer.write(CURRENT, 0));
        assert!(timer.write(CSR, 3));
        timer.tick();
        assert_eq!(timer.read(CURRENT), Some(0));
        assert!(!timer.pending(SYSTICK));

        assert!(timer.write(RELOAD, 2));
        timer.tick();
        assert_eq!(timer.read(CURRENT), Some(1));
        assert!(timer.write(CURRENT, 0xffff_ffff));
        assert_eq!(timer.read(CURRENT), Some(2));
        assert!(!timer.pending(SYSTICK));
        assert_eq!(timer.read(CSR).unwrap() & (1 << 16), 0);

        assert!(timer.write(RELOAD, 0));
        timer.tick();
        timer.tick();
        assert!(timer.pending(SYSTICK));
        timer.acknowledge(SYSTICK);
        timer.tick();
        assert_eq!(timer.read(CURRENT), Some(0));
        assert!(!timer.pending(SYSTICK));
    }
}
