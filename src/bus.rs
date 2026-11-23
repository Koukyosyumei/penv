use std::{
    alloc::{Layout, alloc_zeroed},
    collections::HashMap,
    fmt,
    io::{self, Write},
    sync::Arc,
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
pub struct CommandError;

struct CommandErrorPeripheral {
    base: u32,
    size: u32,
    error: Arc<CommandError>,
}

impl CommandErrorPeripheral {
    fn read_doubleword(&self, address: u32) -> Option<BusError> {
        (address == self.base).then(|| BusError::Command(address, Arc::clone(&self.error)))
    }
}

#[derive(Debug)]
pub enum BusError {
    Unmapped(u32, usize),
    Command(u32, Arc<CommandError>),
    Denied(u32),
    Page(u32),
    Misaligned(u32, usize),
    Output(io::Error),
}

impl BusError {
    pub fn at_virtual(self, address: u32) -> Self {
        match self {
            Self::Unmapped(_, width) => Self::Unmapped(address, width),
            Self::Command(_, fault) => Self::Command(address, fault),
            _ => self,
        }
    }
    pub fn access_fault_address(&self) -> Option<u32> {
        match self {
            Self::Unmapped(address, _) | Self::Command(address, _) => Some(*address),
            _ => None,
        }
    }
}

impl fmt::Display for BusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unmapped(address, width) => {
                write!(f, "unmapped RAM access at {address:#010x} ({width} bytes)")
            }
            Self::Command(address, _) => write!(f, "peripheral CommandError at {address:#010x}"),
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
    aliases: Vec<u32>,
    bytes: Vec<u8>,
}

struct BytePeripheral {
    base: u32,
    bytes: Vec<u8>,
}

impl BytePeripheral {
    fn offset(&self, address: u32) -> Option<usize> {
        let offset = address.checked_sub(self.base)? as usize;
        (offset < self.bytes.len()).then_some(offset)
    }
}

struct ByteSpan {
    start: u32,
    offsets: std::ops::Range<usize>,
    peripheral: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum FixtureValue {
    Enums([u8; 2]),
    Integer(i32),
    ConstructorCounter(u32),
    Text(Option<String>),
    Boolean(bool),
    Range(u64, u64),
    NullObject,
    NullPeripheralReference,
    Collections(FixtureCollections),
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct FixtureCollections {
    pub int_list: Option<Vec<i32>>,
    pub int_array: Option<Vec<i32>>,
    pub string_list: Option<Vec<String>>,
    pub int_string_dict: Option<HashMap<i32, String>>,
    pub cpu_list: Option<Vec<Arc<str>>>,
    pub int_cpu_dict: Option<HashMap<i32, Arc<str>>>,
    pub cpu_int_dict: Option<HashMap<Arc<str>, i32>>,
}

#[derive(Clone, Default)]
pub(crate) struct FixtureCpuProperties {
    pub enum_value: u8,
    pub placeholder: Option<String>,
    pub other: Option<Box<FixtureCpuProperties>>,
}

#[derive(Default)]
pub struct Bus {
    regions: Vec<Region>,
    byte_peripherals: Vec<BytePeripheral>,
    command_error_peripherals: Vec<CommandErrorPeripheral>,
    faulting_regions: Vec<(u32, Option<u32>, bool, bool)>,
    constant_mmio: Vec<ConstantMmio>,
    empty_peripherals: HashMap<String, Vec<(u32, u32)>>,
    fixture_cpus: HashMap<String, Arc<str>>,
    fixture_cpu_properties: HashMap<Arc<str>, FixtureCpuProperties>,
    fixture_values: HashMap<String, FixtureValue>,
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
    bus_tags: Vec<BusTag>,
    unhandled_access: UnhandledAccess,
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

#[derive(Clone, Copy, Default)]
pub enum UnhandledAccess {
    #[default]
    Preserve,
    Report,
    DoNotReport,
    ThrowException,
    ThrowExceptionIfNotTagged,
}

#[derive(Clone, Copy)]
struct BusTag {
    start: u32,
    end: u32,
    value: u32,
    throws: bool,
}

struct GpioWire {
    source: (u32, u8),
    target: (u32, u8),
    level: bool,
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
                            region.offset(reserved, 1).is_some_and(|reserved_offset| {
                                offset < reserved_offset + 4 && reserved_offset < offset + width
                            })
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
        Ok(Self {
            base,
            aliases: Vec::new(),
            bytes,
        })
    }

    fn bases(&self) -> impl Iterator<Item = u32> + '_ {
        std::iter::once(self.base).chain(self.aliases.iter().copied())
    }

    fn offset(&self, address: u32, width: usize) -> Option<usize> {
        self.bases().find_map(|base| {
            let offset = address.checked_sub(base)? as usize;
            (width <= self.bytes.len().checked_sub(offset)?).then_some(offset)
        })
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

    pub fn map_faulting_memory(&mut self, base: u32, offset: Option<u32>) -> Result<(), String> {
        self.map_faulting_memory_access(base, offset, true, true)
    }

    pub fn map_faulting_memory_access(
        &mut self,
        base: u32,
        offset: Option<u32>,
        reads: bool,
        writes: bool,
    ) -> Result<(), String> {
        if offset.is_some_and(|offset| offset >= 0x1000) {
            return Err("fault offset outside mock peripheral".into());
        }
        self.map_ram(base, 0x1000)?;
        self.faulting_regions.push((base, offset, reads, writes));
        Ok(())
    }

    #[cfg(test)]
    pub fn set_faulting_memory_offset(&mut self, base: u32, offset: Option<u32>) {
        self.faulting_regions
            .iter_mut()
            .find(|region| region.0 == base)
            .unwrap()
            .1 = offset;
    }

    fn faulting_address(&self, address: u32, width: usize, write: bool) -> Option<u32> {
        self.faulting_regions
            .iter()
            .find_map(|&(base, offset, reads, writes)| {
                if !(if write { writes } else { reads }) {
                    return None;
                }
                let relative = address.wrapping_sub(base);
                if relative >= 0x1000 {
                    return None;
                }
                match offset {
                    None => Some(address),
                    Some(offset) if offset.wrapping_sub(relative) < width as u32 => {
                        Some(base + offset)
                    }
                    _ => None,
                }
            })
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

    pub fn register_empty_peripheral(
        &mut self,
        name: String,
        ranges: Vec<(u32, u32)>,
    ) -> Result<(), String> {
        if self.empty_peripherals.contains_key(&name) {
            return Err(format!(
                "NameSettingException: duplicate peripheral name {name:?}"
            ));
        }
        let first_mapping = self.constant_mmio.len();
        for &(base, size) in &ranges {
            if let Err(error) = self.map_constant_mmio(ConstantMmio::zeroed(base, size)) {
                self.constant_mmio.truncate(first_mapping);
                return Err(format!("RegistrationException: {error}"));
            }
        }
        self.empty_peripherals.insert(name, ranges);
        Ok(())
    }

    pub fn empty_peripheral_ranges(&self, name: &str) -> Option<&[(u32, u32)]> {
        self.empty_peripherals.get(name).map(Vec::as_slice)
    }

    pub(crate) fn register_fixture_value(
        &mut self,
        name: String,
        range: (u32, u32),
        value: FixtureValue,
    ) -> Result<(), String> {
        self.register_empty_peripheral(name.clone(), vec![range])?;
        self.fixture_values.insert(name, value);
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn fixture_value(&self, path: &str) -> Option<&FixtureValue> {
        self.fixture_values.get(path)
    }

    #[cfg(test)]
    pub fn fixture_enum_value(&self, path: &str) -> Option<[u8; 2]> {
        match self.fixture_value(path)? {
            FixtureValue::Enums(values) => Some(*values),
            _ => None,
        }
    }

    pub(crate) fn register_fixture_cpu(
        &mut self,
        parent: &str,
        name: &str,
        variable: &str,
        properties: &FixtureCpuProperties,
    ) -> Result<(), String> {
        if parent != "sysbus" && !self.empty_peripherals.contains_key(parent) {
            return Err(format!(
                "RegistrationException: unregistered parent {parent:?}"
            ));
        }
        let path = format!("{parent}.{name}");
        let identity = self
            .fixture_cpus
            .values()
            .find(|identity| identity.as_ref() == variable)
            .cloned()
            .unwrap_or_else(|| Arc::from(variable));
        match self.fixture_cpus.entry(path) {
            std::collections::hash_map::Entry::Vacant(slot) => {
                self.fixture_cpu_properties
                    .entry(identity.clone())
                    .or_insert_with(|| properties.clone());
                slot.insert(identity);
                Ok(())
            }
            std::collections::hash_map::Entry::Occupied(_) => {
                Err("NameSettingException: duplicate child name".into())
            }
        }
    }

    #[cfg(test)]
    pub fn fixture_cpu_name(&self, path: &str) -> Option<&str> {
        self.fixture_cpu(path).map(AsRef::as_ref)
    }

    #[cfg(test)]
    pub fn fixture_cpu_enum(&self, path: &str) -> Option<u8> {
        Some(self.fixture_cpu_properties(path)?.enum_value)
    }

    #[cfg(test)]
    pub(crate) fn fixture_cpu_properties(&self, path: &str) -> Option<&FixtureCpuProperties> {
        self.fixture_cpu_properties.get(self.fixture_cpu(path)?)
    }

    pub(crate) fn fixture_cpu(&self, path: &str) -> Option<&Arc<str>> {
        self.fixture_cpus.get(path)
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

    pub fn set_timer_pins(&mut self, pins: Vec<TimerPin>) {
        self.timer_pins = pins;
    }

    pub fn connect_gpio(
        &mut self,
        source: (u32, u8),
        receiver: u32,
        input: u8,
        input_count: Option<u8>,
        receiver_name: &str,
    ) -> Result<(), String> {
        if let Some(count) = input_count
            && input >= count
        {
            let suffix = match (input % 100, input % 10) {
                (11..=13, _) => "th",
                (_, 1) => "st",
                (_, 2) => "nd",
                (_, 3) => "rd",
                _ => "th",
            };
            return Err(format!(
                "Cannot connect {input}{suffix} input of {receiver_name}; it has only {count} GPIO inputs."
            ));
        }
        let target = (receiver, input);
        if !self
            .gpio_wires
            .iter()
            .any(|wire| wire.source == source && wire.target == target)
        {
            self.gpio_wires.push(GpioWire {
                source,
                target,
                level: false,
            });
        }
        Ok(())
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
        for uart in &mut self.stm32_uarts {
            if f4_clocked(self.f4_rcc.as_ref(), uart.base) {
                uart.tick(rate);
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

    pub fn set_unhandled_access(&mut self, mode: UnhandledAccess) {
        self.unhandled_access = mode;
    }

    pub fn map_bus_tag(&mut self, start: u32, end: u32, value: u32) -> Result<(), String> {
        self.map_bus_tag_with_fault(start, end, value, false)
    }

    pub fn map_bus_tag_with_fault(
        &mut self,
        start: u32,
        end: u32,
        value: u32,
        throws: bool,
    ) -> Result<(), String> {
        if start > end {
            return Err("invalid sysbus Tag range".into());
        }
        let mut intersections = self
            .bus_tags
            .iter()
            .enumerate()
            .filter(|(_, tag)| start <= tag.end && end >= tag.start);
        if let Some((index, parent)) = intersections.next() {
            if start < parent.start || end > parent.end || intersections.next().is_some() {
                return Err("subtag must be completely contained in one existing tag".into());
            }
            let parent = self.bus_tags.remove(index);
            if parent.start < start {
                self.bus_tags.push(BusTag {
                    end: start - 1,
                    ..parent
                });
            }
            if end < parent.end {
                self.bus_tags.push(BusTag {
                    start: end + 1,
                    ..parent
                });
            }
        }
        self.bus_tags.push(BusTag {
            start,
            end,
            value,
            throws,
        });
        Ok(())
    }

    fn unhandled_value(&self, error: BusError) -> Result<u32, BusError> {
        let BusError::Unmapped(address, width) = error else {
            return Err(error);
        };
        let tag = self.bus_tags.iter().find(|tag| {
            address >= tag.start
                && address
                    .checked_add(width as u32 - 1)
                    .is_some_and(|end| end <= tag.end)
        });
        if tag.is_some_and(|tag| tag.throws)
            || matches!(self.unhandled_access, UnhandledAccess::ThrowException)
        {
            return Err(error);
        }
        if matches!(self.unhandled_access, UnhandledAccess::Report) {
            eprintln!("sysbus: {error}");
        }
        if let Some(tag) = tag {
            return Ok(tag.value & (u32::MAX >> (32 - width * 8)));
        }
        match self.unhandled_access {
            UnhandledAccess::Report | UnhandledAccess::DoNotReport => Ok(0),
            _ => Err(error),
        }
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
            || self.command_error_peripherals.iter().any(|device| {
                let base = u64::from(device.base);
                start < base + u64::from(device.size) && base < end
            })
            || self.byte_peripherals.iter().any(|device| {
                let base = u64::from(device.base);
                start < base + device.bytes.len() as u64 && base < end
            })
            || self.regions.iter().any(|region| {
                region.bases().any(|base| {
                    let other = u64::from(base);
                    start < other + region.bytes.len() as u64 && other < end
                })
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
        frequency: u32,
    ) -> Result<(), String> {
        let start = u64::from(base);
        let end = start + u64::from(layout.size());
        if base & 3 != 0 || end > 1 << 32 || self.overlaps(start, end) {
            return Err(format!(
                "invalid or overlapping STM32 USART range at {base:#010x}"
            ));
        }
        self.stm32_uarts
            .push(Stm32Usart::new(base, irq, layout, frequency));
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

    pub fn map_ram_alias(&mut self, base: u32, alias: u32) -> Result<(), String> {
        let index = self
            .regions
            .iter()
            .position(|region| region.base == base)
            .ok_or("memory alias requires an existing backing region")?;
        let start = u64::from(alias);
        let end = start + self.regions[index].bytes.len() as u64;
        if end > 1 << 32 || self.overlaps(start, end) {
            return Err(format!(
                "invalid or overlapping memory alias at {alias:#010x}"
            ));
        }
        self.regions[index].aliases.push(alias);
        Ok(())
    }

    pub fn permits_unmapped_binary_bytes(&self) -> bool {
        matches!(
            self.unhandled_access,
            UnhandledAccess::Report | UnhandledAccess::DoNotReport
        )
    }

    pub fn map_command_error_peripheral(
        &mut self,
        base: u32,
        size: u32,
        error: Arc<CommandError>,
    ) -> Result<(), String> {
        let start = u64::from(base);
        let end = start + u64::from(size);
        if base & 3 != 0 || size < 4 || end > 1 << 32 || self.overlaps(start, end) {
            return Err(format!(
                "invalid or overlapping command-error peripheral at {base:#010x}"
            ));
        }
        self.command_error_peripherals
            .push(CommandErrorPeripheral { base, size, error });
        Ok(())
    }

    pub fn map_byte_peripheral(&mut self, base: u32, size: u32) -> Result<(), String> {
        let start = u64::from(base);
        let end = start + u64::from(size);
        if size == 0 || end > 1 << 32 || self.overlaps(start, end) {
            return Err(format!(
                "invalid or overlapping byte peripheral at {base:#010x}"
            ));
        }
        self.byte_peripherals.push(BytePeripheral {
            base,
            bytes: vec![0; size as usize],
        });
        Ok(())
    }

    fn byte_spans(&self, address: u32, size: usize) -> Result<Vec<ByteSpan>, BusError> {
        let start = u64::from(address);
        let end = start
            .checked_add(size as u64)
            .filter(|&end| end <= 1 << 32)
            .ok_or(BusError::Unmapped(address, size))?;
        let mut spans = Vec::new();
        let mut add = |base: u32, size: usize, peripheral| {
            let base = u64::from(base);
            let lo = start.max(base);
            let hi = end.min(base + size as u64);
            if lo < hi {
                spans.push(ByteSpan {
                    start: lo as u32,
                    offsets: (lo - start) as usize..(hi - start) as usize,
                    peripheral,
                });
            }
        };
        for region in &self.regions {
            for base in region.bases() {
                add(base, region.bytes.len(), false);
            }
        }
        for device in &self.byte_peripherals {
            add(device.base, device.bytes.len(), true);
        }
        // Two mappings can share a backing. Apply writes in address order.
        spans.sort_by_key(|span| span.start);
        Ok(spans)
    }

    pub fn find_bytes(&self, pattern: &[u8]) -> Result<Vec<u32>, String> {
        const CHUNK: usize = 1 << 20;
        if pattern.is_empty() {
            return Err("Pattern cannot be null or empty.".into());
        }
        if pattern.len() > CHUNK {
            return Err(format!("Pattern cannot be larger than {CHUNK}."));
        }

        let mut mappings: Vec<_> = self
            .regions
            .iter()
            // Fault-injection peripherals have byte backing for bus access,
            // but are not registered memory.
            .filter(|region| {
                !self
                    .faulting_regions
                    .iter()
                    .any(|fault| fault.0 == region.base)
            })
            .flat_map(|region| {
                region.bases().map(|base| {
                    let start = u64::from(base);
                    start..start + region.bytes.len() as u64
                })
            })
            .collect();
        mappings.sort_unstable_by_key(|range| range.start);
        let mut ranges: Vec<std::ops::Range<u64>> = Vec::new();
        for mapping in mappings {
            if let Some(last) = ranges.last_mut()
                && mapping.start <= last.end
            {
                last.end = last.end.max(mapping.end);
            } else {
                ranges.push(mapping);
            }
        }

        let mut matches = Vec::new();
        let mut buffer = vec![0; CHUNK + pattern.len() - 1];
        for range in ranges {
            let mut cursor = range.start;
            while cursor + pattern.len() as u64 <= range.end {
                let count = (range.end - cursor).min(buffer.len() as u64) as usize;
                for span in self
                    .byte_spans(cursor as u32, count)
                    .map_err(|e| e.to_string())?
                {
                    let bytes = self
                        .regions
                        .iter()
                        .find_map(|region| {
                            let offset = region.offset(span.start, span.offsets.len())?;
                            Some(&region.bytes[offset..offset + span.offsets.len()])
                        })
                        .ok_or("missing mapped RAM during byte search")?;
                    buffer[span.offsets].copy_from_slice(bytes);
                }
                for (offset, window) in buffer[..count]
                    .windows(pattern.len())
                    .take(CHUNK)
                    .enumerate()
                {
                    if window == pattern {
                        matches.push((cursor + offset as u64) as u32);
                    }
                }
                cursor += CHUNK as u64;
            }
        }
        Ok(matches)
    }

    pub fn write_bytes(&mut self, address: u32, input: &[u8]) -> Result<(), BusError> {
        for span in self.byte_spans(address, input.len())? {
            let bytes = &input[span.offsets];
            if span.peripheral {
                for (offset, &byte) in bytes.iter().enumerate() {
                    self.write(
                        span.start + offset as u32,
                        1,
                        u32::from(byte),
                        &mut io::sink(),
                    )?;
                }
            } else {
                self.memory(span.start, bytes.len())?.copy_from_slice(bytes);
            }
        }
        Ok(())
    }

    #[cfg(test)]
    fn read_bytes(&mut self, address: u32, size: usize) -> Result<Vec<u8>, BusError> {
        let spans = self.byte_spans(address, size)?;
        let mut output = vec![0; size];
        for span in spans {
            let bytes = &mut output[span.offsets];
            if span.peripheral {
                for (offset, byte) in bytes.iter_mut().enumerate() {
                    *byte = self.read::<1>(span.start + offset as u32)? as u8;
                }
            } else {
                let memory = EthernetDma {
                    regions: &mut self.regions,
                    reservations: &mut self.reservations,
                };
                bytes.copy_from_slice(memory.read(span.start, bytes.len()).unwrap());
            }
        }
        Ok(output)
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
        if address & 3 != 0 {
            return Err(BusError::Misaligned(address, 4));
        }
        let value = self.read_cortex_data::<4>(address)?;
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
        if let Some(address) = self.faulting_address(address, N, false) {
            return Err(BusError::Unmapped(address, N));
        }
        if N == 4
            && let Some(error) = self
                .command_error_peripherals
                .iter()
                .find_map(|device| device.read_doubleword(address))
        {
            return Err(error);
        }
        if N == 1
            && let Some(value) = self
                .byte_peripherals
                .iter()
                .find_map(|device| device.offset(address).map(|offset| device.bytes[offset]))
        {
            return Ok(u32::from(value));
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
        self.read_ram::<N>(address)
            .or_else(|error| self.unhandled_value(error))
    }

    pub fn write_cortex_stack(&mut self, address: u32, value: u32) -> Result<(), BusError> {
        if let Some(address) = self.faulting_address(address, 4, true) {
            return Err(BusError::Unmapped(address, 4));
        }
        match self.guest_memory(address, 4) {
            Ok(memory) => memory.copy_from_slice(&value.to_le_bytes()),
            // An unhandled stack write follows the open-bus default.
            Err(BusError::Unmapped(..)) => {}
            Err(error) => return Err(error),
        }
        Ok(())
    }

    pub fn read_cortex_data<const N: usize>(&mut self, address: u32) -> Result<u32, BusError> {
        if N == 4 && address == crate::nvic::DHCSR && self.nvic.is_some() {
            return Ok(0);
        }
        self.read::<N>(address)
    }

    pub fn read_cortex_unaligned_data<const N: usize>(
        &self,
        address: u32,
    ) -> Result<u32, BusError> {
        if let Some(address) = self.faulting_address(address, N, false) {
            return Err(BusError::Unmapped(address, N));
        }
        self.read_unaligned_ram::<N>(address)
    }

    pub fn write_cortex_unaligned_data(
        &mut self,
        address: u32,
        width: usize,
        value: u32,
    ) -> Result<(), BusError> {
        if let Some(address) = self.faulting_address(address, width, true) {
            return Err(BusError::Unmapped(address, width));
        }
        self.guest_memory(address, width)?
            .copy_from_slice(&value.to_le_bytes()[..width]);
        Ok(())
    }

    pub fn read_cortex_instruction<const N: usize>(&self, address: u32) -> Result<u32, BusError> {
        if let Some(address) = self.faulting_address(address, N, false) {
            return Err(BusError::Unmapped(address, N));
        }
        self.read_ram::<N>(address)
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
        if let Some(address) = self.faulting_address(address, width, true) {
            return Err(BusError::Unmapped(address, width));
        }
        if width == 1
            && let Some(byte) = self.byte_peripherals.iter_mut().find_map(|device| {
                device
                    .offset(address)
                    .map(|offset| &mut device.bytes[offset])
            })
        {
            *byte = value as u8;
            return Ok(());
        }
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
                self.unhandled_value(error)?;
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

#[cfg(test)]
mod gpio_connection_tests {
    use super::*;

    const SOURCE: (u32, u8) = (0, 0);
    const DESTINATION: u32 = 0x10;
    const UNCONSTRAINED: &str = "Antmicro.Renode.UnitTests.Mocks.MockReceiver";
    const CONSTRAINED: &str = "Antmicro.Renode.UnitTests.Mocks.MockReceiverConstrained";

    #[test]
    fn renode_gpio_propagates_connected_endpoint() {
        let mut bus = Bus::default();
        bus.connect_gpio(SOURCE, DESTINATION, 2, None, UNCONSTRAINED)
            .unwrap();
        let endpoint = &bus.gpio_wires[0];
        assert_eq!(endpoint.target, (DESTINATION, 2));
    }

    #[test]
    fn renode_gpio_endpoints_are_empty_when_disconnected() {
        let bus = Bus::default();
        assert!(bus.gpio_wires.is_empty());
    }

    #[test]
    fn renode_gpio_connects_bounded_input() {
        let mut bus = Bus::default();
        bus.connect_gpio(SOURCE, DESTINATION, 2, Some(5), CONSTRAINED)
            .unwrap();
        assert_eq!(bus.gpio_wires[0].target, (DESTINATION, 2));
    }

    #[test]
    fn renode_gpio_rejects_out_of_range_input() {
        let mut bus = Bus::default();
        let error = bus
            .connect_gpio(SOURCE, DESTINATION, 10, Some(5), CONSTRAINED)
            .unwrap_err();
        assert_eq!(
            error,
            "Cannot connect 10th input of Antmicro.Renode.UnitTests.Mocks.MockReceiverConstrained; it has only 5 GPIO inputs."
        );
        assert!(bus.gpio_wires.is_empty());
    }
}

#[cfg(test)]
mod unhandled_bus_tests {
    use super::*;
    use std::io::sink;

    const MISSING: u32 = 0xabcd_1234;
    const TAG: u32 = 0x1000;

    fn bus(mode: UnhandledAccess) -> Bus {
        let mut bus = Bus::default();
        bus.set_unhandled_access(mode);
        bus
    }

    fn assert_faults(bus: &mut Bus, address: u32) {
        assert!(matches!(bus.read::<1>(address), Err(BusError::Unmapped(a, 1)) if a == address));
        assert!(
            matches!(bus.write(address, 1, 0, &mut sink()), Err(BusError::Unmapped(a, 1)) if a == address)
        );
    }

    #[test]
    fn renode_sysbus_preserves_identity_of_peripheral_command_error() {
        let mut bus = bus(UnhandledAccess::Report);
        let expected = Arc::new(CommandError);
        bus.map_command_error_peripheral(0x1000, 0x100, Arc::clone(&expected))
            .unwrap();
        let Err(BusError::Command(address, actual)) = bus.read::<4>(0x1000) else {
            panic!("expected the registered peripheral error");
        };
        assert_eq!(address, 0x1000);
        assert!(Arc::ptr_eq(&actual, &expected));
    }

    #[test]
    fn renode_sysbus_default_returns_zero() {
        assert_eq!(bus(UnhandledAccess::Report).read::<1>(MISSING).unwrap(), 0);
    }

    #[test]
    fn renode_sysbus_throw_mode_faults_on_unhandled_access() {
        assert_faults(&mut bus(UnhandledAccess::ThrowException), MISSING);
    }

    #[test]
    fn renode_sysbus_throw_if_untagged_faults_on_unhandled_access() {
        assert_faults(
            &mut bus(UnhandledAccess::ThrowExceptionIfNotTagged),
            MISSING,
        );
    }

    #[test]
    fn renode_sysbus_throw_if_untagged_allows_tagged_access() {
        let mut bus = bus(UnhandledAccess::ThrowExceptionIfNotTagged);
        bus.map_bus_tag(0xabcd_1200, 0xabcd_12ff, 0x42).unwrap();
        bus.write(MISSING, 1, 0, &mut sink()).unwrap();
        assert_eq!(bus.read::<1>(MISSING).unwrap(), 0x42);
    }

    #[test]
    fn renode_sysbus_tag_forces_address_error() {
        let mut bus = bus(UnhandledAccess::Report);
        bus.map_bus_tag_with_fault(TAG, TAG + 0xff, 0, true)
            .unwrap();
        assert_faults(&mut bus, TAG);
    }

    #[test]
    fn renode_sysbus_tag_supplies_default_value() {
        let mut bus = bus(UnhandledAccess::Report);
        bus.map_bus_tag(TAG, TAG + 0xff, 0x42).unwrap();
        bus.write(TAG, 1, 0, &mut sink()).unwrap();
        assert_eq!(bus.read::<1>(TAG).unwrap(), 0x42);
    }

    #[test]
    fn renode_sysbus_tag_forces_address_error_with_reporting_off() {
        let mut bus = bus(UnhandledAccess::DoNotReport);
        bus.map_bus_tag_with_fault(TAG, TAG + 0xff, 0, true)
            .unwrap();
        assert_faults(&mut bus, TAG);
    }

    #[test]
    fn renode_sysbus_subtag_preserves_fault_policy_on_both_sides() {
        let mut bus = bus(UnhandledAccess::Report);
        bus.map_bus_tag_with_fault(TAG, TAG + 0xff, 0, true)
            .unwrap();
        bus.map_bus_tag(0x1010, 0x101f, 0).unwrap();
        for address in [0x1000, 0x10f0] {
            assert!(
                matches!(bus.read::<1>(address), Err(BusError::Unmapped(a, 1)) if a == address)
            );
        }
        assert_eq!(bus.read::<1>(0x1010).unwrap(), 0);
    }
}

#[cfg(test)]
mod mapped_byte_tests {
    use super::*;

    const ALIAS: u32 = 0xc000_0000;
    const BYTES: [u8; 8] = [0, 1, 2, 3, 4, 5, 6, 7];

    fn aliased_segment() -> Bus {
        let mut bus = Bus::default();
        bus.map_ram(0, 16).unwrap();
        bus.map_ram_alias(0, ALIAS).unwrap();
        bus
    }

    #[test]
    fn renode_mapped_bytes_write_fits_inside_segment() {
        let mut bus = aliased_segment();
        bus.write_bytes(4, &BYTES).unwrap();
        assert_eq!(bus.read_bytes(4, 8).unwrap(), BYTES);
        assert_eq!(bus.read_bytes(ALIAS + 4, 8).unwrap(), BYTES);
    }

    #[test]
    fn renode_mapped_bytes_partial_write_at_beginning() {
        let mut bus = aliased_segment();
        bus.write_bytes(10, &BYTES).unwrap();
        assert_eq!(bus.read_bytes(10, 8).unwrap(), [0, 1, 2, 3, 4, 5, 0, 0]);
    }

    #[test]
    fn renode_mapped_bytes_partial_write_at_end() {
        let mut bus = aliased_segment();
        bus.write_bytes(ALIAS - 4, &BYTES).unwrap();
        assert_eq!(
            bus.read_bytes(ALIAS - 4, 8).unwrap(),
            [0, 0, 0, 0, 4, 5, 6, 7]
        );
        assert_eq!(bus.read_bytes(0, 4).unwrap(), [4, 5, 6, 7]);
    }

    #[test]
    fn renode_mapped_bytes_write_to_hole() {
        let mut bus = aliased_segment();
        bus.write_bytes(100, &BYTES).unwrap();
        assert_eq!(bus.read_bytes(100, 8).unwrap(), [0; 8]);
    }

    #[test]
    fn renode_mapped_bytes_write_overlaps_both_ends() {
        let mut bus = aliased_segment();
        let input = (0..32).collect::<Vec<_>>();
        bus.write_bytes(ALIAS - 4, &input).unwrap();
        let expected = [&[0; 4][..], &input[4..20], &[0; 12]].concat();
        assert_eq!(bus.read_bytes(ALIAS - 4, 32).unwrap(), expected);
    }

    #[test]
    fn renode_mapped_bytes_reads_both_memory_segments() {
        check_mixed_transfer(true, true);
    }

    #[test]
    fn renode_mapped_bytes_reads_memory_then_peripheral() {
        check_mixed_transfer(true, false);
    }

    #[test]
    fn renode_mapped_bytes_reads_peripheral_then_memory() {
        check_mixed_transfer(false, true);
    }

    #[test]
    fn renode_mapped_bytes_reads_both_peripherals() {
        check_mixed_transfer(false, false);
    }

    #[test]
    fn renode_find_byte_pattern_crosses_memory_and_buffer_boundaries() {
        const BUFFER: u32 = 1 << 20;
        let mut bus = Bus::default();
        bus.map_ram(0, BUFFER as usize).unwrap();
        bus.map_ram(BUFFER, BUFFER as usize).unwrap();
        bus.write_bytes(0, &BYTES).unwrap();
        bus.write_bytes(BUFFER - 4, &BYTES).unwrap();
        assert_eq!(bus.find_bytes(&BYTES).unwrap(), [0, BUFFER - 4]);
    }

    #[test]
    fn byte_search_keeps_holes_and_peripherals_outside_mapped_ranges() {
        let mut bus = Bus::default();
        bus.map_ram(0, 2).unwrap();
        bus.map_byte_peripheral(2, 2).unwrap();
        bus.map_ram(4, 2).unwrap();
        bus.map_faulting_memory(0x1000, None).unwrap();
        bus.memory(0x1000, 2).unwrap().copy_from_slice(&[1, 2]);
        bus.map_ram(u32::MAX - 1, 2).unwrap();
        bus.write_bytes(0, &[1, 2, 1, 2, 1, 2]).unwrap();
        bus.write_bytes(u32::MAX - 1, &[1, 2]).unwrap();
        assert_eq!(bus.find_bytes(&[1, 2]).unwrap(), [0, 4, u32::MAX - 1]);
        assert!(bus.find_bytes(&[2, 1]).unwrap().is_empty());
    }

    fn check_mixed_transfer(first_memory: bool, second_memory: bool) {
        let mut bus = Bus::default();
        for (base, memory) in [(50, first_memory), (200, second_memory)] {
            if memory {
                bus.map_ram(base, 100).unwrap();
            } else {
                bus.map_byte_peripheral(base, 100).unwrap();
            }
        }
        let input = (0..350)
            .map(|value| (value % 255) as u8)
            .collect::<Vec<_>>();
        bus.write_bytes(0, &input).unwrap();
        let mut expected = vec![0; 350];
        for span in [50..150, 200..300] {
            expected[span.clone()].copy_from_slice(&input[span]);
        }
        assert_eq!(bus.read_bytes(0, 350).unwrap(), expected);
    }
}
