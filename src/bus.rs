use std::{
    alloc::{Layout, alloc_zeroed},
    fmt,
    fs::File,
    io::{self, Read, Write},
    path::Path,
};

use crate::{
    clint::Clint,
    constant_mmio::ConstantMmio,
    designware_usb::DesignWareUsb,
    dwt::Dwt,
    ethernet::Controller,
    litex,
    litex_eth::{Ethernet, Phy, Registration},
    litex_i2s::I2s,
    litex_panel::{ButtonAction, Panel},
    litex_sd::{Card, Controller as SdController},
    litex_video::Framebuffer,
    murax::{Gpio, Timer},
    nrf52840::Peripheral as NrfPeripheral,
    ns16550::Ns16550,
    nvic::{ActiveExceptions, Nvic, VTOR},
    plic::Plic,
    stm32_adc::Stm32F0Adc,
    stm32_can::Stm32Can,
    stm32_can::{Filters, StmCan},
    stm32_crc::Stm32Crc,
    stm32_crc::Stm32F0Crc,
    stm32_dma::Stm32Dma,
    stm32_exti::Exti,
    stm32_flash::Stm32F0Flash,
    stm32_gpio::Stm32Gpio,
    stm32_gpio_board::{GpioButton, GpioLed},
    stm32_i2c::Stm32I2c,
    stm32_iwdg::Iwdg,
    stm32_rcc::Stm32F0Rcc,
    stm32_rtc::Stm32Rtc,
    stm32_spi::Stm32F0Spi,
    stm32_timer::Stm32Timer,
    stm32_uart::{Stm32Irq, Stm32Usart},
    stm32f1_i2c::F1I2c,
    stm32f4_flash::F4Flash,
    stm32f4_power::F4Power,
    stm32f4_rcc::F4Rcc,
    stm32f4_spi::F4Spi,
    stm32f4_system::F4System,
    stm32h7_adc::Adc,
    stm32h7_adc_dma::{AdcDma, DmaMux, StreamTarget},
    stm32h7_bdma::Bdma,
    stm32h7_crypto::Crypto,
    stm32h7_dma2d::Dma2d,
    stm32h7_dsi::Dsi,
    stm32h7_exti::H7Exti,
    stm32h7_fdcan::Fdcan,
    stm32h7_flash::FlashController,
    stm32h7_gpio::Stm32Gpio as H7Gpio,
    stm32h7_hsem::Hsem,
    stm32h7_i2c::I2c,
    stm32h7_lptim::LpTimer,
    stm32h7_ltdc::Ltdc,
    stm32h7_net_control::{Change, NetworkControl},
    stm32h7_qspi::Qspi,
    stm32h7_rng::Rng,
    stm32h7_rtc::Stm32Rtc as H7Rtc,
    stm32h7_sdmmc::Sdmmc,
    stm32h7_spi::Spi,
    stm32h7_timer::Timer as H7Timer,
    systick::SysTick,
    uart::{LiteXTxTiming, LiteXUart, MuraxUart, UartHistory, UartWaitMode},
};

// Cortex-M peripheral time advances by 10 ns per execution step.
pub(crate) const CORTEX_M_STEP_HZ: u64 = 100_000_000;

pub const RAM_BASE: u32 = 0x8000_0000;
pub const UART_TX: u32 = 0x1000_0000;

#[derive(Debug)]
pub enum BusError {
    Unmapped(u32, usize),
    Denied(u32),
    Page(u32),
    Misaligned(u32, usize),
    Output(io::Error),
    AudioOutput(io::Error),
}

impl BusError {
    pub fn at_virtual(self, address: u32) -> Self {
        match self {
            Self::Unmapped(_, width) => Self::Unmapped(address, width),
            _ => self,
        }
    }
}

impl fmt::Display for BusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unmapped(address, width) => {
                write!(f, "unmapped RAM access at {address:#010x} ({width} bytes)")
            }
            Self::Page(address) => write!(f, "page fault at {address:#010x}"),
            Self::Denied(address) => write!(
                f,
                "physical memory protection denied access at {address:#010x}"
            ),
            Self::Misaligned(address, width) => {
                write!(f, "misaligned {width}-byte access at {address:#010x}")
            }
            Self::Output(error) => write!(f, "UART: {error}"),
            Self::AudioOutput(error) => write!(f, "LiteX I2S: {error}"),
        }
    }
}

pub(crate) struct Region {
    base: u64,
    mapped: usize,
    aliases: Vec<u32>,
    bytes: Vec<u8>,
    flash_pages: Option<Vec<bool>>,
}

struct LiteXTimer {
    bases: Vec<u32>,
    timer: litex::Timer,
    vex_irq: Option<(usize, u8)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct Reservation64 {
    guest: u64,
    backing: usize,
    offset: usize,
    width: usize,
}

struct PwmLed {
    name: String,
    base: u32,
    pin: u8,
    inverted: bool,
    routes: [Option<(u32, u8)>; 16],
}

#[derive(Debug)]
struct RamAlias {
    cluster: bool,
    backing: usize,
    offset: usize,
    harts: u64,
    base: u64,
    length: usize,
}

struct Tohost {
    address: u64,
    bytes: Box<[u8; Self::SIZE]>,
}

impl Tohost {
    const SIZE: usize = 0x1000;

    fn range(&self, address: u64, width: usize) -> Option<std::ops::Range<usize>> {
        let start = usize::try_from(address.checked_sub(self.address)?).ok()?;
        let end = start.checked_add(width)?;
        (end <= Self::SIZE && matches!(width, 1 | 2 | 4 | 8)).then_some(start..end)
    }
}

#[derive(Default)]
pub struct Bus {
    regions: Vec<Region>,
    silencers: Vec<(u32, u32)>,
    tags: Vec<(u32, u32)>,
    aliases: Vec<RamAlias>,
    pub rv64_devices: crate::rv64_devices::Devices,
    rv64_reservations: [Option<Reservation64>; crate::machine::MAX_HARTS],
    constant_mmio: Vec<ConstantMmio>,
    pub ethernet: Vec<Controller>,
    h7_network_control: Option<NetworkControl>,
    reservations: [Option<(u32, usize)>; crate::machine::MAX_HARTS],
    nrf_peripherals: Vec<NrfPeripheral>,
    nrf_tags: Vec<(u32, u32, u32)>,
    rv64_tohost: Option<u64>,
    robot_tohost: Option<Tohost>,
    pub rv64_test_exit: Option<u64>,
    pub(crate) renode_unmapped_data: bool,
    pub(crate) renode_lrsc: bool,
    uart_tx: Option<u32>,
    pub uart: Option<MuraxUart>,
    ns16550: Option<Ns16550>,
    litex_uart: Option<LiteXUart>,
    litex_timer: Option<LiteXTimer>,
    pub litex_cpu_timer: Option<litex::CpuTimer>,
    litex_soc: Option<litex::SocController>,
    litex_io: Vec<(Vec<u32>, litex::IoDevice)>,
    litex_gpio_irqs: Vec<(usize, usize, u8)>,
    tohost: Option<(u32, bool)>,
    pub tohost_exit: Option<u64>,
    litex_eth: Option<Ethernet>,
    litex_panel: Option<Panel>,
    pub litex_i2s: Vec<I2s>,
    litex_sd: Option<SdController>,
    pub litex_video: Option<Framebuffer>,
    litex_flash: Option<(Vec<u32>, usize, crate::litex_flash::Flash)>,
    stm32_uarts: Vec<Stm32Usart>,
    stm32_uart_history: Vec<Option<UartHistory>>,
    stm32_uart_input: usize,
    step_hz: Option<u32>,
    exti: Option<Exti>,
    stm32_gpios: Vec<Stm32Gpio>,
    gpio_exti_levels: u32,
    gpio_leds: Vec<(u32, GpioLed)>,
    gpio_buttons: Vec<GpioButton>,
    gpio_timer_inputs: Vec<(u32, u8, u32, u8)>,
    led_events: bool,
    stm32_timers: Vec<Stm32Timer>,
    stm32_spis: Vec<Stm32F0Spi>,
    stm32_i2cs: Vec<Stm32I2c>,
    stm32_crc: Option<Stm32F0Crc>,
    stm32_cans: Vec<Stm32Can>,
    stm32_adc: Option<Stm32F0Adc>,
    stm32_adc_name: Option<String>,
    stm32_dma: Option<Stm32Dma>,
    stm32f0_tags: Vec<std::ops::RangeInclusive<u32>>,
    stm32_rcc: Option<Stm32F0Rcc>,
    stm32_flash: Option<Stm32F0Flash>,
    stm32_rtc: Option<Stm32Rtc>,
    iwdgs: Vec<Iwdg>,
    h7_exti: Option<H7Exti>,
    h7_adc: Vec<Adc>,
    h7_bdma: Vec<Bdma>,
    h7_adc_dma: Vec<AdcDma>,
    h7_dma_mux: Vec<DmaMux>,
    f4_flash: Option<F4Flash>,
    f4_power: Option<F4Power>,
    f4_rcc: Option<F4Rcc>,
    f4_system: Option<F4System>,
    f4_gated_ranges: Vec<(u32, u32)>,
    h7_flash: Option<FlashController>,
    h7_gpio: Vec<H7Gpio>,
    h7_gpio_buttons: Vec<(String, u32, u8, bool)>,
    h7_gpio_leds: Vec<(String, u32, u8, bool)>,
    gpio_pwm_leds: Vec<PwmLed>,
    h7_rtc: Vec<H7Rtc>,
    h7_spi: Vec<Spi>,
    f4_spi: Vec<F4Spi>,
    h7_i2c: Vec<I2c>,
    f1_i2c: Vec<F1I2c>,
    h7_rng: Vec<Rng>,
    h7_qspi: Vec<Qspi>,
    h7_timers: Vec<H7Timer>,
    h7_lptimers: Vec<LpTimer>,
    h7_crc: Vec<Stm32Crc>,
    h7_fdcan: Vec<Fdcan>,
    h7_dma2d: Vec<Dma2d>,
    h7_hsem: Option<Hsem>,
    h7_sdmmc: Vec<Sdmmc>,
    h7_crypto: Vec<Crypto>,
    h7_dsi: Vec<Dsi>,
    designware_usb: Vec<DesignWareUsb>,
    h7_ltdc: Option<Ltdc>,
    h7_step: u64,
    stm32_can: Vec<StmCan>,
    bitbands: Vec<(u32, u32, u32)>,
    bus_tags: Vec<(u32, u32, u32)>,
    timer_pins: Vec<TimerPin>,
    gpio_wires: Vec<GpioWire>,
    f4_gpio_leds: Vec<(String, u32, u8)>,
    gpio_exti_outputs: [Option<u8>; 16],
    plic: Option<Plic>,
    pub systick: Option<SysTick>,
    pub dwt: Option<Dwt>,
    pub nvic: Option<Nvic>,
    nvic_name: Option<String>,
    external_nvic_levels: u32,
    pub clint: Option<Clint>,
    gpio: Option<(u32, Gpio)>,
    pub timer: Option<(u32, Timer)>,
}

pub struct GpioWire {
    pub source: (u32, u8),
    pub target: (u32, u8),
    pub level: bool,
}

pub struct TimerPin {
    pub timer: u32,
    pub gpio: u32,
    pub channel: usize,
    pub pin: u8,
    pub function: u8,
    pub capture: bool,
}

pub struct EthernetDma<'a> {
    regions: &'a mut [Region],
    reservations: &'a mut [Option<(u32, usize)>],
}

impl<'a> EthernetDma<'a> {
    pub fn read(&self, address: u32, width: usize) -> Option<&[u8]> {
        self.regions.iter().find_map(|region| {
            let offset = region.offset(u64::from(address), width)?;
            Some(&region.bytes[offset..offset + width])
        })
    }

    pub fn write(&mut self, address: u32, bytes: &[u8]) -> bool {
        let memory = EthernetDma {
            regions: self.regions,
            reservations: self.reservations,
        };
        if let Ok(target) = memory.memory(address, bytes.len()) {
            target.copy_from_slice(bytes);
            true
        } else {
            false
        }
    }

    pub fn read_descriptor(&self, address: u32) -> Option<[u32; 4]> {
        let bytes = self.read(address, 16)?;
        let (words, _) = bytes.as_chunks::<4>();
        Some(<[_; 4]>::try_from(words).ok()?.map(u32::from_le_bytes))
    }

    pub fn write_descriptor(&mut self, address: u32, words: [u32; 4]) -> bool {
        let words = words.map(u32::to_le_bytes);
        self.write(address, words.as_flattened())
    }

    fn memory(self, address: u32, width: usize) -> Result<&'a mut [u8], BusError> {
        for region in self.regions {
            if let Some(offset) = region.offset(u64::from(address), width) {
                if width != 0 {
                    for reservation in self.reservations.iter_mut() {
                        if reservation.is_some_and(|(reserved, _)| {
                            let reserved = reserved & !3;
                            let address = region.base + offset as u64;
                            address < u64::from(reserved) + 4
                                && u64::from(reserved) < address + width as u64
                        }) {
                            *reservation = None;
                        }
                    }
                }
                return Ok(&mut region.bytes[offset..offset + width]);
            }
        }
        Err(BusError::Unmapped(address, width))
    }
}

impl Region {
    fn new(base: u64, size: usize, mapped: usize) -> Result<Self, String> {
        if size == 0 || mapped > size || base.checked_add(mapped as u64).is_none() {
            return Err(format!("invalid RAM range at {base:#010x} ({size} bytes)"));
        }
        let layout = Layout::array::<u8>(size).map_err(|error| error.to_string())?;
        // A zeroed allocation allows demand-zero pages on hosts that support them.
        // SAFETY: The layout is valid and nonzero. The pointer is checked below.
        let allocation = unsafe { alloc_zeroed(layout) };
        if allocation.is_null() {
            return Err(format!("could not allocate {size} bytes of guest RAM"));
        }
        // SAFETY: Global allocation with u8 alignment and size bytes, all initialized.
        // Vec takes ownership and deallocates with the same layout.
        let bytes = unsafe { Vec::from_raw_parts(allocation, size, size) };
        Ok(Self {
            base,
            aliases: Vec::new(),
            mapped,
            bytes,
            flash_pages: None,
        })
    }

    fn bytes_mut(&mut self, offset: usize, width: usize) -> &mut [u8] {
        if let Some(pages) = self.flash_pages.as_mut()
            && width != 0
        {
            for (page, initialized) in pages
                .iter_mut()
                .enumerate()
                .take((offset + width - 1) / 4096 + 1)
                .skip(offset / 4096)
            {
                if !*initialized {
                    self.bytes[page * 4096..(page + 1) * 4096].fill(0xff);
                    *initialized = true;
                }
            }
        }
        &mut self.bytes[offset..offset + width]
    }

    pub(crate) fn flash_size(&self) -> usize {
        self.bytes.len()
    }

    pub(crate) fn flash_read(&self, offset: usize) -> u8 {
        if self
            .flash_pages
            .as_ref()
            .is_some_and(|pages| !pages[offset / 4096])
        {
            0xff
        } else {
            self.bytes[offset]
        }
    }

    pub(crate) fn flash_program(&mut self, offset: usize, byte: u8) {
        if offset < self.bytes.len() {
            self.bytes_mut(offset, 1)[0] &= byte;
        }
    }

    pub(crate) fn flash_erase(&mut self, offset: usize, size: usize) {
        let end = (offset + size).min(self.bytes.len());
        if offset < end
            && let Some(pages) = self.flash_pages.as_mut()
        {
            pages[offset / 4096..end / 4096].fill(false);
        }
    }

    fn offset(&self, address: u64, width: usize) -> Option<usize> {
        let at = |base| {
            let offset = usize::try_from(address.checked_sub(base)?).ok()?;
            (width <= self.mapped.checked_sub(offset)?).then_some(offset)
        };
        at(self.base).or_else(|| self.aliases.iter().find_map(|&base| at(u64::from(base))))
    }
}

impl Bus {
    pub fn new(size: usize) -> Result<Self, String> {
        let mut bus = Self {
            uart_tx: Some(UART_TX),
            ..Self::default()
        };
        bus.map_ram(RAM_BASE, size)?;
        Ok(bus)
    }

    pub fn watch_tohost(&mut self, address: u32, act: bool) -> Result<(), String> {
        if address & 7 != 0
            || self.read_ram::<4>(address).is_err()
            || self.read_ram::<4>(address + 4).is_err()
        {
            return Err("--tohost requires an aligned 8-byte address in RAM".into());
        }
        self.tohost = Some((address, act));
        Ok(())
    }

    fn observe_tohost(
        &mut self,
        address: u32,
        width: usize,
        uart: &mut impl Write,
    ) -> Result<(), BusError> {
        let Some((base, act)) = self.tohost else {
            return Ok(());
        };
        // ACT's RV32 Sail model commits each packet with a separate upper-word store.
        if self.tohost_exit.is_some()
            || address != if act { base + 4 } else { base }
            || (act && width != 4)
        {
            return Ok(());
        }
        let low = self.read_ram::<4>(base)?;
        let high = self.read_ram::<4>(base + 4)?;
        if act && high == 0x0101_0000 {
            uart.write_all(&[low as u8]).map_err(BusError::Output)?;
        } else if (!act || high == 0) && low & 1 != 0 {
            self.tohost_exit = Some(((u64::from(high) << 32) | u64::from(low)) >> 1);
        }
        Ok(())
    }

    pub fn use_renode_unmapped_data(&mut self) {
        self.renode_unmapped_data = true;
    }

    pub fn map_ram(&mut self, base: u32, size: usize) -> Result<(), String> {
        if u64::from(base).saturating_add(size as u64) > 1 << 32 {
            return Err(format!("invalid RAM range at {base:#010x} ({size} bytes)"));
        }
        self.map_ram64(u64::from(base), size)
    }

    pub fn map_ram64(&mut self, base: u64, size: usize) -> Result<(), String> {
        self.map_ram_backing64(base, size, size).map(|_| ())
    }

    pub fn map_ram_backing64(
        &mut self,
        base: u64,
        size: usize,
        mapped: usize,
    ) -> Result<usize, String> {
        self.check_ram_range(base, mapped)?;
        let index = self.regions.len();
        self.regions.push(Region::new(base, size, mapped)?);
        Ok(index)
    }

    pub(crate) fn allocate_ram_backing(&mut self, size: usize) -> Result<usize, String> {
        let index = self.regions.len();
        self.regions.push(Region::new(0, size, 0)?);
        Ok(index)
    }

    pub fn map_alias64(
        &mut self,
        backing: usize,
        base: u64,
        length: usize,
        offset: usize,
        harts: u64,
        cluster: bool,
    ) -> Result<(), String> {
        if harts != 0 {
            let end = base
                .checked_add(length as u64)
                .ok_or("RAM range overflow")?;
            if length == 0
                || self.aliases.iter().any(|alias| {
                    alias.harts & harts != 0
                        && base < alias.base + alias.length as u64
                        && alias.base < end
                })
            {
                return Err("empty or overlapping CPU-scoped RAM range".into());
            }
        } else {
            self.check_ram_range(base, length)?;
        }
        if offset
            .checked_add(length)
            .is_none_or(|end| end > self.regions[backing].bytes.len())
        {
            return Err("RAM alias is larger than its backing".into());
        }
        self.aliases.push(RamAlias {
            cluster,
            backing,
            offset,
            harts,
            base,
            length,
        });
        Ok(())
    }

    pub(crate) fn move_cpu_ram_alias(
        &mut self,
        hart: usize,
        old: u64,
        new: u64,
    ) -> Result<(), String> {
        let mask = 1 << hart;
        let index = self
            .aliases
            .iter()
            .position(|alias| alias.base == old && alias.harts == mask && !alias.cluster)
            .ok_or("no RAM registration exclusive to the selected CPU at the source address")?;
        let length = self.aliases[index].length as u64;
        let end = new.checked_add(length).ok_or("RAM range overflow")?;
        if self.aliases.iter().enumerate().any(|(other, alias)| {
            other != index
                && alias.harts & mask != 0
                && new < alias.base + alias.length as u64
                && alias.base < end
        }) {
            return Err("destination overlaps a CPU-scoped RAM registration".into());
        }
        self.aliases[index].base = new;
        Ok(())
    }

    fn check_ram_range(&self, base: u64, size: usize) -> Result<(), String> {
        let end = base.checked_add(size as u64).ok_or("RAM range overflow")?;
        if size == 0 {
            return Err("RAM range must be nonempty".into());
        }
        if self.overlaps(base, end) {
            return Err(format!(
                "overlapping RAM range at {base:#010x} ({size} bytes)"
            ));
        }
        Ok(())
    }

    pub fn map_ram_views(&mut self, bases: &[u32], size: usize) -> Result<(), String> {
        let Some((&base, aliases)) = bases.split_first() else {
            return Err("expected at least one RAM address".into());
        };
        for (index, &base) in bases.iter().enumerate() {
            let start = u64::from(base);
            let end = start.saturating_add(size as u64);
            if end > 1 << 32
                || self.overlaps(start, end)
                || bases[..index].iter().any(|&other| {
                    let other = u64::from(other);
                    start < other + size as u64 && other < end
                })
            {
                return Err(format!("invalid or overlapping RAM view at {base:#010x}"));
            }
        }
        let mut region = Region::new(u64::from(base), size, size)?;
        region.aliases.extend_from_slice(aliases);
        self.regions.push(region);
        Ok(())
    }

    pub fn map_ethernet(&mut self, device: Controller) -> Result<(), String> {
        for (index, mapping) in device.mappings().iter().enumerate() {
            let start = u64::from(mapping.base);
            let end = start + u64::from(mapping.size);
            if mapping.base & 3 != 0
                || mapping.size == 0
                || end > 1 << 32
                || self.overlaps(start, end)
                || device.mappings()[..index].iter().any(|other| {
                    let base = u64::from(other.base);
                    start < base + u64::from(other.size) && base < end
                })
            {
                return Err(format!(
                    "invalid or overlapping Ethernet mapping at {start:#010x}"
                ));
            }
        }
        self.ethernet.push(device);
        Ok(())
    }

    pub fn map_h7_network_control(&mut self, control: NetworkControl) -> Result<(), String> {
        if control.rcc_base & 3 != 0
            || control.syscfg_base.is_some_and(|base| base & 3 != 0)
            || control
                .ranges()
                .any(|(start, end)| end > 1 << 32 || self.overlaps(start, end))
            || control
                .ranges()
                .zip(control.ranges().skip(1))
                .any(|((start, end), (next, next_end))| start < next_end && next < end)
        {
            return Err("invalid or overlapping STM32H7 clock control registers".into());
        }
        if let Some(nic) = control.nic {
            let Some(Controller::Stm32h7(device)) = self.ethernet.get_mut(nic) else {
                return Err("invalid STM32H7 Ethernet clock binding".into());
            };
            device.set_clock(false);
        }
        self.h7_network_control = Some(control);
        Ok(())
    }

    pub fn map_constant_mmio(&mut self, peripheral: ConstantMmio) -> Result<(), String> {
        let start = u64::from(peripheral.base);
        let end = start + u64::from(peripheral.size);
        if end > 1 << 32 || self.overlaps(start, end) {
            return Err(format!(
                "invalid or overlapping peripheral at {start:#010x}"
            ));
        }
        self.constant_mmio.push(peripheral);
        Ok(())
    }

    pub fn map_dwt(&mut self, base: u32, frequency: u32) -> Result<(), String> {
        let device = Dwt::new(base, frequency);
        let [(start, end), (control, control_end)] = device.ranges();
        if base & 3 != 0
            || end > 1 << 32
            || (start < control_end && control < end)
            || self.dwt.is_some()
            || self.overlaps(start, end)
            || self.overlaps(control, control_end)
        {
            return Err("invalid or overlapping DWT mapping".into());
        }
        self.dwt = Some(device);
        Ok(())
    }

    pub fn map_iwdg(&mut self, device: Iwdg) -> Result<(), String> {
        self.check_peripheral_range(device.base, Iwdg::SIZE)?;
        self.iwdgs.push(device);
        Ok(())
    }

    pub fn set_step_hz(&mut self, rate: u32) {
        self.step_hz = Some(rate);
        if let Some(timer) = &mut self.systick {
            timer.set_step_rate(rate);
        }
    }

    pub fn step_hz(&self) -> u32 {
        self.step_hz
            .unwrap_or(crate::machine::VIRTUAL_STEPS_PER_SECOND)
    }

    pub fn tick_systick(&mut self) {
        if let Some(timer) = &mut self.systick {
            let before = timer.pending_exceptions();
            timer.tick();
            if timer.pending_exceptions() & !before != 0
                && let Some(nvic) = &mut self.nvic
            {
                nvic.signal_pending_event();
            }
        }
    }

    pub fn tick_watchdogs(&mut self) -> bool {
        let rate = self.step_hz();
        self.iwdgs.iter_mut().any(|watchdog| watchdog.tick(rate))
    }

    pub fn reset_cortex_m_devices(&mut self) {
        self.reservations.fill(None);
        if let Some(device) = &mut self.nvic {
            device.reset();
        }
        if let Some(device) = &mut self.systick {
            device.reset();
        }
        if let Some(device) = &mut self.dwt {
            device.reset();
        }
        if let Some(device) = &mut self.exti {
            device.reset(self.gpio_exti_levels);
        }
        if let Some(device) = &mut self.h7_exti {
            device.reset();
        }
        if let Some(device) = &mut self.f4_flash {
            device.reset();
        }
        if let Some(device) = &mut self.f4_power {
            device.reset();
        }
        if let Some(control) = &mut self.h7_network_control {
            if let Some(nic) = control.nic
                && let Controller::Stm32h7(device) = &mut self.ethernet[nic]
            {
                device.reset();
                device.set_clock(false);
            }
            *control = NetworkControl::new(control.rcc_base, control.syscfg_base, control.nic);
        }
        for device in &mut self.stm32_uarts {
            device.reset();
        }
        for device in &mut self.iwdgs {
            device.reset();
        }
        for device in &mut self.h7_lptimers {
            device.reset();
        }
        for device in &mut self.h7_timers {
            device.reset();
        }
        for device in &mut self.h7_gpio {
            device.reset();
        }
        for device in &mut self.h7_spi {
            device.reset();
        }
        for device in &mut self.f4_spi {
            device.reset();
        }
        for device in &mut self.h7_i2c {
            device.reset();
        }
        for device in &mut self.f1_i2c {
            device.reset();
        }
        for device in &mut self.h7_qspi {
            device.reset();
        }
        for device in &mut self.h7_rng {
            device.reset();
        }
        for device in &mut self.h7_crc {
            device.reset();
        }
        for device in &mut self.designware_usb {
            device.reset();
        }
        for device in &mut self.h7_dsi {
            device.reset();
        }
        for device in &mut self.h7_crypto {
            device.reset();
        }
        for device in &mut self.h7_sdmmc {
            device.reset();
        }
        if let Some(device) = &mut self.h7_hsem {
            device.reset();
        }
        for device in &mut self.h7_dma2d {
            device.reset();
        }
        if let Some(device) = &mut self.h7_ltdc {
            device.reset();
        }
        for device in &mut self.h7_adc {
            device.reset();
        }
        for device in &mut self.h7_adc_dma {
            device.reset();
        }
        for device in &mut self.h7_dma_mux {
            device.reset();
        }
        for device in &mut self.h7_bdma {
            device.reset();
        }
        for device in &mut self.stm32_can {
            device.reset();
        }
        if let Some(rcc) = &mut self.f4_rcc {
            rcc.watchdog_reset();
        }
        if let Some(system) = &mut self.f4_system {
            *system = F4System::default();
        }
        self.refresh_f4_clocks();
    }

    pub fn map_f4_rcc(&mut self, rcc: F4Rcc) -> Result<(), String> {
        self.check_peripheral_range(rcc.base, F4Rcc::SIZE)?;
        for address in F4System::REGISTERS {
            self.check_peripheral_range(address, 4)?;
            if u64::from(address) >= u64::from(rcc.base)
                && u64::from(address) < u64::from(rcc.base) + u64::from(F4Rcc::SIZE)
            {
                return Err("STM32F4 RCC overlaps a system register".into());
            }
        }
        self.f4_gated_ranges.extend(
            self.stm32_uarts
                .iter()
                .map(|uart| (uart.base, uart.size()))
                .chain(self.f4_power.iter().map(|pwr| (pwr.base, F4Power::SIZE)))
                .chain(self.f4_spi.iter().map(|spi| (spi.base, F4Spi::SIZE)))
                .chain(self.f1_i2c.iter().map(|i2c| (i2c.base, F1I2c::SIZE)))
                .chain(self.stm32_can.iter().map(|can| (can.base, StmCan::SIZE)))
                .chain(self.h7_gpio.iter().map(|gpio| (gpio.base, gpio.size)))
                .chain(
                    self.h7_timers
                        .iter()
                        .map(|timer| (timer.base, H7Timer::SIZE)),
                )
                .chain(self.h7_adc_dma.iter().map(|dma| (dma.base, AdcDma::SIZE)))
                .chain(self.h7_rtc.iter().map(|rtc| (rtc.base, H7Rtc::SIZE))),
        );
        self.f4_rcc = Some(rcc);
        self.f4_system = Some(F4System::default());
        self.refresh_f4_clocks();
        Ok(())
    }

    fn f4_access_blocked(&self, address: u32) -> bool {
        self.f4_rcc.as_ref().is_some_and(|rcc| {
            self.f4_gated_ranges
                .iter()
                .any(|&(base, size)| address.wrapping_sub(base) < size && !rcc.enabled(base))
        })
    }

    fn refresh_f4_clocks(&mut self) {
        let Some(rcc) = &self.f4_rcc else { return };
        if let Some(systick) = &mut self.systick {
            systick.set_frequency(rcc.hclk());
        }
        for timer in &mut self.h7_timers {
            if let Some(frequency) = rcc.timer_frequency(timer.base) {
                timer.set_frequency(frequency);
            }
        }
        if let Some(rtc) = self.h7_rtc.iter_mut().find(|rtc| rcc.rtc == Some(rtc.base))
            && let Some(frequency) = rcc.rtc_frequency()
        {
            rtc.set_frequency(frequency);
        }
    }

    fn write_f4_rcc(&mut self, address: u32, width: usize, value: u32) {
        let rcc = self.f4_rcc.as_mut().unwrap();
        let backup_unlocked = self.f4_power.as_ref().is_none_or(F4Power::backup_unlocked);
        rcc.write(address, width, value, backup_unlocked);
        if let Some(system) = &mut self.f4_system {
            if rcc.adc_in_reset() {
                system.reset_adc();
            }
            if rcc.syscfg_in_reset() {
                system.reset_syscfg();
            }
        }
        if rcc.timer9_in_reset()
            && let Some(timer) = self
                .h7_timers
                .iter_mut()
                .find(|timer| rcc.timer9 == Some(timer.base))
        {
            timer.reset();
        }
        if rcc.rtc_in_reset() {
            if let Some(rtc) = self.h7_rtc.iter_mut().find(|rtc| rcc.rtc == Some(rtc.base)) {
                rtc.reset();
            }
            if let Some(pwr) = &mut self.f4_power {
                pwr.reset_backup_regulator();
            }
        }
        self.refresh_f4_clocks();
    }

    pub fn map_f4_power(&mut self, base: u32) -> Result<(), String> {
        self.check_peripheral_range(base, F4Power::SIZE)?;
        if self.f4_power.is_some() {
            return Err("only one STM32F4 power controller is supported".into());
        }
        self.f4_power = Some(F4Power::new(base));
        Ok(())
    }

    pub fn map_f4_flash(&mut self, device: F4Flash) -> Result<(), String> {
        let [(base, size), (option_base, option_size)] = device.mappings();
        self.check_peripheral_range(base, size)?;
        self.check_peripheral_range(option_base, option_size)?;
        if self.f4_flash.is_some()
            || (u64::from(base) < u64::from(option_base) + u64::from(option_size)
                && u64::from(option_base) < u64::from(base) + u64::from(size))
        {
            return Err("duplicate or overlapping STM32F4 flash mapping".into());
        }
        self.f4_flash = Some(device);
        Ok(())
    }

    pub fn map_h7_flash(&mut self, device: FlashController) -> Result<(), String> {
        let start = u64::from(device.base);
        let end = start + u64::from(FlashController::SIZE);
        if self.h7_flash.is_some()
            || device.base & 3 != 0
            || end > 1 << 32
            || self.overlaps(start, end)
        {
            return Err("invalid or overlapping STM32H7 flash controller mapping".into());
        }
        self.h7_flash = Some(device);
        Ok(())
    }

    pub fn map_h7_adc(&mut self, device: Adc) -> Result<(), String> {
        self.check_peripheral_range(device.base, Adc::SIZE)?;
        self.h7_adc.push(device);
        Ok(())
    }

    pub fn map_h7_adc_dma(&mut self, device: AdcDma) -> Result<(), String> {
        self.check_peripheral_range(device.base, AdcDma::SIZE)?;
        self.h7_adc_dma.push(device);
        Ok(())
    }

    pub fn map_h7_dma_mux(&mut self, device: DmaMux) -> Result<(), String> {
        self.check_peripheral_range(device.base, DmaMux::SIZE)?;
        self.h7_dma_mux.push(device);
        Ok(())
    }

    pub fn map_h7_bdma(&mut self, device: Bdma) -> Result<(), String> {
        self.check_peripheral_range(device.base, Bdma::SIZE)?;
        self.h7_bdma.push(device);
        Ok(())
    }

    pub fn add_h7_gpio_button(&mut self, name: String, base: u32, pin: u8, inverted: bool) {
        self.h7_gpio_buttons.push((name, base, pin, inverted));
    }

    pub fn gpio_button(&mut self, name: &str, pressed: bool) -> Result<(), String> {
        let (_, base, pin, inverted) = self
            .h7_gpio_buttons
            .iter()
            .find(|(key, ..)| key == name)
            .ok_or_else(|| format!("unknown GPIO button {name:?}"))?;
        let port = self
            .h7_gpio
            .iter_mut()
            .find(|port| port.base == *base)
            .ok_or_else(|| format!("unmapped GPIO button {name:?}"))?;
        port.set_external(*pin, pressed ^ inverted);
        Ok(())
    }

    pub fn ltdc_frames_per_second(&mut self, fps: u32) -> Result<(), String> {
        self.h7_ltdc
            .as_mut()
            .ok_or("no LTDC configured")?
            .set_frame_rate(fps)
    }

    pub fn add_h7_gpio_led(&mut self, name: String, base: u32, pin: u8, inverted: bool) {
        self.h7_gpio_leds.push((name, base, pin, inverted));
    }

    pub fn h7_gpio_led_states(&self) -> impl Iterator<Item = (&str, bool)> {
        self.h7_gpio_leds
            .iter()
            .filter_map(|(name, base, pin, inverted)| {
                let port = self.h7_gpio.iter().find(|port| port.base == *base)?;
                Some((name.as_str(), (port.pins() & (1 << pin) != 0) ^ inverted))
            })
    }

    pub fn add_gpio_pwm_led(
        &mut self,
        name: String,
        base: u32,
        pin: u8,
        inverted: bool,
        routes: [Option<(u32, u8)>; 16],
    ) {
        self.gpio_pwm_leds.push(PwmLed {
            name,
            base,
            pin,
            inverted,
            routes,
        });
    }

    pub fn gpio_pwm_led_levels(&self) -> impl Iterator<Item = (&str, f64)> {
        self.gpio_pwm_leds.iter().filter_map(|led| {
            let port = self.h7_gpio.iter().find(|port| port.base == led.base)?;
            let level = if let Some(af) = port.alternate_function(led.pin) {
                led.routes[af]
                    .and_then(|(base, channel)| {
                        let timer = self.h7_timers.iter().find(|timer| {
                            timer.base == base
                                && timer_clocked(self.h7_network_control.as_ref(), timer)
                        })?;
                        timer.pwm_level(channel)
                    })
                    .unwrap_or(0.0)
            } else if port.pins() & (1 << led.pin) != 0 {
                1.0
            } else {
                0.0
            };
            Some((
                led.name.as_str(),
                if led.inverted { 1.0 - level } else { level },
            ))
        })
    }

    pub fn map_h7_gpio(&mut self, port: H7Gpio) -> Result<(), String> {
        self.check_peripheral_range(port.base, port.size)?;
        self.h7_gpio.push(port);
        Ok(())
    }

    pub fn set_gpio_exti_outputs(&mut self, outputs: [Option<u8>; 16]) {
        self.gpio_exti_outputs = outputs;
    }

    pub fn map_h7_rtc(&mut self, rtc: H7Rtc) -> Result<(), String> {
        self.check_peripheral_range(rtc.base, H7Rtc::SIZE)?;
        self.h7_rtc.push(rtc);
        Ok(())
    }

    pub fn map_h7_ltdc(&mut self, device: Ltdc) -> Result<(), String> {
        if self.h7_ltdc.is_some() {
            return Err("only one LTDC is supported".into());
        }
        self.check_peripheral_range(device.base, Ltdc::SIZE)?;
        self.h7_ltdc = Some(device);
        Ok(())
    }

    pub fn capture_ltdc(&self, path: &std::path::Path) -> Result<(), String> {
        let device = self.h7_ltdc.as_ref().ok_or("no LTDC framebuffer")?;
        if !ltdc_clocked(self.h7_network_control.as_ref(), device) {
            return Err("LTDC clock is disabled or LTDC is in reset".into());
        }
        device.capture(path, |address, width| {
            self.regions.iter().find_map(|region| {
                let offset = region.offset(u64::from(address), width)?;
                Some(&region.bytes[offset..offset + width])
            })
        })
    }

    pub fn map_designware_usb(&mut self, device: DesignWareUsb) -> Result<(), String> {
        self.check_peripheral_range(device.base, DesignWareUsb::SIZE)?;
        self.designware_usb.push(device);
        Ok(())
    }

    pub fn map_h7_dsi(&mut self, device: Dsi) -> Result<(), String> {
        self.check_peripheral_range(device.base, Dsi::SIZE)?;
        self.h7_dsi.push(device);
        Ok(())
    }

    pub fn map_h7_crypto(&mut self, device: Crypto) -> Result<(), String> {
        self.check_peripheral_range(device.base, Crypto::SIZE)?;
        self.h7_crypto.push(device);
        Ok(())
    }

    pub fn attach_sd_image(&mut self, image: Vec<u8>) -> Result<(), String> {
        match self.h7_sdmmc.as_mut_slice() {
            [controller] => controller.attach(image),
            _ => Err("--sd-image requires exactly one SDMMC controller".into()),
        }
    }

    pub(crate) fn attach_sd_file(
        &mut self,
        name: &str,
        path: &Path,
        capacity: u64,
    ) -> Result<(), String> {
        let name = name.strip_prefix("sysbus.").unwrap_or(name);
        if let Some(controller) = self
            .rv64_devices
            .mpfs_sd
            .iter_mut()
            .find(|sd| sd.name == name)
        {
            return controller
                .attach(path, capacity)
                .map_err(|error| format!("{}: {error}", path.display()));
        }
        if let Some(controller) = self.h7_sdmmc.iter_mut().find(|sd| sd.name == name) {
            return controller.attach_file(path, capacity);
        }
        let file = std::fs::File::open(path).map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                format!("Could not find image file '{}'", path.display())
            } else {
                format!("{}: {error}", path.display())
            }
        })?;
        let card =
            Card::new(file, capacity).map_err(|error| format!("{}: {error}", path.display()))?;
        self.attach_litex_sd(name, card)
    }

    pub fn map_h7_sdmmc(&mut self, device: Sdmmc) -> Result<(), String> {
        self.check_peripheral_range(device.base, Sdmmc::SIZE)?;
        self.h7_sdmmc.push(device);
        Ok(())
    }

    pub fn map_h7_hsem(&mut self, device: Hsem) -> Result<(), String> {
        if self.h7_hsem.is_some() {
            return Err("only one STM32H7 HSEM is supported".into());
        }
        self.check_peripheral_range(device.base, Hsem::SIZE)?;
        self.h7_hsem = Some(device);
        Ok(())
    }

    pub fn map_h7_dma2d(&mut self, device: Dma2d) -> Result<(), String> {
        self.check_peripheral_range(device.base, Dma2d::SIZE)?;
        self.h7_dma2d.push(device);
        Ok(())
    }

    pub fn map_f4_can(&mut self, can: StmCan) -> Result<(), String> {
        self.check_peripheral_range(can.base, StmCan::SIZE)?;
        self.stm32_can.push(can);
        Ok(())
    }

    fn can_filters(&self, index: usize) -> &Filters {
        let device = &self.stm32_can[index];
        let owner = match device.master {
            Some(base) => self
                .stm32_can
                .iter()
                .find(|candidate| candidate.base == base)
                .unwrap(),
            None => device,
        };
        owner.filters.as_ref().unwrap()
    }

    fn write_stm32_can(&mut self, index: usize, address: u32, value: u32) {
        let device = &mut self.stm32_can[index];
        let offset = address - device.base;
        if offset >= 0x200 {
            if let Some(filters) = &mut device.filters {
                filters.write(offset, value);
            }
            return;
        }
        device.write(offset, value);
        let slave = device.master.is_some();
        for frame in device.transmit().into_iter().flatten() {
            if let Some((fifo, filter)) = self.can_filters(index).accept(slave, frame[0]) {
                self.stm32_can[index].receive(fifo, filter, frame);
            }
        }
    }

    pub fn map_h7_fdcan(&mut self, fdcan: Fdcan) -> Result<(), String> {
        self.check_peripheral_range(fdcan.base, Fdcan::SIZE)?;
        self.h7_fdcan.push(fdcan);
        Ok(())
    }

    pub fn map_h7_crc(&mut self, crc: Stm32Crc) -> Result<(), String> {
        self.check_peripheral_range(crc.base, Stm32Crc::SIZE)?;
        self.h7_crc.push(crc);
        Ok(())
    }

    pub fn map_h7_lptim(&mut self, timer: LpTimer) -> Result<(), String> {
        self.check_peripheral_range(timer.base, LpTimer::SIZE)?;
        self.h7_lptimers.push(timer);
        Ok(())
    }

    pub fn map_h7_timer(&mut self, timer: H7Timer) -> Result<(), String> {
        self.check_peripheral_range(timer.base, H7Timer::SIZE)?;
        self.h7_timers.push(timer);
        Ok(())
    }

    pub fn map_h7_qspi(&mut self, qspi: Qspi) -> Result<(), String> {
        self.check_peripheral_range(qspi.base, Qspi::SIZE)?;
        self.h7_qspi.push(qspi);
        Ok(())
    }

    pub fn map_h7_rng(&mut self, rng: Rng) -> Result<(), String> {
        self.check_peripheral_range(rng.base, Rng::SIZE)?;
        self.h7_rng.push(rng);
        Ok(())
    }

    pub fn map_f1_i2c(&mut self, i2c: F1I2c) -> Result<(), String> {
        self.check_peripheral_range(i2c.base, F1I2c::SIZE)?;
        self.f1_i2c.push(i2c);
        Ok(())
    }

    pub fn map_h7_i2c(&mut self, i2c: I2c) -> Result<(), String> {
        self.check_peripheral_range(i2c.base, I2c::SIZE)?;
        self.h7_i2c.push(i2c);
        Ok(())
    }

    pub fn map_f4_spi(&mut self, spi: F4Spi) -> Result<(), String> {
        self.check_peripheral_range(spi.base, F4Spi::SIZE)?;
        self.f4_spi.push(spi);
        Ok(())
    }

    pub fn map_h7_spi(&mut self, spi: Spi) -> Result<(), String> {
        self.check_peripheral_range(spi.base, Spi::SIZE)?;
        self.h7_spi.push(spi);
        Ok(())
    }

    fn check_peripheral_range(&self, base: u32, size: u32) -> Result<(), String> {
        let start = u64::from(base);
        let end = start + u64::from(size);
        if base & 3 != 0 || end > 1 << 32 || self.overlaps(start, end) {
            return Err(format!("invalid or overlapping peripheral at {base:#010x}"));
        }
        Ok(())
    }

    pub fn map_gpio_led(&mut self, name: String, gpio: u32, pin: u8) {
        self.f4_gpio_leds.push((name, gpio, pin));
    }

    #[cfg(test)]
    pub fn f4_gpio_led(&self, name: &str) -> Option<bool> {
        let (_, gpio, pin) = self.f4_gpio_leds.iter().find(|(led, _, _)| led == name)?;
        self.h7_gpio
            .iter()
            .find(|port| port.base == *gpio)
            .map(|port| port.pins() & (1 << pin) != 0)
    }

    pub fn set_gpio_links(&mut self, pins: Vec<TimerPin>, wires: Vec<GpioWire>) {
        self.timer_pins = pins;
        self.gpio_wires = wires;
    }

    fn sample_timer_pins(&mut self) {
        if self.timer_pins.is_empty() && self.gpio_wires.is_empty() {
            return;
        }
        for gpio in &mut self.h7_gpio {
            gpio.clear_timer_output();
            gpio.clear_wired_input();
        }
        let clocked = |timer: &H7Timer| {
            timer_clocked(self.h7_network_control.as_ref(), timer)
                && f4_clocked(self.f4_rcc.as_ref(), timer.base)
        };
        for route in &self.timer_pins {
            if let Some(timer) = self
                .h7_timers
                .iter()
                .find(|timer| timer.base == route.timer && clocked(timer))
                && let Some(high) = timer.output(route.channel)
                && let Some(gpio) = self.h7_gpio.iter_mut().find(|gpio| gpio.base == route.gpio)
                && gpio.alternate_selected(route.pin, route.function)
            {
                gpio.drive_timer_output(route.pin, high);
            }
        }
        for wire in &mut self.gpio_wires {
            let (base, pin) = wire.source;
            wire.level = self
                .h7_gpio
                .iter()
                .any(|gpio| gpio.base == base && gpio.pins() & (1 << pin) != 0);
        }
        for wire in self.gpio_wires.iter().filter(|wire| wire.level) {
            let (base, pin) = wire.target;
            if let Some(gpio) = self.h7_gpio.iter_mut().find(|gpio| gpio.base == base) {
                gpio.set_wired_input(pin);
            }
        }
        for timer in self.h7_timers.iter_mut().filter(|timer| clocked(timer)) {
            let levels = std::array::from_fn(|channel| {
                self.timer_pins
                    .iter()
                    .filter(|wire| {
                        wire.capture && wire.timer == timer.base && wire.channel == channel
                    })
                    .any(|wire| {
                        self.h7_gpio.iter().any(|gpio| {
                            gpio.base == wire.gpio
                                && gpio.alternate_selected(wire.pin, wire.function)
                                && gpio.pins() & (1 << wire.pin) != 0
                        })
                    })
            });
            timer.sample_inputs(levels);
        }
    }

    pub fn tick_h7_peripherals(&mut self) {
        self.h7_step = self.h7_step.wrapping_add(1);
        let step = self.h7_step;
        for i2c in &mut self.h7_i2c {
            for (base, pin, level) in i2c.sample_magnetometers(step) {
                if let Some(port) = self.h7_gpio.iter_mut().find(|port| port.base == base) {
                    port.set_external(pin, level);
                }
            }
        }
        self.sample_timer_pins();
        let rate = self.step_hz();
        let control = self.h7_network_control.as_ref();
        if let Some(display) = &mut self.h7_ltdc
            && ltdc_clocked(control, display)
        {
            display.tick(step);
        }
        let mut dma = EthernetDma {
            regions: &mut self.regions,
            reservations: &mut self.reservations,
        };
        for timer in &mut self.h7_lptimers {
            if lptim_clocked(control, timer) {
                timer.tick();
            }
        }
        for timer in &mut self.h7_timers {
            if timer_clocked(control, timer) && f4_clocked(self.f4_rcc.as_ref(), timer.base) {
                timer.tick(rate);
            }
        }
        for led in &self.gpio_pwm_leds {
            let Some(port) = self.h7_gpio.iter_mut().find(|port| port.base == led.base) else {
                continue;
            };
            let level = port
                .alternate_function(led.pin)
                .and_then(|af| led.routes[af])
                .and_then(|(base, channel)| {
                    let timer = self
                        .h7_timers
                        .iter()
                        .find(|timer| timer.base == base && timer_clocked(control, timer))?;
                    timer.pwm_pin(channel)
                });
            port.drive_alternate(led.pin, level);
        }
        for rtc in &mut self.h7_rtc {
            if rtc_clocked(control, rtc) && f4_clocked(self.f4_rcc.as_ref(), rtc.base) {
                rtc.tick(rate);
            }
        }
        for controller in &mut self.h7_bdma {
            if bdma_clocked(control, controller) {
                controller.tick(&mut dma);
            }
        }
        for adc in &mut self.h7_adc {
            if !adc_clocked(control, adc) {
                continue;
            }
            adc.tick(rate);
            let Some((request, value)) = adc.take_request() else {
                continue;
            };
            let source = adc.base + Adc::DATA;
            let mut transferred = false;
            if let Some(mux) = self
                .h7_dma_mux
                .iter()
                .find(|mux| mux.base == request.endpoint)
            {
                for target in mux.targets(request.id) {
                    transferred |= request_h7_stream(
                        target,
                        source,
                        value,
                        control,
                        &mut self.h7_adc_dma,
                        &mut self.h7_bdma,
                        &mut dma,
                    );
                }
            } else if let Some(controller) = self
                .h7_bdma
                .iter_mut()
                .find(|dma| dma.base == request.endpoint)
                && bdma_clocked(control, controller)
            {
                transferred =
                    controller.request(usize::from(request.id - 1), source, value, &mut dma);
            }
            if transferred {
                adc.data_read();
            }
        }
    }

    pub fn map_bitband(&mut self, base: u32, size: u32, target: u32) -> Result<(), String> {
        if !matches!(target, 0x2000_0000 | 0x4000_0000)
            || base != target + 0x0200_0000
            || size == 0
            || size > 0x0200_0000
            || size % 32 != 0
        {
            return Err("invalid Cortex-M bit-band aperture".into());
        }
        self.check_peripheral_range(base, size)?;
        self.bitbands.push((base, size, target));
        Ok(())
    }

    fn bitband(&self, address: u32) -> Option<(u32, u32)> {
        self.bitbands.iter().find_map(|&(base, size, target)| {
            let offset = address.wrapping_sub(base);
            (offset < size).then_some((target + (offset / 128) * 4, 1 << ((offset / 4) & 31)))
        })
    }

    pub fn map_bus_tag(&mut self, start: u32, end: u32, value: u32) -> Result<(), String> {
        if start > end {
            return Err("invalid sysbus Tag range".into());
        }
        self.bus_tags.push((start, end, value));
        Ok(())
    }

    fn bus_tag(&self, address: u32, width: usize) -> Option<u32> {
        let last = address.checked_add(width as u32 - 1)?;
        let matching = self
            .bus_tags
            .iter()
            .filter(|&&(start, end, _)| address >= start && last <= end);
        let tag = if self.f4_rcc.is_some() {
            matching.into_iter().next()
        } else {
            matching.into_iter().next_back()
        };
        tag.map(|&(_, _, value)| value)
    }

    pub fn map_ram_alias(&mut self, base: u32, alias: u32, size: usize) -> Result<(), String> {
        let start = u64::from(alias);
        let end = start.saturating_add(size as u64);
        if end > 1 << 32 || self.overlaps(start, end) {
            return Err(format!("invalid or overlapping RAM alias at {alias:#010x}"));
        }
        let region = self
            .regions
            .iter_mut()
            .find(|region| region.base == u64::from(base) && region.mapped == size)
            .ok_or("RAM alias has no matching source mapping")?;
        region.aliases.push(alias);
        Ok(())
    }

    pub fn add_nrf_tag(&mut self, start: u32, end: u32, value: u32) {
        self.nrf_tags.push((start, end, value));
    }

    fn nrf_tag(&self, address: u32, width: usize) -> Option<u32> {
        let last = address.checked_add(width as u32 - 1)?;
        self.nrf_tags
            .iter()
            .rev()
            .find_map(|&(start, end, value)| (start <= address && last <= end).then_some(value))
    }

    pub fn map_nrf_peripheral(&mut self, device: NrfPeripheral) -> Result<(), String> {
        let start = u64::from(device.base);
        let end = start + u64::from(device.size());
        if device.base & 3 != 0 || end > 1 << 32 || self.overlaps(start, end) {
            return Err("invalid or overlapping nRF52840 peripheral mapping".into());
        }
        self.nrf_peripherals.push(device);
        Ok(())
    }

    pub fn nrf_gpio_sample(&self, base: u32) -> (u32, bool) {
        self.nrf_peripherals
            .iter()
            .find(|device| device.base == base)
            .and_then(NrfPeripheral::gpio_sample)
            .unwrap_or_default()
    }

    pub fn nrf_gpio_driven_high(&self, base: u32) -> u32 {
        self.nrf_peripherals
            .iter()
            .find(|device| device.base == base)
            .and_then(NrfPeripheral::gpio_driven_high)
            .unwrap_or_default()
    }

    pub fn nrf_gpio_task(&mut self, base: u32, pin: u32, action: u32) {
        if let Some(device) = self
            .nrf_peripherals
            .iter_mut()
            .find(|device| device.base == base)
        {
            device.gpio_task(pin, action);
            let high = device.gpio_driven_high().unwrap_or_default();
            self.release_nrf_spi_cs(base, high);
        }
    }

    pub fn release_nrf_spi_cs(&mut self, gpio: u32, pins: u32) {
        for device in &mut self.nrf_peripherals {
            device.release_spi_cs(gpio, pins);
        }
    }

    pub fn nrf_ppi_event_supported(&self, address: u32) -> bool {
        self.nrf_peripherals
            .iter()
            .any(|device| device.ppi_event_supported(address))
    }

    pub fn nrf_ppi_task_supported(&self, address: u32) -> bool {
        self.nrf_peripherals
            .iter()
            .any(|device| device.ppi_task_supported(address))
    }

    pub fn set_nrf_pdm_input(&mut self, input: Vec<u8>) -> Result<(), String> {
        if input.len() % 2 != 0 {
            return Err("PDM input must contain whole signed 16-bit little-endian samples".into());
        }
        let mut devices = self
            .nrf_peripherals
            .iter_mut()
            .filter(|device| device.is_pdm());
        let device = devices
            .next()
            .ok_or("--pdm-input requires an nRF52840 PDM peripheral")?;
        if devices.next().is_some() {
            return Err("--pdm-input requires exactly one PDM peripheral".into());
        }
        device.set_pdm_input(input);
        Ok(())
    }

    pub fn configure_nrf_i2s_pcm(&mut self, input: Vec<u8>, capture: bool) -> Result<(), String> {
        let mut devices = self
            .nrf_peripherals
            .iter_mut()
            .filter(|device| device.is_i2s());
        let device = devices
            .next()
            .ok_or("I2S PCM requires an nRF52840 I2S peripheral")?;
        if devices.next().is_some() {
            return Err("I2S PCM requires exactly one I2S peripheral".into());
        }
        device.configure_i2s_pcm(input, capture);
        Ok(())
    }

    pub fn nrf_i2s_pcm_output(&self) -> Option<&[u8]> {
        self.nrf_peripherals
            .iter()
            .find_map(NrfPeripheral::i2s_pcm_output)
    }

    pub fn advance_nrf_audio(&mut self) -> Result<(), BusError> {
        for index in 0..self.nrf_peripherals.len() {
            if self.nrf_peripherals[index].audio_transfer_due() {
                let mut device = self.nrf_peripherals.remove(index);
                let result = device.complete_audio(self);
                self.nrf_peripherals.insert(index, device);
                result?;
            }
        }
        Ok(())
    }

    pub fn attach_radio_medium(&mut self) -> Result<(), String> {
        let mut radios = self
            .nrf_peripherals
            .iter_mut()
            .filter(|device| device.is_radio());
        let radio = radios
            .next()
            .ok_or("--ble-peer requires an nRF52840 RADIO")?;
        if radios.next().is_some() {
            return Err("--ble-peer requires exactly one RADIO per node".into());
        }
        radio.attach_radio_medium();
        Ok(())
    }

    pub fn tick_radio_air(&mut self) -> Option<crate::nrf52840::AirFrame> {
        self.nrf_peripherals
            .iter_mut()
            .find_map(NrfPeripheral::tick_radio_air)
    }

    pub fn receive_radio_air(&mut self, frame: crate::nrf52840::AirFrame) -> Result<(), BusError> {
        let Some(index) = self
            .nrf_peripherals
            .iter()
            .position(NrfPeripheral::is_radio)
        else {
            return Ok(());
        };
        let mut radio = self.nrf_peripherals.remove(index);
        let result = radio.receive_radio_air(frame, self);
        self.nrf_peripherals.insert(index, radio);
        result
    }

    pub fn advance_nrf_ppi(&mut self, output: &mut impl Write) -> Result<(), BusError> {
        let mut events = Vec::new();
        for device in &mut self.nrf_peripherals {
            device.take_raised_events(&mut events);
        }
        let Some(ppi) = self.nrf_peripherals.iter().find(|device| device.is_ppi()) else {
            return Ok(());
        };
        let mut tasks = Vec::new();
        for event in events {
            ppi.ppi_tasks(event, &mut tasks);
        }
        for address in tasks {
            self.write(address, 4, 1, output)?;
        }
        Ok(())
    }

    pub fn has_nrf_uart(&self, name: &str) -> bool {
        self.nrf_peripherals
            .iter()
            .any(|device| device.is_uart() && device.name == name)
    }

    pub(crate) fn memory_slice(&self, address: u32, length: usize) -> Result<&[u8], BusError> {
        self.regions
            .iter()
            .find_map(|region| {
                let offset = region.offset(u64::from(address), length)?;
                Some(&region.bytes[offset..offset + length])
            })
            .ok_or(BusError::Unmapped(address, length))
    }

    pub(crate) fn overlaps(&self, start: u64, end: u64) -> bool {
        if self
            .bitbands
            .iter()
            .any(|&(base, size, _)| overlaps_range(start, end, base, size))
        {
            return true;
        }

        self.nrf_peripherals
            .iter()
            .any(|device| overlaps_range(start, end, device.base, device.size()))
            || self.h7_network_control.as_ref().is_some_and(|control| {
                control
                    .ranges()
                    .any(|(base, limit)| start < limit && base < end)
            })
            || (self.nvic.is_some() && Nvic::overlaps(start, end))
            || self.dwt.as_ref().is_some_and(|device| {
                device
                    .ranges()
                    .into_iter()
                    .any(|(base, limit)| start < limit && base < end)
            })
            || (self.systick.is_some() && SysTick::overlaps(start, end))
            || self
                .uart_tx
                .is_some_and(|address| (start..end).contains(&u64::from(address)))
            || self.uart.as_ref().is_some_and(|uart| {
                let other = u64::from(uart.base);
                start < other + MuraxUart::SIZE && other < end
            })
            || self.litex_uart.as_ref().is_some_and(|uart| {
                uart.bases
                    .iter()
                    .any(|&base| overlaps_range(start, end, base, LiteXUart::SIZE))
            })
            || self.stm32_cans.iter().any(|can| {
                let other = u64::from(can.base);
                start < other + u64::from(can.size) && other < end
            })
            || self.stm32_i2cs.iter().any(|i2c| {
                let other = u64::from(i2c.base);
                start < other + u64::from(Stm32I2c::SIZE) && other < end
            })
            || self.stm32_spis.iter().any(|spi| {
                let other = u64::from(spi.base);
                start < other + u64::from(Stm32F0Spi::SIZE) && other < end
            })
            || self.stm32_timers.iter().any(|timer| {
                let other = u64::from(timer.base);
                start < other + u64::from(Stm32Timer::SIZE) && other < end
            })
            || self.stm32_gpios.iter().any(|gpio| {
                let other = u64::from(gpio.base);
                start < other + u64::from(gpio.size) && other < end
            })
            || self.stm32_dma.as_ref().is_some_and(|dma| {
                let other = u64::from(dma.base);
                start < other + u64::from(Stm32Dma::SIZE) && other < end
            })
            || self.stm32_adc.as_ref().is_some_and(|adc| {
                let other = u64::from(adc.base);
                start < other + u64::from(Stm32F0Adc::SIZE) && other < end
            })
            || self.stm32_crc.as_ref().is_some_and(|crc| {
                let other = u64::from(crc.base);
                start < other + u64::from(Stm32F0Crc::SIZE) && other < end
            })
            || self.stm32_rtc.as_ref().is_some_and(|rtc| {
                let other = u64::from(rtc.base);
                start < other + u64::from(Stm32Rtc::SIZE) && other < end
            })
            || self.stm32_flash.as_ref().is_some_and(|flash| {
                let other = u64::from(flash.base);
                let options = u64::from(Stm32F0Flash::OPTION_BASE);
                (start < other + u64::from(Stm32F0Flash::SIZE) && other < end)
                    || (start < options + u64::from(Stm32F0Flash::OPTION_SIZE) && options < end)
            })
            || self.stm32_rcc.as_ref().is_some_and(|rcc| {
                let other = u64::from(rcc.base);
                start < other + u64::from(Stm32F0Rcc::SIZE) && other < end
            })
            || self.litex_timer.as_ref().is_some_and(|device| {
                device
                    .bases
                    .iter()
                    .any(|&base| overlaps_range(start, end, base, device.timer.size()))
            })
            || self
                .litex_cpu_timer
                .as_ref()
                .is_some_and(|timer| overlaps_range(start, end, timer.base, litex::CpuTimer::SIZE))
            || self.litex_eth.as_ref().is_some_and(|eth| {
                eth.registrations.iter().any(|registration| {
                    overlaps_range(start, end, registration.base, registration.size)
                })
            })
            || self.litex_flash.as_ref().is_some_and(|(bases, _, _)| {
                bases
                    .iter()
                    .any(|&base| overlaps_range(start, end, base, crate::litex_flash::Flash::SIZE))
            })
            || self.litex_panel.as_ref().is_some_and(|panel| {
                panel
                    .bases
                    .iter()
                    .any(|&base| overlaps_range(start, end, base, Panel::SIZE))
            })
            || self.litex_i2s.iter().any(|i2s| {
                overlaps_range(start, end, i2s.csr, I2s::SIZE)
                    || overlaps_range(start, end, i2s.buffer, i2s.buffer_size)
            })
            || self.litex_sd.as_ref().is_some_and(|sd| {
                sd.registrations.iter().any(|registration| {
                    overlaps_range(start, end, registration.base, registration.size)
                })
            })
            || self.litex_video.as_ref().is_some_and(|video| {
                overlaps_range(start, end, video.dma, Framebuffer::DMA_SIZE)
                    || overlaps_range(start, end, video.vtg, Framebuffer::VTG_SIZE)
            })
            || self.litex_io.iter().any(|(bases, device)| {
                bases
                    .iter()
                    .any(|&base| overlaps_range(start, end, base, device.size()))
            })
            || self
                .litex_soc
                .as_ref()
                .is_some_and(|soc| overlaps_range(start, end, soc.base, soc.size()))
            || (self.f4_system.is_some()
                && F4System::REGISTERS.into_iter().any(|address| {
                    let base = u64::from(address);
                    start < base + 4 && base < end
                }))
            || self.f4_rcc.as_ref().is_some_and(|device| {
                let base = u64::from(device.base);
                start < base + u64::from(F4Rcc::SIZE) && base < end
            })
            || self.f4_power.as_ref().is_some_and(|device| {
                let base = u64::from(device.base);
                start < base + u64::from(F4Power::SIZE) && base < end
            })
            || self.f4_flash.as_ref().is_some_and(|device| {
                device.mappings().into_iter().any(|(base, size)| {
                    let base = u64::from(base);
                    start < base + u64::from(size) && base < end
                })
            })
            || self.h7_flash.as_ref().is_some_and(|device| {
                let base = u64::from(device.base);
                start < base + u64::from(FlashController::SIZE) && base < end
            })
            || self
                .h7_adc
                .iter()
                .map(|device| (device.base, Adc::SIZE))
                .chain(
                    self.h7_adc_dma
                        .iter()
                        .map(|device| (device.base, AdcDma::SIZE)),
                )
                .chain(
                    self.h7_dma_mux
                        .iter()
                        .map(|device| (device.base, DmaMux::SIZE)),
                )
                .chain(self.h7_bdma.iter().map(|device| (device.base, Bdma::SIZE)))
                .chain(self.h7_gpio.iter().map(|port| (port.base, port.size)))
                .chain(self.h7_rtc.iter().map(|rtc| (rtc.base, H7Rtc::SIZE)))
                .chain(self.f4_spi.iter().map(|spi| (spi.base, F4Spi::SIZE)))
                .chain(self.h7_spi.iter().map(|spi| (spi.base, Spi::SIZE)))
                .chain(self.f1_i2c.iter().map(|i2c| (i2c.base, F1I2c::SIZE)))
                .chain(self.h7_i2c.iter().map(|i2c| (i2c.base, I2c::SIZE)))
                .chain(self.h7_rng.iter().map(|rng| (rng.base, Rng::SIZE)))
                .chain(self.h7_qspi.iter().map(|qspi| (qspi.base, Qspi::SIZE)))
                .chain(self.iwdgs.iter().map(|device| (device.base, Iwdg::SIZE)))
                .chain(
                    self.h7_lptimers
                        .iter()
                        .map(|timer| (timer.base, LpTimer::SIZE)),
                )
                .chain(
                    self.h7_timers
                        .iter()
                        .map(|timer| (timer.base, H7Timer::SIZE)),
                )
                .chain(self.h7_crc.iter().map(|crc| (crc.base, Stm32Crc::SIZE)))
                .chain(self.stm32_can.iter().map(|can| (can.base, StmCan::SIZE)))
                .chain(self.h7_fdcan.iter().map(|fdcan| (fdcan.base, Fdcan::SIZE)))
                .chain(self.h7_ltdc.iter().map(|device| (device.base, Ltdc::SIZE)))
                .chain(
                    self.h7_sdmmc
                        .iter()
                        .map(|device| (device.base, Sdmmc::SIZE)),
                )
                .chain(
                    self.designware_usb
                        .iter()
                        .map(|usb| (usb.base, DesignWareUsb::SIZE)),
                )
                .chain(
                    self.h7_crypto
                        .iter()
                        .map(|device| (device.base, Crypto::SIZE)),
                )
                .chain(self.h7_dsi.iter().map(|device| (device.base, Dsi::SIZE)))
                .chain(self.h7_hsem.iter().map(|device| (device.base, Hsem::SIZE)))
                .chain(
                    self.h7_dma2d
                        .iter()
                        .map(|device| (device.base, Dma2d::SIZE)),
                )
                .any(|(base, size)| {
                    let base = u64::from(base);
                    start < base + u64::from(size) && base < end
                })
            || self.h7_exti.as_ref().is_some_and(|exti| {
                let other = u64::from(exti.base);
                start < other + u64::from(H7Exti::SIZE) && other < end
            })
            || self.exti.as_ref().is_some_and(|exti| {
                let other = u64::from(exti.base);
                start < other + u64::from(Exti::SIZE) && other < end
            })
            || self.stm32_uarts.iter().any(|uart| {
                let other = u64::from(uart.base);
                start < other + u64::from(uart.size()) && other < end
            })
            || self.ns16550.as_ref().is_some_and(|uart| {
                let other = u64::from(uart.base);
                start < other + uart.size() && other < end
            })
            || self.plic.as_ref().is_some_and(|plic| {
                let other = u64::from(plic.base);
                start < other + Plic::SIZE && other < end
            })
            || self.clint.as_ref().is_some_and(|clint| {
                let other = u64::from(clint.base);
                start < other + Clint::SIZE && other < end
            })
            || [
                (self.gpio.as_ref().map(|(base, _)| *base), 0xc),
                (self.timer.as_ref().map(|(base, _)| *base), 0x5c),
            ]
            .into_iter()
            .any(|(base, size)| {
                base.is_some_and(|base| {
                    let other = u64::from(base);
                    start < other + size && other < end
                })
            })
            || self
                .silencers
                .iter()
                .any(|&(base, size)| overlaps_range(start, end, base, size))
            || self
                .ethernet
                .iter()
                .flat_map(|device| device.mappings())
                .any(|mapping| {
                    let base = u64::from(mapping.base);
                    start < base + u64::from(mapping.size) && base < end
                })
            || self.constant_mmio.iter().any(|peripheral| {
                let base = u64::from(peripheral.base);
                start < base + u64::from(peripheral.size) && base < end
            })
            || self.regions.iter().any(|region| {
                let other = region.base;
                (start < other + region.mapped as u64 && other < end)
                    || region.aliases.iter().any(|&base| {
                        let other = u64::from(base);
                        start < other + region.mapped as u64 && other < end
                    })
            })
            || self
                .aliases
                .iter()
                .any(|alias| start < alias.base + alias.length as u64 && alias.base < end)
    }

    pub fn map_clint(&mut self, base: u32, targets: usize) -> Result<(), String> {
        self.map_clint_with_frequency(base, targets, Clint::STEP_HZ)
    }

    pub fn map_clint_with_frequency(
        &mut self,
        base: u32,
        targets: usize,
        frequency: u32,
    ) -> Result<(), String> {
        let start = u64::from(base);
        let end = start + Clint::SIZE;
        if base & 3 != 0 || end > 1 << 32 || self.clint.is_some() || self.overlaps(start, end) {
            return Err(format!(
                "invalid or overlapping CLINT range at {base:#010x}"
            ));
        }
        self.clint = Some(Clint::new(base, targets, frequency));
        Ok(())
    }

    pub fn map_uart(&mut self, base: u32, irq_connected: bool) -> Result<(), String> {
        let start = u64::from(base);
        let end = start + MuraxUart::SIZE;
        if base & 3 != 0 || end > 1 << 32 || self.uart.is_some() || self.overlaps(start, end) {
            return Err(format!("invalid or overlapping UART range at {base:#010x}"));
        }
        self.uart = Some(MuraxUart::new(base, irq_connected));
        Ok(())
    }

    pub fn map_plic(
        &mut self,
        base: u32,
        sources: u32,
        outputs: &[u32],
        priorities: bool,
    ) -> Result<(), String> {
        if !(1..=63).contains(&sources) {
            return Err("PLIC requires 1–63 interrupt sources".into());
        }
        let start = u64::from(base);
        let end = start + Plic::SIZE;
        if base & 3 != 0 || end > 1 << 32 || self.plic.is_some() || self.overlaps(start, end) {
            return Err(format!("invalid or overlapping PLIC range at {base:#010x}"));
        }
        self.plic = Some(Plic::new(base, sources, outputs).with_priorities(priorities));
        Ok(())
    }

    pub fn map_ns16550(
        &mut self,
        base: u32,
        source: Option<u32>,
        wide: bool,
    ) -> Result<(), String> {
        let uart = Ns16550::new(base, source, wide);
        let start = u64::from(base);
        let end = start + uart.size();
        if (wide && base & 3 != 0)
            || end > 1 << 32
            || self.uart.is_some()
            || self.ns16550.is_some()
            || self.overlaps(start, end)
        {
            return Err(format!(
                "invalid or overlapping NS16550 range at {base:#010x}"
            ));
        }
        self.ns16550 = Some(uart);
        Ok(())
    }

    pub fn map_nrf_nvic(
        &mut self,
        mask: u8,
        uart: Option<u8>,
        cortex_m4: bool,
    ) -> Result<(), String> {
        self.map_nvic(mask, uart)?;
        if cortex_m4 {
            self.nvic.as_mut().unwrap().enable_nrf_mpu();
        }
        Ok(())
    }

    pub fn map_nvic(&mut self, priority_mask: u8, uart_irq: Option<u8>) -> Result<(), String> {
        if Nvic::RANGES
            .into_iter()
            .any(|(start, end)| self.overlaps(u64::from(start), u64::from(end)))
        {
            return Err("Cortex-M NVIC registers overlap an existing mapping".into());
        }
        self.nvic = Some(Nvic::new(priority_mask, uart_irq));
        Ok(())
    }

    pub fn set_nvic_name(&mut self, name: String) {
        self.nvic_name = Some(name);
    }

    pub fn drive_nvic_line(&mut self, target: &str, line: u8, high: bool) -> Result<(), String> {
        self.validate_nvic_name(target)?;
        let mask = 1u32 << line;
        if high {
            self.external_nvic_levels |= mask;
            // Latch a pulse even when both commands occur before the next CPU step.
            self.nvic
                .as_mut()
                .unwrap()
                .set_line(line, true, &ActiveExceptions::default());
        } else {
            self.external_nvic_levels &= !mask;
        }
        Ok(())
    }

    fn validate_nvic_name(&self, target: &str) -> Result<(), String> {
        let name = target.strip_prefix("sysbus.").unwrap_or(target);
        if self.nvic.is_none() || self.nvic_name.as_deref() != Some(name) {
            return Err(format!("unknown Cortex-M NVIC {target:?}"));
        }
        Ok(())
    }

    pub fn set_nvic_clocked(&mut self, target: &str, clocked: bool) -> Result<(), String> {
        self.validate_nvic_name(target)?;
        self.systick
            .as_mut()
            .ok_or("NVIC Clocked requires a SysTick counter")?
            .set_clocked(clocked);
        Ok(())
    }

    pub fn advance_stm32_dma(&mut self, output: &mut impl Write) -> Result<(), BusError> {
        let Some(dma) = self
            .stm32_dma
            .as_ref()
            .filter(|dma| dma.has_enabled_channels())
        else {
            return Ok(());
        };
        let mut requests = self.stm32_adc.as_ref().map_or(0, Stm32F0Adc::pending_dma);
        for uart in &self.stm32_uarts {
            requests |= uart.pending_dma();
        }
        for spi in &self.stm32_spis {
            requests |= spi.pending_dma();
        }
        for i2c in &self.stm32_i2cs {
            requests |= i2c.pending_dma();
        }
        let Some(transfer) = dma.next_transfer(requests) else {
            return Ok(());
        };
        let value = match transfer.read_width {
            1 => self.read::<1>(transfer.from),
            2 => self.read::<2>(transfer.from),
            4 => self.read::<4>(transfer.from),
            _ => Err(BusError::Unmapped(transfer.from, transfer.read_width)),
        };
        let result = value.and_then(|value| match transfer.write_width {
            width @ (1 | 2 | 4) => self.write(transfer.to, width, value, output),
            width => Err(BusError::Unmapped(transfer.to, width)),
        });
        self.stm32_dma
            .as_mut()
            .unwrap()
            .complete(transfer, result.is_ok());
        match result {
            Err(error @ BusError::Output(_)) => Err(error),
            _ => Ok(()),
        }
    }

    pub fn reset_cortex_m_if_requested(&mut self, scheduled: bool) -> bool {
        let system = self.nvic.as_mut().is_some_and(Nvic::take_reset_request);
        let reload = self
            .stm32_flash
            .as_mut()
            .is_some_and(Stm32F0Flash::take_reload_request);
        if !scheduled
            && !system
            && !reload
            && !self
                .nrf_peripherals
                .iter()
                .any(NrfPeripheral::reset_requested)
        {
            return false;
        }
        self.reservations.fill(None);
        for device in &mut self.nrf_peripherals {
            device.reset();
        }
        if let Some(nvic) = self.nvic.as_mut() {
            nvic.reset();
        }
        if let Some(systick) = self.systick.as_mut() {
            systick.reset();
        }
        if let Some(uart) = self.litex_uart.as_mut() {
            *uart = LiteXUart::new(uart.bases.clone(), None);
        }
        if let Some(flash) = self.stm32_flash.as_mut() {
            flash.reset(reload);
        }
        if let Some(rcc) = self.stm32_rcc.as_mut() {
            rcc.reset(reload);
        }
        if let Some(adc) = self.stm32_adc.as_mut() {
            adc.reset();
        }
        if let Some(dma) = self.stm32_dma.as_mut() {
            dma.reset();
        }
        if let Some(crc) = self.stm32_crc.as_mut() {
            crc.reset();
        }
        for uart in &mut self.stm32_uarts {
            uart.reset();
        }
        for port in &mut self.stm32_gpios {
            port.reset();
        }
        for timer in &mut self.stm32_timers {
            timer.reset();
        }
        for spi in &mut self.stm32_spis {
            spi.reset();
        }
        for i2c in &mut self.stm32_i2cs {
            i2c.reset();
        }
        for can in &mut self.stm32_cans {
            can.reset();
        }
        // SRAM, flash contents, external devices and the RTC backup domain stay powered.
        self.refresh_gpio_wiring();
        if let Some(exti) = self.exti.as_mut() {
            exti.reset(self.gpio_exti_levels);
        }
        true
    }

    pub fn sample_nvic(&mut self, active: &ActiveExceptions, sleeping: bool) {
        if let Some((index, ports)) = self
            .nrf_peripherals
            .iter()
            .enumerate()
            .find_map(|(index, device)| device.gpiote_ports().map(|ports| (index, ports)))
        {
            let samples = ports.map(|base| self.nrf_gpio_sample(base));
            self.nrf_peripherals[index].gpiote_sample(samples);
        }
        let (timer_irqs, timer_dma) = self.tick_stm32_timers();
        if let Some(nvic) = self.nvic.as_mut() {
            nvic.set_uart_level(
                self.litex_uart
                    .as_ref()
                    .is_some_and(LiteXUart::interrupt_pending),
                active,
            );
            for device in &mut self.nrf_peripherals {
                device.tick(sleeping);
                nvic.set_line(device.irq, device.pending(), active);
            }
            if let Some(line) = self.h7_flash.as_ref().and_then(FlashController::interrupt) {
                nvic.set_line(line, true, active);
            }
            for line in self.ethernet.iter().filter_map(Controller::nvic_irq) {
                nvic.set_line(line, true, active);
            }
            let mut exti_inputs = [self.gpio_exti_levels, 0, 0];
            for uart in &self.stm32_uarts {
                if f4_clocked(self.f4_rcc.as_ref(), uart.base) && uart.interrupt_pending() {
                    match uart.irq {
                        Some(Stm32Irq::Nvic(line)) => nvic.set_line(line, true, active),
                        Some(Stm32Irq::Exti(line)) => {
                            exti_inputs[usize::from(line / 32)] |= 1 << (line % 32)
                        }
                        None => {}
                    }
                }
            }
            let control = self.h7_network_control.as_ref();
            for route in self
                .h7_adc
                .iter()
                .filter(|device| adc_clocked(control, device))
                .filter_map(Adc::interrupt)
                .chain(
                    self.h7_bdma
                        .iter()
                        .filter(|device| bdma_clocked(control, device))
                        .flat_map(Bdma::interrupts),
                )
                .chain(
                    self.h7_rtc
                        .iter()
                        .filter(|device| {
                            rtc_clocked(control, device)
                                && f4_clocked(self.f4_rcc.as_ref(), device.base)
                        })
                        .flat_map(H7Rtc::interrupts),
                )
                .chain(
                    self.f4_spi
                        .iter()
                        .filter(|spi| f4_clocked(self.f4_rcc.as_ref(), spi.base))
                        .filter_map(F4Spi::interrupt),
                )
                .chain(
                    self.h7_spi
                        .iter()
                        .filter(|spi| spi_clocked(control, spi))
                        .filter_map(Spi::interrupt),
                )
                .chain(
                    self.f1_i2c
                        .iter()
                        .filter(|i2c| f4_clocked(self.f4_rcc.as_ref(), i2c.base))
                        .flat_map(F1I2c::interrupts),
                )
                .chain(
                    self.h7_i2c
                        .iter()
                        .filter(|i2c| i2c_clocked(control, i2c))
                        .filter_map(I2c::interrupt),
                )
                .chain(
                    self.h7_lptimers
                        .iter()
                        .filter(|timer| lptim_clocked(control, timer))
                        .filter_map(LpTimer::interrupt),
                )
                .chain(
                    self.h7_timers
                        .iter()
                        .filter(|timer| {
                            timer_clocked(control, timer)
                                && f4_clocked(self.f4_rcc.as_ref(), timer.base)
                        })
                        .flat_map(H7Timer::interrupts),
                )
            {
                match route {
                    Stm32Irq::Nvic(line) => nvic.set_line(line, true, active),
                    Stm32Irq::Exti(line) => exti_inputs[usize::from(line / 32)] |= 1 << (line % 32),
                }
            }
            for line in self
                .h7_rng
                .iter()
                .filter(|rng| rng_clocked(control, rng))
                .filter_map(Rng::interrupt)
                .chain(self.h7_sdmmc.iter().filter_map(Sdmmc::interrupt))
                .chain(self.h7_crypto.iter().filter_map(Crypto::interrupt))
                .chain(
                    self.stm32_can
                        .iter()
                        .filter(|can| f4_clocked(self.f4_rcc.as_ref(), can.base))
                        .flat_map(StmCan::interrupts),
                )
                .chain(self.h7_fdcan.iter().flat_map(Fdcan::interrupts))
                .chain(
                    self.h7_ltdc
                        .iter()
                        .filter(|device| ltdc_clocked(control, device))
                        .filter_map(Ltdc::interrupt),
                )
                .chain(
                    self.h7_hsem
                        .iter()
                        .filter(|device| hsem_clocked(control, device))
                        .filter_map(Hsem::interrupt),
                )
                .chain(
                    self.h7_dma2d
                        .iter()
                        .filter(|device| dma2d_clocked(control, device))
                        .filter_map(Dma2d::interrupt),
                )
                .chain(
                    self.h7_qspi
                        .iter()
                        .filter(|qspi| qspi_clocked(control, qspi))
                        .filter_map(Qspi::interrupt),
                )
                .chain(
                    self.h7_adc_dma
                        .iter()
                        .filter(|device| {
                            dma_clocked(control, device)
                                && f4_clocked(self.f4_rcc.as_ref(), device.base)
                        })
                        .flat_map(AdcDma::interrupts),
                )
            {
                nvic.set_line(line, true, active);
            }
            if let Some(control) = control {
                for gpio in &self.h7_gpio {
                    let Some(selector) = gpio.syscfg_port else {
                        continue;
                    };
                    let mut pins = gpio.pins();
                    while pins != 0 {
                        let pin = pins.trailing_zeros() as usize;
                        pins &= pins - 1;
                        if let Some(input) = gpio.syscfg_inputs[pin]
                            && control.syscfg_port(input) == selector
                            && let Some(line) = self.gpio_exti_outputs[usize::from(input)]
                        {
                            exti_inputs[usize::from(line / 32)] |= 1 << (line % 32);
                        }
                    }
                }
            }
            for gpio in &self.h7_gpio {
                let pins = gpio.pins();
                let port = gpio.base.wrapping_sub(0x4002_0000) / 0x400;
                for (pin, line) in gpio.exti_inputs.iter().enumerate() {
                    if let Some(line) = line
                        && pins & (1 << pin) != 0
                        && (*line >= 16
                            || self
                                .f4_system
                                .as_ref()
                                .is_none_or(|sys| sys.exti_port(*line) == port))
                    {
                        exti_inputs[usize::from(line / 32)] |= 1 << (line % 32);
                    }
                }
            }
            if let Some(exti) = self.h7_exti.as_mut() {
                for line in exti.sample(exti_inputs) {
                    nvic.set_line(line, true, active);
                }
            }
            if let Some(exti) = self.exti.as_mut() {
                let mut pending = exti.sample(exti_inputs[0]);
                while pending != 0 {
                    nvic.set_line(pending.trailing_zeros() as u8, true, active);
                    pending &= pending - 1;
                }
            }
            if let Some(rtc) = self.stm32_rtc.as_mut() {
                let mut signals = rtc.tick();
                while signals != 0 {
                    let signal = signals.trailing_zeros() as usize;
                    signals &= signals - 1;
                    match rtc.routes[signal] {
                        Some(Stm32Irq::Nvic(line)) => nvic.set_line(line, true, active),
                        Some(Stm32Irq::Exti(line)) => {
                            exti_inputs[usize::from(line / 32)] |= 1 << (line % 32)
                        }
                        None => {}
                    }
                }
            }
            let mut irqs = timer_irqs | self.external_nvic_levels;
            if self
                .stm32_flash
                .as_ref()
                .is_some_and(Stm32F0Flash::interrupt_pending)
            {
                irqs |= 1 << Stm32F0Flash::IRQ;
            }
            if let Some(adc) = self.stm32_adc.as_mut()
                && adc.tick()
                && let Some(line) = adc.irq
            {
                irqs |= 1 << line;
            }
            for can in &self.stm32_cans {
                irqs |= can.interrupts();
            }
            for i2c in &self.stm32_i2cs {
                if i2c.interrupt_pending()
                    && let Some(line) = i2c.irq
                {
                    irqs |= 1 << line;
                }
            }
            for spi in &self.stm32_spis {
                if spi.interrupt_pending()
                    && let Some(line) = spi.irq
                {
                    irqs |= 1 << line;
                }
            }
            if let Some(dma) = self.stm32_dma.as_mut() {
                dma.queue_requests(timer_dma);
                irqs |= dma.interrupts();
            }
            while irqs != 0 {
                nvic.set_line(irqs.trailing_zeros() as u8, true, active);
                irqs &= irqs - 1;
            }
        }
    }

    fn tick_stm32_timers(&mut self) -> (u32, u8) {
        let mut irqs = 0;
        let mut dma_requests = 0;
        let mut gpio_changed = false;
        for timer in &mut self.stm32_timers {
            irqs |= timer.tick();
            let changed = timer.take_output_changes();
            if changed != 0 {
                for (signal, pin) in &timer.gpio_outputs {
                    if changed & (1 << signal) != 0
                        && let Some(port) = self
                            .stm32_gpios
                            .iter_mut()
                            .find(|port| port.base == pin.port)
                    {
                        port.set_alternate_output(pin.pin, pin.function, timer.pin_output(*signal));
                        gpio_changed = true;
                    }
                }
            }
        }
        if gpio_changed {
            self.refresh_gpio_wiring();
        }
        for timer in &mut self.stm32_timers {
            dma_requests |= timer.take_dma_requests();
        }
        (irqs, dma_requests)
    }

    pub fn map_h7_exti(
        &mut self,
        base: u32,
        outputs: [Option<u8>; H7Exti::LINES],
    ) -> Result<(), String> {
        let start = u64::from(base);
        let end = start + u64::from(H7Exti::SIZE);
        if base & 3 != 0 || end > 1 << 32 || self.h7_exti.is_some() || self.overlaps(start, end) {
            return Err(format!(
                "invalid or overlapping STM32H7 EXTI range at {base:#010x}"
            ));
        }
        self.h7_exti = Some(H7Exti::new(base, outputs));
        Ok(())
    }

    pub fn map_exti(
        &mut self,
        base: u32,
        outputs: Vec<Option<u8>>,
        first_direct: u32,
    ) -> Result<(), String> {
        let start = u64::from(base);
        let end = start + u64::from(Exti::SIZE);
        if base & 3 != 0 || end > 1 << 32 || self.overlaps(start, end) {
            return Err(format!(
                "invalid or overlapping STM32 EXTI range at {base:#010x}"
            ));
        }
        self.exti = Some(Exti::new(base, outputs, first_direct));
        Ok(())
    }

    pub fn map_systick(&mut self) -> Result<(), String> {
        if SysTick::RANGES
            .into_iter()
            .any(|(start, end)| self.overlaps(u64::from(start), u64::from(end)))
        {
            return Err("Cortex-M SysTick registers overlap an existing mapping".into());
        }
        self.systick = Some(SysTick::default());
        Ok(())
    }

    fn check_litex_ranges(&self, bases: &[u32], size: u32) -> Result<(), String> {
        for (index, &base) in bases.iter().enumerate() {
            let start = u64::from(base);
            let end = start + u64::from(size);
            if base & 3 != 0
                || end > 1 << 32
                || self.overlaps(start, end)
                || bases[..index]
                    .iter()
                    .any(|&base| overlaps_range(start, end, base, size))
            {
                return Err(format!(
                    "invalid or overlapping LiteX MMIO range at {base:#010x}"
                ));
            }
        }
        Ok(())
    }

    pub fn map_litex_uart(
        &mut self,
        bases: Vec<u32>,
        vex_irq: Option<(usize, u8)>,
    ) -> Result<(), String> {
        self.map_litex_uart_with_timing(bases, vex_irq, None)
    }

    pub fn map_litex_uart_with_timing(
        &mut self,
        bases: Vec<u32>,
        vex_irq: Option<(usize, u8)>,
        timing: Option<LiteXTxTiming>,
    ) -> Result<(), String> {
        self.check_litex_ranges(&bases, LiteXUart::SIZE)?;
        self.litex_uart = Some(LiteXUart::new(bases, vex_irq).with_timing(timing));
        Ok(())
    }

    pub fn tick_litex_uart(&mut self, step: u64, output: &mut impl Write) -> io::Result<()> {
        if let Some(uart) = self.litex_uart.as_mut() {
            uart.tick(step, output)?;
        }
        Ok(())
    }

    pub fn map_litex_timer(
        &mut self,
        bases: Vec<u32>,
        timer: litex::Timer,
        vex_irq: Option<(usize, u8)>,
    ) -> Result<(), String> {
        self.check_litex_ranges(&bases, timer.size())?;
        self.litex_timer = Some(LiteXTimer {
            bases,
            timer,
            vex_irq,
        });
        Ok(())
    }

    pub fn map_litex_timer_alias(&mut self, base: u32) -> Result<(), String> {
        let timer = self
            .litex_timer
            .as_ref()
            .ok_or("--litex-timer-alias requires a LiteX timer")?;
        self.check_litex_ranges(&[base], timer.timer.size())?;
        self.litex_timer.as_mut().unwrap().bases.push(base);
        Ok(())
    }

    pub fn map_litex_cpu_timer(&mut self, base: u32, frequency: u32) -> Result<(), String> {
        self.check_litex_ranges(&[base], litex::CpuTimer::SIZE)?;
        self.litex_cpu_timer = Some(litex::CpuTimer::new(base, frequency));
        Ok(())
    }

    pub fn map_silencer(&mut self, base: u32, size: u32) -> Result<(), String> {
        let start = u64::from(base);
        let end = start + u64::from(size);
        if size == 0 || end > 1 << 32 || self.overlaps(start, end) {
            return Err(format!(
                "invalid or overlapping SilenceRange at {base:#010x}"
            ));
        }
        self.silencers.push((base, size));
        Ok(())
    }

    pub fn set_stm32f0_tags(&mut self, tags: Vec<std::ops::RangeInclusive<u32>>) {
        self.stm32f0_tags = tags;
    }

    fn stm32f0_tagged(&self, address: u32, width: usize) -> bool {
        address.checked_add(width as u32 - 1).is_some_and(|end| {
            self.stm32f0_tags
                .iter()
                .any(|range| range.contains(&address) && range.contains(&end))
        })
    }

    pub fn map_stm32_can(&mut self, can: Stm32Can) -> Result<(), String> {
        let start = u64::from(can.base);
        let end = start + u64::from(can.size);
        if can.base & 3 != 0
            || can.size < 0x320
            || can.size & 3 != 0
            || end > 1 << 32
            || self.overlaps(start, end)
        {
            return Err(format!(
                "invalid or overlapping STM32 CAN range at {:#010x}",
                can.base
            ));
        }
        self.stm32_cans.push(can);
        Ok(())
    }

    pub fn map_stm32_i2c(&mut self, i2c: Stm32I2c) -> Result<(), String> {
        let start = u64::from(i2c.base);
        let end = start + u64::from(Stm32I2c::SIZE);
        if i2c.base & 3 != 0 || end > 1 << 32 || self.overlaps(start, end) {
            return Err(format!(
                "invalid or overlapping STM32 I²C range at {:#010x}",
                i2c.base
            ));
        }
        self.stm32_i2cs.push(i2c);
        Ok(())
    }

    pub fn map_stm32f0_spi(&mut self, spi: Stm32F0Spi) -> Result<(), String> {
        let start = u64::from(spi.base);
        let end = start + u64::from(Stm32F0Spi::SIZE);
        if spi.base & 3 != 0 || end > 1 << 32 || self.overlaps(start, end) {
            return Err(format!(
                "invalid or overlapping STM32F0 SPI range at {:#010x}",
                spi.base
            ));
        }
        self.stm32_spis.push(spi);
        Ok(())
    }

    pub fn map_stm32_timer(&mut self, timer: Stm32Timer) -> Result<(), String> {
        let start = u64::from(timer.base);
        let end = start + u64::from(Stm32Timer::SIZE);
        if timer.base & 3 != 0 || end > 1 << 32 || self.overlaps(start, end) {
            return Err(format!(
                "invalid or overlapping STM32 timer range at {:#010x}",
                timer.base
            ));
        }
        self.stm32_timers.push(timer);
        Ok(())
    }

    pub fn route_gpio_timer_input(&mut self, port: u32, pin: u8, timer: u32, channel: u8) {
        self.gpio_timer_inputs.push((port, pin, timer, channel));
    }

    pub fn add_gpio_led(&mut self, base: u32, led: GpioLed) {
        self.gpio_leds.push((base, led));
        self.refresh_gpio_wiring();
    }

    pub fn gpio_led_index(&self, target: &str) -> Result<usize, String> {
        let path = target.strip_prefix("sysbus.").unwrap_or(target);
        self.gpio_leds
            .iter()
            .position(|(_, led)| match path.split_once('.') {
                Some((parent, name)) => led.parent.as_deref() == Some(parent) && led.name == name,
                None => led.name == path,
            })
            .ok_or_else(|| format!("unknown STM32 GPIO LED {target:?}"))
    }

    pub fn gpio_led(&self, index: usize) -> &GpioLed {
        &self.gpio_leds[index].1
    }

    pub fn add_gpio_button(&mut self, button: GpioButton) {
        self.gpio_buttons.push(button);
        self.set_button(self.gpio_buttons.len() - 1, false);
    }

    pub fn report_led_events(&mut self, enabled: bool) {
        self.led_events = enabled;
    }

    pub fn board_button(&self, name: &str) -> Option<usize> {
        self.gpio_buttons
            .iter()
            .position(|button| button.name == name)
    }

    pub fn set_named_button(&mut self, target: &str, pressed: bool) -> Result<(), String> {
        let target = target.strip_prefix("sysbus.").unwrap_or(target);
        let index = match target.split_once('.') {
            Some((parent, name)) => self
                .gpio_buttons
                .iter()
                .position(|button| button.parent.as_deref() == Some(parent) && button.name == name),
            None => self.board_button(target),
        }
        .ok_or_else(|| format!("unknown STM32 board button {target:?}"))?;
        self.set_button(index, pressed);
        Ok(())
    }

    pub fn set_button(&mut self, index: usize, pressed: bool) {
        let button = &self.gpio_buttons[index];
        if let Some(port) = self
            .stm32_gpios
            .iter_mut()
            .find(|port| port.base == button.port)
        {
            port.set_external(button.pin, pressed ^ button.invert);
            self.refresh_gpio_wiring();
        }
    }

    fn refresh_gpio_wiring(&mut self) {
        let previous = self.gpio_exti_levels;
        self.gpio_exti_levels = self
            .stm32_gpios
            .iter()
            .fold(0, |levels, port| levels | port.exti_levels());
        if let Some(exti) = &mut self.exti {
            exti.update_gpio(previous, self.gpio_exti_levels);
        }
        if !self.gpio_timer_inputs.is_empty() {
            for timer in &mut self.stm32_timers {
                timer.capture_next = 0;
            }
            for &(port, pin, timer, channel) in &self.gpio_timer_inputs {
                if self
                    .stm32_gpios
                    .iter()
                    .any(|gpio| gpio.base == port && gpio.pins() & (1 << pin) != 0)
                    && let Some(timer) = self
                        .stm32_timers
                        .iter_mut()
                        .find(|device| device.base == timer)
                {
                    timer.capture_next |= 1 << channel;
                }
            }
            for timer in &mut self.stm32_timers {
                timer.apply_capture_inputs();
            }
        }
        for (base, led) in &mut self.gpio_leds {
            if let Some(port) = self.stm32_gpios.iter().find(|port| port.base == *base) {
                led.sample(port.pins(), self.led_events);
            }
        }
    }

    pub fn map_stm32_gpio(&mut self, gpio: Stm32Gpio) -> Result<(), String> {
        let start = u64::from(gpio.base);
        let end = start + u64::from(gpio.size);
        if gpio.base & 3 != 0
            || gpio.size < 0x2c
            || gpio.size & 3 != 0
            || end > 1 << 32
            || self.overlaps(start, end)
        {
            return Err(format!(
                "invalid or overlapping STM32 GPIO range at {:#010x}",
                gpio.base
            ));
        }
        self.stm32_gpios.push(gpio);
        Ok(())
    }

    pub fn map_stm32_dma(&mut self, dma: Stm32Dma) -> Result<(), String> {
        let start = u64::from(dma.base);
        let end = start + u64::from(Stm32Dma::SIZE);
        if self.stm32_dma.is_some()
            || dma.base & 3 != 0
            || end > 1 << 32
            || self.overlaps(start, end)
        {
            return Err(format!(
                "invalid or overlapping STM32 DMA range at {:#010x}",
                dma.base
            ));
        }
        self.stm32_dma = Some(dma);
        Ok(())
    }

    pub fn map_stm32f0_adc(&mut self, name: String, adc: Stm32F0Adc) -> Result<(), String> {
        let start = u64::from(adc.base);
        let end = start + u64::from(Stm32F0Adc::SIZE);
        if self.stm32_adc.is_some()
            || adc.base & 3 != 0
            || end > 1 << 32
            || self.overlaps(start, end)
        {
            return Err(format!(
                "invalid or overlapping STM32F0 ADC range at {:#010x}",
                adc.base
            ));
        }
        self.stm32_adc = Some(adc);
        self.stm32_adc_name = Some(name);
        Ok(())
    }

    pub fn map_tag(&mut self, base: u32, size: u32) -> Result<(), String> {
        if size == 0 || u64::from(base) + u64::from(size) > 1 << 32 {
            return Err(format!("invalid Tag range at {base:#010x}"));
        }
        self.tags.push((base, size));
        Ok(())
    }

    fn zero_mmio(&self, address: u32, width: usize) -> bool {
        self.silencers
            .iter()
            .chain(&self.tags)
            .any(|&(base, size)| {
                address >= base && u64::from(address - base) + width as u64 <= u64::from(size)
            })
    }

    pub fn map_litex_eth(
        &mut self,
        name: String,
        registrations: Vec<Registration>,
        irq: Option<(usize, u8)>,
        phy: Option<Phy>,
        wide_csr: bool,
    ) -> Result<(), String> {
        for (index, registration) in registrations.iter().enumerate() {
            let start = u64::from(registration.base);
            let end = start + u64::from(registration.size);
            self.check_litex_ranges(&[registration.base], registration.size)?;
            if registration.size == 0
                || registrations[..index]
                    .iter()
                    .any(|other| overlaps_range(start, end, other.base, other.size))
            {
                return Err(format!(
                    "invalid or overlapping Ethernet range at {start:#010x}"
                ));
            }
        }
        let mut ethernet = Ethernet::new(registrations, irq, phy).with_wide_csr(wide_csr);
        ethernet.name = Some(name);
        self.litex_eth = Some(ethernet);
        Ok(())
    }

    pub fn capture_litex_framebuffer(&self, path: &Path) -> Result<(), String> {
        let video = self.litex_video.as_ref().ok_or("no LiteX framebuffer")?;
        if video.active.is_none() {
            return Err("LiteX framebuffer DMA is disabled".into());
        }
        let file = File::create(path).map_err(|error| format!("{}: {error}", path.display()))?;
        video
            .render(self, io::BufWriter::new(file))
            .map_err(|error| format!("{}: {error}", path.display()))
    }

    pub fn litex_phy_mut(&mut self, name: &str) -> Result<&mut Phy, String> {
        let path = name.strip_prefix("sysbus.").unwrap_or(name);
        self.litex_eth
            .as_mut()
            .and_then(|ethernet| ethernet.phy_mut(path))
            .ok_or_else(|| format!("unknown LiteX Ethernet PHY {name:?}"))
    }

    pub fn attach_litex_tftp(
        &mut self,
        name: &str,
        server: crate::litex_tftp::Server,
    ) -> Result<(), String> {
        let name = name.strip_prefix("sysbus.").unwrap_or(name);
        let ethernet = self
            .litex_eth
            .as_mut()
            .filter(|ethernet| ethernet.name.as_deref() == Some(name))
            .ok_or_else(|| format!("unknown LiteX Ethernet controller {name:?}"))?;
        ethernet.tftp = Some(server);
        Ok(())
    }

    pub fn map_litex_flash(&mut self, bases: Vec<u32>, backing: u32) -> Result<(), String> {
        self.check_litex_ranges(&bases, crate::litex_flash::Flash::SIZE)?;
        let index = self
            .regions
            .iter()
            .position(|region| region.base == u64::from(backing))
            .ok_or("flash backing must be mapped memory")?;
        let region = &mut self.regions[index];
        let size = region.bytes.len();
        if !size.is_power_of_two() || !(65536..=268435456).contains(&size) {
            return Err(
                "Micron flash requires power-of-two backing between 64 KiB and 256 MiB".into(),
            );
        }
        region.flash_pages = Some(vec![false; size / 4096]);
        self.litex_flash = Some((bases, index, crate::litex_flash::Flash::default()));
        Ok(())
    }

    pub fn map_litex_sd(&mut self, sd: SdController) -> Result<(), String> {
        for (index, region) in sd.registrations.iter().enumerate() {
            let start = u64::from(region.base);
            let end = start + u64::from(region.size);
            self.check_litex_ranges(&[region.base], region.size)?;
            if region.size == 0
                || sd.registrations[..index]
                    .iter()
                    .any(|other| overlaps_range(start, end, other.base, other.size))
            {
                return Err(format!("invalid or overlapping SD range at {start:#010x}"));
            }
        }
        self.litex_sd = Some(sd);
        Ok(())
    }

    pub fn attach_litex_sd(&mut self, name: &str, card: Card) -> Result<(), String> {
        let name = name.strip_prefix("sysbus.").unwrap_or(name);
        if let Some(controller) = self
            .litex_sd
            .as_mut()
            .filter(|controller| controller.name == name)
        {
            controller.card = Some(card);
            return Ok(());
        }
        for (_, device) in &mut self.litex_io {
            if let litex::IoDevice::Spi(spi) = device
                && spi.name.as_deref() == Some(name)
            {
                spi.card = Some(Box::new(crate::litex_sd::SpiCard::new(card)));
                return Ok(());
            }
        }
        Err(format!("unknown LiteX SD controller {name:?}"))
    }

    pub fn map_litex_panel(&mut self, panel: Panel) -> Result<(), String> {
        self.check_litex_ranges(&panel.bases, Panel::SIZE)?;
        self.litex_panel = Some(panel);
        Ok(())
    }

    pub fn litex_led_state(&self, name: &str) -> Result<bool, String> {
        self.litex_panel
            .as_ref()
            .and_then(|panel| panel.led_state(name))
            .or_else(|| {
                let path = name.strip_prefix("sysbus.").unwrap_or(name);
                self.litex_io.iter().find_map(|(_, device)| match device {
                    litex::IoDevice::Gpio(gpio) => gpio.led_state(path),
                    _ => None,
                })
            })
            .ok_or_else(|| format!("unknown wired LiteX LED {name:?}"))
    }

    pub fn litex_sensor_mut(
        &mut self,
        name: &str,
    ) -> Result<&mut crate::litex_i2c::Sensor, String> {
        let path = name.strip_prefix("sysbus.").unwrap_or(name);
        self.litex_io
            .iter_mut()
            .find_map(|(_, device)| match device {
                litex::IoDevice::I2c(controller) => controller.sensor_mut(path),
                _ => None,
            })
            .ok_or_else(|| format!("unknown LiteX I2C sensor {name:?}"))
    }

    pub fn has_litex_button(&self, name: &str) -> bool {
        let name = name.rsplit('.').next().unwrap_or(name);
        self.litex_panel
            .as_ref()
            .is_some_and(|panel| panel.has_button(name))
            || self.litex_io.iter().any(|(_, device)| {
                matches!(device, litex::IoDevice::Gpio(gpio) if gpio.has_button(name))
            })
    }

    pub fn set_litex_button(&mut self, name: &str, action: ButtonAction) -> Result<(), String> {
        let name = name.rsplit('.').next().unwrap_or(name);
        if let Some(panel) = self
            .litex_panel
            .as_mut()
            .filter(|panel| panel.has_button(name))
        {
            return panel.button_action(name, action);
        }
        for (_, device) in &mut self.litex_io {
            if let litex::IoDevice::Gpio(gpio) = device
                && gpio.has_button(name)
            {
                return gpio.button_action(name, action);
            }
        }
        Err(format!("unknown LiteX button {name:?}"))
    }

    pub fn map_litex_io(&mut self, bases: Vec<u32>, device: litex::IoDevice) -> Result<(), String> {
        self.check_litex_ranges(&bases, device.size())?;
        if let litex::IoDevice::Gpio(gpio) = &device
            && let Some((hart, line)) = gpio.irq
        {
            self.litex_gpio_irqs.push((self.litex_io.len(), hart, line));
        }
        self.litex_io.push((bases, device));
        Ok(())
    }

    pub fn map_litex_soc(&mut self, base: u32, wide_csr: bool) -> Result<(), String> {
        let soc = litex::SocController::new(base, wide_csr);
        self.check_litex_ranges(&[base], soc.size())?;
        self.litex_soc = Some(soc);
        Ok(())
    }

    pub fn map_litex_video(&mut self, framebuffer: Framebuffer) -> Result<(), String> {
        self.check_litex_ranges(&[framebuffer.dma], Framebuffer::DMA_SIZE)?;
        self.check_litex_ranges(&[framebuffer.vtg], Framebuffer::VTG_SIZE)?;
        let dma = u64::from(framebuffer.dma);
        if overlaps_range(
            dma,
            dma + u64::from(Framebuffer::DMA_SIZE),
            framebuffer.vtg,
            Framebuffer::VTG_SIZE,
        ) {
            return Err("overlapping LiteX framebuffer regions".into());
        }
        self.litex_video = Some(framebuffer);
        Ok(())
    }

    pub fn map_litex_i2s(&mut self, device: I2s) -> Result<(), String> {
        self.check_litex_ranges(&[device.csr], I2s::SIZE)?;
        self.check_litex_ranges(&[device.buffer], device.buffer_size)?;
        if overlaps_range(
            u64::from(device.csr),
            u64::from(device.csr) + u64::from(I2s::SIZE),
            device.buffer,
            device.buffer_size,
        ) {
            return Err("overlapping LiteX I2S regions".into());
        }
        self.litex_i2s.push(device);
        Ok(())
    }

    pub fn tick_litex(&mut self, hart: usize, step: u64) -> u64 {
        let mut lines = 0;
        if let Some(device) = self.litex_timer.as_mut() {
            device.timer.tick();
            if let Some((target, line)) = device.vex_irq
                && target == hart
                && device.timer.interrupt_pending()
            {
                lines |= 1 << line;
            }
        }
        if let Some(uart) = self.litex_uart.as_ref()
            && let Some((target, line)) = uart.vex_irq
            && target == hart
            && uart.interrupt_pending()
        {
            lines |= 1 << line;
        }
        if let Some(eth) = self.litex_eth.as_ref()
            && let Some((target, line)) = eth.vex_irq
            && target == hart
            && eth.interrupt_pending()
        {
            lines |= 1 << line;
        }
        if let Some(panel) = self.litex_panel.as_ref()
            && let Some((target, line)) = panel.irq
            && target == hart
            && panel.interrupt_pending()
        {
            lines |= 1 << line;
        }
        for i2s in &mut self.litex_i2s {
            i2s.tick(step);
            if i2s.irq.0 == hart && i2s.interrupt_pending() {
                lines |= 1 << i2s.irq.1;
            }
        }
        for &(index, target, line) in &self.litex_gpio_irqs {
            if target == hart
                && let litex::IoDevice::Gpio(gpio) = &self.litex_io[index].1
                && gpio.interrupt_pending()
            {
                lines |= 1 << line;
            }
        }
        lines
    }

    pub fn has_litex_irq(&self) -> bool {
        self.litex_timer
            .as_ref()
            .is_some_and(|device| device.vex_irq.is_some())
            || self
                .litex_uart
                .as_ref()
                .is_some_and(|uart| uart.vex_irq.is_some())
            || self
                .litex_eth
                .as_ref()
                .is_some_and(|eth| eth.vex_irq.is_some())
            || self
                .litex_panel
                .as_ref()
                .is_some_and(|panel| panel.irq.is_some())
            || !self.litex_gpio_irqs.is_empty()
            || !self.litex_i2s.is_empty()
    }

    pub fn set_named_adc_input(
        &mut self,
        name: &str,
        channel: usize,
        volts: f64,
    ) -> Result<(), String> {
        if self.stm32_adc_name.as_deref() != Some(name) {
            return Err(format!("unknown STM32 ADC {name:?}"));
        }
        self.set_stm32f0_adc_input(channel, volts)
    }

    pub fn validate_stm32f0_adc_input(&self, channel: usize, volts: f64) -> Result<(), String> {
        self.stm32_adc
            .as_ref()
            .ok_or("ADC input requires an STM32F0 ADC")?
            .validate_input(channel, volts)
    }

    pub fn set_stm32f0_adc_input(&mut self, channel: usize, volts: f64) -> Result<(), String> {
        self.stm32_adc
            .as_mut()
            .ok_or("ADC input requires an STM32F0 ADC")?
            .set_input(channel, volts)
    }

    pub fn map_stm32f0_crc(&mut self, base: u32, configurable_poly: bool) -> Result<(), String> {
        let start = u64::from(base);
        let end = start + u64::from(Stm32F0Crc::SIZE);
        if self.stm32_crc.is_some() || base & 3 != 0 || end > 1 << 32 || self.overlaps(start, end) {
            return Err(format!(
                "invalid or overlapping STM32F0 CRC range at {base:#010x}"
            ));
        }
        self.stm32_crc = Some(Stm32F0Crc::new(base, configurable_poly));
        Ok(())
    }

    pub fn map_stm32_rtc(&mut self, rtc: Stm32Rtc) -> Result<(), String> {
        let start = u64::from(rtc.base);
        let end = start + u64::from(Stm32Rtc::SIZE);
        if self.stm32_rtc.is_some()
            || rtc.base & 3 != 0
            || end > 1 << 32
            || self.overlaps(start, end)
        {
            return Err(format!(
                "invalid or overlapping STM32 RTC range at {:#010x}",
                rtc.base
            ));
        }
        self.stm32_rtc = Some(rtc);
        Ok(())
    }

    pub fn map_stm32f072_flash(&mut self) -> Result<(), String> {
        const BANK: std::ops::Range<u32> = 0x0800_0000..0x0802_0000;
        if !self
            .stm32f0_tags
            .contains(&(Stm32F0Flash::BASE..=0x4002_23ff))
        {
            return Ok(());
        }
        let Some(index) = self.regions.iter().position(|region| {
            region.base == u64::from(BANK.start)
                && region.bytes.len() == (BANK.end - BANK.start) as usize
        }) else {
            return Ok(());
        };
        let start = u64::from(Stm32F0Flash::BASE);
        let options = u64::from(Stm32F0Flash::OPTION_BASE);
        if self.overlaps(start, start + u64::from(Stm32F0Flash::SIZE))
            || self.overlaps(options, options + u64::from(Stm32F0Flash::OPTION_SIZE))
        {
            return Err("STM32F072 flash registers or option bytes overlap another mapping".into());
        }
        self.regions[index].bytes.fill(0xff);
        self.stm32_flash = Some(Stm32F0Flash::new(BANK));
        Ok(())
    }

    pub fn map_stm32f0_rcc(&mut self, base: u32) -> Result<(), String> {
        let start = u64::from(base);
        let end = start + u64::from(Stm32F0Rcc::SIZE);
        if self.stm32_rcc.is_some() || base & 3 != 0 || end > 1 << 32 || self.overlaps(start, end) {
            return Err(format!(
                "invalid or overlapping STM32F0 RCC range at {base:#010x}"
            ));
        }
        self.stm32_rcc = Some(Stm32F0Rcc::new(base));
        Ok(())
    }

    pub fn map_stm32_usart(&mut self, uart: Stm32Usart) -> Result<(), String> {
        let base = uart.base;
        let start = u64::from(base);
        let end = start + u64::from(uart.size());
        if base & 3 != 0 || end > 1 << 32 || self.overlaps(start, end) {
            return Err(format!(
                "invalid or overlapping STM32 USART range at {base:#010x}"
            ));
        }
        self.stm32_uarts.push(uart);
        Ok(())
    }

    pub fn attach_litex_uart_input(
        &mut self,
        receiver: std::sync::mpsc::Receiver<io::Result<Vec<u8>>>,
    ) -> Result<(), String> {
        let uart = self
            .litex_uart
            .as_mut()
            .ok_or("--litex-uart-stdio requires a LiteX UART")?;
        uart.attach_live_input(receiver);
        Ok(())
    }

    pub fn enable_litex_uart_legacy_console(&mut self) -> Result<(), String> {
        let uart = self
            .litex_uart
            .as_mut()
            .ok_or("--litex-uart-legacy-console requires a LiteX UART")?;
        uart.legacy_console_events();
        Ok(())
    }

    pub fn select_uart_input(&mut self, name: &str) {
        if let Ok(index) = self.stm32_usart_index(name) {
            self.stm32_uart_input = index;
        }
    }

    pub fn has_uart_input(&self) -> bool {
        self.uart.is_some()
            || self.ns16550.is_some()
            || self.litex_uart.is_some()
            || !self.stm32_uarts.is_empty()
            || !self.rv64_devices.uarts.is_empty()
            || self.nrf_peripherals.iter().any(NrfPeripheral::is_uart)
    }

    pub fn queue_uart_input(&mut self, bytes: Vec<u8>) -> Result<(), BusError> {
        if let Some(uart) = self.uart.as_mut() {
            uart.queue_input(bytes);
        } else if let Some(uart) = self.ns16550.as_mut() {
            uart.queue_input(bytes);
        } else if let Some(uart) = self.litex_uart.as_mut() {
            uart.queue_input(bytes);
        } else if let Some(uart) = self.stm32_uarts.get_mut(self.stm32_uart_input)
            && f4_clocked(self.f4_rcc.as_ref(), uart.base)
        {
            uart.queue_input(bytes);
        } else if let Some(index) = self.nrf_peripherals.iter().position(NrfPeripheral::is_uart) {
            let mut device = self.nrf_peripherals.remove(index);
            device.queue_input(bytes);
            let result = device.receive(self);
            self.nrf_peripherals.insert(index, device);
            result?;
        } else {
            self.rv64_devices.queue_input(bytes);
        }
        Ok(())
    }

    fn stm32_usart_index(&self, target: &str) -> Result<usize, String> {
        let name = target.strip_prefix("sysbus.").unwrap_or(target);
        self.stm32_uarts
            .iter()
            .position(|uart| uart.name == name)
            .ok_or_else(|| format!("unknown STM32 USART {target:?}"))
    }

    pub fn write_stm32_uart_line(
        &mut self,
        target: &str,
        text: &str,
        ending: &[u8],
    ) -> Result<(), String> {
        let index = self.stm32_usart_index(target)?;
        // Renode writes the low byte of each UTF-16 code unit before the ending.
        self.stm32_uarts[index].queue_input(
            text.encode_utf16()
                .map(|unit| unit as u8)
                .chain(ending.iter().copied()),
        );
        Ok(())
    }

    pub fn record_stm32_uart(&mut self, target: &str) {
        if let Ok(index) = self.stm32_usart_index(target) {
            self.stm32_uart_history
                .resize_with(self.stm32_uarts.len(), || None);
            self.stm32_uart_history[index].get_or_insert_with(UartHistory::default);
        }
    }

    pub fn watch_stm32_uart(
        &mut self,
        target: &str,
        pattern: &str,
        end: UartWaitMode,
    ) -> Result<usize, String> {
        let index = self.stm32_usart_index(target)?;
        self.stm32_uart_history[index]
            .as_mut()
            .expect("a script wait enables history before execution")
            .begin_wait(pattern, end);
        Ok(index)
    }

    pub fn stm32_uart_matched(&self, index: usize) -> bool {
        self.stm32_uart_history[index]
            .as_ref()
            .is_some_and(UartHistory::matched)
    }

    pub fn finish_stm32_uart_watch(&mut self, index: usize) {
        if let Some(history) = &mut self.stm32_uart_history[index] {
            history.finish_wait();
        }
    }

    pub fn tick_plic(&mut self) -> u32 {
        if let Some(uart) = self.ns16550.as_ref()
            && let Some(source) = uart.source
            && let Some(plic) = self.plic.as_mut()
        {
            plic.set_line(source, uart.interrupt_pending());
        }
        self.plic.as_ref().map_or(0, Plic::irq_outputs)
    }

    pub fn has_plic(&self) -> bool {
        self.plic.is_some()
    }

    pub fn map_murax_gpio(&mut self, base: u32) -> Result<(), String> {
        self.check_murax_range(base, 0xc, self.gpio.is_some())?;
        self.gpio = Some((base, Gpio::default()));
        Ok(())
    }

    pub fn map_murax_timer(&mut self, base: u32, connected: bool) -> Result<(), String> {
        self.check_murax_range(base, 0x5c, self.timer.is_some())?;
        let mut timer = Timer::default();
        timer.connected = connected;
        self.timer = Some((base, timer));
        Ok(())
    }

    fn check_murax_range(&self, base: u32, size: u64, already_mapped: bool) -> Result<(), String> {
        let start = u64::from(base);
        let end = start + size;
        if base & 3 != 0 || end > 1 << 32 || already_mapped || self.overlaps(start, end) {
            return Err(format!(
                "invalid or overlapping Murax peripheral range at {base:#010x}"
            ));
        }
        Ok(())
    }

    pub fn save_memory(&self, output: &mut impl Write) -> io::Result<()> {
        output.write_all(&(self.regions.len() as u64).to_le_bytes())?;
        for region in &self.regions {
            output.write_all(&region.base.to_le_bytes())?;
            output.write_all(&(region.bytes.len() as u64).to_le_bytes())?;
            output.write_all(&region.bytes)?;
        }
        Ok(())
    }

    pub fn clear_memory(&mut self) {
        for region in &mut self.regions {
            region.bytes.fill(0);
        }
        self.reservations.fill(None);
        self.rv64_reservations.fill(None);
    }

    pub fn restore_memory(&mut self, input: &mut impl Read) -> io::Result<()> {
        fn read_u64(input: &mut impl Read) -> io::Result<u64> {
            let mut bytes = [0; 8];
            input.read_exact(&mut bytes)?;
            Ok(u64::from_le_bytes(bytes))
        }
        let mismatch = || {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "snapshot memory layout differs from the platform",
            )
        };
        if read_u64(input)? != self.regions.len() as u64 {
            return Err(mismatch());
        }
        let mut contents = Vec::with_capacity(self.regions.len());
        for region in &self.regions {
            if read_u64(input)? != region.base || read_u64(input)? != region.bytes.len() as u64 {
                return Err(mismatch());
            }
            let mut bytes = Vec::new();
            bytes
                .try_reserve_exact(region.bytes.len())
                .map_err(io::Error::other)?;
            bytes.resize(region.bytes.len(), 0);
            input.read_exact(&mut bytes)?;
            contents.push(bytes);
        }
        if input.read(&mut [0])? != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "trailing snapshot data",
            ));
        }
        for (region, bytes) in self.regions.iter_mut().zip(contents) {
            region.bytes = bytes;
        }
        self.reservations.fill(None);
        self.rv64_reservations.fill(None);
        Ok(())
    }

    pub fn memory(&mut self, address: u32, width: usize) -> Result<&mut [u8], BusError> {
        self.memory_for_store(address, width, None)
    }

    pub fn watch_rv64_tohost(&mut self, address: u64) -> Result<(), String> {
        if !address.is_multiple_of(8) || self.ram64(address, 8).is_none() {
            return Err("--tohost requires an aligned 8-byte RAM address".into());
        }
        self.rv64_tohost = Some(address);
        Ok(())
    }

    pub fn observe_rv64_store(&mut self, address: u64, low_byte: u8) {
        if self.rv64_tohost == Some(address) && low_byte & 1 != 0 {
            let bytes = self.ram64(address, 8).unwrap();
            self.rv64_test_exit = Some(u64::from_le_bytes(bytes.try_into().unwrap()) >> 1);
        }
    }

    pub fn watch_robot_tohost(&mut self, address: u64) -> Result<(), String> {
        if !address.is_multiple_of(Tohost::SIZE as u64)
            || address.checked_add(Tohost::SIZE as u64).is_none()
        {
            return Err("--robot-tohost requires a page-aligned 4 KiB registration".into());
        }
        // Robot registers fresh ArrayMemory after LoadELF. CPU accesses see
        // this overlay, while firmware loading and other bus masters see RAM.
        self.robot_tohost = Some(Tohost {
            address,
            bytes: Box::new([0; Tohost::SIZE]),
        });
        self.rv64_test_exit = None;
        Ok(())
    }

    pub fn read_rv64_tohost(&self, address: u64, width: usize) -> Option<u64> {
        let watcher = self.robot_tohost.as_ref()?;
        let range = watcher.range(address, width)?;
        let mut value = [0; 8];
        value[..width].copy_from_slice(&watcher.bytes[range]);
        Some(u64::from_le_bytes(value))
    }

    pub fn write_rv64_tohost(&mut self, address: u64, width: usize, value: u64) -> bool {
        let Some(watcher) = self.robot_tohost.as_mut() else {
            return false;
        };
        let Some(range) = watcher.range(address, width) else {
            return false;
        };
        watcher.bytes[range.clone()].copy_from_slice(&value.to_le_bytes()[..width]);
        if self.rv64_test_exit.is_none() && range.start == 0 && value & 1 != 0 {
            self.rv64_test_exit =
                Some(u64::from_le_bytes(watcher.bytes[..8].try_into().unwrap()) >> 1);
        }
        true
    }

    pub fn memory64(&mut self, address: u64, width: usize) -> Option<&mut [u8]> {
        self.ram_write64(address, width, None)
    }

    pub(crate) fn cpu_memory64(
        &mut self,
        hart: usize,
        address: u64,
        width: usize,
    ) -> Option<&mut [u8]> {
        self.ram_write64(address, width, Some(hart))
    }

    fn ram_write64(
        &mut self,
        address: u64,
        width: usize,
        hart: Option<usize>,
    ) -> Option<&mut [u8]> {
        let (backing, offset) = self.ram_location(address, width, hart)?;
        let region = &mut self.regions[backing];
        if width != 0 {
            let address = region.base + offset as u64;
            for reservation in &mut self.rv64_reservations {
                if !self.renode_lrsc
                    && reservation.is_some_and(|reserved| {
                        reserved.backing == backing
                            && offset < reserved.offset + reserved.width
                            && reserved.offset < offset + width
                    })
                {
                    *reservation = None;
                }
            }
            for (slot, reservation) in self.reservations.iter_mut().enumerate() {
                if hart != Some(slot) && reservation.is_some_and(|(reserved, _)| {
                    let reserved = reserved & !3;
                    address < u64::from(reserved) + 4
                        && u64::from(reserved) < address + width as u64
                }) {
                    *reservation = None;
                }
            }
        }
        Some(region.bytes_mut(offset, width))
    }

    pub fn reserve64(&mut self, hart: usize, guest: u64, address: u64, width: usize) {
        self.rv64_reservations[hart] = self.reservation_key64(hart, guest, address, width);
    }

    fn reservation_key64(
        &self,
        hart: usize,
        guest: u64,
        address: u64,
        width: usize,
    ) -> Option<Reservation64> {
        let (backing, offset) = self.ram_location(address, width, Some(hart))?;
        Some(Reservation64 {
            guest,
            backing,
            offset,
            width,
        })
    }

    pub(crate) fn matches_reservation64(
        &self,
        hart: usize,
        reserved: Option<Reservation64>,
        guest: u64,
        address: u64,
        width: usize,
    ) -> bool {
        let Some((reserved, current)) =
            reserved.zip(self.reservation_key64(hart, guest, address, width))
        else {
            return false;
        };
        if self.renode_lrsc {
            // The fixed tlib bypasses its shared store table in a single-CPU machine.
            return reserved.guest == current.guest;
        }
        reserved.backing == current.backing
            && reserved.offset == current.offset
            && reserved.width == current.width
    }

    pub fn take_reservation64(&mut self, hart: usize) -> Option<Reservation64> {
        self.rv64_reservations[hart].take()
    }

    pub(crate) fn has_cpu_ram_alias(&self) -> bool {
        self.aliases.iter().any(|alias| alias.harts != 0)
    }

    pub fn ram_location64(&self, address: u64, width: usize) -> Option<(usize, usize)> {
        self.ram_location(address, width, None)
    }

    fn ram_location(
        &self,
        address: u64,
        width: usize,
        hart: Option<usize>,
    ) -> Option<(usize, usize)> {
        if let Some(hart) = hart {
            let end = address.checked_add(width as u64)?;
            if let Some(alias) = self.aliases.iter().find(|alias| {
                alias.harts & (1 << hart) != 0
                    && address < alias.base + alias.length as u64
                    && alias.base < end
            }) {
                let offset = address.checked_sub(alias.base)?;
                return (end <= alias.base + alias.length as u64)
                    .then_some((alias.backing, alias.offset + offset as usize));
            }
        }
        for (index, region) in self.regions.iter().enumerate() {
            if let Some(offset) = region.offset(u64::from(address), width) {
                return Some((index, offset));
            }
        }
        for alias in &self.aliases {
            if alias.harts == 0
                && let Some(offset) = address.checked_sub(alias.base)
                && offset <= alias.length as u64
                && width as u64 <= alias.length as u64 - offset
            {
                return Some((alias.backing, alias.offset + offset as usize));
            }
        }
        None
    }

    pub fn find_bytes(&self, pattern: &[u8], start: u64, end: u64) -> Result<Vec<u64>, String> {
        if pattern.is_empty() || pattern.len() > 1 << 20 {
            return Err("FindBytes pattern must contain 1 to 1048576 bytes".into());
        }

        let advance = |mut matched: usize, byte, prefix: &[usize]| {
            while matched != 0 && pattern[matched] != byte {
                matched = prefix[matched - 1];
            }
            matched + usize::from(pattern[matched] == byte)
        };
        let mut prefix = vec![0; pattern.len()];
        for index in 1..pattern.len() {
            prefix[index] = advance(prefix[index - 1], pattern[index], &prefix);
        }

        let mut mappings = Vec::new();
        for region in &self.regions {
            let bytes = &region.bytes[..region.mapped];
            mappings.push((region.base, bytes));
            mappings.extend(region.aliases.iter().map(|&base| (u64::from(base), bytes)));
        }
        for alias in &self.aliases {
            mappings.push((
                alias.base,
                &self.regions[alias.backing].bytes[..alias.length],
            ));
        }
        mappings.sort_unstable_by_key(|&(base, _)| base);

        let mut locations = Vec::new();
        let mut matched = 0;
        let mut previous_end = None;
        for (base, bytes) in mappings {
            let low = start.max(base);
            let high = end.saturating_add(1).min(base + bytes.len() as u64);
            if low >= high {
                continue;
            }
            // Carry a partial match only across adjacent mapped memory.
            if previous_end != Some(low) {
                matched = 0;
            }
            for (offset, &byte) in bytes[(low - base) as usize..(high - base) as usize]
                .iter()
                .enumerate()
            {
                matched = advance(matched, byte, &prefix);
                if matched == pattern.len() {
                    locations.push(low + offset as u64 + 1 - matched as u64);
                    matched = prefix[matched - 1];
                }
            }
            previous_end = Some(high);
        }
        Ok(locations)
}

    pub(crate) fn mapped_ram_ranges64(&self) -> impl Iterator<Item = std::ops::Range<u64>> + '_ {
        self.regions
            .iter()
            .filter(|region| region.mapped != 0)
            .map(|region| region.base..region.base + region.mapped as u64)
            .chain(
                self.aliases
                    .iter()
                    .filter(|alias| alias.harts == 0)
                    .map(|alias| alias.base..alias.base + alias.length as u64),
            )
    }

    pub fn ram64(&self, address: u64, width: usize) -> Option<&[u8]> {
        let (index, offset) = self.ram_location64(address, width)?;
        Some(&self.regions[index].bytes[offset..offset + width])
    }

    pub fn guest_memory(&mut self, address: u32, width: usize) -> Result<&mut [u8], BusError> {
        if self.ram_location64(u64::from(address), width).is_some()
            && let Some(device) = &mut self.h7_flash
        {
            device.note_program(address, width);
        }
        self.memory64(u64::from(address), width)
            .ok_or(BusError::Unmapped(address, width))
    }

    pub(crate) fn memory_for_store(
        &mut self,
        address: u32,
        width: usize,
        writer: Option<usize>,
    ) -> Result<&mut [u8], BusError> {
        if self.ram_location64(u64::from(address), width).is_some()
            && let Some(device) = &mut self.h7_flash
        {
            device.note_program(address, width);
        }
        self.ram_write64(u64::from(address), width, writer)
            .ok_or(BusError::Unmapped(address, width))
    }

    pub fn receive_ethernet(&mut self, nic: usize, frame: &[u8]) {
        let mut dma = EthernetDma {
            regions: &mut self.regions,
            reservations: &mut self.reservations,
        };
        self.ethernet[nic].receive(frame, &mut dma);
    }

    pub fn clear_reservation(&mut self, hart: usize) {
        self.reservations[hart] = None;
    }

    fn canonical_ram_address(&self, address: u32, width: usize) -> Option<u32> {
        let (region, offset) = self.ram_location64(u64::from(address), width)?;
        u32::try_from(self.regions[region].base + offset as u64).ok()
}

    pub(crate) fn cpu_ram64(&self, hart: usize, address: u64, width: usize) -> Option<&[u8]> {
        let (index, offset) = self.ram_location(address, width, Some(hart))?;
        Some(&self.regions[index].bytes[offset..offset + width])
    }

    pub fn load_reserved(&mut self, hart: usize, address: u32) -> Result<u32, BusError> {
        let value = self.read_ram::<4>(address)?;
        self.reservations[hart] = self
            .canonical_ram_address(address, 4)
            .map(|address| (address, 4));
        Ok(value)
    }

    pub fn store_conditional(
        &mut self,
        hart: usize,
        address: u32,
        value: u32,
    ) -> Result<u32, BusError> {
        let canonical = self.canonical_ram_address(address, 4);
        let reserved = self.reservations[hart]
            .take()
            .is_some_and(|reserved| Some(reserved) == canonical.map(|address| (address, 4)));
        if reserved {
            self.atomic_word(address)?
                .copy_from_slice(&value.to_le_bytes());
            Ok(0)
        } else {
            self.read_ram::<4>(address)?;
            Ok(1)
        }
    }

    pub fn thumb_load_exclusive<const N: usize>(&mut self, address: u32) -> Result<u32, String> {
        self.check_thumb_access(address, N, false, false)?;
        if address as usize % N != 0 {
            return Err(BusError::Misaligned(address, N).to_string());
        }
        let canonical = self.canonical_ram_address(address, N);
        let value = if N == 4 && (self.f4_rcc.is_some() || self.h7_network_control.is_some()) {
            self.read::<N>(address)
        } else {
            self.read_ram::<N>(address)
        }
        .map_err(|error| error.to_string())?;
        self.reservations[0] = Some((canonical.unwrap_or(address), N));
        Ok(value)
    }

    pub fn thumb_store_exclusive<const N: usize>(
        &mut self,
        address: u32,
        value: u32,
        output: &mut impl Write,
    ) -> Result<u32, String> {
        self.check_thumb_access(address, N, true, false)?;
        if address as usize % N != 0 {
            return Err(BusError::Misaligned(address, N).to_string());
        }
        let canonical = self
            .canonical_ram_address(address, N)
            .or_else(|| {
                (N == 4 && (self.f4_rcc.is_some() || self.h7_network_control.is_some()))
                    .then_some(address)
            })
            .ok_or_else(|| BusError::Unmapped(address, N).to_string())?;
        if self.reservations[0].take() != Some((canonical, N)) {
            return Ok(1);
        }
        self.write(address, N, value, output)
            .map_err(|error| error.to_string())?;
        Ok(0)
    }

    pub fn atomic_update(
        &mut self,
        address: u32,
        width: usize,
        update: impl FnOnce(u32) -> u32,
    ) -> Result<u32, BusError> {
        if address as usize & (width - 1) != 0 {
            return Err(BusError::Misaligned(address, width));
        }
        let memory = self.memory(address, width)?;
        let mut bytes = [0; 4];
        bytes[..width].copy_from_slice(memory);
        let previous = u32::from_le_bytes(bytes);
        memory.copy_from_slice(&update(previous).to_le_bytes()[..width]);
        Ok(previous)
    }

    pub fn atomic_compare_swap(
        &mut self,
        address: u32,
        width: usize,
        expected: u64,
        replacement: u64,
    ) -> Result<u64, BusError> {
        if address as usize & (width - 1) != 0 {
            return Err(BusError::Misaligned(address, width));
        }
        let memory = self.memory(address, width)?;
        let mut bytes = [0; 8];
        bytes[..width].copy_from_slice(memory);
        let previous = u64::from_le_bytes(bytes);
        if previous == expected & (u64::MAX >> (64 - width * 8)) {
            memory.copy_from_slice(&replacement.to_le_bytes()[..width]);
        }
        Ok(previous)
    }

    fn atomic_word(&mut self, address: u32) -> Result<&mut [u8; 4], BusError> {
        if address & 3 != 0 {
            return Err(BusError::Misaligned(address, 4));
        }
        Ok(self.memory(address, 4)?.try_into().unwrap())
    }

    pub fn thumb_div_zero_trap(&self) -> bool {
        self.nvic.as_ref().is_some_and(Nvic::div_zero_trap)
    }

    pub fn check_thumb_access(
        &self,
        address: u32,
        width: usize,
        write: bool,
        execute: bool,
    ) -> Result<(), String> {
        if self
            .nvic
            .as_ref()
            .is_some_and(|nvic| !nvic.thumb_mpu_permits(address, width, write, execute))
        {
            return Err(format!("Cortex-M MPU denied access at {address:#010x}"));
        }
        Ok(())
    }

    pub fn thumb_fetch(&self, address: u32) -> Result<u32, String> {
        self.check_thumb_access(address, 2, false, true)?;
        self.read_ram::<2>(address)
            .map_err(|error| error.to_string())
    }

    pub fn thumb_read<const N: usize>(&mut self, address: u32) -> Result<u32, String> {
        self.check_thumb_access(address, N, false, false)?;
        if address as usize & (N - 1) != 0
            && self.nvic.as_ref().is_some_and(Nvic::allows_unaligned_data)
            && let Ok(slice) = self.memory_slice(address, N)
        {
            let mut value = [0; 4];
            value[..N].copy_from_slice(slice);
            return Ok(u32::from_le_bytes(value));
        }
        self.read::<N>(address).map_err(|error| error.to_string())
    }

    pub fn thumb_write(
        &mut self,
        address: u32,
        width: usize,
        value: u32,
        uart: &mut impl Write,
    ) -> Result<(), String> {
        self.check_thumb_access(address, width, true, false)?;
        if address as usize & (width - 1) != 0
            && self.nvic.as_ref().is_some_and(Nvic::allows_unaligned_data)
            && let Ok(bytes) = self.memory(address, width)
        {
            bytes.copy_from_slice(&value.to_le_bytes()[..width]);
            return Ok(());
        }
        self.write(address, width, value, uart)
            .map_err(|error| error.to_string())
    }

    pub fn read<const N: usize>(&mut self, address: u32) -> Result<u32, BusError> {
        // Mapped devices cannot overlap RAM. Try the common RAM access first.
        let ram_error = match self.read_ram::<N>(address) {
            Ok(value) => return Ok(value),
            Err(error) => error,
        };
        if N == 4
            && address & 3 == 0
            && let Some(device) = self
                .nrf_peripherals
                .iter_mut()
                .find(|device| device.contains(address))
        {
            return device.read(address).ok_or(BusError::Unmapped(address, N));
        }
        if N == 4
            && let Some(system) = &self.f4_system
            && let Some(rcc) = &self.f4_rcc
            && let Some(value) = system.read(address, rcc)
        {
            return Ok(value);
        }
        if N == 4
            && address & 3 == 0
            && let Some((target, bit)) = self.bitband(address)
        {
            return self
                .read::<4>(target)
                .map(|value| u32::from(value & bit != 0));
        }
        if address as usize & (N - 1) == 0 {
            if let Some(rcc) = &self.f4_rcc
                && rcc.contains(address)
            {
                return Ok(rcc.read(address, N));
            }
            if self.f4_access_blocked(address) {
                return Ok(0);
            }
        }
        if N == 4
            && address & 3 == 0
            && let Some(uart) = self.uart.as_mut()
            && uart.contains(address)
        {
            return Ok(uart.read(address));
        }
        if N == 4
            && let Some(clint) = self.clint.as_ref()
            && address & 3 == 0
            && clint.contains(address)
        {
            return Ok(clint.read(address));
        }
        if N == 4 && address & 3 == 0 {
            if let Some((base, gpio)) = &self.gpio
                && address.wrapping_sub(*base) < 0xc
            {
                return Ok(gpio.read(address - *base));
            }
            if let Some((base, timer)) = &self.timer
                && address.wrapping_sub(*base) < 0x5c
            {
                return Ok(timer.read(address - *base));
            }
        }
        if let Some(uart) = self.ns16550.as_mut()
            && let Some(register) = uart.register(address, N)
        {
            return Ok(uart.read(register));
        }
        if N == 4
            && address & 3 == 0
            && let Some(plic) = self.plic.as_mut()
            && plic.contains(address)
        {
            return Ok(plic.read(address));
        }
        if let Some(uart) = self.litex_uart.as_mut()
            && let Some(offset) = uart.offset(address, N)
            && (N == 1 || address & 3 == 0)
        {
            return Ok(uart.read(offset));
        }
        if (N == 1 || N == 4 && address & 3 == 0)
            && let Some(device) = self.litex_timer.as_ref()
            && let Some(offset) = litex_offset(&device.bases, device.timer.size(), address)
        {
            return Ok(device.timer.read(offset, N));
        }
        if N == 4
            && address & 3 == 0
            && let Some(timer) = self.litex_cpu_timer.as_ref()
            && timer.contains(address)
        {
            return Ok(timer.read(address));
        }
        if N == 4
            && address & 3 == 0
            && let Some(soc) = self.litex_soc.as_ref()
            && soc.contains(address)
        {
            return Ok(soc.read(address));
        }
        if N == 4
            && let Some(timer) = self.systick.as_mut()
            && let Some(value) = timer.read(address)
        {
            return Ok(value);
        }
        if address as usize & (N - 1) == 0
            && let Some(value) = self.nvic.as_ref().and_then(|nvic| nvic.read(address, N))
        {
            return Ok(value);
        }
        if address as usize & (N - 1) == 0
            && let Some(uart) = self
                .stm32_uarts
                .iter_mut()
                .find(|device| device.contains(address))
        {
            return Ok(uart.read(address, N));
        }
        if N == 4
            && address & 3 == 0
            && let Some(exti) = self.h7_exti.as_ref()
            && exti.contains(address)
        {
            return Ok(exti.read(address));
        }
        if N == 4
            && address & 3 == 0
            && let Some(exti) = self.exti.as_ref()
            && exti.contains(address)
        {
            return Ok(exti.read(address));
        }
        if N == 4
            && address & 3 == 0
            && let Some(rtc) = self.stm32_rtc.as_mut()
            && rtc.contains(address)
        {
            return Ok(rtc.read(address));
        }
        if address as usize & (N - 1) == 0
            && let Some(flash) = self.stm32_flash.as_ref()
            && flash.contains_options(address, N)
        {
            return Ok(flash.read_options(address, N));
        }
        if N == 4
            && address & 3 == 0
            && let Some(flash) = self.stm32_flash.as_ref()
            && flash.contains(address)
        {
            return Ok(flash.read(address));
        }
        if N == 4
            && address & 3 == 0
            && let Some(rcc) = self.stm32_rcc.as_ref()
            && rcc.contains(address)
        {
            return Ok(rcc.read(address));
        }
        if N == 4
            && address & 3 == 0
            && let Some(can) = self.stm32_cans.iter().find(|can| can.contains(address))
        {
            return Ok(can.read(address));
        }
        if address & 3 == 0
            && let Some(i2c) = self.stm32_i2cs.iter_mut().find(|i2c| i2c.contains(address))
        {
            let mask = u32::MAX >> (32 - N * 8);
            return Ok(i2c.read(address) & mask);
        }
        if address as usize & (N - 1) == 0
            && let Some(spi) = self.stm32_spis.iter_mut().find(|spi| spi.contains(address))
        {
            let mask = u32::MAX >> (32 - N * 8);
            return Ok(spi.read(address, N) & mask);
        }
        if matches!(N, 2 | 4)
            && address as usize & (N - 1) == 0
            && let Some(timer) = self
                .stm32_timers
                .iter()
                .find(|timer| timer.contains(address))
        {
            let value = timer.read(address & !3);
            return Ok(if N == 2 {
                (value >> ((address & 2) * 8)) & 0xffff
            } else {
                value
            });
        }
        if address as usize & (N - 1) == 0
            && let Some(value) = self.litex_io.iter().find_map(|(bases, device)| {
                let offset = device.offset(bases, address, N)?;
                Some(device.read(offset, N))
            })
        {
            return Ok(value);
        }
        if address as usize & (N - 1) == 0
            && let Some(value) = self
                .ethernet
                .iter()
                .find_map(|device| device.read(address, N))
        {
            return Ok(value);
        }
        if N == 4
            && let Some(value) = self
                .h7_network_control
                .as_ref()
                .and_then(|control| control.read(address))
        {
            return Ok(value);
        }
        if address as usize & (N - 1) == 0
            && let Some(value) = self.constant_mmio.iter().find_map(|peripheral| {
                peripheral
                    .offset(address, N)
                    .map(|offset| peripheral.read(offset, N))
            })
        {
            return Ok(value);
        }
        if address as usize & (N - 1) == 0
            && let Some(eth) = self.litex_eth.as_mut()
            && let Some((region, offset)) = eth.locate(address, N)
        {
            return Ok(eth.read(region, offset, N));
        }
        if address as usize & (N - 1) == 0
            && let Some((bases, _, flash)) = self.litex_flash.as_ref()
            && let Some(offset) = litex_offset(bases, crate::litex_flash::Flash::SIZE, address)
        {
            let value = flash.read(offset & !3) >> ((offset & 3) * 8);
            return Ok(value & (u32::MAX >> ((4 - N) * 8)));
        }
        if N == 4
            && address & 3 == 0
            && let Some(panel) = self.litex_panel.as_ref()
            && let Some(offset) = litex_offset(&panel.bases, Panel::SIZE, address)
        {
            return Ok(panel.read(offset));
        }
        if N == 4
            && address & 3 == 0
            && let Some(sd) = self.litex_sd.as_ref()
            && let Some((region, offset)) = sd.locate(address)
        {
            return Ok(sd.read(region, offset));
        }
        if N == 4
            && address & 3 == 0
            && let Some(video) = self.litex_video.as_ref()
            && let Some(value) = video.read(address)
        {
            return Ok(value);
        }
        if (N == 1 || (N == 4 && address & 3 == 0))
            && let Some(value) = self
                .litex_i2s
                .iter_mut()
                .find_map(|i2s| i2s.read(address & !3))
        {
            return Ok(if N == 1 {
                (value >> ((address & 3) * 8)) & 0xff
            } else {
                value
            });
        }
        if N == 4
            && let Some(value) = self.dwt.as_ref().and_then(|device| device.read(address))
        {
            return Ok(value);
        }
        if address as usize & (N - 1) == 0
            && let Some(device) = self.f4_power.as_ref()
            && device.contains(address)
        {
            return Ok(device.read(address, N));
        }
        if address as usize & (N - 1) == 0
            && let Some(value) = self
                .f4_flash
                .as_ref()
                .and_then(|device| device.read(address, N))
        {
            return Ok(value);
        }
        if N == 4
            && address & 3 == 0
            && let Some(device) = self.h7_flash.as_ref()
            && device.contains(address)
        {
            return Ok(device.read(address));
        }
        if N == 4 && address & 3 == 0 {
            let control = self.h7_network_control.as_ref();
            if let Some(device) = self
                .h7_adc
                .iter_mut()
                .find(|device| device.contains(address))
            {
                return Ok(if adc_clocked(control, device) {
                    device.read(address)
                } else {
                    0
                });
            }
            if let Some(device) = self
                .h7_adc_dma
                .iter()
                .find(|device| device.contains(address))
            {
                return Ok(if dma_clocked(control, device) {
                    device.read(address)
                } else {
                    0
                });
            }
            if let Some(device) = self
                .h7_dma_mux
                .iter()
                .find(|device| device.contains(address))
            {
                return Ok(device.read(address));
            }
        }
        if N == 4
            && address & 3 == 0
            && let Some(device) = self.h7_bdma.iter().find(|device| device.contains(address))
        {
            return Ok(if bdma_clocked(self.h7_network_control.as_ref(), device) {
                device.read(address)
            } else {
                0
            });
        }
        if matches!(N, 2 | 4)
            && address as usize & (N - 1) == 0
            && let Some(gpio) = self
                .stm32_gpios
                .iter_mut()
                .find(|gpio| gpio.contains(address))
        {
            return Ok(if N == 2 {
                gpio.read_halfword(address)
            } else {
                gpio.read(address)
            });
        }
        if matches!(N, 2 | 4)
            && address as usize & (N - 1) == 0
            && let Some(port) = self.h7_gpio.iter_mut().find(|port| port.contains(address))
        {
            return Ok(if !gpio_clocked(self.h7_network_control.as_ref(), port) {
                0
            } else if N == 2 {
                port.read_halfword(address)
            } else {
                port.read(address)
            });
        }
        if N == 4
            && address & 3 == 0
            && let Some(dma) = &self.stm32_dma
            && dma.contains(address)
        {
            return Ok(dma.read(address));
        }
        if matches!(N, 2 | 4)
            && address & 3 == 0
            && let Some(adc) = self.stm32_adc.as_mut()
            && adc.contains(address)
        {
            return Ok(adc.read(address));
        }
        if address as usize & (N - 1) == 0
            && let Some(crc) = &self.stm32_crc
            && crc.contains(address)
        {
            return Ok(crc.read(address, N));
        }
        if N == 4
            && address & 3 == 0
            && let Some(rtc) = self
                .h7_rtc
                .iter_mut()
                .find(|device| device.contains(address))
        {
            return Ok(if rtc_clocked(self.h7_network_control.as_ref(), rtc) {
                rtc.read(address)
            } else {
                0
            });
        }
        if address as usize & (N - 1) == 0
            && let Some(spi) = self.f4_spi.iter_mut().find(|spi| spi.contains(address))
        {
            return Ok(spi.read(address, N));
        }
        if matches!(N, 1 | 2 | 4)
            && address as usize & (N - 1) == 0
            && let Some(spi) = self.h7_spi.iter_mut().find(|spi| spi.contains(address))
        {
            return Ok(if spi_clocked(self.h7_network_control.as_ref(), spi) {
                spi.read(address) & (u32::MAX >> (32 - 8 * N))
            } else {
                0
            });
        }
        if address as usize & (N - 1) == 0
            && let Some(i2c) = self.f1_i2c.iter_mut().find(|i2c| i2c.contains(address))
        {
            return Ok(i2c.read(address, N));
        }
        if matches!(N, 1 | 2 | 4)
            && address as usize & (N - 1) == 0
            && let Some(i2c) = self.h7_i2c.iter_mut().find(|i2c| i2c.contains(address))
        {
            return Ok(if i2c_clocked(self.h7_network_control.as_ref(), i2c) {
                i2c.read(address) & (u32::MAX >> (32 - 8 * N))
            } else {
                0
            });
        }
        if N == 4
            && address & 3 == 0
            && let Some(rng) = self.h7_rng.iter_mut().find(|rng| rng.contains(address))
        {
            return Ok(if rng_clocked(self.h7_network_control.as_ref(), rng) {
                rng.read(address)
            } else {
                0
            });
        }
        if address as usize & (N - 1) == 0
            && let Some(qspi) = self.h7_qspi.iter_mut().find(|qspi| qspi.contains(address))
        {
            let readable = N == 4 || address - qspi.base == Qspi::DATA;
            return Ok(
                if readable && qspi_clocked(self.h7_network_control.as_ref(), qspi) {
                    qspi.read(address, N)
                } else {
                    0
                },
            );
        }
        if matches!(N, 2 | 4)
            && address as usize & (N - 1) == 0
            && let Some(timer) = self
                .h7_timers
                .iter_mut()
                .find(|timer| timer.contains(address))
        {
            return Ok(if !timer_clocked(self.h7_network_control.as_ref(), timer) {
                0
            } else if N == 2 {
                timer.read_halfword(address)
            } else {
                timer.read(address)
            });
        }
        if address as usize & (N - 1) == 0
            && let Some(crc) = self.h7_crc.iter().find(|crc| crc.contains(address))
        {
            return Ok(if crc_clocked(self.h7_network_control.as_ref(), crc) {
                crc.read(address, N)
            } else {
                0
            });
        }
        if N == 4
            && address & 3 == 0
            && let Some(index) = self.stm32_can.iter().position(|can| can.contains(address))
        {
            let can = &self.stm32_can[index];
            let offset = address - can.base;
            return Ok(if offset >= 0x200 {
                self.can_filters(index).read(offset)
            } else {
                can.read(offset)
            });
        }
        if N == 4
            && address & 3 == 0
            && let Some(fdcan) = self.h7_fdcan.iter().find(|fdcan| fdcan.contains(address))
        {
            return Ok(fdcan.read(address));
        }
        if N == 4
            && address & 3 == 0
            && let Some(device) = self.h7_dma2d.iter().find(|device| device.contains(address))
        {
            return Ok(if dma2d_clocked(self.h7_network_control.as_ref(), device) {
                device.read(address)
            } else {
                0
            });
        }
        if N == 4
            && address & 3 == 0
            && let Some(device) = self
                .h7_ltdc
                .as_ref()
                .filter(|device| device.contains(address))
        {
            return Ok(if ltdc_clocked(self.h7_network_control.as_ref(), device) {
                device.read(address)
            } else {
                0
            });
        }
        if matches!(N, 1 | 2 | 4)
            && address as usize & (N - 1) == 0
            && let Some(timer) = self
                .h7_lptimers
                .iter()
                .find(|timer| timer.contains(address))
        {
            return Ok(if lptim_clocked(self.h7_network_control.as_ref(), timer) {
                (timer.read(address & !3) >> ((address & 3) * 8)) & (u32::MAX >> (32 - N * 8))
            } else {
                0
            });
        }
        if matches!(N, 2 | 4)
            && address & 3 == 0
            && let Some(device) = self.iwdgs.iter().find(|device| device.contains(address))
        {
            return Ok(device.read(address));
        }
        if matches!(N, 1 | 2 | 4)
            && address as usize & (N - 1) == 0
            && let Some(device) = self
                .h7_hsem
                .as_mut()
                .filter(|device| device.contains(address))
        {
            return Ok(if hsem_clocked(self.h7_network_control.as_ref(), device) {
                device.read(address, N)
            } else {
                0
            });
        }
        if N == 4
            && address & 3 == 0
            && let Some(device) = self
                .h7_sdmmc
                .iter_mut()
                .find(|device| device.contains(address))
        {
            return Ok(device.read(address));
        }
        if N == 4
            && address & 3 == 0
            && let Some(device) = self
                .designware_usb
                .iter()
                .find(|device| device.contains(address))
        {
            return Ok(device.read(address));
        }
        if N == 4
            && address & 3 == 0
            && let Some(device) = self
                .h7_crypto
                .iter_mut()
                .find(|device| device.contains(address))
        {
            return Ok(device.read(address));
        }
        if N == 4
            && address & 3 == 0
            && let Some(device) = self
                .h7_dsi
                .iter_mut()
                .find(|device| device.contains(address))
        {
            return Ok(device.read(address));
        }
        match ram_error {
            error @ BusError::Unmapped(..) => self
                .nrf_tag(address, N)
                .map(|value| value & (u32::MAX >> (32 - N * 8)))
                .or_else(|| {
                    self.bus_tag(address, N)
                        .map(|value| value & (u32::MAX >> (32 - N * 8)))
                })
                .or_else(|| {
                    (self.renode_unmapped_data
                        || self.zero_mmio(address, N)
                        || self.stm32f0_tagged(address, N))
                    .then_some(0)
                })
                .ok_or(error),
            error => Err(error),
        }
    }

    pub fn read_ram<const N: usize>(&self, address: u32) -> Result<u32, BusError> {
        let mut bytes = [0; 4];
        bytes[..N].copy_from_slice(&self.read_ram_bytes::<N>(address)?);
        Ok(u32::from_le_bytes(bytes))
    }

    pub fn read_ram_bytes<const N: usize>(&self, address: u32) -> Result<[u8; N], BusError> {
        if address as usize & (N - 1) != 0 {
            return Err(BusError::Misaligned(address, N));
        }
        self.read_unaligned_ram::<N>(address)
    }

    fn read_unaligned_ram<const N: usize>(&self, address: u32) -> Result<[u8; N], BusError> {
        for region in &self.regions {
            if let Some(offset) = region.offset(u64::from(address), N) {
                let mut bytes = [0; N];
                if region.flash_pages.is_some() {
                    for (index, byte) in bytes[..N].iter_mut().enumerate() {
                        *byte = region.flash_read(offset + index);
                    }
                } else {
                    bytes[..N].copy_from_slice(&region.bytes[offset..offset + N]);
                }
                return Ok(bytes);
            }
        }
        Err(BusError::Unmapped(address, N))
    }

    pub fn write(
        &mut self,
        address: u32,
        width: usize,
        value: u32,
        uart: &mut impl Write,
    ) -> Result<(), BusError> {
        self.write_with_origin(address, width, value, uart, None)
    }

    pub fn write_with_origin(
        &mut self,
        address: u32,
        width: usize,
        value: u32,
        uart: &mut impl Write,
        writer: Option<usize>,
    ) -> Result<(), BusError> {
        if self.uart_tx == Some(address) && width == 1 {
            return uart.write_all(&[value as u8]).map_err(BusError::Output);
        }
        if width == 4
            && let Some(system) = &mut self.f4_system
            && let Some(rcc) = &self.f4_rcc
            && system.write(address, value, rcc)
        {
            return Ok(());
        }
        if address as usize & (width - 1) != 0 {
            return Err(BusError::Misaligned(address, width));
        }
        let ram_error = if self
            .stm32_flash
            .as_ref()
            .is_some_and(|flash| flash.contains_bank(address))
        {
            BusError::Unmapped(address, width)
        } else {
            match self.memory_for_store(address, width, writer) {
                Ok(bytes) => {
                    bytes.copy_from_slice(&value.to_le_bytes()[..width]);
                    return self.observe_tohost(address, width, uart);
                }
                Err(error) => error,
            }
        };
        if width == 4
            && let Some(index) = self
                .nrf_peripherals
                .iter()
                .position(|device| device.contains(address))
        {
            let mut device = self.nrf_peripherals.remove(index);
            let result = device.write(address, value, self, uart);
            self.nrf_peripherals.insert(index, device);
            return result;
        }
        if width == 4
            && let Some((target, bit)) = self.bitband(address)
        {
            let prior = self.read::<4>(target)?;
            return self.write(
                target,
                4,
                (prior & !bit) | if value & 1 != 0 { bit } else { 0 },
                uart,
            );
        }
        if self
            .f4_rcc
            .as_ref()
            .is_some_and(|rcc| rcc.contains(address))
        {
            self.write_f4_rcc(address, width, value);
            return Ok(());
        }
        if self.f4_access_blocked(address) {
            return Ok(());
        }
        if width == 4
            && let Some(device) = self.uart.as_mut()
            && device.contains(address)
        {
            return device.write(address, value, uart).map_err(BusError::Output);
        }
        if width == 4
            && let Some(clint) = self.clint.as_mut()
            && clint.contains(address)
        {
            clint.write(address, value);
            return Ok(());
        }
        if width == 4 {
            if let Some((base, gpio)) = &mut self.gpio
                && address.wrapping_sub(*base) < 0xc
            {
                gpio.write(address - *base, value);
                return Ok(());
            }
            if let Some((base, timer)) = &mut self.timer
                && address.wrapping_sub(*base) < 0x5c
            {
                timer.write(address - *base, value);
                return Ok(());
            }
        }
        if let Some(device) = self.ns16550.as_mut()
            && let Some(register) = device.register(address, width)
        {
            return device
                .write(register, value, uart)
                .map_err(BusError::Output);
        }
        if width == 4
            && let Some(plic) = self.plic.as_mut()
            && plic.contains(address)
        {
            plic.write(address, value);
            return Ok(());
        }
        if let Some(device) = self.litex_uart.as_mut()
            && let Some(offset) = device.offset(address, width)
        {
            return device.write(offset, value, uart).map_err(BusError::Output);
        }
        if matches!(width, 1 | 4)
            && let Some(device) = self.litex_timer.as_mut()
            && let Some(offset) = litex_offset(&device.bases, device.timer.size(), address)
        {
            device.timer.write(offset, width, value);
            return Ok(());
        }
        if width == 4
            && let Some(timer) = self.litex_cpu_timer.as_mut()
            && timer.contains(address)
        {
            timer.write(address, value);
            return Ok(());
        }
        if width == 4
            && let Some(soc) = self.litex_soc.as_mut()
            && soc.contains(address)
        {
            soc.write(address, value);
            return Ok(());
        }
        if width == 4
            && let Some(timer) = self.systick.as_mut()
        {
            let before = timer.pending_exceptions();
            if timer.write(address, value) {
                if timer.pending_exceptions() & !before != 0
                    && let Some(nvic) = &mut self.nvic
                {
                    nvic.signal_pending_event();
                }
                return Ok(());
            }
        }
        if address == VTOR && width == 4 {
            let base = value & !0x7f;
            let mapped = self.read_ram::<4>(base).is_ok();
            if let Some(nvic) = self.nvic.as_mut() {
                if mapped {
                    nvic.set_vector_base(base);
                }
                return Ok(());
            }
        }
        if self
            .nvic
            .as_mut()
            .is_some_and(|nvic| nvic.write(address, width, value))
        {
            return Ok(());
        }
        if let Some((index, device)) = self
            .stm32_uarts
            .iter_mut()
            .enumerate()
            .find(|(_, device)| device.contains(address))
        {
            return device
                .write(address, width, value, |byte| {
                    uart.write_all(&[byte])?;
                    if let Some(Some(history)) = self.stm32_uart_history.get_mut(index) {
                        history.receive(byte);
                    }
                    Ok(())
                })
                .map_err(BusError::Output);
        }
        if width == 4
            && let Some(exti) = self.h7_exti.as_mut()
            && exti.contains(address)
        {
            exti.write(address, value);
            return Ok(());
        }
        if width == 4
            && let Some(exti) = self.exti.as_mut()
            && exti.contains(address)
        {
            exti.write(address, value);
            return Ok(());
        }
        if width == 4
            && let Some(rtc) = self.stm32_rtc.as_mut()
            && rtc.contains(address)
        {
            rtc.write(address, value);
            return Ok(());
        }
        if let Some(flash) = self.stm32_flash.as_mut()
            && flash.contains_options(address, width)
        {
            flash.program_option(address, width, value);
            return Ok(());
        }
        if width == 4
            && let Some(flash) = self.stm32_flash.as_mut()
            && flash.contains(address)
        {
            if let Some(range) = flash.write(address, value) {
                let result = self.memory(range.start, (range.end - range.start) as usize);
                let success = result.map(|bytes| bytes.fill(0xff)).is_ok();
                self.stm32_flash.as_mut().unwrap().finish(success);
            }
            return Ok(());
        }
        for (bases, device) in &mut self.litex_io {
            if let Some(offset) = device.offset(bases, address, width) {
                device.write(offset, width, value);
                return Ok(());
            }
        }
        if let Some(eth) = self.litex_eth.as_mut()
            && let Some((region, offset)) = eth.locate(address, width)
        {
            eth.write(region, offset, width, value);
            return Ok(());
        }
        if let Some((bases, backing, flash)) = self.litex_flash.as_mut()
            && let Some(offset) = litex_offset(bases, crate::litex_flash::Flash::SIZE, address)
        {
            if offset & 3 == 0 {
                flash.write(offset, value, &mut self.regions[*backing]);
            }
            return Ok(());
        }
        if width == 4
            && let Some(rcc) = self.stm32_rcc.as_mut()
            && rcc.contains(address)
        {
            rcc.write(address, value);
            return Ok(());
        }
        if width == 4
            && let Some(can) = self.stm32_cans.iter_mut().find(|can| can.contains(address))
        {
            can.write(address, value);
            return Ok(());
        }
        if address & 3 == 0
            && let Some(i2c) = self.stm32_i2cs.iter_mut().find(|i2c| i2c.contains(address))
        {
            let mask = u32::MAX >> (32 - width * 8);
            i2c.write(address, value & mask);
            return Ok(());
        }
        if let Some(spi) = self.stm32_spis.iter_mut().find(|spi| spi.contains(address)) {
            let mask = u32::MAX >> (32 - width * 8);
            spi.write(address, width, value & mask, self.led_events);
            return Ok(());
        }
        if matches!(width, 2 | 4)
            && let Some(timer) = self
                .stm32_timers
                .iter_mut()
                .find(|timer| timer.contains(address))
        {
            if width == 2 {
                timer.write_halfword(address, value);
            } else {
                timer.write(address, value);
            }
            return Ok(());
        }
        if matches!(width, 2 | 4)
            && let Some(gpio) = self
                .stm32_gpios
                .iter_mut()
                .find(|gpio| gpio.contains(address))
        {
            if width == 2 {
                gpio.write_halfword(address, value);
            } else {
                gpio.write(address, value);
            }
            self.refresh_gpio_wiring();
            return Ok(());
        }
        if width == 4
            && let Some(dma) = self.stm32_dma.as_mut()
            && dma.contains(address)
        {
            dma.write(address, value);
            return Ok(());
        }
        if width == 4
            && let Some(adc) = self.stm32_adc.as_mut()
            && adc.contains(address)
        {
            adc.write(address, value);
            return Ok(());
        }
        if let Some(crc) = self.stm32_crc.as_mut()
            && crc.contains(address)
        {
            crc.write(address, width, value);
            return Ok(());
        }
        if self
            .stm32_flash
            .as_ref()
            .is_some_and(|flash| flash.contains_bank(address))
        {
            let old = self.read_ram::<2>(address & !1)? as u16;
            let flash = self.stm32_flash.as_mut().unwrap();
            if let Some(value) = flash.program(address, width, old, value as u16) {
                self.memory(address, 2)?
                    .copy_from_slice(&value.to_le_bytes());
            }
            return Ok(());
        }
        if width == 4
            && let Some(panel) = self.litex_panel.as_mut()
            && let Some(offset) = litex_offset(&panel.bases, Panel::SIZE, address)
        {
            panel.write(offset, value);
            return Ok(());
        }
        if width == 4
            && let Some((region, offset)) = self.litex_sd.as_ref().and_then(|sd| sd.locate(address))
        {
            let mut sd = self.litex_sd.take().unwrap();
            sd.write(region, offset, value, self);
            self.litex_sd = Some(sd);
            return Ok(());
        }
        if width == 4
            && let Some(control) = &mut self.h7_network_control
            && let Some(change) = control.write(address, value)
        {
            for adc in &mut self.h7_adc {
                if adc.clock.is_some_and(|clock| control.adc_in_reset(clock)) {
                    adc.reset();
                }
            }
            for dma in &mut self.h7_adc_dma {
                if control.dma_in_reset(dma.clock_mask) {
                    dma.reset();
                }
            }
            if control.bdma_in_reset() {
                for dma in self.h7_bdma.iter_mut().filter(|device| device.rcc_bound) {
                    dma.reset();
                }
            }
            for gpio in &mut self.h7_gpio {
                if gpio.rcc_pin.is_some_and(|port| control.gpio_in_reset(port)) {
                    gpio.reset();
                }
            }
            if control.rtc_in_reset() {
                for rtc in self.h7_rtc.iter_mut().filter(|rtc| rtc.rcc_bound) {
                    rtc.reset();
                }
            }
            for i2c in &mut self.h7_i2c {
                if i2c.clock.is_some_and(|clock| control.i2c_in_reset(clock)) {
                    i2c.reset();
                }
            }
            for timer in &mut self.h7_lptimers {
                if timer
                    .clock
                    .is_some_and(|clock| control.lptim_in_reset(clock))
                {
                    timer.reset();
                }
            }
            for timer in &mut self.h7_timers {
                if timer
                    .clock
                    .is_some_and(|clock| control.timer_in_reset(clock))
                {
                    timer.reset();
                }
            }
            if control.crc_in_reset() {
                for crc in self.h7_crc.iter_mut().filter(|crc| crc.rcc_bound) {
                    crc.reset();
                }
            }
            if control.hsem_in_reset()
                && let Some(device) = self.h7_hsem.as_mut().filter(|device| device.rcc_bound)
            {
                device.reset();
            }
            if control.ltdc_in_reset()
                && let Some(device) = self.h7_ltdc.as_mut().filter(|device| device.rcc_bound)
            {
                device.reset();
            }
            if control.dma2d_in_reset() {
                for device in self.h7_dma2d.iter_mut().filter(|device| device.rcc_bound) {
                    device.reset();
                }
            }
            if control.qspi_in_reset() {
                for qspi in self.h7_qspi.iter_mut().filter(|qspi| qspi.rcc_bound) {
                    qspi.reset();
                }
            }
            if control.rng_in_reset() {
                for rng in self.h7_rng.iter_mut().filter(|rng| rng.rcc_bound) {
                    rng.reset();
                }
            }
            if control.spi4_in_reset() {
                for spi in self.h7_spi.iter_mut().filter(|spi| spi.rcc_bound) {
                    spi.reset();
                }
            }
            if let Some(nic) = control.nic
                && let Controller::Stm32h7(device) = &mut self.ethernet[nic]
            {
                match change {
                    Change::None => {}
                    Change::Clock(enabled) => device.set_clock(enabled),
                    Change::Reset => {
                        device.set_clock(false);
                        device.reset();
                    }
                    Change::Interface(interface) => device.set_phy_interface(interface),
                }
            }
            return Ok(());
        }
        let mut dma = EthernetDma {
            regions: &mut self.regions,
            reservations: &mut self.reservations,
        };
        if self
            .ethernet
            .iter_mut()
            .any(|device| device.write(address, width, value, &mut dma))
        {
            return Ok(());
        }
        if width == 4
            && self
                .litex_video
                .as_mut()
                .is_some_and(|video| video.write(address, value))
        {
            return Ok(());
        }
        if matches!(width, 1 | 4) {
            for i2s in &mut self.litex_i2s {
                let value = if width == 1 {
                    (value & 0xff) << ((address & 3) * 8)
                } else {
                    value
                };
                if i2s
                    .write(address & !3, value)
                    .map_err(BusError::AudioOutput)?
                {
                    return Ok(());
                }
            }
        }
        if width == 4
            && self
                .dwt
                .as_mut()
                .is_some_and(|device| device.write(address, value))
        {
            return Ok(());
        }
        if let Some((peripheral, offset)) = self.constant_mmio.iter().find_map(|peripheral| {
            peripheral
                .offset(address, width)
                .map(|offset| (peripheral, offset))
        }) {
            peripheral.write(offset);
            return Ok(());
        }
        if let Some(device) = self.f4_power.as_mut()
            && device.contains(address)
        {
            device.write(address, width, value);
            return Ok(());
        }
        if let Some(device) = self.f4_flash.as_mut()
            && device.contains(address)
        {
            if let Some(range) = device.write(address, width, value) {
                self.memory(range.start, (range.end - range.start) as usize)?
                    .fill(0xff);
            }
            return Ok(());
        }
        if width == 4
            && let Some(device) = self.h7_flash.as_mut()
            && device.contains(address)
        {
            for range in device.write(address, value).into_iter().flatten() {
                self.memory(range.start, (range.end - range.start) as usize)?
                    .fill(0xff);
            }
            return Ok(());
        }
        if width == 4 {
            let control = self.h7_network_control.as_ref();
            if let Some(device) = self
                .h7_adc
                .iter_mut()
                .find(|device| device.contains(address))
            {
                if adc_clocked(control, device) {
                    device.write(address, value);
                }
                return Ok(());
            }
            if let Some(device) = self
                .h7_adc_dma
                .iter_mut()
                .find(|device| device.contains(address))
            {
                if dma_clocked(control, device) {
                    device.write(address, value);
                }
                return Ok(());
            }
            if let Some(device) = self
                .h7_dma_mux
                .iter_mut()
                .find(|device| device.contains(address))
            {
                device.write(address, value);
                return Ok(());
            }
        }
        if width == 4
            && let Some(device) = self
                .h7_bdma
                .iter_mut()
                .find(|device| device.contains(address))
        {
            if bdma_clocked(self.h7_network_control.as_ref(), device) {
                device.write(address, value);
            }
            return Ok(());
        }
        if matches!(width, 2 | 4)
            && let Some(port) = self.h7_gpio.iter_mut().find(|port| port.contains(address))
        {
            if gpio_clocked(self.h7_network_control.as_ref(), port) {
                if width == 2 {
                    port.write_halfword(address, value);
                } else {
                    port.write(address, value);
                }
            }
            return Ok(());
        }
        if width == 4
            && let Some(rtc) = self
                .h7_rtc
                .iter_mut()
                .find(|device| device.contains(address))
        {
            if rtc_clocked(self.h7_network_control.as_ref(), rtc) {
                let backup_unlocked = self.f4_rcc.as_ref().and_then(|rcc| rcc.rtc)
                    != Some(rtc.base)
                    || self.f4_power.as_ref().is_none_or(F4Power::backup_unlocked);
                rtc.write(address, value, backup_unlocked);
            }
            return Ok(());
        }
        if let Some(spi) = self.f4_spi.iter_mut().find(|spi| spi.contains(address)) {
            spi.write(address, width, value);
            if let Some(target) = spi.dma
                && f4_clocked(self.f4_rcc.as_ref(), target.base)
                && let Some(value) = spi.dma_value()
                && request_h7_stream(
                    target,
                    spi.base + F4Spi::DATA,
                    value,
                    self.h7_network_control.as_ref(),
                    &mut self.h7_adc_dma,
                    &mut self.h7_bdma,
                    &mut dma,
                )
            {
                spi.consume_rx();
            }
            return Ok(());
        }
        if let Some(spi) = self.h7_spi.iter_mut().find(|spi| spi.contains(address)) {
            let control = self.h7_network_control.as_ref();
            if spi_clocked(control, spi) {
                let frames = spi.write(address, value & (u32::MAX >> (32 - 8 * width)));
                if let Some(request) = spi.dma
                    && let Some(mux) = self
                        .h7_dma_mux
                        .iter()
                        .find(|mux| mux.base == request.endpoint)
                {
                    let source = spi.base + Spi::RXDR;
                    for _ in 0..frames {
                        for target in mux.targets(request.id) {
                            if request_h7_stream(
                                target,
                                source,
                                0,
                                control,
                                &mut self.h7_adc_dma,
                                &mut self.h7_bdma,
                                &mut dma,
                            ) {
                                spi.consume_rx();
                            }
                        }
                    }
                }
            }
            return Ok(());
        }
        if let Some(i2c) = self.f1_i2c.iter_mut().find(|i2c| i2c.contains(address)) {
            i2c.write(address, width, value);
            return Ok(());
        }
        if let Some(i2c) = self.h7_i2c.iter_mut().find(|i2c| i2c.contains(address)) {
            if i2c_clocked(self.h7_network_control.as_ref(), i2c) {
                i2c.write(address, value & (u32::MAX >> (32 - 8 * width)));
            }
            return Ok(());
        }
        if width == 4
            && let Some(rng) = self.h7_rng.iter_mut().find(|rng| rng.contains(address))
        {
            if rng_clocked(self.h7_network_control.as_ref(), rng) {
                rng.write(address, value);
            }
            return Ok(());
        }
        if let Some(qspi) = self.h7_qspi.iter_mut().find(|qspi| qspi.contains(address)) {
            if (width == 4 || address - qspi.base == Qspi::DATA)
                && qspi_clocked(self.h7_network_control.as_ref(), qspi)
            {
                qspi.write(address, width, value);
            }
            return Ok(());
        }
        if matches!(width, 2 | 4)
            && let Some(timer) = self
                .h7_timers
                .iter_mut()
                .find(|timer| timer.contains(address))
        {
            if timer_clocked(self.h7_network_control.as_ref(), timer) {
                if width == 2 {
                    timer.write_halfword(address, value);
                } else {
                    timer.write(address, value);
                }
            }
            return Ok(());
        }
        if let Some(crc) = self.h7_crc.iter_mut().find(|crc| crc.contains(address)) {
            if crc_clocked(self.h7_network_control.as_ref(), crc) {
                crc.write(address, width, value);
            }
            return Ok(());
        }
        if width == 4
            && let Some(index) = self.stm32_can.iter().position(|can| can.contains(address))
        {
            self.write_stm32_can(index, address, value);
            return Ok(());
        }
        if width == 4
            && let Some(fdcan) = self
                .h7_fdcan
                .iter_mut()
                .find(|fdcan| fdcan.contains(address))
        {
            fdcan.write(address, value);
            return Ok(());
        }
        if width == 4
            && let Some(device) = self
                .h7_dma2d
                .iter_mut()
                .find(|device| device.contains(address))
        {
            if dma2d_clocked(self.h7_network_control.as_ref(), device) {
                device.write(address, value, &mut dma);
            }
            return Ok(());
        }
        if width == 4
            && let Some(device) = self
                .h7_ltdc
                .as_mut()
                .filter(|device| device.contains(address))
        {
            if ltdc_clocked(self.h7_network_control.as_ref(), device) {
                device.write(address, value);
            }
            return Ok(());
        }
        if let Some(timer) = self
            .h7_lptimers
            .iter_mut()
            .find(|timer| timer.contains(address))
        {
            if lptim_clocked(self.h7_network_control.as_ref(), timer) {
                timer.write(address, width, value);
            }
            return Ok(());
        }
        if matches!(width, 2 | 4)
            && address & 3 == 0
            && let Some(device) = self
                .iwdgs
                .iter_mut()
                .find(|device| device.contains(address))
        {
            device.write(address, value);
            return Ok(());
        }
        if let Some(device) = self
            .h7_hsem
            .as_mut()
            .filter(|device| device.contains(address))
        {
            if hsem_clocked(self.h7_network_control.as_ref(), device) {
                device.write(address, width, value);
            }
            return Ok(());
        }
        if width == 4
            && let Some(device) = self
                .h7_sdmmc
                .iter_mut()
                .find(|device| device.contains(address))
        {
            device.write(address, value);
            return Ok(());
        }
        if width == 4
            && let Some(device) = self
                .designware_usb
                .iter_mut()
                .find(|device| device.contains(address))
        {
            device.write(address, value);
            return Ok(());
        }
        if width == 4
            && let Some(device) = self
                .h7_crypto
                .iter_mut()
                .find(|device| device.contains(address))
        {
            device.write(address, value);
            return Ok(());
        }
        if width == 4
            && let Some(device) = self
                .h7_dsi
                .iter_mut()
                .find(|device| device.contains(address))
        {
            device.write(address, value);
            return Ok(());
        }
        if self.renode_unmapped_data
            || self.zero_mmio(address, width)
            || self.stm32f0_tagged(address, width)
            || self.nrf_tag(address, width).is_some()
            || self.bus_tag(address, width).is_some()
        {
            return Ok(());
        }
        Err(ram_error)
    }
}

fn overlaps_range(start: u64, end: u64, base: u32, size: u32) -> bool {
    let base = u64::from(base);
    start < base + u64::from(size) && base < end
}

fn litex_offset(bases: &[u32], size: u32, address: u32) -> Option<u32> {
    bases.iter().find_map(|&base| {
        let offset = address.wrapping_sub(base);
        (offset < size).then_some(offset)
    })
}

#[cfg(test)]
mod stm32_tests {
    use super::*;

    #[test]
    fn litex_spi_loopback_and_gpio_use_shared_registers() {
        let mut bus = Bus::default();
        let spi = 0x6000_2000;
        let spi_shadow = 0xe000_2000;
        let gpio = 0x6000_5800;
        let gpio_shadow = 0xe000_5800;
        bus.map_litex_io(
            vec![spi, spi_shadow],
            litex::IoDevice::Spi(litex::Spi::default()),
        )
        .unwrap();
        bus.map_litex_io(
            vec![gpio, gpio_shadow],
            litex::IoDevice::Gpio(litex::Gpio::new(
                litex::GpioMode::InOut,
                false,
                None,
                Vec::new(),
                Vec::new(),
            )),
        )
        .unwrap();

        bus.write(spi_shadow + 4, 4, 1, &mut io::sink()).unwrap();
        assert_eq!(bus.read::<4>(spi + 0x10).unwrap(), 0xff);
        bus.write(spi + 0x18, 4, 1, &mut io::sink()).unwrap();
        bus.write(spi + 0x0c, 4, 0x134, &mut io::sink()).unwrap();
        bus.write(spi_shadow + 4, 4, 1, &mut io::sink()).unwrap();
        assert_eq!(bus.read::<4>(spi + 8).unwrap(), 1);
        assert_eq!(bus.read::<4>(spi_shadow + 0x10).unwrap(), 0x34);

        bus.write(gpio, 4, u32::MAX, &mut io::sink()).unwrap();
        bus.write(gpio + 4, 4, 0x55aa, &mut io::sink()).unwrap();
        assert_eq!(bus.read::<4>(gpio_shadow).unwrap(), 0);
        assert_eq!(bus.read::<4>(gpio_shadow + 4).unwrap(), 0x55aa);
    }

    #[test]
    fn ram_views_share_contents_and_reservations() {
        let mut bus = Bus::default();
        let base = 0x4000_0000;
        let shadow = 0xc000_0000;
        bus.map_ram_views(&[base, shadow], 0x1000).unwrap();
        assert!(bus.map_ram(shadow + 0x800, 0x1000).is_err());
        assert!(bus.map_ram_views(&[0, 0x800], 0x1000).is_err());
        bus.map_ram(0, 0x1000).unwrap();

        bus.memory(base + 4, 4)
            .unwrap()
            .copy_from_slice(&[1, 2, 3, 4]);
        assert_eq!(bus.load_reserved(0, shadow + 4).unwrap(), 0x0403_0201);
        assert_eq!(bus.store_conditional(0, base + 4, 42).unwrap(), 0);
        assert_eq!(bus.load_reserved(0, base + 4).unwrap(), 42);
        bus.write(shadow + 5, 1, 1, &mut io::sink()).unwrap();
        assert_eq!(bus.store_conditional(0, base + 4, 0).unwrap(), 1);
        assert_eq!(bus.read_ram::<4>(shadow + 4).unwrap(), 0x12a);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_a_line_to_the_named_stm32_usart() {
        let mut bus = Bus::default();
        let usart1 = 0x4001_3800;
        let usart2 = 0x4000_4400;
        bus.map_stm32_usart(Stm32Usart::new("usart1".into(), usart1, None, false))
            .unwrap();
        bus.map_stm32_usart(Stm32Usart::new("usart2".into(), usart2, None, false))
            .unwrap();
        for base in [usart1, usart2] {
            bus.write(base, 4, 0b101, &mut std::io::sink()).unwrap();
        }
        bus.write_stm32_uart_line("sysbus.usart2", "A", b"\r")
            .unwrap();
        assert_eq!(bus.read::<4>(usart1 + 0x1c).unwrap() & (1 << 5), 0);
        assert_eq!(bus.read::<4>(usart2 + 0x24).unwrap(), u32::from(b'A'));
        assert_eq!(bus.read::<4>(usart2 + 0x24).unwrap(), u32::from(b'\r'));
        assert!(
            bus.write_stm32_uart_line("sysbus.usart3", "A", b"\r")
                .is_err()
        );
    }

    #[test]
    fn uart_watch_ignores_other_ports_without_filtering_stdout() {
        let mut bus = Bus::default();
        let usart1 = 0x4001_3800;
        let usart2 = 0x4000_4400;
        let mut output = Vec::new();
        for (name, base) in [("usart1", usart1), ("usart2", usart2)] {
            bus.map_stm32_usart(Stm32Usart::new(name.into(), base, None, false))
                .unwrap();
            bus.write(base, 4, 0b1001, &mut output).unwrap();
        }
        assert!(
            bus.watch_stm32_uart("sysbus.usart3", "OK", UartWaitMode::Lf)
                .is_err()
        );
        bus.record_stm32_uart("sysbus.usart2");
        let index = bus
            .watch_stm32_uart("sysbus.usart2", "OK", UartWaitMode::Lf)
            .unwrap();
        for (base, bytes) in [
            (usart2, b"O".as_slice()),
            (usart1, b"OK\n"),
            (usart2, b"K\r"),
        ] {
            for &byte in bytes {
                bus.write(base + 0x28, 1, byte.into(), &mut output).unwrap();
            }
            assert!(!bus.stm32_uart_matched(index));
        }
        bus.write(usart2 + 0x28, 1, b'\n'.into(), &mut output)
            .unwrap();
        assert!(bus.stm32_uart_matched(index));
        assert_eq!(output, b"OOK\nK\r\n");
        bus.finish_stm32_uart_watch(index);
        assert!(!bus.stm32_uart_matched(index));
    }

    #[test]
    fn renode_stm32_pwm_duty_reaches_gpio_for_all_four_upstream_pulses() {
        use crate::stm32_timer::TimerPin;

        // STM32L072.robot: PWM Should Support GPIO Output, adapted to F072 TIM2.
        const GPIO: u32 = 0x4800_0000;
        const TIM2: u32 = 0x4000_0000;
        for (pulse, expected_percent) in [(5, 0), (85, 33), (127, 50), (250, 100)] {
            let mut bus = Bus::default();
            bus.map_nvic(0xc0, None).unwrap();
            bus.map_stm32_gpio(Stm32Gpio::new(GPIO, 0x400, [2, 0, 0], 8))
                .unwrap();
            let mut timer = Stm32Timer::new(TIM2, CORTEX_M_STEP_HZ as u32, u32::MAX, [0; 8]);
            timer.gpio_outputs.push((
                2,
                TimerPin {
                    port: GPIO,
                    pin: 0,
                    function: 0,
                },
            ));
            bus.map_stm32_timer(timer).unwrap();
            let mut output = io::sink();
            for (offset, value) in [
                (0x2c, 255),
                (0x3c, pulse),
                (0x1c, 6 << 4),
                (0x20, 1 << 8),
                (0, 1),
            ] {
                bus.write(TIM2 + offset, 4, value, &mut output).unwrap();
            }
            let mut high = 0u32;
            for _ in 0..256 {
                bus.sample_nvic(&ActiveExceptions::default(), false);
                high += bus.read::<4>(GPIO + 0x10).unwrap() & 1;
            }
            assert_eq!(high, pulse);
            assert!((high * 100).abs_diff(expected_percent * 256) <= 10 * 256);
        }
    }

    #[test]
    fn renode_stm32_pwm_input_capture_routes_period_and_pulse_to_gpio() {
        use crate::stm32_timer::TimerPin;

        // STM32F4_Discovery.robot: both PWM Capture cases use 1 kHz / 33%.
        const GPIO: u32 = 0x4800_0000;
        const SOURCE: u32 = 0x4000_0000;
        const CAPTURE: u32 = 0x4000_0400;
        for pair in [0, 2] {
            let mut bus = Bus::default();
            bus.map_nvic(0xc0, None).unwrap();
            bus.map_stm32_gpio(Stm32Gpio::new(GPIO, 0x400, [2, 0, 0], 8))
                .unwrap();
            let mut source = Stm32Timer::new(SOURCE, 1_000_000, u32::MAX, [0; 8]);
            source.gpio_outputs.push((
                0,
                TimerPin {
                    port: GPIO,
                    pin: 0,
                    function: 0,
                },
            ));
            bus.map_stm32_timer(source).unwrap();
            bus.map_stm32_timer(Stm32Timer::new(CAPTURE, 1_000_000, 0xffff, [0; 8]))
                .unwrap();
            bus.route_gpio_timer_input(GPIO, 0, CAPTURE, pair);
            let flags = 6 << pair;
            let first_ccr = CAPTURE + 0x34 + 4 * u32::from(pair);
            let mut output = io::sink();
            for (base, offset, value) in [
                (CAPTURE, 0x18 + 2 * u32::from(pair), 0x0201), // direct and paired indirect input
                (CAPTURE, 0x20, 0x31 << (pair * 4)),           // rising and falling
                (CAPTURE, 0, 1),
                (SOURCE, 0x2c, 999),
                (SOURCE, 0x34, 330),
                (SOURCE, 0x18, 0x60),
                (SOURCE, 0x20, 1),
                (SOURCE, 0, 1),
            ] {
                bus.write(base + offset, 4, value, &mut output).unwrap();
            }
            let mut rising = Vec::new();
            let mut falling = Vec::new();
            for _ in 0..135_000 {
                bus.sample_nvic(&ActiveExceptions::default(), false);
                let status = bus.read::<4>(CAPTURE + 0x10).unwrap();
                if status & (2 << pair) != 0 {
                    rising.push(bus.read::<4>(first_ccr).unwrap());
                }
                if status & (4 << pair) != 0 {
                    falling.push(bus.read::<4>(first_ccr + 4).unwrap());
                }
                if status & flags != 0 {
                    bus.write(CAPTURE + 0x10, 4, !flags, &mut output).unwrap();
                }
            }
            assert_eq!(rising.len(), 2);
            assert_eq!(falling.len(), 2);
            let period = rising[1] - rising[0];
            let pulse = falling[1] - rising[1];
            assert_eq!((period, pulse), (1000, 330));
            assert!((1_000_000u32 / period).abs_diff(1000) <= 25);
            assert!((pulse * 100 / period).abs_diff(33) <= 1);
        }
    }

    #[test]
    fn renode_stm32_flash_erase_modes_preserve_the_expected_ranges() {
        // Adapt stm32f4-erase.robot and STM32L072.robot to F072 page geometry.
        const FLASH: u32 = 0x0800_0000;
        const PAGE_SIZE: usize = 2048;
        const BANK_SIZE: usize = 0x20000;
        let mut bus = Bus::default();
        let controller = Stm32F0Flash::BASE;
        bus.map_ram(FLASH, BANK_SIZE).unwrap();
        bus.set_stm32f0_tags(vec![controller..=0x4002_23ff]);
        bus.map_stm32f072_flash().unwrap();
        let write = |bus: &mut Bus, offset, value| {
            bus.write(controller + offset, 4, value, &mut std::io::sink())
                .unwrap();
        };
        // Seed flash as an image loader, equivalent to the upstream fixture setup.
        let page = FLASH + PAGE_SIZE as u32;
        let markers = [
            FLASH,
            FLASH + 2 * PAGE_SIZE as u32,
            FLASH + BANK_SIZE as u32 - 4,
        ];
        for (address, word) in markers
            .into_iter()
            .zip([0xdead_beefu32, 0xcafe_babe, 0x4433_2211])
        {
            bus.memory(address, 4)
                .unwrap()
                .copy_from_slice(&word.to_le_bytes());
        }
        bus.memory(page, PAGE_SIZE).unwrap().fill(0x12);
        write(&mut bus, 4, 0x4567_0123);
        write(&mut bus, 4, 0xcdef_89ab);

        write(&mut bus, 0x14, page + 4);
        write(&mut bus, 0x10, 1 << 6); // STRT alone
        assert_ne!(bus.read::<4>(controller + 0x0c).unwrap() & (1 << 2), 0);
        assert!(
            bus.memory(page, PAGE_SIZE)
                .unwrap()
                .iter()
                .all(|&byte| byte == 0x12)
        );

        write(&mut bus, 0x0c, 1 << 2); // clear PGERR
        write(&mut bus, 0x10, (1 << 1) | (1 << 6)); // PER | STRT
        assert!(
            bus.memory(page, PAGE_SIZE)
                .unwrap()
                .iter()
                .all(|&byte| byte == 0xff)
        );
        for (address, word) in markers
            .into_iter()
            .zip([0xdead_beef, 0xcafe_babe, 0x4433_2211])
        {
            assert_eq!(bus.read::<4>(address).unwrap(), word);
        }

        write(&mut bus, 0x10, (1 << 2) | (1 << 6)); // MER | STRT
        assert!(
            bus.memory(FLASH, BANK_SIZE)
                .unwrap()
                .iter()
                .all(|&byte| byte == 0xff)
        );
    }

    #[test]
    fn stm32f072_flash_preserves_programmed_data_on_reload_and_erases_a_page() {
        let mut bus = Bus::default();
        let flash = 0x0800_0000;
        let controller = Stm32F0Flash::BASE;
        bus.map_ram(flash, 0x20000).unwrap();
        bus.set_stm32f0_tags(vec![controller..=0x4002_23ff]);
        bus.map_stm32f072_flash().unwrap();
        let write = |bus: &mut Bus, offset, value| {
            bus.write(controller + offset, 4, value, &mut std::io::sink())
                .unwrap();
        };
        let unlock = |bus: &mut Bus| {
            write(bus, 4, 0x4567_0123);
            write(bus, 4, 0xcdef_89ab);
        };

        bus.write(flash, 2, 0, &mut std::io::sink()).unwrap();
        assert_eq!(bus.read::<2>(flash).unwrap(), 0xffff);
        unlock(&mut bus);
        write(&mut bus, 0x10, 1);
        bus.write(flash, 2, 0xabcd, &mut std::io::sink()).unwrap();
        write(&mut bus, 0x10, 1 << 13);
        assert!(bus.reset_cortex_m_if_requested(false));
        assert_eq!(bus.read::<2>(flash).unwrap(), 0xabcd);
        assert_ne!(bus.read::<4>(controller + 0x10).unwrap() & (1 << 7), 0);

        unlock(&mut bus);
        write(&mut bus, 0x14, flash);
        write(&mut bus, 0x10, (1 << 1) | (1 << 6));
        assert_eq!(bus.read::<2>(flash).unwrap(), 0xffff);
    }
}

fn ltdc_clocked(control: Option<&NetworkControl>, device: &Ltdc) -> bool {
    !device.rcc_bound || control.is_some_and(NetworkControl::ltdc_enabled)
}

fn hsem_clocked(control: Option<&NetworkControl>, device: &Hsem) -> bool {
    !device.rcc_bound || control.is_some_and(NetworkControl::hsem_enabled)
}

fn dma2d_clocked(control: Option<&NetworkControl>, device: &Dma2d) -> bool {
    !device.rcc_bound || control.is_some_and(NetworkControl::dma2d_enabled)
}

fn request_h7_stream(
    target: StreamTarget,
    source: u32,
    value: u32,
    control: Option<&NetworkControl>,
    streams: &mut [AdcDma],
    basic: &mut [Bdma],
    memory: &mut EthernetDma<'_>,
) -> bool {
    if let Some(controller) = streams
        .iter_mut()
        .find(|dma| dma.base == target.base && dma_clocked(control, dma))
    {
        return controller.request(target.stream, source, value, memory);
    }
    basic
        .iter_mut()
        .find(|dma| dma.base == target.base && bdma_clocked(control, dma))
        .is_some_and(|dma| dma.request(target.stream, source, value, memory))
}

fn crc_clocked(control: Option<&NetworkControl>, crc: &Stm32Crc) -> bool {
    !crc.rcc_bound || control.is_some_and(NetworkControl::crc_enabled)
}

fn lptim_clocked(control: Option<&NetworkControl>, timer: &LpTimer) -> bool {
    timer
        .clock
        .is_none_or(|clock| control.is_some_and(|control| control.lptim_enabled(clock)))
}

fn f4_clocked(rcc: Option<&F4Rcc>, base: u32) -> bool {
    rcc.is_none_or(|rcc| rcc.enabled(base))
}

fn timer_clocked(control: Option<&NetworkControl>, timer: &H7Timer) -> bool {
    timer
        .clock
        .is_none_or(|clock| control.is_some_and(|control| control.timer_enabled(clock)))
}

fn qspi_clocked(control: Option<&NetworkControl>, qspi: &Qspi) -> bool {
    !qspi.rcc_bound || control.is_some_and(NetworkControl::qspi_enabled)
}

fn rng_clocked(control: Option<&NetworkControl>, rng: &Rng) -> bool {
    !rng.rcc_bound || control.is_some_and(NetworkControl::rng_enabled)
}

fn i2c_clocked(control: Option<&NetworkControl>, i2c: &I2c) -> bool {
    i2c.clock
        .is_none_or(|clock| control.is_some_and(|control| control.i2c_enabled(clock)))
}

fn spi_clocked(control: Option<&NetworkControl>, spi: &Spi) -> bool {
    !spi.rcc_bound || control.is_some_and(NetworkControl::spi4_enabled)
}

fn adc_clocked(control: Option<&NetworkControl>, adc: &Adc) -> bool {
    control
        .zip(adc.clock)
        .is_none_or(|(control, clock)| control.adc_enabled(clock))
}

fn dma_clocked(control: Option<&NetworkControl>, dma: &AdcDma) -> bool {
    control.is_none_or(|control| control.dma_enabled(dma.clock_mask))
}

fn bdma_clocked(control: Option<&NetworkControl>, dma: &Bdma) -> bool {
    !dma.rcc_bound || control.is_none_or(NetworkControl::bdma_enabled)
}

fn gpio_clocked(control: Option<&NetworkControl>, port: &H7Gpio) -> bool {
    control
        .zip(port.rcc_pin)
        .is_none_or(|(control, pin)| control.gpio_enabled(pin))
}

fn rtc_clocked(control: Option<&NetworkControl>, rtc: &H7Rtc) -> bool {
    !rtc.rcc_bound || control.is_none_or(NetworkControl::rtc_enabled)
}

#[cfg(test)]
mod find_bytes_tests {
    use super::Bus;

    #[test]
    fn find_bytes_covers_aliases_high_addresses_and_holes() {
        const HIGH: u64 = 1 << 32;
        let mut bus = Bus::default();
        bus.map_ram_views(&[0, 8], 3).unwrap();
        bus.memory64(0, 3).unwrap().copy_from_slice(b"aaa");
        let backing = bus.map_ram_backing64(HIGH, 6, 2).unwrap();
        bus.map_alias64(backing, 11, 2, 0, 0, false).unwrap();
        bus.memory64(HIGH, 2).unwrap().copy_from_slice(b"ab");
        assert_eq!(
            bus.find_bytes(b"aa", 0, u64::MAX).unwrap(),
            [0, 1, 8, 9, 10]
        );
        assert_eq!(bus.find_bytes(b"ab", 0, u64::MAX).unwrap(), [11, HIGH]);
        assert!(bus.find_bytes(b"ab", HIGH, HIGH).unwrap().is_empty());
        assert!(bus.find_bytes(b"ab", 12, 11).unwrap().is_empty());
        assert!(bus.find_bytes(b"", 0, u64::MAX).is_err());
    }
}
