use crate::{stm32h7_adc::AdcClock, stm32h7_timer::TimerClock};

const SIZE: u64 = 0x400;
const ETH_CLOCKS: u32 = 7 << 15;
const ETH_MAC: u32 = 1 << 15;
const ADC12: u32 = 1 << 5;
const ADC3: u32 = 1 << 24;
const CRC: u32 = 1 << 19;
const BDMA: u32 = 1 << 21;
const HSEM: u32 = 1 << 25;
const GPIO_PORTS: u32 = 0x7ff;
const LPTIM1: u32 = 1 << 9;
const LPTIM2_TO_5: u32 = 15 << 9;
const LTDC: u32 = 1 << 3;
const DMA2D: u32 = 1 << 4;
const SPI4: u32 = 1 << 13;
const SYSCFG_CLOCK: u32 = 1 << 1;
const INTERFACE_MASK: u32 = 7 << 21;

pub struct NetworkControl {
    pub rcc_base: u32,
    pub syscfg_base: Option<u32>,
    pub nic: Option<usize>,
    pub hse_frequency: u32,
    clocks: [u32; 67],
    syscfg_power: u32,
    ahb1_reset: u32,
    ahb1_clock: u32,
    ahb1_sleep_clock: u32,
    ahb4_reset: u32,
    ahb4_clock: u32,
    apb1l_clock: u32,
    apb1l_reset: u32,
    ahb2_reset: u32,
    ahb3_reset: u32,
    apb3_reset: u32,
    apb2_clock: u32,
    apb2_reset: u32,
    apb4_clock: u32,
    apb4_reset: u32,
    backup_control: u32,
    lsi_enabled: bool,
    syscfg_mode: u32,
    exticr: [u32; 4],
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
            hse_frequency: 8_000_000,
            clocks: {
                let mut regs = [0; 67];
                regs[0] = 0x25;
                regs[1] = 0x4000_0000;
                regs[0x28 / 4] = 0x0202_0200;
                regs[0x2c / 4] = 0x01ff_0000;
                regs[0x30 / 4] = 0x0101_0280;
                regs
            },
            syscfg_power: 0,
            ahb1_reset: 0,
            ahb1_clock: 0,
            ahb1_sleep_clock: ETH_CLOCKS,
            ahb4_reset: 0,
            ahb4_clock: 0,
            apb1l_clock: 0,
            apb1l_reset: 0,
            ahb2_reset: 0,
            ahb3_reset: 0,
            apb3_reset: 0,
            apb2_clock: 0,
            apb2_reset: 0,
            apb4_clock: 0,
            apb4_reset: 0,
            backup_control: 0,
            lsi_enabled: false,
            syscfg_mode: 0,
            exticr: [0; 4],
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

    pub fn hsem_enabled(&self) -> bool {
        self.ahb4_clock & !self.ahb4_reset & HSEM != 0
    }

    pub fn hsem_in_reset(&self) -> bool {
        self.ahb4_reset & HSEM != 0
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

    pub fn crc_enabled(&self) -> bool {
        self.ahb4_clock & !self.ahb4_reset & CRC != 0
    }

    pub fn crc_in_reset(&self) -> bool {
        self.ahb4_reset & CRC != 0
    }

    pub fn ltdc_enabled(&self) -> bool {
        self.clocks[0xe4 / 4] & LTDC != 0 && !self.ltdc_in_reset()
    }

    pub fn ltdc_in_reset(&self) -> bool {
        self.apb3_reset & LTDC != 0
    }

    pub fn dma2d_enabled(&self) -> bool {
        self.clocks[0xd4 / 4] & DMA2D != 0 && !self.dma2d_in_reset()
    }

    pub fn dma2d_in_reset(&self) -> bool {
        self.ahb3_reset & DMA2D != 0
    }

    pub fn qspi_enabled(&self) -> bool {
        self.clocks[0xd4 / 4] & (1 << 14) != 0 && !self.qspi_in_reset()
    }

    pub fn qspi_in_reset(&self) -> bool {
        self.ahb3_reset & (1 << 14) != 0
    }

    pub fn rng_enabled(&self) -> bool {
        self.clocks[0xdc / 4] & (1 << 6) != 0 && !self.rng_in_reset()
    }

    pub fn rng_in_reset(&self) -> bool {
        self.ahb2_reset & (1 << 6) != 0
    }

    pub fn lptim_enabled(&self, instance: u8) -> bool {
        let enabled = if instance == 1 {
            self.apb1l_clock & LPTIM1
        } else {
            self.apb4_clock & (1 << (instance + 7))
        };
        enabled != 0 && !self.lptim_in_reset(instance)
    }

    pub fn lptim_in_reset(&self, instance: u8) -> bool {
        if instance == 1 {
            self.apb1l_reset & LPTIM1 != 0
        } else {
            self.apb4_reset & (1 << (instance + 7)) != 0
        }
    }

    pub fn usart_enabled(&self, clock: crate::stm32_uart::Stm32UsartClock) -> bool {
        use crate::stm32_uart::Stm32UsartClock::*;
        let (enable, bit) = match clock {
            Apb1Low(bit) => (self.apb1l_clock, bit),
            Apb2(bit) => (self.apb2_clock, bit),
            Apb4(bit) => (self.apb4_clock, bit),
        };
        enable & (1 << bit) != 0 && !self.usart_in_reset(clock)
    }

    pub fn usart_in_reset(&self, clock: crate::stm32_uart::Stm32UsartClock) -> bool {
        use crate::stm32_uart::Stm32UsartClock::*;
        let (reset, bit) = match clock {
            Apb1Low(bit) => (self.apb1l_reset, bit),
            Apb2(bit) => (self.apb2_reset, bit),
            Apb4(bit) => (self.apb4_reset, bit),
        };
        reset & (1 << bit) != 0
    }

    pub fn enable_registered_timer(&mut self, clock: TimerClock) {
        match clock {
            TimerClock::Apb1Low(bit) => self.apb1l_clock |= 1 << bit,
            TimerClock::Apb2(bit) => self.apb2_clock |= 1 << bit,
        }
    }

    fn system_clock(&self) -> Option<u64> {
        let hsi = 64_000_000 >> ((self.clocks[0] >> 3) & 3);
        let oscillator = [hsi, 4_000_000, u64::from(self.hse_frequency)];
        let source = self.clocks[0x10 / 4] & 3;
        if source != 3 {
            return oscillator.get(source as usize).copied();
        }
        let selection = self.clocks[0x28 / 4];
        let reference = oscillator.get((selection & 3) as usize)?;
        let m = u64::from((selection >> 4) & 0x3f).max(1);
        let pll = self.clocks[0x30 / 4];
        let n = u64::from(pll & 0x1ff) + 1;
        let p = u64::from((pll >> 9) & 0x7f) + 1;
        (n >= 4).then_some((reference / m) * n / p)
    }

    pub fn timer_frequency(&self, clock: TimerClock) -> Option<u32> {
        let domain1 = self.clocks[0x18 / 4];
        let domain2 = self.clocks[0x1c / 4];
        let domain_divider = |encoded: u32| {
            if encoded & 8 == 0 {
                1
            } else {
                2u64 << (encoded & 7)
            }
        };
        let apb = match clock {
            TimerClock::Apb1Low(_) => (domain2 >> 4) & 7,
            TimerClock::Apb2(_) => (domain2 >> 8) & 7,
        };
        let timpre = self.clocks[0x10 / 4] & (1 << 15) != 0;
        let timer_divider = if apb < 5 {
            1
        } else {
            1u64 << (apb - 4 - u32::from(timpre))
        };
        let divider =
            domain_divider((domain1 >> 8) & 15) * domain_divider(domain1 & 15) * timer_divider;
        u32::try_from(self.system_clock()? / divider)
            .ok()
            .filter(|frequency| *frequency != 0)
    }

    pub fn timer_enabled(&self, clock: TimerClock) -> bool {
        self.timer_gate_enabled(clock) && !self.timer_in_reset(clock)
    }

    pub fn timer_gate_enabled(&self, clock: TimerClock) -> bool {
        match clock {
            TimerClock::Apb1Low(bit) => self.apb1l_clock & (1 << bit) != 0,
            TimerClock::Apb2(bit) => self.apb2_clock & (1 << bit) != 0,
        }
    }

    pub fn timer_in_reset(&self, clock: TimerClock) -> bool {
        match clock {
            TimerClock::Apb1Low(bit) => self.apb1l_reset & (1 << bit) != 0,
            TimerClock::Apb2(bit) => self.apb2_reset & (1 << bit) != 0,
        }
    }

    pub fn i2c_enabled(&self, instance: u8) -> bool {
        let enabled = if instance == 4 {
            self.apb4_clock & (1 << 7)
        } else {
            self.apb1l_clock & (1 << (instance + 20))
        };
        enabled != 0 && !self.i2c_in_reset(instance)
    }

    pub fn i2c_in_reset(&self, instance: u8) -> bool {
        if instance == 4 {
            self.apb4_reset & (1 << 7) != 0
        } else {
            self.apb1l_reset & (1 << (instance + 20)) != 0
        }
    }

    pub fn spi4_enabled(&self) -> bool {
        self.apb2_clock & !self.apb2_reset & SPI4 != 0
    }

    pub fn spi4_in_reset(&self) -> bool {
        self.apb2_reset & SPI4 != 0
    }

    pub fn rtc_enabled(&self) -> bool {
        self.backup_control & (3 << 15) == 1 << 15
    }

    pub fn rtc_in_reset(&self) -> bool {
        self.backup_control & (1 << 16) != 0
    }

    pub fn gpio_enabled(&self, port: u8) -> bool {
        (self.ahb4_clock & !self.ahb4_reset) & (1 << port) != 0
    }

    pub fn gpio_in_reset(&self, port: u8) -> bool {
        self.ahb4_reset & (1 << port) != 0
    }

    pub fn syscfg_port(&self, line: u8) -> u8 {
        let shift = (line % 4) * 4;
        ((self.exticr[usize::from(line / 4)] >> shift) & 0xf) as u8
    }

    fn syscfg_line_register(&self, address: u32) -> Option<usize> {
        let offset = address.wrapping_sub(self.syscfg_base?);
        (matches!(offset, 8 | 12 | 16 | 20)).then(|| ((offset - 8) / 4) as usize)
    }

    fn syscfg_mode_address(&self, address: u32) -> bool {
        self.syscfg_base
            .is_some_and(|base| address.wrapping_sub(base) == 4)
    }

    fn clock_slot(offset: u32) -> Option<usize> {
        matches!(
            offset,
            0x00 | 0x04
                | 0x10
                | 0x18
                | 0x1c
                | 0x20
                | 0x28
                | 0x2c
                | 0x30
                | 0x34
                | 0x38
                | 0x3c
                | 0x40
                | 0x44
                | 0x4c
                | 0x50
                | 0x54
                | 0x58
                | 0xa0
                | 0xd4
                | 0xdc
                | 0xe4
                | 0xec
                | 0xfc
                | 0x104
                | 0x108
        )
        .then_some((offset / 4) as usize)
    }

    pub fn read(&self, address: u32) -> Option<u32> {
        if self
            .syscfg_base
            .is_some_and(|base| address.wrapping_sub(base) == 0x2c)
        {
            return Some(self.syscfg_power);
        }
        if self
            .syscfg_base
            .is_some_and(|base| address.wrapping_sub(base) == 0x304)
        {
            // UR1 reflects the boot option bytes. The single-core platform leaves M4 boot disabled.
            return Some(0);
        }
        if self.syscfg_mode_address(address) {
            return Some(self.syscfg_mode);
        }
        if let Some(index) = self.syscfg_line_register(address) {
            return Some(self.exticr[index]);
        }
        let offset = address.wrapping_sub(self.rcc_base);
        if let Some(slot) = Self::clock_slot(offset) {
            return Some(self.clocks[slot]);
        }
        Some(match offset {
            0x70 => self.backup_control | ((self.backup_control & 1) << 1),
            0x74 => u32::from(self.lsi_enabled) * 3,
            0x7c => self.ahb3_reset,
            0x80 => self.ahb1_reset,
            0x84 => self.ahb2_reset,
            0x88 => self.ahb4_reset,
            0x8c => self.apb3_reset,
            0x90 => self.apb1l_reset,
            0x98 => self.apb2_reset,
            0x9c => self.apb4_reset,
            0xd8 => self.ahb1_clock,
            0xe0 => self.ahb4_clock,
            0xe8 => self.apb1l_clock,
            0xf0 => self.apb2_clock,
            0xf4 => self.apb4_clock,
            0x100 => self.ahb1_sleep_clock,
            _ => return None,
        })
    }

    pub fn write(&mut self, address: u32, value: u32) -> Option<Change> {
        if self
            .syscfg_base
            .is_some_and(|base| address.wrapping_sub(base) == 0x2c)
        {
            if self.apb4_clock & SYSCFG_CLOCK != 0 {
                self.syscfg_power = value & 1;
            }
            return Some(Change::None);
        }
        if self.syscfg_mode_address(address) {
            if self.apb4_clock & SYSCFG_CLOCK == 0 {
                return Some(Change::None);
            }
            self.syscfg_mode = value & INTERFACE_MASK;
            return Some(Change::Interface(self.syscfg_mode >> 21));
        }
        if let Some(index) = self.syscfg_line_register(address) {
            if self.apb4_clock & SYSCFG_CLOCK != 0 {
                self.exticr[index] = value & 0xffff;
            }
            return Some(Change::None);
        }
        let offset = address.wrapping_sub(self.rcc_base);
        if let Some(slot) = Self::clock_slot(offset) {
            self.clocks[slot] = match offset {
                0 => {
                    const PLLS: u32 = (1 << 24) | (1 << 26) | (1 << 28);
                    const OSCILLATORS: [u32; 4] = [1, 1 << 7, 1 << 16, 0];
                    let old = self.clocks[0];
                    let source = (self.clocks[0x10 / 4] & 3) as usize;
                    let pll_source = (self.clocks[0x28 / 4] & 3) as usize;
                    let required = if source == 3 {
                        (1 << 24) | (old & OSCILLATORS[pll_source])
                    } else {
                        old & OSCILLATORS[source]
                    };
                    let mut control = (value & 0x150d_d29b) | required | (old & (1 << 19));
                    if pll_source == 0 && (old | value) & PLLS != 0 {
                        control = (control & !(3 << 3)) | (old & (3 << 3));
                    }
                    let ready = ((control & (PLLS | (1 << 7) | (1 << 12) | (1 << 16))) << 1)
                        | ((control & 1) << 2);
                    control | ready | (1 << 5)
                }
                0x04 => value & 0x7fff_ffff,
                0x10 => {
                    let source = value & 7;
                    let source = if source < 4 {
                        source
                    } else {
                        self.clocks[slot] & 7
                    };
                    (value & !0x3f) | source | (source << 3)
                }
                0x28 if self.clocks[0] & ((1 << 24) | (1 << 26) | (1 << 28)) != 0 => {
                    (value & !3) | (self.clocks[slot] & 3)
                }
                0x30 => {
                    let divp = (value >> 9) & 0x7f;
                    if self.clocks[0] & (1 << 24) != 0 {
                        self.clocks[slot]
                    } else if divp != 0 && divp & 1 == 0 {
                        (value & !(0x7f << 9)) | (self.clocks[slot] & (0x7f << 9))
                    } else {
                        value
                    }
                }
                0xa0 => value & 0xf, // GCR boot requests are latched without a second CPU.
                _ => value,
            };
            return Some(Change::None);
        }
        Some(match offset {
            0x70 => {
                self.backup_control = value & 0x0001_837d;
                Change::None
            }
            0x74 => {
                self.lsi_enabled = value & 1 != 0;
                Change::None
            }
            0x7c => {
                self.ahb3_reset = value & (DMA2D | (1 << 14));
                Change::None
            }
            0x80 => {
                self.ahb1_reset = value & (ETH_MAC | ADC12 | 3);
                if self.ahb1_reset & ETH_MAC != 0 {
                    Change::Reset
                } else {
                    Change::Clock(self.ahb1_clock & ETH_MAC != 0)
                }
            }
            0x84 => {
                self.ahb2_reset = value & (1 << 6);
                Change::None
            }
            0x88 => {
                self.ahb4_reset = value & (ADC3 | CRC | BDMA | HSEM | GPIO_PORTS);
                Change::None
            }
            0x8c => {
                self.apb3_reset = value & LTDC;
                Change::None
            }
            0x90 => {
                self.apb1l_reset = value & ((7 << 21) | 0x1ff | LPTIM1 | (0xf << 17) | (3 << 30));
                Change::None
            }
            0x9c => {
                self.apb4_reset = value & ((1 << 7) | LPTIM2_TO_5 | (1 << 3));
                Change::None
            }
            0x98 => {
                self.apb2_reset = value & (SPI4 | TimerClock::apb2_mask() | (3 << 4));
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
            0xe8 => {
                self.apb1l_clock = value;
                Change::None
            }
            0xf0 => {
                self.apb2_clock = value;
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

#[cfg(test)]
mod upstream_rcc_tests {
    use super::*;
    use crate::machine::Machine;

    const BASE: u32 = 0x1000;
    const CR: u32 = BASE;
    const CFGR: u32 = BASE + 0x10;
    const PLLCKSELR: u32 = BASE + 0x28;
    const PLL1DIVR: u32 = BASE + 0x30;
    const HSI: u32 = (1 << 0) | (1 << 2);
    const CSI: u32 = 1 << 7;
    const HSE: u32 = 1 << 16;
    const PLL1: u32 = 1 << 24;
    const DIV_READY: u32 = 1 << 5;

    struct Fixture(Machine);

    impl Fixture {
        fn new() -> Self {
            let mut machine = Machine::without_default_ram();
            machine
                .bus
                .map_h7_network_control(NetworkControl::new(BASE, None, None))
                .unwrap();
            Self(machine)
        }

        fn read(&mut self, address: u32) -> u32 {
            self.0.bus.read::<4>(address).unwrap()
        }

        fn write(&mut self, address: u32, value: u32) {
            self.0.bus.write(address, 4, value, &mut std::io::sink()).unwrap();
        }
    }

    fn check_oscillator(source: u32, on: u32, expected: u32) {
        let mut rcc = Fixture::new();
        let read = rcc.read(CR);
        rcc.write(CR, read | on);
        rcc.write(CFGR, source);
        rcc.write(CR, 0);
        assert_eq!(rcc.read(CR), expected | DIV_READY);
    }

    #[test]
    fn should_prevent_disabling_hsi() {
        let mut rcc = Fixture::new();
        rcc.write(CR, 0);
        assert_eq!(rcc.read(CR), HSI | DIV_READY);
    }

    #[test]
    fn should_prevent_disabling_csi() {
        check_oscillator(1, CSI, CSI | CSI << 1);
    }

    #[test]
    fn should_prevent_disabling_hse() {
        check_oscillator(2, HSE, HSE | HSE << 1);
    }

    #[test]
    fn should_prevent_disabling_pll1() {
        check_oscillator(3, PLL1, PLL1 | PLL1 << 1 | HSI);
    }

    #[test]
    fn should_prevent_plls_modification_while_running() {
        let mut rcc = Fixture::new();
        let initial = rcc.read(CR);
        for (pll, source) in [(PLL1, 1), (1 << 26, 2), (1 << 28, 3)] {
            rcc.write(CR, initial | pll);
            let before = rcc.read(PLLCKSELR);
            rcc.write(PLLCKSELR, before | source);
            assert_eq!(rcc.read(PLLCKSELR), before);
        }
    }

    #[test]
    fn should_keep_hsecsson_enabled() {
        let mut rcc = Fixture::new();
        let before = rcc.read(CR);
        rcc.write(CR, before | 1 << 19);
        rcc.write(CR, before & !(1 << 19));
        assert_eq!(rcc.read(CR) & (1 << 19), 1 << 19);
    }

    #[test]
    fn should_prevent_invalid_writes_to_divp1() {
        let mut rcc = Fixture::new();
        let before = rcc.read(PLL1DIVR);
        let mask = 0x7f << 9;
        for (written, expected) in [(3, 3), (2, 3), (0, 0)] {
            rcc.write(PLL1DIVR, (before & !mask) | (written << 9));
            assert_eq!(rcc.read(PLL1DIVR) & mask, expected << 9);
        }
    }
}
