use crate::stm32h7_adc::AdcClock;

const SIZE: u64 = 0x400;
const ETH_CLOCKS: u32 = 7 << 15;
const ETH_MAC: u32 = 1 << 15;
const ADC12: u32 = 1 << 5;
const ADC3: u32 = 1 << 24;
const BDMA: u32 = 1 << 21;
const SYSCFG_CLOCK: u32 = 1 << 1;
const INTERFACE_MASK: u32 = 7 << 21;

pub struct NetworkControl {
    pub rcc_base: u32,
    pub syscfg_base: Option<u32>,
    pub nic: Option<usize>,
    ahb1_reset: u32,
    ahb1_clock: u32,
    ahb1_sleep_clock: u32,
    ahb4_reset: u32,
    ahb4_clock: u32,
    apb4_clock: u32,
    syscfg_mode: u32,
}

pub enum Change {
    None,
    Clock(bool),
    Reset,
    Interface(u32),
}

impl NetworkControl {
    pub fn new(rcc_base: u32, syscfg_base: Option<u32>, nic: Option<usize>) -> Self {
        Self {
            rcc_base,
            syscfg_base,
            nic,
            ahb1_reset: 0,
            ahb1_clock: 0,
            ahb1_sleep_clock: ETH_CLOCKS,
            ahb4_reset: 0,
            ahb4_clock: 0,
            apb4_clock: 0,
            syscfg_mode: 0,
        }
    }

    pub fn ranges(&self) -> impl Iterator<Item = (u64, u64)> + '_ {
        [Some(self.rcc_base), self.syscfg_base]
            .into_iter()
            .flatten()
            .map(|base| {
                let start = u64::from(base);
                (start, start + SIZE)
            })
    }

    pub fn adc_enabled(&self, clock: AdcClock) -> bool {
        match clock {
            AdcClock::Adc12 => (self.ahb1_clock & !self.ahb1_reset) & ADC12 != 0,
            AdcClock::Adc3 => (self.ahb4_clock & !self.ahb4_reset) & ADC3 != 0,
        }
    }

    pub fn adc_in_reset(&self, clock: AdcClock) -> bool {
        match clock {
            AdcClock::Adc12 => self.ahb1_reset & ADC12 != 0,
            AdcClock::Adc3 => self.ahb4_reset & ADC3 != 0,
        }
    }

    pub fn bdma_enabled(&self) -> bool {
        self.ahb4_clock & !self.ahb4_reset & BDMA != 0
    }

    pub fn bdma_in_reset(&self) -> bool {
        self.ahb4_reset & BDMA != 0
    }

    pub fn dma_enabled(&self, mask: u32) -> bool {
        mask == 0 || (self.ahb1_clock & !self.ahb1_reset) & mask != 0
    }

    pub fn dma_in_reset(&self, mask: u32) -> bool {
        self.ahb1_reset & mask != 0
    }

    fn syscfg_mode_address(&self, address: u32) -> bool {
        self.syscfg_base
            .is_some_and(|base| address.wrapping_sub(base) == 4)
    }

    pub fn read(&self, address: u32) -> Option<u32> {
        if self.syscfg_mode_address(address) {
            return Some(self.syscfg_mode);
        }
        Some(match address.wrapping_sub(self.rcc_base) {
            0x80 => self.ahb1_reset,
            0x88 => self.ahb4_reset,
            0xd8 => self.ahb1_clock,
            0xe0 => self.ahb4_clock,
            0xf4 => self.apb4_clock,
            0x100 => self.ahb1_sleep_clock,
            _ => return None,
        })
    }

    pub fn write(&mut self, address: u32, value: u32) -> Option<Change> {
        if self.syscfg_mode_address(address) {
            if self.apb4_clock & SYSCFG_CLOCK == 0 {
                return Some(Change::None);
            }
            self.syscfg_mode = value & INTERFACE_MASK;
            return Some(Change::Interface(self.syscfg_mode >> 21));
        }
        Some(match address.wrapping_sub(self.rcc_base) {
            0x80 => {
                self.ahb1_reset = value & (ETH_MAC | ADC12 | 3);
                if self.ahb1_reset & ETH_MAC != 0 {
                    Change::Reset
                } else {
                    Change::Clock(self.ahb1_clock & ETH_MAC != 0)
                }
            }
            0x88 => {
                self.ahb4_reset = value & (ADC3 | BDMA);
                Change::None
            }
            0xd8 => {
                self.ahb1_clock = value;
                Change::Clock(value & ETH_MAC != 0 && self.ahb1_reset & ETH_MAC == 0)
            }
            0xe0 => {
                self.ahb4_clock = value;
                Change::None
            }
            0xf4 => {
                self.apb4_clock = value;
                Change::None
            }
            0x100 => {
                self.ahb1_sleep_clock = value;
                Change::None
            }
            _ => return None,
        })
    }
}
