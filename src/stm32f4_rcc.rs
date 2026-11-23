const HSI: u32 = 16_000_000;
const HSE: u32 = 8_000_000;
const HSION: u32 = 1;
const HSEON: u32 = 1 << 16;
const PLLON: u32 = 1 << 24;
const PLLI2SON: u32 = 1 << 26;
const PLLSRC: u32 = 1 << 22;
const PLL_FIELDS: u32 = 0x0f43_7fff;
const BD_RST: u32 = 1 << 16;
const RTC_EN: u32 = 1 << 15;
const RTC_SEL: u32 = 3 << 8;
const IWDG_RST: u32 = 1 << 29;
const REMOVE_RESET_FLAGS: u32 = 1 << 24;

#[derive(Clone, Copy)]
enum BusClock {
    Ahb1 = 0,
    Apb1 = 4,
    Apb2 = 5,
}

pub struct F4Rcc {
    pub base: u32,
    pub rtc: Option<u32>,
    pub timer9: Option<u32>,
    cr: u32,
    pll: u32,
    plli2s: u32,
    cfgr: u32,
    source: u32,
    gates: [u32; 6],
    resets: [u32; 6],
    bdcr: u32,
    lsi: bool,
    reset_flags: u32,
}

impl F4Rcc {
    pub const MODEL: &str = "Miscellaneous.STM32F4_RCC";
    pub const SIZE: u32 = 0x400;

    pub fn new(base: u32, rtc: Option<u32>, timer9: Option<u32>) -> Self {
        Self {
            base,
            rtc,
            timer9,
            cr: 0x481,
            pll: 0x2400_3010,
            plli2s: 0x2000_3000,
            cfgr: 0,
            source: 0,
            gates: [1 << 20, 0, 0, 0, 0, 0],
            resets: [0; 6],
            bdcr: 0,
            lsi: false,
            reset_flags: 0x0e00_0000,
        }
    }

    pub fn watchdog_reset(&mut self) {
        let (bdcr, reset_flags) = (self.bdcr, self.reset_flags | IWDG_RST);
        *self = Self::new(self.base, self.rtc, self.timer9);
        self.bdcr = bdcr;
        self.reset_flags = reset_flags;
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    pub fn read(&self, address: u32, width: usize) -> u32 {
        let offset = address - self.base;
        let value = match offset & !3 {
            0 => {
                self.cr
                    | ((self.cr & (HSION | HSEON)) << 1)
                    | (u32::from(self.pll_ready()) << 25)
                    | (u32::from(self.cr & PLLI2SON != 0 && self.cr & self.pll_input().0 != 0)
                        << 27)
            }
            4 => self.pll,
            8 => self.cfgr | (self.source << 2),
            0x10..=0x24 => self.resets[((offset - 0x10) / 4) as usize],
            0x30..=0x44 => self.gates[((offset - 0x30) / 4) as usize],
            0x70 => self.bdcr | ((self.bdcr & 1) << 1),
            0x84 => self.plli2s,
            0x74 => self.reset_flags | (u32::from(self.lsi) * 3),
            _ => 0,
        };
        (value >> ((offset & 3) * 8)) & (u32::MAX >> (32 - width * 8))
    }

    pub fn write(&mut self, address: u32, width: usize, value: u32, backup_unlocked: bool) {
        let offset = address - self.base;
        let shift = (offset & 3) * 8;
        let lanes = (u32::MAX >> (32 - width * 8)) << shift;
        let bits = (value << shift) & lanes;
        let merge =
            |previous: u32, writable: u32| (previous & !(lanes & writable)) | (bits & writable);
        match offset & !3 {
            0 => {
                let source = if self.source == 2 {
                    PLLON
                } else {
                    1 << (16 * self.source)
                };
                let pll_input = if self.cr & (PLLON | PLLI2SON) == 0 {
                    0
                } else {
                    self.pll_input().0
                };
                self.cr = merge(self.cr, 0x050f_00f9) | (self.cr & (source | pll_input));
            }
            0x84 if self.cr & PLLI2SON == 0 => self.plli2s = merge(self.plli2s, 0x7000_7fc0),
            4 if self.cr & PLLON == 0 => self.pll = merge(self.pll, PLL_FIELDS),
            8 => self.cfgr = merge(self.cfgr, 0xffff_fcf3),
            0x10..=0x24 if offset & !3 != 0x1c => {
                let reset = &mut self.resets[((offset - 0x10) / 4) as usize];
                *reset = merge(*reset, u32::MAX);
            }
            0x30..=0x44 if offset & !3 != 0x3c => {
                let gate = &mut self.gates[((offset - 0x30) / 4) as usize];
                *gate = merge(*gate, u32::MAX);
            }
            0x70 if backup_unlocked => {
                let selection = if self.bdcr & RTC_SEL == 0 { RTC_SEL } else { 0 };
                self.bdcr = merge(self.bdcr, BD_RST | RTC_EN | selection | 5);
                if self.bdcr & BD_RST != 0 {
                    self.bdcr = BD_RST;
                }
            }
            0x74 => {
                if lanes & 1 != 0 {
                    self.lsi = bits & 1 != 0;
                }
                if bits & REMOVE_RESET_FLAGS != 0 {
                    self.reset_flags = 0;
                }
            }
            _ => {}
        }
        let requested = self.cfgr & 3;
        let ready = match requested {
            0 => self.cr & HSION != 0,
            1 => self.cr & HSEON != 0,
            2 => self.pll_ready(),
            _ => false,
        };
        if ready {
            self.source = requested;
        }
    }

    fn pll_input(&self) -> (u32, u32) {
        if self.pll & PLLSRC == 0 {
            (HSION, HSI)
        } else {
            (HSEON, HSE)
        }
    }

    fn pll_ready(&self) -> bool {
        self.cr & PLLON != 0
            && self.cr & self.pll_input().0 != 0
            && self.pll & 0x3f >= 2
            && (self.pll >> 6) & 0x1ff != 0
    }

    pub fn hclk(&self) -> u32 {
        let system = match self.source {
            1 => u64::from(HSE),
            2 => {
                let m = self.pll & 0x3f;
                let n = (self.pll >> 6) & 0x1ff;
                let p = 2 * (((self.pll >> 16) & 3) + 1);
                u64::from(self.pll_input().1) * u64::from(n) / u64::from(m * p)
            }
            _ => u64::from(HSI),
        };
        let hpre = ((self.cfgr >> 4) & 15) as usize;
        let shift = [0, 0, 0, 0, 0, 0, 0, 0, 1, 2, 3, 4, 6, 7, 8, 9][hpre];
        (system >> shift).clamp(1, u64::from(u32::MAX)) as u32
    }

    pub fn timer_frequency(&self, base: u32) -> Option<u32> {
        let (bus, _) = timer_clock(base)?;
        let offset = if matches!(bus, BusClock::Apb1) {
            10
        } else {
            13
        };
        let prescaler = (self.cfgr >> offset) & 7;
        Some(self.hclk() >> prescaler.saturating_sub(4))
    }

    pub fn rtc_frequency(&self) -> Option<u32> {
        if self.bdcr & RTC_EN == 0 {
            return None;
        }
        let hse_divider = (self.cfgr >> 16) & 31;
        match (self.bdcr >> 8) & 3 {
            1 if self.bdcr & 1 != 0 => Some(32_768),
            2 if self.lsi => Some(32_000),
            3 if hse_divider >= 2 && self.cr & HSEON != 0 => Some(HSE / hse_divider),
            _ => None,
        }
    }

    pub fn rtc_in_reset(&self) -> bool {
        self.bdcr & BD_RST != 0
    }

    pub fn adc_in_reset(&self) -> bool {
        self.resets[BusClock::Apb2 as usize] & (1 << 8) != 0
    }

    pub fn adc_common_enabled(&self) -> bool {
        self.gates[BusClock::Apb2 as usize] & (7 << 8) != 0 && !self.adc_in_reset()
    }

    pub fn timer9_in_reset(&self) -> bool {
        self.resets[BusClock::Apb2 as usize] & (1 << 16) != 0
    }

    pub fn enabled(&self, base: u32) -> bool {
        use BusClock::*;
        if self.rtc == Some(base) {
            return self.rtc_frequency().is_some();
        }
        let route = match base {
            0x4002_0000..=0x4002_2800 => Some((Ahb1, (base - 0x4002_0000) / 0x400)),
            0x4002_6000 => Some((Ahb1, 21)),
            0x4002_6400 => Some((Ahb1, 22)),
            0x4000_3800 => Some((Apb1, 14)),
            0x4000_3c00 => Some((Apb1, 15)),
            0x4000_4400..=0x4000_5000 => Some((Apb1, 17 + (base - 0x4000_4400) / 0x400)),
            0x4000_5400..=0x4000_5c00 => Some((Apb1, 21 + (base - 0x4000_5400) / 0x400)),
            0x4000_6400 => Some((Apb1, 25)),
            0x4000_6800 => Some((Apb1, 26)),
            0x4000_7000 => Some((Apb1, 28)),
            0x4001_1000 => Some((Apb2, 4)),
            0x4001_3000 => Some((Apb2, 12)),
            _ => timer_clock(base),
        };
        route.is_none_or(|(bus, bit)| {
            let index = bus as usize;
            self.gates[index] & !self.resets[index] & (1 << bit) != 0
        })
    }
}

fn timer_clock(base: u32) -> Option<(BusClock, u32)> {
    use BusClock::*;
    match base {
        0x4000_0000..=0x4000_2000 => Some((Apb1, (base - 0x4000_0000) / 0x400)),
        0x4001_0000 => Some((Apb2, 0)),
        0x4001_0400 => Some((Apb2, 1)),
        0x4001_4000..=0x4001_4800 => Some((Apb2, 16 + (base - 0x4001_4000) / 0x400)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_switch_waits_for_source_and_apb_prescalers_feed_timers() {
        let mut rcc = F4Rcc::new(0, None, None);
        rcc.write(4, 4, 8 | (192 << 6) | (2 << 16) | PLLSRC | (4 << 24), false);
        rcc.write(8, 4, 2 | (5 << 10) | (4 << 13), false);
        assert_eq!(rcc.hclk(), 16_000_000);
        assert_eq!(rcc.read(8, 4) & 0xc, 0);
        rcc.write(0, 4, HSION | PLLON, false);
        assert_eq!(rcc.read(0, 4) & (1 << 25), 0);
        rcc.write(0, 4, HSION | PLLON | HSEON, false);
        assert_eq!(rcc.read(8, 4) & 0xc, 8);
        assert_eq!(rcc.hclk(), 32_000_000);
        assert_eq!(rcc.timer_frequency(0x4000_0000), Some(16_000_000));
        assert_eq!(rcc.timer_frequency(0x4001_4000), Some(32_000_000));
        rcc.write(8, 4, 2 | (8 << 4), false);
        assert_eq!(rcc.hclk(), 16_000_000);
    }

    #[test]
    fn watchdog_reset_restores_clocks_but_preserves_backup_domain_and_latches_cause() {
        let mut rcc = F4Rcc::new(0, Some(0x4000_2800), None);
        rcc.write(0x70, 4, RTC_EN | (1 << 8) | 1, true);
        rcc.write(0x70, 4, RTC_EN | (2 << 8) | 1, true);
        rcc.write(0x40, 4, 1 << 19, false);
        assert!(rcc.enabled(0x4000_4c00));
        rcc.watchdog_reset();
        assert!(!rcc.enabled(0x4000_4c00));
        assert_eq!(rcc.rtc_frequency(), Some(32_768));
        assert_eq!(rcc.read(0x74, 4) & IWDG_RST, IWDG_RST);
        rcc.write(0x74, 4, REMOVE_RESET_FLAGS, false);
        assert_eq!(rcc.read(0x74, 4), 0);
    }
}
