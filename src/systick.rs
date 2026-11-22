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
}

impl SysTick {
    pub const RANGES: [(u32, u32); 2] = [(CSR, CALIBRATION + 4), (ICSR, ICSR + 4)];

    pub fn overlaps(start: u64, end: u64) -> bool {
        Self::RANGES
            .into_iter()
            .any(|(low, high)| start < u64::from(high) && end > u64::from(low))
    }

    pub fn tick(&mut self) {
        if self.control & 1 == 0 {
            return;
        }
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
