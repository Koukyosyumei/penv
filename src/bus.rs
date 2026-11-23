use std::{
    alloc::{Layout, alloc_zeroed},
    fmt,
    io::{self, Write},
};

use crate::{
    clint::Clint,
    constant_mmio::ConstantMmio,
    dwt::Dwt,
    ethernet::Controller,
    murax::{Gpio, Timer},
    ns16550::Ns16550,
    nvic::{ActiveExceptions, Nvic, VTOR},
    plic::Plic,
    stm32_can::{Filters, StmCan},
    stm32_crc::Stm32Crc,
    stm32_exti::Exti,
    stm32_iwdg::Iwdg,
    stm32_uart::{Stm32Irq, Stm32Usart, UsartLayout},
    stm32f1_i2c::F1I2c,
    stm32f4_flash::F4Flash,
    stm32f4_power::F4Power,
    stm32f4_rcc::F4Rcc,
    stm32f4_spi::F4Spi,
    stm32f4_system::F4System,
    stm32h7_adc::Adc,
    stm32h7_adc_dma::{AdcDma, DmaMux, StreamTarget},
    stm32h7_bdma::Bdma,
    stm32h7_exti::H7Exti,
    stm32h7_fdcan::Fdcan,
    stm32h7_flash::FlashController,
    stm32h7_gpio::Stm32Gpio,
    stm32h7_i2c::I2c,
    stm32h7_net_control::{Change, NetworkControl},
    stm32h7_qspi::Qspi,
    stm32h7_rng::Rng,
    stm32h7_rtc::Stm32Rtc,
    stm32h7_spi::Spi,
    stm32h7_timer::Timer as H7Timer,
    systick::SysTick,
    uart::{LiteXUart, MuraxUart},
};

pub const RAM_BASE: u32 = 0x8000_0000;
pub const UART_TX: u32 = 0x1000_0000;

#[derive(Debug)]
pub enum BusError {
    Unmapped(u32, usize),
    Denied(u32),
    Page(u32),
    Misaligned(u32, usize),
    Output(io::Error),
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
        }
    }
}

struct Region {
    base: u32,
    bytes: Vec<u8>,
}

#[derive(Default)]
pub struct Bus {
    regions: Vec<Region>,
    constant_mmio: Vec<ConstantMmio>,
    pub ethernet: Vec<Controller>,
    h7_network_control: Option<NetworkControl>,
    reservations: [Option<u32>; crate::machine::MAX_HARTS],
    uart_tx: Option<u32>,
    pub uart: Option<MuraxUart>,
    ns16550: Option<Ns16550>,
    litex_uart: Option<LiteXUart>,
    stm32_uarts: Vec<Stm32Usart>,
    stm32_uart_names: Vec<String>,
    stm32_uart_input: usize,
    iwdgs: Vec<Iwdg>,
    step_hz: Option<u32>,
    exti: Option<Exti>,
    h7_exti: Option<H7Exti>,
    h7_adc: Vec<Adc>,
    h7_bdma: Vec<Bdma>,
    h7_adc_dma: Vec<AdcDma>,
    h7_dma_mux: Vec<DmaMux>,
    f4_flash: Option<F4Flash>,
    f4_power: Option<F4Power>,
    f4_rcc: Option<F4Rcc>,
    f4_system: Option<F4System>,
    // Upstream F4 RCC leaves the declared TIMx clocks ungated and does not
    // drive the NVIC's SysTick frequency.
    pub f4_repl_timing: bool,
    f4_gated_ranges: Vec<(u32, u32)>,
    h7_flash: Option<FlashController>,
    h7_gpio: Vec<Stm32Gpio>,
    h7_rtc: Vec<Stm32Rtc>,
    h7_spi: Vec<Spi>,
    f4_spi: Vec<F4Spi>,
    h7_i2c: Vec<I2c>,
    f1_i2c: Vec<F1I2c>,
    h7_rng: Vec<Rng>,
    h7_qspi: Vec<Qspi>,
    h7_timers: Vec<H7Timer>,
    h7_crc: Vec<Stm32Crc>,
    h7_fdcan: Vec<Fdcan>,
    stm32_can: Vec<StmCan>,
    bitbands: Vec<(u32, u32, u32)>,
    bus_tags: Vec<(u32, u32, u32)>,
    timer_pins: Vec<TimerPin>,
    gpio_wires: Vec<GpioWire>,
    gpio_leds: Vec<(String, u32, u8)>,
    gpio_exti_outputs: [Option<u8>; 16],
    plic: Option<Plic>,
    pub systick: Option<SysTick>,
    pub dwt: Option<Dwt>,
    pub nvic: Option<Nvic>,
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
    reservations: &'a mut [Option<u32>],
}

impl<'a> EthernetDma<'a> {
    pub fn read(&self, address: u32, width: usize) -> Option<&[u8]> {
        self.regions.iter().find_map(|region| {
            let offset = region.offset(address, width)?;
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
            if let Some(offset) = region.offset(address, width) {
                if width != 0 {
                    for reservation in self.reservations.iter_mut() {
                        if reservation.is_some_and(|reserved| {
                            u64::from(address) < u64::from(reserved) + 4
                                && u64::from(reserved) < u64::from(address) + width as u64
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
    fn new(base: u32, size: usize) -> Result<Self, String> {
        let end = u64::from(base).saturating_add(size as u64);
        if size == 0 || end > 1 << 32 {
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
        Ok(Self { base, bytes })
    }

    fn offset(&self, address: u32, width: usize) -> Option<usize> {
        let offset = address.checked_sub(self.base)? as usize;
        (width <= self.bytes.len().checked_sub(offset)?).then_some(offset)
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

    pub fn map_ram(&mut self, base: u32, size: usize) -> Result<(), String> {
        let start = u64::from(base);
        let end = start.saturating_add(size as u64);
        if self.overlaps(start, end) {
            return Err(format!(
                "overlapping RAM range at {base:#010x} ({size} bytes)"
            ));
        }
        self.regions.push(Region::new(base, size)?);
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
    }

    pub fn step_hz(&self) -> u32 {
        self.step_hz
            .unwrap_or(crate::machine::VIRTUAL_STEPS_PER_SECOND)
    }

    pub fn tick_systick(&mut self) {
        let rate = self.step_hz();
        if let Some(timer) = &mut self.systick {
            let before = timer.pending_mask();
            timer.tick(rate);
            if timer.pending_mask() & !before != 0
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
            device.reset();
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
                .chain(self.h7_rtc.iter().map(|rtc| (rtc.base, Stm32Rtc::SIZE))),
        );
        self.f4_rcc = Some(rcc);
        self.f4_system = Some(F4System::default());
        self.refresh_f4_clocks();
        Ok(())
    }

    fn f4_access_blocked(&self, address: u32) -> bool {
        self.f4_rcc.as_ref().is_some_and(|rcc| {
            self.f4_gated_ranges.iter().any(|&(base, size)| {
                address.wrapping_sub(base) < size
                    && !rcc.enabled(base)
                    && !(self.f4_repl_timing
                        && self.h7_timers.iter().any(|timer| timer.base == base))
            })
        })
    }

    fn refresh_f4_clocks(&mut self) {
        let Some(rcc) = &self.f4_rcc else { return };
        if !self.f4_repl_timing {
            if let Some(systick) = &mut self.systick {
                systick.set_frequency(rcc.hclk());
            }
            for timer in &mut self.h7_timers {
                if let Some(frequency) = rcc.timer_frequency(timer.base) {
                    timer.set_frequency(frequency);
                }
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

    pub fn map_h7_gpio(&mut self, port: Stm32Gpio) -> Result<(), String> {
        self.check_peripheral_range(port.base, port.size)?;
        self.h7_gpio.push(port);
        Ok(())
    }

    pub fn set_gpio_exti_outputs(&mut self, outputs: [Option<u8>; 16]) {
        self.gpio_exti_outputs = outputs;
    }

    pub fn map_h7_rtc(&mut self, rtc: Stm32Rtc) -> Result<(), String> {
        self.check_peripheral_range(rtc.base, Stm32Rtc::SIZE)?;
        self.h7_rtc.push(rtc);
        Ok(())
    }

    pub fn map_stm32_can(&mut self, can: StmCan) -> Result<(), String> {
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
        self.gpio_leds.push((name, gpio, pin));
    }

    #[cfg(test)]
    pub fn gpio_led(&self, name: &str) -> Option<bool> {
        let (_, gpio, pin) = self.gpio_leds.iter().find(|(led, _, _)| led == name)?;
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
                && (self.f4_repl_timing || f4_clocked(self.f4_rcc.as_ref(), timer.base))
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
        self.sample_timer_pins();
        let rate = self.step_hz();
        let control = self.h7_network_control.as_ref();
        let mut dma = EthernetDma {
            regions: &mut self.regions,
            reservations: &mut self.reservations,
        };
        for timer in &mut self.h7_timers {
            if timer_clocked(control, timer)
                && (self.f4_repl_timing || f4_clocked(self.f4_rcc.as_ref(), timer.base))
            {
                timer.tick(rate);
            }
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
        self.bus_tags
            .iter()
            .find_map(|&(start, end, value)| (address >= start && last <= end).then_some(value))
    }

    fn overlaps(&self, start: u64, end: u64) -> bool {
        self.bitbands.iter().any(|&(base, size, _)| {
            let base = u64::from(base);
            start < base + u64::from(size) && base < end
        }) || self.h7_network_control.as_ref().is_some_and(|control| {
            control
                .ranges()
                .any(|(base, limit)| start < limit && base < end)
        }) || (self.nvic.is_some() && Nvic::overlaps(start, end))
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
                let other = u64::from(uart.base);
                start < other + u64::from(LiteXUart::SIZE) && other < end
            })
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
                .chain(self.h7_rtc.iter().map(|rtc| (rtc.base, Stm32Rtc::SIZE)))
                .chain(self.iwdgs.iter().map(|device| (device.base, Iwdg::SIZE)))
                .chain(self.f4_spi.iter().map(|spi| (spi.base, F4Spi::SIZE)))
                .chain(self.h7_spi.iter().map(|spi| (spi.base, Spi::SIZE)))
                .chain(self.f1_i2c.iter().map(|i2c| (i2c.base, F1I2c::SIZE)))
                .chain(self.h7_i2c.iter().map(|i2c| (i2c.base, I2c::SIZE)))
                .chain(self.h7_rng.iter().map(|rng| (rng.base, Rng::SIZE)))
                .chain(self.h7_qspi.iter().map(|qspi| (qspi.base, Qspi::SIZE)))
                .chain(
                    self.h7_timers
                        .iter()
                        .map(|timer| (timer.base, H7Timer::SIZE)),
                )
                .chain(self.h7_crc.iter().map(|crc| (crc.base, Stm32Crc::SIZE)))
                .chain(self.stm32_can.iter().map(|can| (can.base, StmCan::SIZE)))
                .chain(self.h7_fdcan.iter().map(|fdcan| (fdcan.base, Fdcan::SIZE)))
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
                let other = u64::from(region.base);
                start < other + region.bytes.len() as u64 && other < end
            })
    }

    pub fn map_clint(&mut self, base: u32, targets: usize) -> Result<(), String> {
        let start = u64::from(base);
        let end = start + Clint::SIZE;
        if base & 3 != 0 || end > 1 << 32 || self.clint.is_some() || self.overlaps(start, end) {
            return Err(format!(
                "invalid or overlapping CLINT range at {base:#010x}"
            ));
        }
        self.clint = Some(Clint::new(base, targets));
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

    pub fn map_plic(&mut self, base: u32, sources: u32, outputs: &[u32]) -> Result<(), String> {
        if !(1..=63).contains(&sources) {
            return Err("PLIC requires 1–63 interrupt sources".into());
        }
        let start = u64::from(base);
        let end = start + Plic::SIZE;
        if base & 3 != 0 || end > 1 << 32 || self.plic.is_some() || self.overlaps(start, end) {
            return Err(format!("invalid or overlapping PLIC range at {base:#010x}"));
        }
        self.plic = Some(Plic::new(base, sources, outputs));
        Ok(())
    }

    pub fn map_ns16550(&mut self, base: u32, source: u32, wide: bool) -> Result<(), String> {
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

    pub fn sample_nvic(&mut self, active: &ActiveExceptions) {
        if let Some(nvic) = self.nvic.as_mut() {
            nvic.set_uart_level(
                self.litex_uart
                    .as_ref()
                    .is_some_and(LiteXUart::interrupt_pending),
                active,
            );
            if let Some(line) = self.h7_flash.as_ref().and_then(FlashController::interrupt) {
                nvic.set_line(line, true, active);
            }
            for line in self.ethernet.iter().filter_map(Controller::nvic_irq) {
                nvic.set_line(line, true, active);
            }
            let mut exti_inputs = [0; 3];
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
                        .flat_map(Stm32Rtc::interrupts),
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
                    self.h7_timers
                        .iter()
                        .filter(|timer| {
                            timer_clocked(control, timer)
                                && (self.f4_repl_timing
                                    || f4_clocked(self.f4_rcc.as_ref(), timer.base))
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
                .chain(
                    self.stm32_can
                        .iter()
                        .filter(|can| f4_clocked(self.f4_rcc.as_ref(), can.base))
                        .flat_map(StmCan::interrupts),
                )
                .chain(self.h7_fdcan.iter().flat_map(Fdcan::interrupts))
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
                for line in exti.sample(exti_inputs[0]) {
                    nvic.set_line(line, true, active);
                }
            }
        }
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

    pub fn map_litex_uart(&mut self, base: u32) -> Result<(), String> {
        let start = u64::from(base);
        let end = start + u64::from(LiteXUart::SIZE);
        if base & 3 != 0 || end > 1 << 32 || self.overlaps(start, end) {
            return Err(format!(
                "invalid or overlapping LiteX UART range at {base:#010x}"
            ));
        }
        self.litex_uart = Some(LiteXUart::new(base));
        Ok(())
    }

    pub fn map_stm32_usart(
        &mut self,
        name: &str,
        base: u32,
        irq: Option<Stm32Irq>,
        layout: UsartLayout,
    ) -> Result<(), String> {
        let start = u64::from(base);
        let end = start + u64::from(layout.size());
        if base & 3 != 0 || end > 1 << 32 || self.overlaps(start, end) {
            return Err(format!(
                "invalid or overlapping STM32 USART range at {base:#010x}"
            ));
        }
        self.stm32_uarts.push(Stm32Usart::new(base, irq, layout));
        self.stm32_uart_names.push(name.into());
        Ok(())
    }

    pub fn select_uart_input(&mut self, name: &str) {
        if let Some(index) = self.stm32_uart_names.iter().position(|uart| uart == name) {
            self.stm32_uart_input = index;
        }
    }

    pub fn has_uart_input(&self) -> bool {
        self.uart.is_some()
            || self.ns16550.is_some()
            || self.litex_uart.is_some()
            || !self.stm32_uarts.is_empty()
    }

    pub fn queue_uart_input(&mut self, bytes: Vec<u8>) {
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
        }
    }

    pub fn tick_plic(&mut self) -> u32 {
        if let Some(uart) = self.ns16550.as_ref()
            && let Some(plic) = self.plic.as_mut()
        {
            plic.set_line(uart.source, uart.interrupt_pending());
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

    pub fn memory(&mut self, address: u32, width: usize) -> Result<&mut [u8], BusError> {
        EthernetDma {
            regions: &mut self.regions,
            reservations: &mut self.reservations,
        }
        .memory(address, width)
    }

    pub fn guest_memory(&mut self, address: u32, width: usize) -> Result<&mut [u8], BusError> {
        let memory = EthernetDma {
            regions: &mut self.regions,
            reservations: &mut self.reservations,
        }
        .memory(address, width)?;
        if let Some(device) = &mut self.h7_flash {
            device.note_program(address, width);
        }
        Ok(memory)
    }

    pub fn receive_ethernet(&mut self, nic: usize, frame: &[u8]) {
        let mut dma = EthernetDma {
            regions: &mut self.regions,
            reservations: &mut self.reservations,
        };
        self.ethernet[nic].receive(frame, &mut dma);
    }

    pub fn read_exclusive(&mut self, address: u32) -> Result<u32, BusError> {
        let value = self.read::<4>(address)?;
        self.reservations[0] = Some(address);
        Ok(value)
    }

    pub fn write_exclusive(
        &mut self,
        address: u32,
        value: u32,
        output: &mut impl Write,
    ) -> Result<u32, BusError> {
        if address & 3 != 0 {
            return Err(BusError::Misaligned(address, 4));
        }
        if self.reservations[0].take() != Some(address) {
            return Ok(1);
        }
        self.write(address, 4, value, output)?;
        Ok(0)
    }

    pub fn clear_reservation(&mut self, hart: usize) {
        self.reservations[hart] = None;
    }

    pub fn load_reserved(&mut self, hart: usize, address: u32) -> Result<u32, BusError> {
        let value = self.read_ram::<4>(address)?;
        self.reservations[hart] = Some(address);
        Ok(value)
    }

    pub fn store_conditional(
        &mut self,
        hart: usize,
        address: u32,
        value: u32,
    ) -> Result<u32, BusError> {
        let reserved = self.reservations[hart].take() == Some(address);
        if reserved {
            self.atomic_word(address)?
                .copy_from_slice(&value.to_le_bytes());
            Ok(0)
        } else {
            self.read_ram::<4>(address)?;
            Ok(1)
        }
    }

    pub fn atomic_update(
        &mut self,
        address: u32,
        update: impl FnOnce(u32) -> u32,
    ) -> Result<u32, BusError> {
        let word = self.atomic_word(address)?;
        let previous = u32::from_le_bytes(*word);
        *word = update(previous).to_le_bytes();
        Ok(previous)
    }

    fn atomic_word(&mut self, address: u32) -> Result<&mut [u8; 4], BusError> {
        if address & 3 != 0 {
            return Err(BusError::Misaligned(address, 4));
        }
        Ok(self.memory(address, 4)?.try_into().unwrap())
    }

    pub fn read<const N: usize>(&mut self, address: u32) -> Result<u32, BusError> {
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
            && let Some(i2c) = self.h7_i2c.iter().find(|i2c| i2c.contains(address))
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
        if matches!(N, 2 | 4)
            && address & 3 == 0
            && let Some(device) = self.iwdgs.iter().find(|device| device.contains(address))
        {
            return Ok(device.read(address));
        }
        self.read_ram::<N>(address).or_else(|error| {
            self.bus_tag(address, N)
                .map(|value| value & (u32::MAX >> (32 - N * 8)))
                .ok_or(error)
        })
    }

    pub fn read_ram<const N: usize>(&self, address: u32) -> Result<u32, BusError> {
        if address as usize & (N - 1) != 0 {
            return Err(BusError::Misaligned(address, N));
        }
        self.read_unaligned_ram::<N>(address)
    }

    pub fn read_unaligned_ram<const N: usize>(&self, address: u32) -> Result<u32, BusError> {
        for region in &self.regions {
            if let Some(offset) = region.offset(address, N) {
                let mut bytes = [0; 4];
                bytes[..N].copy_from_slice(&region.bytes[offset..offset + N]);
                return Ok(u32::from_le_bytes(bytes));
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
        if width == 4
            && let Some(timer) = self.systick.as_mut()
        {
            let before = timer.pending_mask();
            if timer.write(address, value) {
                if timer.pending_mask() & !before != 0
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
        if let Some(device) = self
            .stm32_uarts
            .iter_mut()
            .find(|device| device.contains(address))
        {
            return device
                .write(address, width, value, uart)
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
        match self.guest_memory(address, width) {
            Ok(memory) => memory.copy_from_slice(&value.to_le_bytes()[..width]),
            Err(error) => {
                if self.bus_tag(address, width).is_none() {
                    return Err(error);
                }
            }
        }
        Ok(())
    }
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

fn gpio_clocked(control: Option<&NetworkControl>, port: &Stm32Gpio) -> bool {
    control
        .zip(port.rcc_pin)
        .is_none_or(|(control, pin)| control.gpio_enabled(pin))
}

fn rtc_clocked(control: Option<&NetworkControl>, rtc: &Stm32Rtc) -> bool {
    !rtc.rcc_bound || control.is_none_or(NetworkControl::rtc_enabled)
}
