use std::{
    alloc::{Layout, alloc_zeroed},
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    fmt,
    fs::File,
    io::{self, Read, Write},
    path::Path,
    rc::{Rc, Weak},
    sync::Arc,
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
    stm32_rcc::{Stm32F0Rcc, Stm32F4Rcc},
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
    uart::{
        BinaryUartHistory, BinaryUartMatch, BinaryUartPattern, FailingUartPattern, LiteXTxTiming,
        LiteXUart, MuraxUart, UartHistory, UartWaitMode,
    },
};

// Cortex-M peripheral clocks use a 10 ns time quantum.
pub(crate) const CORTEX_M_STEP_HZ: u64 = 100_000_000;

pub const RAM_BASE: u32 = 0x8000_0000;
const TRIVIAL_PERIPHERAL_SIZE: u32 = 0x1000;

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
    AddressError(u32),
    Denied(u32),
    SlaveFault(u32),
    Page(u32),
    Misaligned(u32, usize),
    Output(io::Error),
    AudioOutput(io::Error),
    CommandError(Arc<dyn std::error::Error + Send + Sync>),
    Python(String),
}

impl BusError {
    fn command_error(error: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::CommandError(Arc::new(error))
    }

    pub fn at_virtual(self, address: u32) -> Self {
        match self {
            Self::Unmapped(_, width) => Self::Unmapped(address, width),
            Self::Command(_, fault) => Self::Command(address, fault),
            Self::AddressError(_) => Self::AddressError(address),
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
            Self::Command(address, _) => write!(f, "peripheral CommandError at {address:#010x}"),
            Self::AddressError(address) => write!(
                f,
                "Bus access failed with error: AddressError at {address:#010x}"
            ),
            Self::Python(error) => f.write_str(error),
            Self::SlaveFault(address) => write!(f, "bus slave fault at {address:#010x}"),
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
            Self::CommandError(error) => {
                write!(f, "Bus access failed with error: CommandError: {error}")
            }
        }
    }
}

impl std::error::Error for BusError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::CommandError(error) => Some(error.as_ref()),
            Self::Output(error) | Self::AudioOutput(error) => Some(error),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum UnhandledAccess {
    DoNotReport,
    Report,
    ThrowException,
    ThrowExceptionIfNotTagged,
}

#[derive(Clone, Copy)]
struct BusTag {
    start: u32,
    end: u32,
    default: u64,
    throws: bool,
}

#[derive(Clone, Copy)]
pub enum UartWatch {
    Port(usize),
    Tester(usize),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TesterPort {
    Stm32(usize),
    Trivial,
    LiteX,
    Nrf(usize),
    Rv64(usize),
}

enum TesterHistory {
    Text(UartHistory),
    Binary(BinaryUartHistory),
}

impl TesterHistory {
    fn receive(&mut self, byte: u8) {
        match self {
            Self::Text(history) => history.receive(byte),
            Self::Binary(history) => history.receive(byte),
        }
    }

    fn matched(&self) -> bool {
        match self {
            Self::Text(history) => history.matched(),
            Self::Binary(history) => history.matched(),
        }
    }

    fn finish_wait(&mut self) {
        match self {
            Self::Text(history) => history.finish_wait(),
            Self::Binary(history) => history.finish_wait(),
        }
    }
}

struct LiteXTimer {
    bases: Vec<u32>,
    timer: litex::Timer,
    vex_irq: Option<(usize, u8)>,
}

pub(crate) struct Region {
    base: u64,
    mapped: usize,
    aliases: Vec<u32>,
    privileged: bool,
    name: Option<String>,
    log_access: bool,
    bytes: Vec<u8>,
    flash_pages: Option<Vec<bool>>,
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

const FAULTING_PERIPHERAL_SIZE: usize = 0x1000;

struct FaultingPeripheral {
    base: u32,
    fault_offset: Option<usize>,
    fault_on_writes: bool,
    bytes: Box<[u8; FAULTING_PERIPHERAL_SIZE]>,
}

impl FaultingPeripheral {
    fn offset(&self, address: u32) -> Option<usize> {
        let offset = address.wrapping_sub(self.base) as usize;
        (offset < FAULTING_PERIPHERAL_SIZE).then_some(offset)
    }

    fn check_byte(&self, offset: usize, transaction: u32, write: bool) -> Result<(), BusError> {
        if offset >= FAULTING_PERIPHERAL_SIZE
            || ((!write || self.fault_on_writes)
                && self.fault_offset.is_none_or(|fault| fault == offset))
        {
            return Err(BusError::SlaveFault(transaction));
        }
        Ok(())
    }
}

struct RamIoRead {
    name: String,
    offset: usize,
    width: usize,
    value: u32,
}

#[derive(Clone)]
pub enum CpuHook {
    Write32(u32, u32),
    Log(String),
    Pause,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum NrfGpioReceiver {
    Led(usize),
    Connector(usize),
}

type NrfGpioRoutes = crate::nrf52840::gpio_line::GpioRoutes<NrfGpioReceiver>;

#[derive(Default)]
struct OwnedNrfGpioRoutes(Rc<RefCell<NrfGpioRoutes>>);

impl Drop for OwnedNrfGpioRoutes {
    fn drop(&mut self) {
        // External observers can retain the route table after the owning bus is gone.
        *self.0.borrow_mut() = NrfGpioRoutes::default();
    }
}

struct NrfGpioConnector {
    name: String,
    output: RefCell<crate::nrf52840::gpio_line::GpioLine<usize>>,
    led: Weak<RefCell<GpioLed>>,
}

#[derive(Default)]
pub struct Bus {
    monitor_log_level: Option<i8>,
    pub monitor_user_state: String,
    traceable_memory: Vec<(String, Vec<u32>, u32)>,
    traced_memory: Vec<(u32, u32, String)>,
    #[cfg(test)]
    pub faulting_accesses: Option<std::ops::RangeInclusive<u32>>,
    #[cfg(test)]
    pub faulting_device: Option<std::ops::RangeInclusive<u32>>,
    #[cfg(test)]
    command_fault_device: Option<(
        std::ops::Range<u32>,
        Arc<dyn std::error::Error + Send + Sync>,
    )>,
    #[cfg(test)]
    pub faulting_stack_writes: Option<std::ops::RangeInclusive<u32>>,
    #[cfg(test)]
    byte_peripheral: std::collections::BTreeMap<u32, u8>,
    command_error_peripherals: Vec<CommandErrorPeripheral>,
    mock_byte_peripherals: Vec<(std::ops::Range<u64>, std::collections::HashMap<u32, u8>)>,
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
    trace_rcc: bool,
    pydevs: Vec<crate::pydev::Peripheral>,
    thumb_read_watchpoints: BTreeSet<(u32, usize)>,
    pub thumb_read_watchpoint_hit: bool,
    pub cpu_hooks: BTreeMap<u32, Vec<CpuHook>>,
    pub thumb_block_pc_log: bool,
    pub itstate_log_hook: Option<crate::thumb::opcode_counter::HookPhase>,
    pub thumb_trace: Option<crate::thumb::trace::Trace>,
    pub thumb_opcodes: crate::thumb::opcode_counter::OpcodeCounters,
    pub thumb_opcode_hooks: crate::thumb::opcode_counter::OpcodeLogHooks,
    pub semihosting: Option<crate::semihosting::Semihosting>,
    ram_io_pages: BTreeSet<u32>,
    ram_io_logs: Vec<RamIoRead>,
    faulting_peripherals: Vec<FaultingPeripheral>,
    thumb_data_fault: Option<u32>,
    pub thumb_negative_priority: bool,
    pub thumb_unprivileged: bool,
    pub fault_on_unmapped_thumb_data: bool,
    pub report_unmapped_thumb_data: bool,
    trivial_peripherals: Vec<u32>,
    nrf_cpu_hz: Option<u64>,
    nrf_clock_remainder: u64,
    uart_tx: Option<u32>,
    trivial_uart: Option<(u32, String)>,
    uart_logs: Vec<(TesterPort, String, Vec<u8>)>,
    named_stm32_gpios: Vec<(String, u32)>,
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
    uart_testers: Vec<(TesterPort, TesterHistory)>,
    default_uart_tester: Option<usize>,
    exti: Option<Exti>,
    stm32_gpios: Vec<Stm32Gpio>,
    gpio_exti_levels: u32,
    gpio_leds: Vec<(u32, GpioLed)>,
    nrf_led_routes: OwnedNrfGpioRoutes,
    nrf_gpio_connectors: Vec<Option<Rc<NrfGpioConnector>>>,
    nrf_gpio_sinks: Vec<Weak<NrfGpioConnector>>,
    nrf_leds: Vec<Option<Rc<RefCell<GpioLed>>>>,
    gpio_buttons: Vec<Option<GpioButton>>,
    gpio_timer_inputs: Vec<(u32, u8, u32, u8)>,
    gpio_links: Vec<(u32, u8, u32, u8)>,
    led_events: bool,
    stm32_timers: Vec<Stm32Timer>,
    stm32_spis: Vec<Stm32F0Spi>,
    stm32_i2cs: Vec<Stm32I2c>,
    stm32_crc: Option<Stm32F0Crc>,
    stm32_cans: Vec<Stm32Can>,
    stm32_adc: Option<Stm32F0Adc>,
    stm32_adc_name: Option<String>,
    stm32_dma: Option<Stm32Dma>,
    policy_tags: Vec<BusTag>,
    pub unhandled_access: Option<UnhandledAccess>,
    stm32f0_tags: Vec<std::ops::RangeInclusive<u32>>,
    stm32_rcc: Option<Stm32F0Rcc>,
    stm32f4_rcc: Option<Stm32F4Rcc>,
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
    // The upstream F4 REPL supplies independent TIMx and SysTick clocks.
    pub f4_repl_timing: bool,
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
    h7_sdmmc_names: Vec<String>,
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
    pub(crate) clic: Option<crate::clic::Clic>,
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

fn invalidate_reservations(reservations: &mut [Option<u32>], written: std::ops::Range<u64>) {
    for reservation in reservations {
        if reservation.is_some_and(|reserved| {
            written.start < u64::from(reserved) + 4 && u64::from(reserved) < written.end
        }) {
            *reservation = None;
        }
    }
}

fn memory_byte_range(address: u32, count: usize) -> Result<std::ops::Range<u64>, String> {
    let start = u64::from(address);
    let end = start
        .checked_add(count as u64)
        .filter(|&end| end <= 1 << 32)
        .ok_or("SystemBus byte range exceeds address space")?;
    Ok(start..end)
}

impl Region {
    fn bases32(&self) -> impl Iterator<Item = u32> + '_ {
        u32::try_from(self.base)
            .ok()
            .into_iter()
            .chain(self.aliases.iter().copied())
    }

    fn byte_window(
        &self,
        base: u32,
        request: &std::ops::Range<u64>,
    ) -> Option<(std::ops::Range<usize>, std::ops::Range<usize>)> {
        let base = u64::from(base);
        let start = base.max(request.start);
        let end = (base + self.bytes.len() as u64).min(request.end);
        (start < end).then(|| {
            (
                (start - request.start) as usize..(end - request.start) as usize,
                (start - base) as usize..(end - base) as usize,
            )
        })
    }

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
            privileged: false,
            name: None,
            log_access: false,
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
    pub fn set_nrf_cpu_mips(&mut self, mips: u32) -> Result<(), String> {
        if mips == 0 {
            return Err("PerformanceInMips requires a positive integer".into());
        }
        if self.nrf_peripherals.is_empty() {
            return Err("PerformanceInMips requires an nRF52840 platform".into());
        }
        self.nrf_cpu_hz = Some(u64::from(mips) * 1_000_000);
        self.nrf_clock_remainder = 0;
        Ok(())
    }

    pub fn nrf_clock_ticks(&mut self) -> u64 {
        let Some(hz) = self.nrf_cpu_hz else {
            return 1;
        };
        // Accumulate clock fractions across instruction slots and system resets.
        let elapsed = self.nrf_clock_remainder + CORTEX_M_STEP_HZ;
        self.nrf_clock_remainder = elapsed % hz;
        elapsed / hz
    }
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

    pub fn map_bus_faulting_peripheral(
        &mut self,
        base: u32,
        fault_offset: Option<usize>,
    ) -> Result<(), String> {
        let start = u64::from(base);
        let end = start + FAULTING_PERIPHERAL_SIZE as u64;
        if end > 1 << 32 || self.overlaps(start, end) {
            return Err(format!(
                "invalid or overlapping faulting peripheral at {base:#010x}"
            ));
        }
        if fault_offset.is_some_and(|offset| offset >= FAULTING_PERIPHERAL_SIZE) {
            return Err("FaultOffset is outside the faulting peripheral".into());
        }
        self.faulting_peripherals.push(FaultingPeripheral {
            base,
            fault_offset,
            fault_on_writes: true,
            bytes: Box::new([0; FAULTING_PERIPHERAL_SIZE]),
        });
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn set_mock_fault_offset(&mut self, base: u32, offset: usize) {
        let device = self
            .faulting_peripherals
            .iter_mut()
            .find(|device| device.base == base)
            .unwrap();
        device.fault_offset = Some(offset);
    }

    pub fn set_mock_fault_on_writes(&mut self, base: u32, enabled: bool) {
        if let Some(device) = self
            .faulting_peripherals
            .iter_mut()
            .find(|device| device.base == base)
        {
            device.fault_on_writes = enabled;
        }
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

    pub fn map_trivial_peripheral(&mut self, base: u32) -> Result<(), String> {
        let start = u64::from(base);
        let end = start + u64::from(TRIVIAL_PERIPHERAL_SIZE);
        if base & 3 != 0 || end > 1 << 32 || self.overlaps(start, end) {
            return Err(format!(
                "invalid or overlapping trivial peripheral at {base:#010x}"
            ));
        }
        self.trivial_peripherals.push(base);
        Ok(())
    }

    fn trivial_peripheral_at(&self, address: u32) -> bool {
        address & 3 == 0
            && self
                .trivial_peripherals
                .iter()
                .any(|base| address.wrapping_sub(*base) < TRIVIAL_PERIPHERAL_SIZE)
    }

    pub fn map_privileged_ram(&mut self, base: u32, size: usize) -> Result<(), String> {
        self.map_ram(base, size)?;
        self.regions.last_mut().unwrap().privileged = true;
        Ok(())
    }

    pub fn map_named_ram(&mut self, name: &str, base: u32, size: usize) -> Result<(), String> {
        self.map_ram(base, size)?;
        self.regions.last_mut().unwrap().name = Some(name.into());
        Ok(())
    }

    pub fn log_ram_access(&mut self, name: &str) -> Result<(), String> {
        let region = self
            .regions
            .iter_mut()
            .find(|region| region.name.as_deref() == Some(name))
            .ok_or_else(|| format!("no named RAM peripheral {name:?}"))?;
        region.log_access = true;
        Ok(())
    }

    pub fn set_page_access_via_io(&mut self, address: u32, enabled: bool) {
        let page = address & !0xfff;
        if enabled {
            self.ram_io_pages.insert(page);
        } else {
            self.ram_io_pages.remove(&page);
        }
    }

    fn log_ram_io_read(&mut self, address: u32, width: usize, value: u32) {
        if self.ram_io_pages.contains(&(address & !0xfff))
            && let Some((name, offset)) = self.regions.iter().find_map(|region| {
                if !region.log_access {
                    return None;
                }
                Some((
                    region.name.as_deref()?,
                    region.offset(u64::from(address), width)?,
                ))
            })
        {
            self.ram_io_logs.push(RamIoRead {
                name: name.into(),
                offset,
                width,
                value,
            });
        }
    }

    pub fn write_ram_io_logs(&mut self, pc: u32, output: &mut impl Write) -> Result<(), String> {
        for RamIoRead {
            name,
            offset,
            width,
            value,
        } in self.ram_io_logs.drain(..)
        {
            writeln!(
                output,
                "{name}: [cpu: {pc:#x}] ReadUInt{} from {offset:#x} (unknown), returned {value:#x}",
                width * 8
            )
            .map_err(|error| format!("RAM I/O log: {error}"))?;
        }
        Ok(())
    }

    pub fn high_memory(&mut self, address: u64, width: usize) -> Result<&mut [u8], String> {
        self.memory64(address, width)
            .ok_or_else(|| format!("unmapped {width}-byte access at {address:#018x}"))
    }

    pub fn map_global_ram(&mut self, base: u64, size: usize) -> Result<(), String> {
        let end = base.checked_add(size as u64).ok_or("RAM range overflow")?;
        if base < 1 << 32 && end > 1 << 32 {
            return Err("RAM registration crosses the 32-bit address boundary".into());
        }
        self.map_ram64(base, size)
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

    pub(crate) fn unregister_from_address(&mut self, address: u32) -> Result<(), String> {
        if let Some(index) = self
            .constant_mmio
            .iter()
            .position(|peripheral| peripheral.base == address)
        {
            self.constant_mmio.swap_remove(index);
            return Ok(());
        }
        if let Some(index) = self
            .h7_timers
            .iter()
            .position(|timer| timer.base == address)
        {
            self.h7_timers.swap_remove(index);
            for port in &mut self.h7_gpio {
                port.disconnect_timer(address);
            }
            for led in &mut self.gpio_pwm_leds {
                for route in &mut led.routes {
                    if route.is_some_and(|(base, _)| base == address) {
                        *route = None;
                    }
                }
            }
            return Ok(());
        }
        self.unregister_ram(address)
    }

    pub(crate) fn unregister_ram(&mut self, address: u32) -> Result<(), String> {
        let index = self
            .regions
            .iter()
            .position(|region| {
                region.base == u64::from(address) || region.aliases.contains(&address)
            })
            .ok_or_else(|| format!("no RAM registration at {address:#010x}"))?;
        let region = &mut self.regions[index];
        if region.base == u64::from(address) {
            if let Some(base) = region.aliases.pop() {
                region.base = u64::from(base);
            } else {
                if self.aliases.iter().any(|alias| alias.backing == index) {
                    return Err("cannot unregister RAM with CPU-specific aliases".into());
                }
                self.regions.remove(index);
                for alias in &mut self.aliases {
                    if alias.backing > index {
                        alias.backing -= 1;
                    }
                }
            }
        } else {
            region.aliases.retain(|&base| base != address);
        }
        self.reservations.fill(None);
        self.rv64_reservations.fill(None);
        Ok(())
    }

    pub(crate) fn read_memory_bytes(&self, address: u32, count: usize) -> Result<Vec<u8>, String> {
        let request = memory_byte_range(address, count)?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(count)
            .map_err(|_| "cannot allocate SystemBus byte read")?;
        bytes.resize(count, 0);
        for region in &self.regions {
            for base in region.bases32() {
                if let Some((buffer, memory)) = region.byte_window(base, &request) {
                    bytes[buffer].copy_from_slice(&region.bytes[memory]);
                }
            }
        }
        Ok(bytes)
    }

    pub(crate) fn read_bus_bytes(
        &mut self,
        address: u32,
        count: usize,
        only_memory: bool,
    ) -> Result<Vec<u8>, String> {
        let range = memory_byte_range(address, count)?;
        let peripherals = !range.is_empty() && self.peripheral_overlaps(range.start, range.end);
        if only_memory && peripherals {
            return Err("SystemBus memory-only transfer intersects a peripheral".into());
        }
        let mut bytes = self.read_memory_bytes(address, count)?;
        if peripherals {
            for (offset, byte) in bytes.iter_mut().enumerate() {
                let location = u64::from(address) + offset as u64;
                if self.peripheral_overlaps(location, location + 1) {
                    *byte = self
                        .read::<1>(location as u32)
                        .map_err(|error| error.to_string())? as u8;
                }
            }
        }
        Ok(bytes)
    }

    pub(crate) fn write_bus_bytes(
        &mut self,
        address: u32,
        bytes: &[u8],
        only_memory: bool,
        uart: &mut impl Write,
    ) -> Result<(), String> {
        let range = memory_byte_range(address, bytes.len())?;
        if range.is_empty() || !self.peripheral_overlaps(range.start, range.end) {
            return self.write_memory_bytes(address, bytes);
        }
        if only_memory {
            return Err("SystemBus memory-only transfer intersects a peripheral".into());
        }
        for (location, &byte) in range.zip(bytes) {
            if self.overlaps(location, location + 1) {
                self.write(location as u32, 1, u32::from(byte), uart)
                    .map_err(|error| error.to_string())?;
            }
        }
        Ok(())
    }

    pub(crate) fn find_memory_bytes(
        &self,
        pattern: &[u8],
        start: u32,
        end: u32,
    ) -> Result<Vec<u32>, String> {
        self.find_bytes(pattern, u64::from(start), u64::from(end))
            .map(|hits| hits.into_iter().map(|hit| hit as u32).collect())
    }

    pub(crate) fn write_memory_bytes(&mut self, address: u32, bytes: &[u8]) -> Result<(), String> {
        let request = memory_byte_range(address, bytes.len())?;
        let mut windows = Vec::new();
        for (index, region) in self.regions.iter().enumerate() {
            for base in region.bases32() {
                if let Some((buffer, memory)) = region.byte_window(base, &request) {
                    windows.push((buffer, index, memory));
                }
            }
        }
        windows.sort_unstable_by_key(|(buffer, _, _)| buffer.start);
        for (buffer, index, memory) in windows {
            let region = &mut self.regions[index];
            let offset = memory.start;
            let width = memory.len();
            let destination = region.base + offset as u64;
            self.memory64(destination, width)
                .ok_or("RAM registration disappeared")?
                .copy_from_slice(&bytes[buffer]);
        }
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

    pub fn map_h7_network_control(&mut self, mut control: NetworkControl) -> Result<(), String> {
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
        Self::initialize_timer_clocks(&mut self.h7_timers, &mut control);
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

    pub(crate) fn monitor_step_hz(&self) -> u64 {
        if self.nrf_peripherals.is_empty() {
            u64::from(self.step_hz())
        } else {
            self.nrf_cpu_hz.unwrap_or(CORTEX_M_STEP_HZ)
        }
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
                .any(|&(base, size)| {
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

    pub fn gpio_led_states(&self) -> impl Iterator<Item = (&str, bool)> {
        self.h7_gpio_led_states()
    }

    pub(crate) fn tag_with_policy(
        &mut self,
        range: std::ops::RangeInclusive<u32>,
        value: u32,
        throws: bool,
    ) -> Result<(), String> {
        let start = *range.start();
        let size = range
            .end()
            .checked_sub(start)
            .and_then(|n| n.checked_add(1))
            .ok_or("invalid bus tag bounds")?;
        self.tag(start, size, value.into(), throws)
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

    pub fn set_timer_input(&mut self, name: &str, channel: u8, high: bool) -> Result<(), String> {
        let timer = self
            .h7_timers
            .iter_mut()
            .find(|timer| timer.name == name)
            .ok_or_else(|| format!("unknown STM32 timer {name:?}"))?;
        if timer_clocked(self.h7_network_control.as_ref(), timer) {
            timer.set_input(channel, high);
        }
        Ok(())
    }

    fn initialize_timer_clocks(timers: &mut [H7Timer], control: &mut NetworkControl) {
        for timer in timers {
            if let Some(clock) = timer.clock {
                control.enable_registered_timer(clock);
                if let Some(frequency) = control.timer_frequency(clock) {
                    timer.frequency = frequency;
                }
            }
        }
    }

    pub fn rcc_base(&self) -> Result<u32, String> {
        self.h7_network_control
            .as_ref()
            .map(|rcc| rcc.rcc_base)
            .ok_or_else(|| "no STM32H7 RCC configured".into())
    }

    pub fn trace_rcc(&mut self) -> Result<(), String> {
        self.rcc_base()?;
        self.trace_rcc = true;
        Ok(())
    }

    pub fn timer_properties(&self, name: &str) -> Result<(u32, bool), String> {
        let timer = self
            .h7_timers
            .iter()
            .find(|timer| timer.name == name)
            .ok_or_else(|| format!("unknown STM32 timer {name:?}"))?;
        let enabled = timer.clock.is_none_or(|clock| {
            self.h7_network_control
                .as_ref()
                .is_some_and(|rcc| rcc.timer_gate_enabled(clock))
        });
        Ok((timer.frequency, enabled))
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

    pub fn find_imu(&mut self, name: &str) -> Result<&mut crate::lsm6dso::Lsm6dso, String> {
        self.h7_i2c
            .iter_mut()
            .find_map(|bus| bus.find_imu(name))
            .ok_or_else(|| format!("unknown LSM6DSO sensor {name:?}"))
    }

    pub fn feed_magnetic(&mut self, name: &str, file: &std::path::Path) -> Result<(), String> {
        self.h7_i2c
            .iter_mut()
            .find_map(|bus| bus.find_magnetometer(name))
            .ok_or_else(|| format!("unknown IIS2MDC sensor {name:?}"))?
            .feed_resd(file)
    }

    pub fn feed_ina228(
        &mut self,
        name: &str,
        file: &std::path::Path,
        input: usize,
        channel: u16,
    ) -> Result<(), String> {
        self.h7_i2c
            .iter_mut()
            .find_map(|bus| bus.find_power_monitor(name))
            .ok_or_else(|| format!("unknown INA228 sensor {name:?}"))?
            .feed(file, input, channel, self.h7_step)
    }

    pub fn feed_pressure(
        &mut self,
        name: &str,
        path: &std::path::Path,
        temperature: bool,
    ) -> Result<(), String> {
        if temperature
            && let Some(sensor) = self
                .h7_i2c
                .iter_mut()
                .find_map(|bus| bus.find_power_monitor(name))
        {
            return sensor.feed(path, 2, 0, self.h7_step);
        }
        let nanoseconds = self
            .h7_step
            .saturating_mul(1_000_000_000 / u64::from(crate::machine::VIRTUAL_STEPS_PER_SECOND));
        self.find_pressure(name)?
            .feed_samples(path, temperature, nanoseconds)
    }

    pub fn find_pressure(&mut self, name: &str) -> Result<&mut crate::lps25hb::Lps25hb, String> {
        self.h7_i2c
            .iter_mut()
            .find_map(|bus| bus.find_pressure(name))
            .ok_or_else(|| format!("unknown pressure sensor {name:?}"))
    }

    pub fn feed_illuminance(&mut self, name: &str, path: &std::path::Path) -> Result<(), String> {
        self.h7_i2c
            .iter_mut()
            .find_map(|bus| bus.find_light(name))
            .ok_or_else(|| format!("unknown illuminance sensor {name:?}"))?
            .feed_illuminance(path)
    }

    pub fn set_illuminance(&mut self, name: &str, value: f64) -> Result<(), String> {
        self.h7_i2c
            .iter_mut()
            .find_map(|bus| bus.find_light(name))
            .ok_or_else(|| format!("unknown illuminance sensor {name:?}"))?
            .set_illuminance(value)
    }

    pub fn touch_input(
        &mut self,
        name: &str,
        event: crate::ft5336::PointerEvent,
    ) -> Result<(), String> {
        let device = self
            .h7_i2c
            .iter_mut()
            .find_map(|bus| bus.find_touch(name))
            .ok_or_else(|| format!("unknown touchscreen {name:?}"))?;
        device.input(event);
        Ok(())
    }

    pub fn feed_imu_samples(&mut self, name: &str, file: &std::path::Path) -> Result<(), String> {
        let start = self.h7_step + u64::from(crate::machine::VIRTUAL_STEPS_PER_SECOND) / 10;
        self.find_imu(name)?.load_samples(file, start, 12)
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

    pub(crate) fn name_stm32_gpio(&mut self, name: String, base: u32) {
        self.named_stm32_gpios.push((name, base));
    }

    pub(crate) fn connect_stm32_gpio(&mut self, source: &str, output: u8, target: &str, input: u8) -> Result<(), String> {
        let port = |name: &str| self.named_stm32_gpios.iter()
            .find(|(port, _)| port == name).map(|(_, base)| *base)
            .ok_or_else(|| format!("unsupported STM32 GPIO port {name:?}"));
        let (source, target) = ((port(source)?, output), (port(target)?, input));
        if output >= 16 || input >= 16 {
            return Err("STM32 GPIO pins must be between 0 and 15".into());
        }
        if !self.gpio_wires.iter().any(|wire| wire.source == source && wire.target == target) {
            self.gpio_wires.push(GpioWire { source, target, level: false });
        }
        Ok(())
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
        self.h7_step = self.h7_step.wrapping_add(1);
        let step = self.h7_step;
        for i2c in &mut self.h7_i2c {
            for (base, pin, level) in i2c.sample_sensor_outputs(step) {
                if let Some(port) = self.h7_gpio.iter_mut().find(|port| port.base == base) {
                    port.set_external(pin, level);
                }
            }
        }
        self.sample_timer_pins();
        let rate = self.step_hz();
        let control = self.h7_network_control.as_ref();
        for uart in &mut self.stm32_uarts {
            if usart_clocked(control, uart) {
                uart.tick_receive();
            }
        }
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
            if timer_clocked(control, timer) && (self.f4_repl_timing || f4_clocked(self.f4_rcc.as_ref(), timer.base)) {
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
        for port in &mut self.h7_gpio {
            for (base, channel, level) in port.timer_input_changes() {
                if let Some(timer) = self
                    .h7_timers
                    .iter_mut()
                    .find(|timer| timer.base == base && timer_clocked(control, timer))
                {
                    timer.set_input(channel, level);
                }
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

    fn apply_nrf_input(&mut self, base: u32, pin: u8, high: bool) {
        if let Some(port) = self
            .nrf_peripherals
            .iter_mut()
            .find(|port| port.base == base)
        {
            port.set_gpio_input(pin, high);
        }
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
            self.sample_nrf_gpio_outputs(base, high);
        }
    }

    pub fn sample_nrf_gpio_outputs(&mut self, gpio: u32, pins: u32) {
        Self::sample_nrf_led_routes(
            &self.nrf_led_routes.0.borrow(),
            &self.nrf_gpio_connectors,
            &self.nrf_leds,
            gpio,
            pins,
            self.led_events,
        );
        for device in &mut self.nrf_peripherals {
            device.release_spi_cs(gpio, pins);
        }
    }

    fn sample_nrf_led_routes(
        routes: &NrfGpioRoutes,
        connectors: &[Option<Rc<NrfGpioConnector>>],
        leds: &[Option<Rc<RefCell<GpioLed>>>],
        gpio: u32,
        pins: u32,
        report: bool,
    ) {
        for (pin, line) in routes.lines(gpio) {
            let high = pins & (1 << pin) != 0;
            for endpoint in line.endpoints() {
                match endpoint.receiver {
                    NrfGpioReceiver::Led(index) => leds[index]
                        .as_ref()
                        .unwrap()
                        .borrow_mut()
                        .sample_level(high, report),
                    NrfGpioReceiver::Connector(index) => {
                        let connector = connectors[index].as_ref().unwrap();
                        if !connector.output.borrow().endpoints().is_empty()
                            && let Some(led) = connector.led.upgrade()
                        {
                            led.borrow_mut().sample_level(high, report);
                        }
                    }
                }
            }
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

    pub fn nrf_adxl_input(&mut self, name: &str) -> Option<&mut [i16; 3]> {
        self.nrf_peripherals
            .iter_mut()
            .find_map(|device| device.adxl_input(name))
    }

    pub fn set_nrf_camera_image(&mut self, image: Vec<u8>) -> Result<(), String> {
        if image.len() > 0xff_ffff {
            return Err("ArduCAM image exceeds the 24-bit FIFO size".into());
        }
        let mut image = Some(image);
        if self
            .nrf_peripherals
            .iter_mut()
            .any(|device| device.set_camera_image(&mut image))
        {
            Ok(())
        } else {
            Err("--nrf-camera-image requires an nRF52840 ArduCAM".into())
        }
    }

    pub fn set_nrf_lsm9ds1_samples(&mut self, input: &str) -> Result<(), String> {
        let samples = self
            .nrf_peripherals
            .iter_mut()
            .find_map(NrfPeripheral::lsm9ds1_samples)
            .ok_or("--lsm9ds1-samples requires an nRF52840 LSM9DS1 IMU")?;
        let mut parsed = Vec::new();
        for (index, line) in input.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let values = line
                .split_whitespace()
                .map(str::parse::<f64>)
                .collect::<Result<Vec<_>, _>>();
            match values.as_deref() {
                Ok([x, y, z]) if [x, y, z].iter().all(|value| value.is_finite()) => {
                    parsed.push([*x, *y, *z]);
                }
                _ => return Err(format!("invalid LSM9DS1 sample at line {}", index + 1)),
            }
        }
        samples.extend(parsed);
        Ok(())
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

    pub fn attach_uart_link(&mut self, name: &str) -> Result<usize, String> {
        let index = self
            .nrf_peripherals
            .iter()
            .position(|device| device.is_uart() && device.name == name)
            .ok_or_else(|| format!("unknown nRF52840 UART {name:?}"))?;
        self.nrf_peripherals[index].link_uart();
        Ok(index)
    }

    pub fn exchange_uart_link(
        &mut self,
        local: usize,
        peer: &mut Self,
        remote: usize,
    ) -> Result<(), BusError> {
        let local_tx = self.nrf_peripherals[local].take_uart_link_output();
        let remote_tx = peer.nrf_peripherals[remote].take_uart_link_output();
        self.receive_uart_link(local, remote_tx)?;
        peer.receive_uart_link(remote, local_tx)
    }

    fn receive_uart_link(&mut self, index: usize, bytes: Vec<u8>) -> Result<(), BusError> {
        if bytes.is_empty() {
            return Ok(());
        }
        let mut device = self.nrf_peripherals.remove(index);
        device.queue_uart_link_input(bytes);
        let result = device.receive(self);
        self.nrf_peripherals.insert(index, device);
        result
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

    pub fn nrf_uart_irq(&mut self, name: &str, level: Option<bool>) -> Result<bool, String> {
        let uart = self
            .nrf_peripherals
            .iter_mut()
            .find(|device| device.is_uart() && device.name == name)
            .ok_or_else(|| format!("unsupported UART IRQ target {name:?}"))?;
        if let Some(level) = level {
            uart.set_uart_irq(level);
        }
        Ok(uart.pending())
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

    fn peripheral_overlaps(&self, start: u64, end: u64) -> bool {
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
                    || self.pydevs.iter().any(|device| device.overlaps(start, end))
                    || self.trivial_peripherals.iter().any(|&base| {
                        let base = u64::from(base);
                        start < base + u64::from(TRIVIAL_PERIPHERAL_SIZE) && base < end
                    })
                    || self.faulting_peripherals.iter().any(|device| {
                        let base = u64::from(device.base);
                        start < base + FAULTING_PERIPHERAL_SIZE as u64 && base < end
                    })
                    || self.nrf_peripherals.iter().any(|device| {
                        let other = u64::from(device.base);
                        start < other + u64::from(device.size()) && other < end
                    })
                    || self
                        .nvic
                        .as_ref()
                        .is_some_and(|_| Nvic::overlaps(start, end))
            })
            || (self.systick.is_some() && SysTick::overlaps(start, end))
            || self
                .uart_tx
                .is_some_and(|address| (start..end).contains(&u64::from(address)))
            || self.trivial_uart.as_ref().is_some_and(|(base, _)| {
                let base = u64::from(*base);
                start < base + 4 && base < end
            })
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
            || self.stm32f4_rcc.as_ref().is_some_and(|rcc| {
                let other = u64::from(rcc.base);
                start < other + u64::from(Stm32F4Rcc::SIZE) && other < end
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
            || self.clic.as_ref().is_some_and(|clic| {
                let other = u64::from(clic.base);
                start < other + u64::from(crate::clic::Clic::SIZE) && other < end
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
            || self
                .command_error_peripherals
                .iter()
                .any(|device| overlaps_range(start, end, device.base, device.size))
            || self.constant_mmio.iter().any(|peripheral| {
                let base = u64::from(peripheral.base);
                start < base + u64::from(peripheral.size) && base < end
            })
    }

    pub(crate) fn overlaps(&self, start: u64, end: u64) -> bool {
        self.peripheral_overlaps(start, end)
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

    pub(crate) fn map_clic(&mut self, base: u32, name: &str, machine_levels: u8) -> Result<(), String> {
        let start = u64::from(base);
        let end = start + u64::from(crate::clic::Clic::SIZE);
        if end > 1 << 32 || self.clic.is_some() || self.overlaps(start, end) {
            return Err("invalid or overlapping CLIC mapping".into());
        }
        self.clic = Some(crate::clic::Clic::new(base, name, machine_levels));
        Ok(())
    }

    pub fn map_trivial_uart(&mut self, base: u32, name: &str) -> Result<(), String> {
        let start = u64::from(base);
        if self.trivial_uart.is_some() || base & 3 != 0 || self.overlaps(start, start + 4) {
            return Err("invalid, duplicate, or overlapping TrivialUart".into());
        }
        self.trivial_uart = Some((base, name.to_owned()));
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

    pub fn nvic_register_address(&self, target: &str, offset: u32) -> Result<u32, String> {
        self.validate_nvic_name(target)?;
        if offset >= 0x1000 {
            return Err(format!("NVIC register offset out of range: {offset:#x}"));
        }
        Ok(0xe000_e000 + offset)
    }

    pub(crate) fn register_traceable_memory(&mut self, name: &str, addresses: &[u32], size: u32) {
        self.traceable_memory.push((name.to_owned(), addresses.to_vec(), size));
    }

    pub(crate) fn set_trace_memory(&mut self, name: &str, enabled: bool) -> Result<(), String> {
        let (_, addresses, size) = self.traceable_memory.iter()
            .find(|(registered, _, _)| registered == name)
            .ok_or_else(|| format!("unsupported peripheral access logging target {name:?}"))?;
        self.traced_memory.retain(|(_, _, registered)| registered != name);
        if enabled {
            self.traced_memory.extend(addresses.iter().map(|&base| (base, *size, name.to_owned())));
        }
        Ok(())
    }

    fn log_memory_access(&self, write: bool, address: u32, width: usize, value: u32) {
        for (base, size, name) in &self.traced_memory {
            let Some(offset) = address.checked_sub(*base) else { continue };
            if u64::from(offset) + width as u64 > u64::from(*size) {
                continue;
            }
            let access = match width {
                1 => "Byte",
                2 => "Word",
                4 => "DoubleWord",
                _ => continue,
            };
            let value = value & (u32::MAX >> (32 - width * 8));
            let message = if write {
                format!("{name}: Write{access} to 0x{offset:X}, value 0x{value:X}")
            } else {
                format!("{name}: Read{access} from 0x{offset:X}, returned 0x{value:X}")
            };
            crate::monitor_log::record(1, &message);
            if self.monitor_log_enabled(1) {
                eprintln!("{message}");
            }
        }
    }

    pub fn set_monitor_log_level(&mut self, level: i8) {
        self.monitor_log_level = Some(level);
        crate::monitor_log::set_level(level);
    }

    pub fn monitor_log_enabled(&self, severity: i8) -> bool {
        severity >= self.monitor_log_level.unwrap_or(1)
    }

    pub fn enable_nvic_log(&mut self, target: &str) -> Result<(), String> {
        self.validate_nvic_name(target)?;
        self.systick
            .as_mut()
            .ok_or("NVIC logging requires SysTick")?
            .set_noisy();
        Ok(())
    }

    fn validate_nvic_name(&self, target: &str) -> Result<(), String> {
        let name = target.strip_prefix("sysbus.").unwrap_or(target);
        if self.nvic.is_none() || self.nvic_name.as_deref() != Some(name) {
            return Err(format!("unknown Cortex-M NVIC {target:?}"));
        }
        Ok(())
    }

    pub fn pend_nvic_exception(&mut self, target: &str, exception: u32) -> Result<(), String> {
        use crate::systick::{ICSR, NMI, PENDSV, SYSTICK};

        self.validate_nvic_name(target)?;
        let pending = match u8::try_from(exception) {
            Ok(irq @ 16..=47) => {
                self.nvic
                    .as_mut()
                    .unwrap()
                    .set_line(irq - 16, true, &ActiveExceptions::default());
                return Ok(());
            }
            Ok(crate::nvic::HARDFAULT) => {
                self.nvic.as_mut().unwrap().set_hardfault_pending(true);
                return Ok(());
            }
            Ok(NMI) => 1 << 31,
            Ok(PENDSV) => 1 << 28,
            Ok(SYSTICK) => 1 << 26,
            _ => return Err(format!("unsupported Cortex-M exception {exception}")),
        };
        self.systick
            .as_mut()
            .ok_or("system exception pending bits require SCB ICSR")?
            .write(ICSR, pending);
        Ok(())
    }

    pub fn set_frequency(&mut self, target: &str, frequency: u32) -> Result<(), String> {
        let name = target.strip_prefix("sysbus.").unwrap_or(target);
        if let Some(timer) = self
            .stm32_timers
            .iter_mut()
            .find(|timer| timer.name.as_deref() == Some(name))
        {
            if frequency == 0 {
                return Err("STM32 timer Frequency must be positive".into());
            }
            timer.set_frequency(frequency);
            return Ok(());
        }

        self.validate_nvic_name(target)?;
        if frequency == 0 {
            return Err("NVIC Frequency must be positive".into());
        }
        self.systick
            .as_mut()
            .ok_or("NVIC Frequency requires a SysTick counter")?
            .set_frequency(frequency);
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

    pub fn tick_stm32_uarts(&mut self) {
        for uart in &mut self.stm32_uarts {
            uart.tick();
        }
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
            Err(error @ (BusError::Output(_) | BusError::CommandError(_))) => Err(error),
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
        self.reset_cortex_m_device_state(reload);
        true
    }

    pub fn reset_cortex_m_machine_devices(&mut self) {
        self.reset_cortex_m_devices();
        self.reset_cortex_m_device_state(false);
    }

    fn reset_cortex_m_device_state(&mut self, reload: bool) {
        if let Some(handler) = self.semihosting.as_mut() {
            handler.reset();
        }
        self.reservations.fill(None);
        for device in &mut self.faulting_peripherals {
            device.bytes.fill(0);
        }
        for device in &mut self.nrf_peripherals {
            device.reset();
        }
        if let Some(nvic) = self.nvic.as_mut() {
            nvic.reset();
        }
        if let Some(systick) = self.systick.as_mut() {
            systick.reset();
        }
        if let Some(dwt) = self.dwt.as_mut() {
            dwt.reset();
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
        if let Some(rcc) = self.stm32f4_rcc.as_mut() {
            *rcc = Stm32F4Rcc::new(rcc.base);
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
                if f4_clocked(self.f4_rcc.as_ref(), uart.base)
                    && usart_clocked(self.h7_network_control.as_ref(), uart)
                    && uart.interrupt_pending()
                {
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
                                && (self.f4_repl_timing || f4_clocked(self.f4_rcc.as_ref(), timer.base))
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

    pub fn map_pydev(
        &mut self,
        source: &str,
        base: u32,
        size: u32,
        initable: bool,
    ) -> Result<(), String> {
        let base = u64::from(base);
        let end = base + u64::from(size);
        if end > 1 << 32 || self.overlaps(base, end) {
            return Err("overlapping or invalid Python peripheral range".into());
        }
        self.pydevs.push(crate::pydev::Peripheral::new(
            source,
            base,
            u64::from(size),
            initable,
        )?);
        Ok(())
    }

    pub fn add_thumb_read_watchpoint(&mut self, address: u32, width: usize) {
        self.thumb_read_watchpoints.insert((address, width));
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

    pub(crate) fn write_rv64_peripheral(
        &mut self, hart: usize, address: u32, width: usize, value: u32, output: &mut impl Write,
    ) -> Option<io::Result<()>> {
        if self.uart_testers.is_empty() && self.uart_logs.is_empty() {
            return self.rv64_devices.write_for_hart(hart, address, width, value, output);
        }
        let Some(index) = self.rv64_devices.uart_for_hart(hart, address) else {
            return self.rv64_devices.write_for_hart(hart, address, width, value, output);
        };
        let mut bytes = Vec::new();
        let result = self.rv64_devices.write_for_hart(hart, address, width, value, &mut bytes)?;
        Some(result.and_then(|()| {
            output.write_all(&bytes)?;
            let console = self.monitor_log_enabled(1);
            Self::observe_uart(&mut self.uart_testers, &mut self.uart_logs, TesterPort::Rv64(index), &bytes, console);
            Ok(())
        }))
    }

    pub fn tick_litex_uart(&mut self, step: u64, output: &mut impl Write) -> io::Result<()> {
        if let Some(uart) = self.litex_uart.as_mut() {
            let mut bytes = Vec::new();
            uart.tick(step, &mut bytes)?;
            output.write_all(&bytes)?;
            let console = self.monitor_log_enabled(1);
            Self::observe_uart(&mut self.uart_testers, &mut self.uart_logs, TesterPort::LiteX, &bytes, console);
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

    pub fn tag(&mut self, start: u32, size: u32, default: u64, throws: bool) -> Result<(), String> {
        let end = size
            .checked_sub(1)
            .and_then(|last| start.checked_add(last))
            .ok_or("tag range must be nonempty and within the 32-bit bus")?;
        let tag = BusTag {
            start,
            end,
            default,
            throws,
        };
        if let Some(index) = self
            .policy_tags
            .iter()
            .position(|tag| tag.start <= end && start <= tag.end)
        {
            let parent = self.policy_tags[index];
            if start < parent.start || end > parent.end {
                return Err("subtag must be completely contained in a single existing tag".into());
            }
            self.policy_tags.swap_remove(index);
            if parent.start < start {
                self.policy_tags.push(BusTag {
                    end: start - 1,
                    ..parent
                });
            }
            if end < parent.end {
                self.policy_tags.push(BusTag {
                    start: end + 1,
                    ..parent
                });
            }
        }
        self.policy_tags.push(tag);
        Ok(())
    }

    fn unhandled_value(&self, address: u32, width: usize, monitor: bool) -> Result<u32, BusError> {
        let tag = self
            .policy_tags
            .iter()
            .find(|tag| tag.start <= address && address <= tag.end);
        let tagged = tag.is_some() || self.stm32f0_tagged(address, width);
        if tag.is_some_and(|tag| tag.throws)
            || self.unhandled_access == Some(UnhandledAccess::ThrowException)
            || (!tagged
                && self.unhandled_access == Some(UnhandledAccess::ThrowExceptionIfNotTagged))
        {
            return Err(BusError::AddressError(address));
        }
        if tagged || monitor || self.unhandled_access == Some(UnhandledAccess::DoNotReport) {
            let mask = u32::MAX >> (32 - width * 8);
            return Ok(tag.map_or(0, |tag| tag.default as u32 & mask));
        }
        Err(BusError::Unmapped(address, width))
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

    pub fn route_gpio_pin(&mut self, source: u32, output: u8, target: u32, input: u8) {
        self.gpio_links.push((source, output, target, input));
    }

    pub fn route_gpio_timer_input(&mut self, port: u32, pin: u8, timer: u32, channel: u8) {
        self.gpio_timer_inputs.push((port, pin, timer, channel));
    }

    pub fn add_gpio_led(&mut self, base: u32, led: GpioLed) {
        self.gpio_leds.push((base, led));
        self.refresh_gpio_wiring();
    }

    pub fn add_nrf_gpio_led(
        &mut self,
        base: u32,
        mut led: GpioLed,
        input: u32,
    ) -> Result<(), String> {
        self.nrf_led_routes
            .0
            .borrow_mut()
            .line_mut(base, led.pin)
            .connect(
                NrfGpioReceiver::Led(self.nrf_leds.len()),
                &led.name,
                1,
                input,
            )?;
        let driven = self.nrf_gpio_driven_high(base);
        led.sample_level(driven & (1 << led.pin) != 0, self.led_events);
        self.nrf_leds.push(Some(Rc::new(RefCell::new(led))));
        Ok(())
    }

    pub fn bridge_nrf_led(&mut self, connector: &str, name: &str) -> Result<(), String> {
        self.bridge_nrf_led_from(connector, name, None).map(|_| ())
    }

    fn bridge_nrf_led_from(
        &mut self,
        connector: &str,
        name: &str,
        source: Option<(&mut Bus, u32, u8)>,
    ) -> Result<Rc<NrfGpioConnector>, String> {
        let led = self
            .nrf_leds
            .iter()
            .position(|led| led.as_ref().is_some_and(|led| led.borrow().name == name))
            .ok_or_else(|| format!("unknown nRF GPIO LED {name:?}"))?;
        let receiver = NrfGpioReceiver::Led(led);
        let mut output = crate::nrf52840::gpio_line::GpioLine::default();
        output.connect(led, name, 1, 0)?;
        let bridge = Rc::new(NrfGpioConnector {
            name: connector.into(),
            output: RefCell::new(output),
            led: Rc::downgrade(self.nrf_leds[led].as_ref().unwrap()),
        });
        if let Some((source_bus, base, pin)) = source {
            source_bus.attach_nrf_gpio_connector(base, pin, &bridge)?;
        } else {
            let (base, pin) = self
                .nrf_led_routes
                .0
                .borrow()
                .source_of(&receiver)
                .ok_or_else(|| format!("nRF GPIO LED {name:?} has no direct connection"))?;
            self.attach_nrf_gpio_connector(base, pin, &bridge)?;
        }
        self.nrf_led_routes
            .0
            .borrow_mut()
            .unregister_peripheral(None, Some(&receiver));
        self.nrf_gpio_sinks.retain(|sink| sink.strong_count() != 0);
        self.nrf_gpio_sinks.push(Rc::downgrade(&bridge));
        Ok(bridge)
    }

    fn attach_nrf_gpio_connector(
        &mut self,
        base: u32,
        pin: u8,
        bridge: &Rc<NrfGpioConnector>,
    ) -> Result<(), String> {
        if pin >= 32
            || !self
                .nrf_peripherals
                .iter()
                .any(|device| device.base == base && device.gpio_driven_high().is_some())
        {
            return Err(format!("invalid nRF GPIO connector source {base:#x}:{pin}"));
        }
        if self
            .nrf_gpio_connectors
            .iter()
            .flatten()
            .any(|item| item.name == bridge.name)
        {
            return Err(format!("duplicate nRF GPIO connector {:?}", bridge.name));
        }
        self.nrf_led_routes
            .0
            .borrow_mut()
            .line_mut(base, pin)
            .connect(
                NrfGpioReceiver::Connector(self.nrf_gpio_connectors.len()),
                &bridge.name,
                1,
                0,
            )?;
        self.nrf_gpio_connectors.push(Some(bridge.clone()));
        Ok(())
    }

    pub fn remove_nrf_gpio_connector(&mut self, name: &str) -> Result<(), String> {
        let index = self
            .nrf_gpio_connectors
            .iter()
            .position(|item| item.as_ref().is_some_and(|item| item.name == name))
            .ok_or_else(|| format!("unknown nRF GPIO connector {name:?}"))?;
        self.nrf_led_routes
            .0
            .borrow_mut()
            .unregister_peripheral(None, Some(&NrfGpioReceiver::Connector(index)));
        self.nrf_gpio_connectors[index] = None;
        Ok(())
    }

    pub fn unregister_nrf_gpio_peripheral(&mut self, name: &str) -> Result<(), String> {
        if let Some(receiver) = self
            .nrf_leds
            .iter()
            .position(|led| led.as_ref().is_some_and(|led| led.borrow().name == name))
        {
            let endpoint = NrfGpioReceiver::Led(receiver);
            self.nrf_led_routes
                .0
                .borrow_mut()
                .unregister_peripheral(None, Some(&endpoint));
            for connector in self.nrf_gpio_sinks.iter().filter_map(Weak::upgrade) {
                connector.output.borrow_mut().disconnect_receiver(&receiver);
            }
            self.nrf_leds[receiver] = None;
        } else if let Some(source) = self
            .nrf_peripherals
            .iter()
            .position(|device| device.name == name && device.gpio_driven_high().is_some())
        {
            let port = self.nrf_peripherals.remove(source);
            self.nrf_led_routes
                .0
                .borrow_mut()
                .unregister_peripheral(Some(port.base), None);
        } else {
            return Err(format!("unknown nRF GPIO peripheral {name:?}"));
        }
        Ok(())
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
        self.gpio_buttons.push(Some(button));
        self.set_button(self.gpio_buttons.len() - 1, false);
    }

    pub fn report_led_events(&mut self, enabled: bool) {
        self.led_events = enabled;
    }

    pub fn board_button(&self, name: &str) -> Option<usize> {
        self.gpio_buttons
            .iter()
            .position(|button| button.as_ref().is_some_and(|button| button.name == name))
    }

    pub fn set_named_button(&mut self, target: &str, pressed: bool) -> Result<(), String> {
        let target = target.strip_prefix("sysbus.").unwrap_or(target);
        let index = match target.split_once('.') {
            Some((parent, name)) => self.gpio_buttons.iter().position(|button| {
                button.as_ref().is_some_and(|button| {
                    button.parent.as_deref() == Some(parent) && button.name == name
                })
            }),
            None => self.board_button(target),
        }
        .ok_or_else(|| format!("unknown STM32 board button {target:?}"))?;
        self.set_button(index, pressed);
        Ok(())
    }

    pub fn set_button(&mut self, index: usize, pressed: bool) {
        let Some(button) = &self.gpio_buttons[index] else {
            return;
        };
        for &(base, pin) in button.endpoints.as_slice() {
            if let Some(port) = self.stm32_gpios.iter_mut().find(|port| port.base == base) {
                port.set_external(pin, pressed ^ button.invert);
            }
            if let Some(port) = self
                .nrf_peripherals
                .iter_mut()
                .find(|port| port.base == base)
            {
                port.set_gpio_input(pin, pressed ^ button.invert);
            }
        }
        self.refresh_gpio_wiring();
    }

    pub(crate) fn refresh_gpio_wiring(&mut self) {
        if !self.gpio_links.is_empty() {
            for port in &mut self.stm32_gpios {
                port.wired_inputs = 0;
            }
            for &(source, output, target, input) in &self.gpio_links {
                if self
                    .stm32_gpios
                    .iter()
                    .any(|port| port.base == source && port.driven_pins() & (1 << output) != 0)
                    && let Some(port) = self.stm32_gpios.iter_mut().find(|port| port.base == target)
                {
                    port.wired_inputs |= 1 << input;
                }
            }
        }
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
        for port in &self.nrf_peripherals {
            if let Some(high) = port.gpio_driven_high() {
                Self::sample_nrf_led_routes(
                    &self.nrf_led_routes.0.borrow(),
                    &self.nrf_gpio_connectors,
                    &self.nrf_leds,
                    port.base,
                    high,
                    self.led_events,
                );
            }
        }
    }

    pub fn unregister_stm32_gpio(&mut self, target: &str) -> Result<(), String> {
        let name = target.strip_prefix("sysbus.").unwrap_or(target);
        let index = self
            .stm32_gpios
            .iter()
            .position(|port| port.name.as_deref() == Some(name))
            .ok_or_else(|| format!("unknown STM32 GPIO port {target:?}"))?;
        let base = self.stm32_gpios.remove(index).base;
        self.gpio_leds.retain(|(port, _)| *port != base);
        for slot in &mut self.gpio_buttons {
            if let Some(button) = slot {
                if button.parent.as_deref() == Some(name) {
                    // Scheduled button events refer to the original slot number.
                    *slot = None;
                } else {
                    button.endpoints.disconnect_receiver(base);
                }
            }
        }
        for timer in &mut self.stm32_timers {
            timer.gpio_outputs.retain(|(_, pin)| pin.port != base);
        }
        // Sample after removing the port, while old capture routes can still
        // propagate the cleared level to timers.
        self.refresh_gpio_wiring();
        self.gpio_timer_inputs.retain(|(port, ..)| *port != base);
        self.gpio_links
            .retain(|(source, _, target, _)| *source != base && *target != base);
        Ok(())
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

    pub fn map_stm32f4_rcc(&mut self, base: u32) -> Result<(), String> {
        let start = u64::from(base);
        let end = start + u64::from(Stm32F4Rcc::SIZE);
        if self.stm32f4_rcc.is_some() || base & 3 != 0 || end > 1 << 32 || self.overlaps(start, end)
        {
            return Err(format!(
                "invalid or overlapping STM32F4 RCC range at {base:#010x}"
            ));
        }
        self.stm32f4_rcc = Some(Stm32F4Rcc::new(base));
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

    pub fn stm32_usart_mut(&mut self, name: &str) -> Result<&mut Stm32Usart, String> {
        self.stm32_uarts
            .iter_mut()
            .find(|uart| uart.name == name)
            .ok_or_else(|| format!("unknown STM32 UART {name:?}"))
    }

    pub fn write_stm32_uart_line(
        &mut self,
        name: &str,
        text: &str,
        ending: &[u8],
    ) -> Result<(), String> {
        let port = self.tester_port(name)?;
        let bytes = text.bytes().chain(ending.iter().copied());
        if port == TesterPort::LiteX {
            self.litex_uart.as_mut().unwrap().queue_input(bytes.collect());
            return Ok(());
        }
        if let TesterPort::Rv64(_) = port {
            self.rv64_devices.select_uart(name.trim_start_matches("sysbus."))?;
            self.rv64_devices.queue_input(bytes.collect());
            return Ok(());
        }
        if let TesterPort::Nrf(index) = port {
            let mut uart = self.nrf_peripherals.remove(index);
            uart.queue_input(bytes.collect());
            let result = uart.receive(self).map_err(|e| e.to_string());
            self.nrf_peripherals.insert(index, uart);
            return result;
        }
        let name = name.strip_prefix("sysbus.").unwrap_or(name);
        let uart = self
            .stm32_uarts
            .iter_mut()
            .find(|uart| uart.name == name)
            .ok_or_else(|| format!("unknown STM32 UART {name:?}"))?;
        if usart_clocked(self.h7_network_control.as_ref(), uart) {
            uart.queue_input(text.bytes().chain(ending.iter().copied()));
        }
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
            || self
                .semihosting
                .as_ref()
                .is_some_and(|handler| handler.console)
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
            && usart_clocked(self.h7_network_control.as_ref(), uart)
        {
            uart.queue_input(bytes);
        } else if let Some(index) = self.nrf_peripherals.iter().position(NrfPeripheral::is_uart) {
            let mut device = self.nrf_peripherals.remove(index);
            device.queue_input(bytes);
            let result = device.receive(self);
            self.nrf_peripherals.insert(index, device);
            result?;
        } else if let Some(handler) = self.semihosting.as_mut().filter(|handler| handler.console) {
            handler.queue_input(bytes);
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

    pub fn record_stm32_uart(&mut self, target: &str) {
        if let Ok(index) = self.stm32_usart_index(target) {
            self.stm32_uart_history
                .resize_with(self.stm32_uarts.len(), || None);
            self.stm32_uart_history[index].get_or_insert_with(UartHistory::default);
        }
    }

    fn is_trivial_uart(&self, target: &str) -> bool {
        let target = target.strip_prefix("sysbus.").unwrap_or(target);
        self.trivial_uart.as_ref().is_some_and(|(_, name)| name == target)
    }

    fn tester_port(&self, target: &str) -> Result<TesterPort, String> {
        let name = target.strip_prefix("sysbus.").unwrap_or(target);
        if self.is_trivial_uart(target) {
            Ok(TesterPort::Trivial)
        } else if self.litex_uart.is_some() && name == "uart" {
            Ok(TesterPort::LiteX)
        } else if let Some(index) = self.nrf_peripherals.iter()
            .position(|device| device.is_uart() && device.name == name)
        {
            Ok(TesterPort::Nrf(index))
        } else if let Some(index) = self.rv64_devices.uart_index(name) {
            Ok(TesterPort::Rv64(index))
        } else {
            self.stm32_usart_index(target).map(TesterPort::Stm32)
                .map_err(|_| format!("UART peripheral not found or of wrong type: {target:?}"))
        }
    }

    pub(crate) fn log_uart(&mut self, target: &str) -> Result<(), String> {
        let port = self.tester_port(target)?;
        if !self.uart_logs.iter().any(|(existing, _, _)| *existing == port) {
            self.uart_logs.push((port, target.to_owned(), Vec::new()));
        }
        Ok(())
    }

    fn observe_uart(
        testers: &mut [(TesterPort, TesterHistory)],
        logs: &mut [(TesterPort, String, Vec<u8>)],
        port: TesterPort,
        bytes: &[u8],
        console: bool,
    ) {
        for &byte in bytes {
            for (tester, history) in testers.iter_mut() {
                if *tester == port {
                    history.receive(byte);
                }
            }
            for (analyzer, name, line) in logs.iter_mut() {
                if *analyzer != port {
                    continue;
                }
                if byte == b'\n' {
                    let message = format!("{name}: {}", String::from_utf8_lossy(line).trim_end_matches('\r'));
                    crate::monitor_log::record(1, &message);
                    if console { eprintln!("{message}"); }
                    line.clear();
                } else if line.len() < crate::uart::UART_HISTORY_BYTES {
                    line.push(byte);
                }
            }
        }
    }

    pub fn create_uart_tester(&mut self, target: &str, binary: bool) -> Result<usize, String> {
        let port = self.tester_port(target)?;
        if let Some(id) = self
            .uart_testers
            .iter()
            .position(|(index, _)| *index == port)
        {
            return Ok(id);
        }
        let id = self.uart_testers.len();
        let history = if binary {
            TesterHistory::Binary(BinaryUartHistory::default())
        } else {
            TesterHistory::Text(UartHistory::default())
        };
        self.uart_testers.push((port, history));
        Ok(id)
    }

    pub fn set_default_uart_tester(&mut self, id: Option<usize>) -> Result<(), String> {
        if let Some(id) = id
            && id >= self.uart_testers.len()
        {
            return Err(format!(
                "Tester #{id} was not found. Create a tester before setting it as default"
            ));
        }
        self.default_uart_tester = id;
        Ok(())
    }

    fn uart_tester(&mut self, id: Option<usize>) -> Result<(usize, &mut TesterHistory), String> {
        let id = match id.or(self.default_uart_tester) {
            Some(id) => id,
            None if self.uart_testers.len() == 1 => 0,
            None if self.uart_testers.is_empty() => return Err("There are no testers available.".into()),
            None => return Err("There is more than one tester available - please specify ID of the desired tester.".into()),
        };
        let (_, history) = self
            .uart_testers
            .get_mut(id)
            .ok_or_else(|| format!("Tester for given ID={id} was not found."))?;
        Ok((id, history))
    }

    #[cfg(test)]
    pub fn watch_uart_tester(
        &mut self,
        id: Option<usize>,
        pattern: &str,
    ) -> Result<UartWatch, String> {
        self.watch_uart_tester_text(id, pattern, None, UartWaitMode::Lf)
    }

    pub fn watch_uart_tester_text(
        &mut self, id: Option<usize>, pattern: &str, regex: Option<regex::Regex>, mode: UartWaitMode,
    ) -> Result<UartWatch, String> {
        let (id, TesterHistory::Text(history)) = self.uart_tester(id)? else {
            return Err("Attempt to wait for text on a tester configured in binary mode.".into());
        };
        history.begin_text_wait(pattern, regex, mode);
        Ok(UartWatch::Tester(id))
    }

    pub(crate) fn uart_activity(&mut self) -> Result<(usize, u64), String> {
        let (id, TesterHistory::Text(history)) = self.uart_tester(None)? else {
            return Err("unsupported idle check on a binary UART tester".into());
        };
        Ok((id, history.activity))
    }

    pub(crate) fn uart_activity_changed(&self, id: usize, previous: u64) -> bool {
        matches!(&self.uart_testers[id].1, TesterHistory::Text(history) if history.activity != previous)
    }

    pub fn uart_tester_matched_line(&self, watch: UartWatch) -> Option<String> {
        let UartWatch::Tester(id) = watch else { return None };
        match &self.uart_testers.get(id)?.1 {
            TesterHistory::Text(history) => history.matched_line(),
            TesterHistory::Binary(_) => None,
        }
    }

    pub fn watch_uart_bytes(
        &mut self,
        id: Option<usize>,
        pattern: &BinaryUartPattern,
        match_start: bool,
    ) -> Result<UartWatch, String> {
        let (id, TesterHistory::Binary(history)) = self.uart_tester(id)? else {
            return Err("Attempt to wait for bytes on a tester configured in text mode.".into());
        };
        history.begin_pattern(pattern, match_start);
        Ok(UartWatch::Tester(id))
    }

    pub fn set_uart_failure(
        &mut self,
        id: Option<usize>,
        pattern: FailingUartPattern,
        register: bool,
    ) -> Result<(), String> {
        let (_, TesterHistory::Text(history)) = self.uart_tester(id)? else {
            return Err("Failing UART strings require a text-mode tester.".into());
        };
        history.set_failure(pattern, register);
        Ok(())
    }

    pub fn uart_watch_failure(&self, watch: UartWatch) -> Option<&str> {
        if let UartWatch::Tester(id) = watch
            && let TesterHistory::Text(history) = &self.uart_testers[id].1
        {
            return history.failure();
        }
        None
    }

    pub fn take_uart_byte_match(&mut self, watch: UartWatch) -> Option<BinaryUartMatch> {
        if let UartWatch::Tester(id) = watch
            && let TesterHistory::Binary(history) = &mut self.uart_testers[id].1
        {
            return history.take_result();
        }
        None
    }

    pub fn watch_stm32_uart(
        &mut self,
        target: &str,
        pattern: &str,
        end: UartWaitMode,
    ) -> Result<UartWatch, String> {
        let index = self.stm32_usart_index(target)?;
        self.stm32_uart_history[index]
            .as_mut()
            .expect("a script wait enables history before execution")
            .begin_wait(pattern, end);
        Ok(UartWatch::Port(index))
    }

    pub fn stm32_uart_matched(&self, watch: UartWatch) -> bool {
        match watch {
            UartWatch::Port(index) => self.stm32_uart_history[index].as_ref().unwrap().matched(),
            UartWatch::Tester(id) => self.uart_testers[id].1.matched(),
        }
    }

    pub fn finish_stm32_uart_watch(&mut self, watch: UartWatch) {
        match watch {
            UartWatch::Port(index) => self.stm32_uart_history[index]
                .as_mut()
                .unwrap()
                .finish_wait(),
            UartWatch::Tester(id) => self.uart_testers[id].1.finish_wait(),
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

    pub fn permits_unmapped_binary_bytes(&self) -> bool {
        matches!(
            self.unhandled_access,
            Some(UnhandledAccess::Report | UnhandledAccess::DoNotReport)
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

    pub(crate) fn image_ram_spans(
        &self, mut position: u64, end: u64,
    ) -> Result<Vec<std::ops::Range<u64>>, String> {
        let mut spans = Vec::new();
        while position < end {
            let mut low = 1;
            let mut high = (end - position) as usize;
            if self.ram_location(position, low, None).is_none() {
                return Err(format!("unmapped RAM access at {position:#010x}"));
            }
            while low < high {
                let mid = low + (high - low).div_ceil(2);
                if self.ram_location(position, mid, None).is_some() {
                    low = mid;
                } else {
                    high = mid - 1;
                }
            }
            let next = position + low as u64;
            spans.push(position..next);
            position = next;
        }
        Ok(spans)
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
                if hart != Some(slot)
                    && reservation.is_some_and(|(reserved, _)| {
                        let reserved = reserved & !3;
                        address < u64::from(reserved) + 4
                            && u64::from(reserved) < address + width as u64
                    })
                {
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

    fn byte_range(address: u32, count: usize) -> Result<std::ops::Range<u64>, BusError> {
        let start = u64::from(address);
        let end = start.saturating_add(count as u64);
        if end > 1 << 32 {
            return Err(BusError::Unmapped(address, count));
        }
        Ok(start..end)
    }

    // Monitor bulk transfers use zero-fill/read and clipping/write semantics
    // across holes. Typed guest accesses keep their existing bus faults.
    pub fn read_bytes(
        &mut self,
        address: u32,
        count: usize,
    ) -> Result<impl Iterator<Item = Result<u8, BusError>> + '_, BusError> {
        Ok(Self::byte_range(address, count)?.map(move |address| {
            match self.read_mapped::<1>(address as u32) {
                Ok(value) => Ok(value as u8),
                Err(BusError::Unmapped(..)) => Ok(0),
                Err(error) => Err(error),
            }
        }))
    }

    pub fn write_bytes(
        &mut self,
        address: u32,
        bytes: &[u8],
        uart: &mut impl Write,
    ) -> Result<(), BusError> {
        for (address, &byte) in Self::byte_range(address, bytes.len())?.zip(bytes) {
            let address = address as u32;
            if let Ok(memory) = self.memory(address, 1) {
                memory[0] = byte;
            } else {
                match self.write_mapped(address, 1, u32::from(byte), uart, None) {
                    Ok(()) | Err(BusError::Unmapped(..)) => {}
                    Err(error) => return Err(error),
                }
            }
        }
        Ok(())
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

    fn check_thumb_bus_context(&self, address: u32, width: usize) -> Result<(), String> {
        if self.thumb_unprivileged
            && self.regions.iter().any(|region| {
                region.privileged
                    && (0..width).any(|offset| {
                        region
                            .offset(u64::from(address.wrapping_add(offset as u32)), 1)
                            .is_some()
                    })
            })
        {
            return Err(format!(
                "Cortex-M bus registration denied access at {address:#010x}"
            ));
        }
        Ok(())
    }

    pub fn apply_cpu_hooks(&mut self, pc: u32, output: &mut impl Write) -> Result<bool, BusError> {
        let Some(hooks) = self.cpu_hooks.get(&pc).cloned() else {
            return Ok(false);
        };
        for hook in hooks {
            match hook {
                CpuHook::Write32(address, value) => self.write(address, 4, value, output)?,
                CpuHook::Log(message) => {
                    crate::monitor_log::record(1, &message);
                    if self.monitor_log_enabled(1) {
                        writeln!(output, "{message}").map_err(BusError::Output)?;
                    }
                }
                CpuHook::Pause => {
                    writeln!(output, "Machine paused").map_err(BusError::Output)?;
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    pub fn thumb_fetch(&self, address: u32) -> Result<u32, String> {
        self.check_thumb_bus_context(address, 2)?;
        self.check_thumb_access(address, 2, false, true)?;
        self.read_ram::<2>(address)
            .map_err(|error| error.to_string())
    }

    pub fn thumb_read<const N: usize>(&mut self, address: u32) -> Result<u32, String> {
        self.check_thumb_bus_context(address, N)?;
        self.check_thumb_access(address, N, false, false)?;
        if N == 4
            && address == crate::nvic::DHCSR
            && self.nvic.as_ref().is_some_and(Nvic::has_debug_status)
        {
            return Ok(0);
        }
        let value = if address as usize & (N - 1) != 0
            && self.nvic.as_ref().is_some_and(Nvic::allows_unaligned_data)
            && let Ok(slice) = self.memory_slice(address, N)
        {
            let mut value = [0; 4];
            value[..N].copy_from_slice(slice);
            u32::from_le_bytes(value)
        } else {
            self.read::<N>(address)
                .or_else(|error| self.thumb_data_error(error, false))?
        };
        self.log_ram_io_read(address, N, value);
        self.thumb_read_watchpoint_hit |= self.thumb_read_watchpoints.contains(&(address, N));
        Ok(value)
    }

    pub fn thumb_write(
        &mut self,
        address: u32,
        width: usize,
        value: u32,
        uart: &mut impl Write,
    ) -> Result<(), String> {
        self.check_thumb_bus_context(address, width)?;
        self.check_thumb_access(address, width, true, false)?;
        if address as usize & (width - 1) != 0
            && self.nvic.as_ref().is_some_and(Nvic::allows_unaligned_data)
            && let Ok(bytes) = self.memory(address, width)
        {
            bytes.copy_from_slice(&value.to_le_bytes()[..width]);
            return Ok(());
        }
        self.write(address, width, value, uart)
            .or_else(|error| self.thumb_data_error(error, true))
    }

    fn thumb_data_error<T: Default>(&mut self, error: BusError, write: bool) -> Result<T, String> {
        let address = match error {
            BusError::SlaveFault(address) => address,
            BusError::Unmapped(address, width)
                if self.report_unmapped_thumb_data
                    && !self.overlaps(u64::from(address), u64::from(address) + width as u64) =>
            {
                let access = if write { "Write" } else { "Read" };
                let width = match width {
                    1 => "Byte",
                    2 => "Word",
                    4 => "DoubleWord",
                    _ => return Err(error.to_string()),
                };
                let direction = if write { "to" } else { "from" };
                eprintln!(
                    "sysbus: {access}{width} {direction} non existing peripheral at 0x{address:X}"
                );
                return Ok(T::default());
            }
            BusError::Unmapped(address, width)
                if self.fault_on_unmapped_thumb_data
                    && !self.overlaps(u64::from(address), u64::from(address) + width as u64) =>
            {
                address
            }
            _ => return Err(error.to_string()),
        };
        if self.thumb_negative_priority
            && let Some(nvic) = self
                .nvic
                .as_mut()
                .filter(|nvic| nvic.ignores_negative_priority_bus_fault())
        {
            nvic.record_precise_bus_fault(address, false);
            return Ok(T::default());
        }
        self.thumb_data_fault = Some(address);
        Err(error.to_string())
    }

    pub fn take_thumb_data_fault(&mut self) -> Option<u32> {
        self.thumb_data_fault.take()
    }

    fn read_faulting_peripheral<const N: usize>(
        &self,
        address: u32,
    ) -> Option<Result<u32, BusError>> {
        let (device, offset) = self
            .faulting_peripherals
            .iter()
            .find_map(|device| Some((device, device.offset(address)?)))?;
        Some((|| {
            let mut value = [0; 4];
            for (index, byte) in value[..N].iter_mut().enumerate() {
                device.check_byte(offset + index, address, false)?;
                *byte = device.bytes[offset + index];
            }
            Ok(u32::from_le_bytes(value))
        })())
    }

    pub fn read_cortex_m_ram_or_mock_word(&self, address: u32) -> Result<u32, BusError> {
        self.read_faulting_peripheral::<4>(address)
            .unwrap_or_else(|| self.read_ram::<4>(address))
    }

    pub fn read<const N: usize>(&mut self, address: u32) -> Result<u32, BusError> {
        self.read_scalar::<N>(address, false)
    }

    pub fn read_scalar<const N: usize>(
        &mut self,
        address: u32,
        monitor: bool,
    ) -> Result<u32, BusError> {
        match self.read_mapped::<N>(address) {
            Ok(value) => {
                self.log_memory_access(false, address, N, value);
                Ok(value)
            }
            Err(BusError::Unmapped(..)) => self.unhandled_value(address, N, monitor),
            result => result,
        }
    }

    fn read_mapped<const N: usize>(&mut self, address: u32) -> Result<u32, BusError> {
        if N == 4
            && let Some(error) = self
                .command_error_peripherals
                .iter()
                .find_map(|device| device.read_doubleword(address))
        {
            return Err(error);
        }
        #[cfg(test)]
        if N == 4
            && let Some((region, error)) = &self.command_fault_device
            && region.contains(&address)
        {
            return Err(BusError::CommandError(Arc::clone(error)));
        }
        #[cfg(test)]
        if self
            .faulting_device
            .as_ref()
            .is_some_and(|range| range.contains(&address))
        {
            return Err(BusError::AddressError(address));
        }
        #[cfg(test)]
        if N == 1
            && let Some(&value) = self.byte_peripheral.get(&address)
        {
            return Ok(u32::from(value));
        }
        // Mapped devices cannot overlap RAM. Try the common RAM access first.
        if let Some(device) = self
            .pydevs
            .iter_mut()
            .find(|device| device.contains(address.into(), N as u32))
        {
            return device
                .access(address.into(), N as u32, None)
                .map(|value| value as u32)
                .map_err(BusError::Python);
        }
        if N == 4 && self.trivial_peripheral_at(address) {
            return Ok(0);
        }
        if let Some(result) = self.read_faulting_peripheral::<N>(address) {
            return result;
        }
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
            let value = device.read(address);
            let signals: Vec<_> = device.sensor_irqs().collect();
            for (gpio, pin, high) in signals {
                self.apply_nrf_input(gpio, pin, high);
            }
            return value.ok_or(BusError::Unmapped(address, N));
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
        if matches!(N, 1 | 4)
            && let Some(clic) = self.clic.as_ref()
            && let Some(offset) = clic.offset(address, N)
        {
            return Ok(clic.read(offset, N));
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
            && let Some(value) = self.dwt.as_ref().and_then(|dwt| dwt.read(address))
        {
            return Ok(value);
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
            return Ok(if usart_clocked(self.h7_network_control.as_ref(), uart) {
                uart.read(address, N)
            } else {
                0
            });
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
            && let Some(rcc) = self.stm32f4_rcc.as_ref()
            && rcc.contains(address)
        {
            return Ok(rcc.read(address));
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
        if let Some(value) = self.constant_mmio.iter().find_map(|peripheral| {
            peripheral
                .offset(address, 1)
                .map(|offset| peripheral.read(offset, N))
        }) {
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
            && let Some(crc) = self.stm32_crc.as_mut()
            && crc.contains(address)
        {
            return crc.read(address, N).map_err(BusError::command_error);
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
            && let Some(crc) = self.h7_crc.iter_mut().find(|crc| crc.contains(address))
        {
            return if crc_clocked(self.h7_network_control.as_ref(), crc) {
                crc.read(address, N).map_err(BusError::command_error)
            } else {
                Ok(0)
            };
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
        #[cfg(test)]
        if self
            .faulting_accesses
            .as_ref()
            .is_some_and(|range| range.contains(&address))
        {
            return Err(BusError::Unmapped(address, N));
        }
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

    fn write_faulting_peripheral(
        &mut self,
        address: u32,
        width: usize,
        value: u32,
    ) -> Option<Result<(), BusError>> {
        let device = self
            .faulting_peripherals
            .iter_mut()
            .find(|device| device.offset(address).is_some())?;
        let offset = device.offset(address).unwrap();
        Some((|| {
            for (index, byte) in value.to_le_bytes()[..width].iter().enumerate() {
                device.check_byte(offset + index, address, true)?;
                device.bytes[offset + index] = *byte;
            }
            Ok(())
        })())
    }

    pub fn write_cortex_m_exception_stack(
        &mut self,
        stack: u32,
        frame: &[u32],
    ) -> Result<(), BusError> {
        if let Ok(memory) = self.memory(stack, frame.len() * 4) {
            for (word, value) in memory.as_chunks_mut::<4>().0.iter_mut().zip(frame) {
                *word = value.to_le_bytes();
            }
            return Ok(());
        }
        for (index, &value) in frame.iter().enumerate() {
            let address = stack.wrapping_add(index as u32 * 4);
            self.write_faulting_peripheral(address, 4, value)
                .ok_or(BusError::Unmapped(address, 4))??;
        }
        Ok(())
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

    pub fn write_scalar(
        &mut self,
        address: u32,
        width: usize,
        value: u32,
        uart: &mut impl Write,
        monitor: bool,
    ) -> Result<(), BusError> {
        match self.write_mapped(address, width, value, uart, None) {
            Ok(()) => {
                self.log_memory_access(true, address, width, value);
                Ok(())
            }
            Err(BusError::Unmapped(..)) => {
                self.unhandled_value(address, width, monitor).map(|_| ())
            }
            result => result,
        }
    }

    pub fn write_with_origin(
        &mut self,
        address: u32,
        width: usize,
        value: u32,
        uart: &mut impl Write,
        writer: Option<usize>,
    ) -> Result<(), BusError> {
        match self.write_mapped(address, width, value, uart, writer) {
            Ok(()) => {
                self.log_memory_access(true, address, width, value);
                Ok(())
            }
            Err(BusError::Unmapped(..)) => self.unhandled_value(address, width, false).map(|_| ()),
            result => result,
        }
    }

    fn write_mapped(
        &mut self,
        address: u32,
        width: usize,
        value: u32,
        uart: &mut impl Write,
        writer: Option<usize>,
    ) -> Result<(), BusError> {
        if let Some(device) = self
            .pydevs
            .iter_mut()
            .find(|device| device.contains(address.into(), width as u32))
        {
            return device
                .access(address.into(), width as u32, Some(value.into()))
                .map(|_| ())
                .map_err(BusError::Python);
        }
        if width == 4 && self.trivial_peripheral_at(address) {
            return Ok(());
        }
        if let Some(result) = self.write_faulting_peripheral(address, width, value) {
            return result;
        }
        if self.trivial_uart.as_ref().is_some_and(|(base, _)| *base == address)
            && matches!(width, 1 | 2 | 4)
        {
            let byte = value as u8;
            uart.write_all(&[byte]).map_err(BusError::Output)?;
            let console = self.monitor_log_enabled(1);
            Self::observe_uart(&mut self.uart_testers, &mut self.uart_logs, TesterPort::Trivial, &[byte], console);
            return Ok(());
        }
        #[cfg(test)]
        if self
            .faulting_device
            .as_ref()
            .is_some_and(|range| range.contains(&address))
        {
            return Err(BusError::AddressError(address));
        }
        #[cfg(test)]
        if width == 1
            && let Some(byte) = self.byte_peripheral.get_mut(&address)
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
            let mut bytes = Vec::new();
            let is_uart = device.is_uart();
            let result = if is_uart {
                device.write(address, value, self, &mut bytes)
            } else {
                device.write(address, value, self, uart)
            };
            for (gpio, pin, high) in device.sensor_irqs() {
                self.apply_nrf_input(gpio, pin, high);
            }
            self.nrf_peripherals.insert(index, device);
            if is_uart {
                uart.write_all(&bytes).map_err(BusError::Output)?;
                let console = self.monitor_log_enabled(1);
                Self::observe_uart(&mut self.uart_testers, &mut self.uart_logs, TesterPort::Nrf(index), &bytes, console);
            }
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
        if matches!(width, 1 | 4)
            && let Some(clic) = self.clic.as_mut()
            && let Some(offset) = clic.offset(address, width)
        {
            clic.write(offset, width, value);
            return Ok(());
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
            let mut bytes = Vec::new();
            device.write(offset, value, &mut bytes).map_err(BusError::Output)?;
            uart.write_all(&bytes).map_err(BusError::Output)?;
            let console = self.monitor_log_enabled(1);
            Self::observe_uart(&mut self.uart_testers, &mut self.uart_logs, TesterPort::LiteX, &bytes, console);
            return Ok(());
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
        if width == 4
            && let Some(timer) = self.systick.as_mut()
        {
            let zero_reload = timer.enabling_with_zero_reload(address, value);
            if timer.write(address, value) {
                if zero_reload {
                    writeln!(uart, "Systick_NS enabled but it won't be started as long as the reload value is zero")
                        .map_err(BusError::Output)?;
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
            if !usart_clocked(self.h7_network_control.as_ref(), device) {
                return Ok(());
            }
            let console = self.monitor_log_level.unwrap_or(1) <= 1;
            return device
                .write(address, width, value, |byte| {
                    uart.write_all(&[byte])?;
                    if let Some(Some(history)) = self.stm32_uart_history.get_mut(index) {
                        history.receive(byte);
                    }
                    Self::observe_uart(&mut self.uart_testers, &mut self.uart_logs,
                        TesterPort::Stm32(index), &[byte], console);
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
            && let Some(rcc) = self.stm32f4_rcc.as_mut()
            && rcc.contains(address)
        {
            rcc.write(address, value);
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
            return crc
                .write(address, width, value)
                .map_err(BusError::command_error);
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
            for uart in &mut self.stm32_uarts {
                if uart
                    .clock
                    .is_some_and(|clock| control.usart_in_reset(clock))
                {
                    uart.reset();
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
            let offset = address.wrapping_sub(control.rcc_base);
            let clock_changed = matches!(offset, 0 | 0x10 | 0x18 | 0x1c | 0x28 | 0x30);
            for timer in &mut self.h7_timers {
                if let Some(clock) = timer.clock {
                    if clock_changed && let Some(frequency) = control.timer_frequency(clock) {
                        timer.frequency = frequency;
                    }
                    let reset_register = match clock {
                        crate::stm32h7_timer::TimerClock::Apb1Low(_) => 0x90,
                        crate::stm32h7_timer::TimerClock::Apb2(_) => 0x98,
                    };
                    if offset == reset_register && control.timer_in_reset(clock) {
                        if self.trace_rcc {
                            let message = format!("Resetting {}", clock.alias().unwrap_or(&timer.name));
                            crate::monitor_log::record(0, &message);
                            eprintln!("{message}");
                        }
                        timer.reset();
                    }
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
                crc.write(address, width, value)
                    .map_err(BusError::command_error)?;
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
    fn renode_systembus_propagates_peripheral_command_error_identity() {
        let cause: Arc<dyn std::error::Error + Send + Sync> =
            Arc::new(io::Error::other("device rejected command"));
        let mut bus = Bus {
            command_fault_device: Some((0x1000..0x1100, Arc::clone(&cause))),
            unhandled_access: Some(UnhandledAccess::ThrowException),
            ..Bus::default()
        };
        let error = bus.read_scalar::<4>(0x1000, true).unwrap_err();
        let BusError::CommandError(actual) = &error else {
            panic!("{error}");
        };
        assert!(Arc::ptr_eq(&cause, actual));
        assert_eq!(
            error.to_string(),
            "Bus access failed with error: CommandError: device rejected command"
        );
        assert!(matches!(
            bus.read_scalar::<4>(0x1100, true),
            Err(BusError::AddressError(0x1100))
        ));
    }

    #[test]
    fn renode_gpio_unregister_receiver_detaches_only_its_endpoints() {
        use crate::stm32_gpio_board::GpioEndpoints;

        let mut bus = Bus::default();
        let receiver = 0x4800_0000;
        let other = 0x4800_0400;
        for (address, name) in [(receiver, "receiver"), (other, "other")] {
            let mut port = Stm32Gpio::new(address, 0x400, [0; 3], 8);
            port.name = Some(name.into());
            bus.map_stm32_gpio(port).unwrap();
        }
        for (name, pins) in [
            ("single", &[(receiver, 1)][..]),
            ("shared", &[(receiver, 1), (other, 2), (other, 2)][..]),
            ("untouched", &[(other, 3)][..]),
        ] {
            let mut endpoints = GpioEndpoints::default();
            for &(port, pin) in pins {
                endpoints.connect(port, name, pin, 16).unwrap();
            }
            bus.add_gpio_button(GpioButton::new(name.into(), None, endpoints, false));
        }
        let mut child = GpioEndpoints::default();
        child.connect(other, "other", 4, 16).unwrap();
        bus.add_gpio_button(GpioButton::new(
            "child".into(),
            Some("receiver".into()),
            child,
            false,
        ));
        let endpoints = |bus: &Bus, index: usize| {
            bus.gpio_buttons[index]
                .as_ref()
                .unwrap()
                .endpoints
                .as_slice()
                .to_vec()
        };
        assert_eq!(endpoints(&bus, 0), [(receiver, 1)]);
        assert_eq!(endpoints(&bus, 1), [(receiver, 1), (other, 2)]);
        bus.unregister_stm32_gpio("sysbus.receiver").unwrap();
        assert!(endpoints(&bus, 0).is_empty());
        assert_eq!(endpoints(&bus, 1), [(other, 2)]);
        assert_eq!(endpoints(&bus, 2), [(other, 3)]);
        assert!(bus.board_button("child").is_none());
        bus.set_button(3, true); // a previously scheduled event for the removed child
        assert!(matches!(
            bus.read::<4>(receiver + 0x10),
            Err(BusError::Unmapped(..))
        ));
        bus.set_named_button("shared", true).unwrap();
        bus.set_named_button("untouched", true).unwrap();
        assert_eq!(bus.read::<4>(other + 0x10).unwrap(), (1 << 2) | (1 << 3));
    }

    #[test]
    fn renode_systembus_unhandled_access_policies() {
        let mut bus = Bus::default();
        let missing = 0xabcd_1234;
        let mut output = io::sink();
        assert_eq!(bus.read_scalar::<1>(missing, true).unwrap(), 0);
        assert!(matches!(
            bus.read::<1>(missing),
            Err(BusError::Unmapped(..))
        ));
        for policy in [
            UnhandledAccess::ThrowException,
            UnhandledAccess::ThrowExceptionIfNotTagged,
        ] {
            bus.unhandled_access = Some(policy);
            assert!(
                matches!(bus.read_scalar::<1>(missing, true), Err(BusError::AddressError(addr)) if addr == missing)
            );
            assert!(
                matches!(bus.write_scalar(missing, 1, 0, &mut output, true), Err(BusError::AddressError(addr)) if addr == missing)
            );
        }

        bus.tag(0xabcd_1200, 0x100, 0x42, false).unwrap();
        bus.write_scalar(missing, 1, 0, &mut output, true).unwrap();
        assert_eq!(bus.read_scalar::<1>(missing, true).unwrap(), 0x42);
        assert_eq!(
            bus.read_bytes(missing, 1).unwrap().next().unwrap().unwrap(),
            0
        );
        bus.unhandled_access = Some(UnhandledAccess::ThrowException);
        assert!(matches!(
            bus.read_scalar::<1>(missing, true),
            Err(BusError::AddressError(_))
        ));
    }

    #[test]
    fn renode_systembus_tag_overrides_unhandled_policy_and_splits() {
        let mut bus = Bus::default();
        let mut output = io::sink();
        bus.tag(0x1000, 0x100, 0, true).unwrap();
        for policy in [None, Some(UnhandledAccess::DoNotReport)] {
            bus.unhandled_access = policy;
            assert!(matches!(
                bus.read_scalar::<1>(0x1000, true),
                Err(BusError::AddressError(0x1000))
            ));
            assert!(matches!(
                bus.write_scalar(0x1000, 1, 0, &mut output, true),
                Err(BusError::AddressError(0x1000))
            ));
        }
        bus.tag(0x1010, 0x10, 0, false).unwrap();
        assert!(matches!(
            bus.read_scalar::<1>(0x1000, true),
            Err(BusError::AddressError(0x1000))
        ));
        assert_eq!(bus.read_scalar::<1>(0x1010, true).unwrap(), 0);
        assert!(matches!(
            bus.read_scalar::<1>(0x10f0, true),
            Err(BusError::AddressError(0x10f0))
        ));
        assert!(bus.tag(0x100f, 2, 0, false).is_err());

        let mut bus = Bus::default();
        bus.tag(0x1000, 0x100, 0x42, false).unwrap();
        bus.write_scalar(0x1000, 1, 0, &mut output, true).unwrap();
        assert_eq!(bus.read_scalar::<1>(0x1000, true).unwrap(), 0x42);
        bus.map_ram(0x1000, 0x100).unwrap();
        bus.write_scalar(0x1000, 1, 0x77, &mut output, true)
            .unwrap();
        assert_eq!(bus.read_scalar::<1>(0x1000, true).unwrap(), 0x77);
    }

    #[test]
    fn renode_systembus_bulk_memory_clips_writes_and_zero_fills_gaps() {
        let high = 0xc000_0000;
        let bytes: Vec<u8> = (0..8).collect();
        let large: Vec<u8> = (0..32).collect();
        let mut start_overlap = bytes[..6].to_vec();
        start_overlap.extend([0; 2]);
        let mut end_overlap = vec![0; 4];
        end_overlap.extend(&bytes[4..]);
        let mut containing_overlap = vec![0; 4];
        containing_overlap.extend(&large[4..20]);
        containing_overlap.extend([0; 12]);
        for (addr, input, expected) in [
            (4, &bytes, bytes.clone()),
            (10, &bytes, start_overlap),
            (high - 4, &bytes, end_overlap),
            (100, &bytes, vec![0; 8]),
            (high - 4, &large, containing_overlap),
        ] {
            let mut bus = Bus::default();
            bus.map_ram(0, 16).unwrap();
            bus.map_ram(high, 16).unwrap();
            bus.write_bytes(addr, input, &mut io::sink()).unwrap();
            let read = bus
                .read_bytes(addr, input.len())
                .unwrap()
                .collect::<Result<Vec<_>, _>>();
            assert_eq!(read.unwrap(), expected, "range starting at {addr:#x}");
            assert!(matches!(
                bus.read::<1>(100),
                Err(BusError::Unmapped(100, 1))
            ));
        }
    }

    #[test]
    fn renode_systembus_bulk_transfers_between_memory_and_byte_peripherals() {
        let bytes: Vec<_> = (0..350).map(|index| (index % 255) as u8).collect();
        let mut expected = vec![0; 350];
        expected[50..150].copy_from_slice(&bytes[50..150]);
        expected[200..300].copy_from_slice(&bytes[200..300]);
        for first_memory in [true, false] {
            for second_memory in [true, false] {
                let mut bus = Bus::default();
                for (base, is_memory) in [(50, first_memory), (200, second_memory)] {
                    if is_memory {
                        bus.map_ram(base, 100).unwrap();
                    } else {
                        bus.byte_peripheral
                            .extend((base..base + 100).map(|addr| (addr, 0)));
                    }
                }
                bus.write_bytes(0, &bytes, &mut io::sink()).unwrap();
                let read = bus
                    .read_bytes(0, 350)
                    .unwrap()
                    .collect::<Result<Vec<_>, _>>();
                assert_eq!(read.unwrap(), expected, "{first_memory}/{second_memory}");
                for (&address, &value) in &bus.byte_peripheral {
                    assert_eq!(value, bytes[address as usize]);
                }
            }
        }
    }

    #[test]
    fn renode_systembus_finds_bytes_across_mappings_and_mebibyte_boundary() {
        const BLOCK: u32 = 1 << 20;
        let pattern: Vec<u8> = (0..8).collect();
        let mut bus = Bus::default();
        // Register second memory first to also check address-ordered results.
        bus.map_ram(BLOCK, BLOCK as usize).unwrap();
        bus.map_ram(0, BLOCK as usize).unwrap();
        bus.write_bytes(0, &pattern, &mut io::sink()).unwrap();
        bus.write_bytes(BLOCK - 4, &pattern, &mut io::sink())
            .unwrap();
        assert_eq!(
            bus.find_bytes(&pattern, 0, u64::from(u32::MAX)).unwrap(),
            [0, u64::from(BLOCK - 4)]
        );
        assert_eq!(
            bus.find_bytes(&pattern, 1, u64::from(BLOCK + 3)).unwrap(),
            [u64::from(BLOCK - 4)]
        );
        assert!(
            bus.find_bytes(&pattern, 1, u64::from(BLOCK + 2))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn systembus_find_bytes_skips_gaps_and_peripherals_and_keeps_overlapping_matches() {
        let mut bus = Bus::default();
        bus.map_ram(0, 3).unwrap();
        bus.map_ram(4, 2).unwrap();
        bus.map_ram(u32::MAX, 1).unwrap();
        bus.byte_peripheral.insert(3, b'a');
        bus.write_bytes(0, b"aaaaaa", &mut io::sink()).unwrap();
        bus.write_bytes(u32::MAX, b"a", &mut io::sink()).unwrap();
        assert_eq!(bus.find_bytes(b"aa", 0, 5).unwrap(), [0, 1, 4]);
        assert_eq!(
            bus.find_bytes(b"a", u64::from(u32::MAX), u64::from(u32::MAX))
                .unwrap(),
            [u64::from(u32::MAX)]
        );
        assert!(bus.find_bytes(b"", 0, 5).is_err());
        assert!(bus.find_bytes(b"a", 5, 0).unwrap().is_empty());
    }

    #[test]
    fn monitor_bulk_ranges_reject_address_wrap_before_writing() {
        let mut bus = Bus::default();
        bus.map_ram(0, 16).unwrap();
        bus.map_ram(u32::MAX - 15, 16).unwrap();
        assert!(bus.write_bytes(u32::MAX, &[1, 2], &mut io::sink()).is_err());
        assert!(bus.read_bytes(u32::MAX, 2).is_err());
        assert_eq!(bus.read::<1>(u32::MAX).unwrap(), 0);
        assert_eq!(bus.read::<1>(0).unwrap(), 0);
        bus.write_bytes(u32::MAX, &[3], &mut io::sink()).unwrap();
        assert_eq!(
            bus.read_bytes(u32::MAX, 1)
                .unwrap()
                .next()
                .unwrap()
                .unwrap(),
            3
        );
    }

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
    fn uart_tester_starts_at_attachment_and_has_an_independent_cursor() {
        let mut bus = Bus::default();
        let mut output = io::sink();
        let base = 0x4001_3800;
        bus.map_stm32_usart(Stm32Usart::new("usart1".into(), base, None, false))
            .unwrap();
        bus.record_stm32_uart("usart1");
        bus.write(base, 4, 9, &mut output).unwrap();
        for byte in b"before\n" {
            bus.write(base + 0x28, 1, u32::from(*byte), &mut output)
                .unwrap();
        }
        assert_eq!(bus.create_uart_tester("usart1", false).unwrap(), 0);
        assert_eq!(bus.create_uart_tester("sysbus.usart1", false).unwrap(), 0);
        let watch = bus.watch_uart_tester(None, "before").unwrap();
        assert!(!bus.stm32_uart_matched(watch));
        bus.finish_stm32_uart_watch(watch);
        for byte in b"after\n" {
            bus.write(base + 0x28, 1, u32::from(*byte), &mut output)
                .unwrap();
        }
        let watch = bus.watch_uart_tester(None, "after").unwrap();
        assert!(bus.stm32_uart_matched(watch));
        bus.finish_stm32_uart_watch(watch);
        let direct = bus
            .watch_stm32_uart("usart1", "after", UartWaitMode::Lf)
            .unwrap();
        assert!(bus.stm32_uart_matched(direct));
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
        const RECEIVER: u32 = 0x4800_0c00;
        const SOURCE: u32 = 0x4000_0000;
        const CAPTURE: u32 = 0x4000_0400;
        for pairs in [&[0][..], &[0, 2][..]] {
            let mut bus = Bus::default();
            bus.map_nvic(0xc0, None).unwrap();
            bus.map_stm32_gpio(Stm32Gpio::new(GPIO, 0x400, [2 << 12, 0, 0], 8))
                .unwrap();
            bus.map_stm32_gpio(Stm32Gpio::new(RECEIVER, 0x400, [0; 3], 8))
                .unwrap();
            bus.route_gpio_pin(GPIO, 6, RECEIVER, 12);
            let mut source = Stm32Timer::new(SOURCE, 1_000_000, u32::MAX, [0; 8]);
            source.gpio_outputs.push((
                0,
                TimerPin {
                    port: GPIO,
                    pin: 6,
                    function: 0,
                },
            ));
            bus.map_stm32_timer(source).unwrap();
            bus.map_stm32_timer(Stm32Timer::new(CAPTURE, 1_000_000, 0xffff, [0; 8]))
                .unwrap();
            let mut enable = 0;
            let mut output = io::sink();
            for &pair in pairs {
                bus.route_gpio_timer_input(RECEIVER, 12, CAPTURE, pair);
                bus.write(CAPTURE + 0x18 + 2 * u32::from(pair), 4, 0x0201, &mut output)
                    .unwrap();
                enable |= 0x31 << (pair * 4);
            }
            for (base, offset, value) in [
                (CAPTURE, 0x20, enable),
                (CAPTURE, 0, 1),
                (SOURCE, 0x2c, 999),
                (SOURCE, 0x34, 330),
                (SOURCE, 0x18, 0x60),
                (SOURCE, 0x20, 1),
                (SOURCE, 0, 1),
            ] {
                bus.write(base + offset, 4, value, &mut output).unwrap();
            }
            let mut captures = [const { Vec::new() }; 4];
            for _ in 0..135_000 {
                bus.sample_nvic(&ActiveExceptions::default(), false);
                let flags = bus.read::<4>(CAPTURE + 0x10).unwrap();
                for (channel, samples) in captures.iter_mut().enumerate() {
                    if flags & (2 << channel) != 0 {
                        samples.push(bus.read::<4>(CAPTURE + 0x34 + 4 * channel as u32).unwrap());
                    }
                }
                if flags != 0 {
                    bus.write(CAPTURE + 0x10, 4, !flags, &mut output).unwrap();
                }
            }
            for &pair in pairs {
                let rising = &captures[usize::from(pair)];
                let falling = &captures[usize::from(pair) + 1];
                assert_eq!(rising.len(), 2);
                assert_eq!(falling.len(), 2);
                let period = rising[1] - rising[0];
                let pulse = falling[1] - rising[1];
                assert_eq!((period, pulse), (1000, 330));
                assert!((1_000_000u32 / period).abs_diff(1000) <= 25);
                assert!((pulse * 100 / period).abs_diff(33) <= 1);
            }
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

fn usart_clocked(control: Option<&NetworkControl>, uart: &Stm32Usart) -> bool {
    control
        .zip(uart.clock)
        .is_none_or(|(control, clock)| control.usart_enabled(clock))
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

#[cfg(test)]
mod unhandled_access_cases {
    use super::*;

    fn address_error(bus: &mut Bus, address: u32) {
        assert!(matches!(bus.read::<1>(address), Err(BusError::Unmapped(a, 1)) if a == address));
        assert!(
            matches!(bus.write(address, 1, 0, &mut std::io::sink()), Err(BusError::Unmapped(a, 1)) if a == address)
        );
    }

    #[test]
    fn upstream_preserve_exception_when_splitting_tag() {
        let mut bus = Bus {
            unhandled_access: Some(UnhandledAccess::Report),
            ..Bus::default()
        };
        bus.tag(0x1000, 0x10ff - 0x1000 + 1, 0, true).unwrap();
        bus.tag(0x1010, 0x101f - 0x1010 + 1, 0, false).unwrap();
        assert!(matches!(
            bus.read::<1>(0x1000),
            Err(BusError::Unmapped(0x1000, 1))
        ));
        assert_eq!(bus.read::<1>(0x1010).unwrap(), 0);
        assert!(matches!(
            bus.read::<1>(0x10f0),
            Err(BusError::Unmapped(0x10f0, 1))
        ));
    }

    #[test]
    fn reject_tag_spanning_multiple_ranges_without_changing_them() {
        let mut bus = Bus::default();
        bus.tag(0x1000, 0x10ff - 0x1000 + 1, 0x42, false).unwrap();
        bus.tag(0x1010, 0x101f - 0x1010 + 1, 0x12, false).unwrap();
        assert!(bus.tag(0x1008, 0x1018 - 0x1008 + 1, 0, false).is_err());
        assert_eq!(bus.read::<1>(0x1008).unwrap(), 0x42);
        assert_eq!(bus.read::<1>(0x1018).unwrap(), 0x12);
        assert_eq!(bus.read::<1>(0x1020).unwrap(), 0x42);
    }

    #[test]
    fn upstream_default_unmapped_read_returns_zero() {
        assert_eq!(Bus::default().read::<1>(0xabcd1234).unwrap(), 0);
    }

    #[test]
    fn raw_bus_retains_unmapped_faults() {
        address_error(&mut Bus::new(4096).unwrap(), 0xabcd1234);
    }

    #[test]
    fn upstream_unmapped_throw_exception() {
        let mut bus = Bus {
            unhandled_access: Some(UnhandledAccess::ThrowException),
            ..Bus::default()
        };
        address_error(&mut bus, 0xabcd1234);
    }

    #[test]
    fn upstream_unmapped_throw_if_not_tagged() {
        let mut bus = Bus {
            unhandled_access: Some(UnhandledAccess::ThrowExceptionIfNotTagged),
            ..Bus::default()
        };
        address_error(&mut bus, 0xabcd1234);
    }

    #[test]
    fn upstream_tagged_throw_if_not_tagged() {
        let mut bus = Bus {
            unhandled_access: Some(UnhandledAccess::ThrowExceptionIfNotTagged),
            ..Bus::default()
        };
        bus.tag(0xabcd1200, 0xabcd12ff - 0xabcd1200 + 1, 0x42, false)
            .unwrap();
        bus.write(0xabcd1234, 1, 0, &mut std::io::sink()).unwrap();
        assert_eq!(bus.read::<1>(0xabcd1234).unwrap(), 0x42);
    }

    #[test]
    fn upstream_tag_forces_exception() {
        let mut bus = Bus {
            unhandled_access: Some(UnhandledAccess::Report),
            ..Bus::default()
        };
        bus.tag(0x1000, 0x10ff - 0x1000 + 1, 0, true).unwrap();
        address_error(&mut bus, 0x1000);
    }

    #[test]
    fn upstream_tag_without_exception() {
        let mut bus = Bus {
            unhandled_access: Some(UnhandledAccess::Report),
            ..Bus::default()
        };
        bus.tag(0x1000, 0x10ff - 0x1000 + 1, 0x42, false).unwrap();
        bus.write(0x1000, 1, 0, &mut std::io::sink()).unwrap();
        assert_eq!(bus.read::<1>(0x1000).unwrap(), 0x42);
    }

    #[test]
    fn upstream_tag_exception_reporting_disabled() {
        let mut bus = Bus {
            unhandled_access: Some(UnhandledAccess::DoNotReport),
            ..Bus::default()
        };
        bus.tag(0x1000, 0x10ff - 0x1000 + 1, 0, true).unwrap();
        address_error(&mut bus, 0x1000);
    }
}

#[cfg(test)]
mod memory_segment_cases {
    use super::*;

    const BYTES: [u8; 8] = [0, 1, 2, 3, 4, 5, 6, 7];

    fn exercise(address: u32, bytes: &[u8], expected: &[u8]) {
        let mut bus = Bus::default();
        bus.map_ram(0, 16).unwrap();
        bus.map_ram(0xc000_0000, 16).unwrap();
        bus.write_memory_bytes(address, bytes).unwrap();
        assert_eq!(
            bus.read_memory_bytes(address, bytes.len()).unwrap(),
            expected
        );
    }

    fn unregister_fixture(size: u32) {
        let mut bus = Bus::default();
        bus.map_ram(0, size as usize).unwrap();
        bus.map_ram_alias(0, size, size as usize).unwrap();
        bus.map_ram_alias(0, 2 * size, size as usize).unwrap();
        bus.write(0, 4, 0x12345678, &mut std::io::sink()).unwrap();
        bus.unregister_ram(size).unwrap();
        assert_eq!(bus.read::<4>(0).unwrap(), 0x12345678);
        assert_eq!(bus.read::<4>(2 * size).unwrap(), 0x12345678);
        assert_eq!(bus.read::<4>(size).unwrap(), 0);
    }

    #[test]
    fn upstream_unregister_only_requested_range_0x800() {
        unregister_fixture(0x800);
    }

    #[test]
    fn upstream_unregister_only_requested_range_0x4000() {
        unregister_fixture(0x4000);
    }

    #[test]
    fn upstream_find_byte_pattern() {
        const BUFFER: u32 = 1 << 20;
        let mut bus = Bus::default();
        bus.map_ram(0, BUFFER as usize).unwrap();
        bus.map_ram(BUFFER, BUFFER as usize).unwrap();
        bus.write_memory_bytes(0, &BYTES).unwrap();
        bus.write_memory_bytes(BUFFER - 4, &BYTES).unwrap();
        assert_eq!(
            bus.find_memory_bytes(&BYTES, 0, u32::MAX).unwrap(),
            [0, BUFFER - 4]
        );
    }

    #[test]
    fn byte_search_skips_holes_and_respects_inclusive_end() {
        let mut bus = Bus::default();
        bus.map_ram(10, 2).unwrap();
        bus.map_ram(13, 2).unwrap();
        bus.write_memory_bytes(10, &[1, 2, 0, 3, 4]).unwrap();
        assert!(
            bus.find_memory_bytes(&[2, 0, 3], 10, 14)
                .unwrap()
                .is_empty()
        );
        assert_eq!(bus.find_memory_bytes(&[3, 4], 13, 14).unwrap(), [13]);
        assert!(bus.find_memory_bytes(&[3, 4], 13, 13).unwrap().is_empty());
    }

    #[test]
    fn bulk_write_invalidates_only_overlapping_reservations() {
        let mut bus = Bus::default();
        bus.map_ram(0, 16).unwrap();
        bus.reservations[0] = Some((8, 4));
        bus.reservations[1] = Some((4, 4));
        bus.write_memory_bytes(10, &BYTES).unwrap();
        assert_eq!(bus.reservations[..2], [None, Some((4, 4))]);
    }

    #[test]
    fn upstream_write_to_memory_segment() {
        exercise(4, &BYTES, &BYTES);
    }

    #[test]
    fn upstream_partial_write_at_beginning() {
        exercise(10, &BYTES, &[0, 1, 2, 3, 4, 5, 0, 0]);
    }

    #[test]
    fn upstream_partial_write_at_end() {
        exercise(0xc000_0000 - 4, &BYTES, &[0, 0, 0, 0, 4, 5, 6, 7]);
    }

    #[test]
    fn upstream_write_to_hole() {
        exercise(100, &BYTES, &[0; 8]);
    }

    #[test]
    fn upstream_overlapping_write() {
        let bytes: Vec<_> = (0..32).collect();
        let mut expected = [0; 32];
        expected[4..20].copy_from_slice(&bytes[4..20]);
        exercise(0xc000_0000 - 4, &bytes, &expected);
    }

    #[test]
    fn upstream_read_bytes_both_memory() {
        read_bytes_fixture(true, true);
    }

    #[test]
    fn upstream_read_bytes_memory_peripheral() {
        read_bytes_fixture(true, false);
    }

    #[test]
    fn upstream_read_bytes_peripheral_memory() {
        read_bytes_fixture(false, true);
    }

    #[test]
    fn upstream_read_bytes_both_peripherals() {
        read_bytes_fixture(false, false);
    }

    fn read_bytes_fixture(first_memory: bool, second_memory: bool) {
        let mut bus = Bus::default();
        for (base, memory) in [(50, first_memory), (200, second_memory)] {
            if memory {
                bus.map_ram(base, 100).unwrap();
            } else {
                bus.mock_byte_peripherals
                    .push((u64::from(base)..u64::from(base) + 100, Default::default()));
            }
        }
        let bytes: Vec<_> = (0..350).map(|value| (value % 255) as u8).collect();
        bus.write_bus_bytes(0, &bytes, false, &mut std::io::sink())
            .unwrap();
        let mut expected = [0; 350];
        for range in [50..150, 200..300] {
            expected[range.clone()].copy_from_slice(&bytes[range]);
        }
        assert_eq!(bus.read_bus_bytes(0, 350, false).unwrap(), expected);
    }
}

#[cfg(test)]
mod gpio_detaching_cases {
    use super::*;

    fn gpio_bus(receivers: usize) -> Bus {
        let mut bus = Bus::default();
        let mut gpio = H7Gpio::new(0, 0x400, [0; 3], 16);
        for index in 0..receivers {
            let base = (index as u32 + 1) * 0x400;
            bus.map_h7_timer(H7Timer::new(format!("timer{index}"), base, 100, 1000, None))
                .unwrap();
            gpio.connect_timer_input(index as u8, base, index as u32 + 1)
                .unwrap();
        }
        bus.map_h7_gpio(gpio).unwrap();
        bus
    }

    #[test]
    fn upstream_detach_numbered_gpio_on_receiver_removal() {
        let mut bus = gpio_bus(1);
        bus.unregister_from_address(0x400).unwrap();
        assert_eq!(bus.h7_gpio[0].timer_endpoints(0).count(), 0);
    }

    #[test]
    fn unregister_receiver_removes_duplicate_free_fanout_from_multiple_sources() {
        let mut bus = gpio_bus(2);
        let mut gpio = H7Gpio::new(0xc00, 0x400, [0; 3], 16);
        gpio.connect_timer_input(0, 0x800, 1).unwrap();
        bus.map_h7_gpio(gpio).unwrap();
        let gpio = &mut bus.h7_gpio[0];
        gpio.connect_timer_input(0, 0x800, 1).unwrap();
        gpio.connect_timer_input(0, 0x800, 1).unwrap();
        assert!(gpio.timer_endpoints(0).eq([(0x400, 1), (0x800, 1)]));
        assert!(bus.h7_gpio[1].timer_endpoints(0).eq([(0x800, 1)]));
        bus.unregister_from_address(0x800).unwrap();
        assert!(bus.h7_gpio[0].timer_endpoints(0).eq([(0x400, 1)]));
        assert_eq!(bus.h7_gpio[1].timer_endpoints(0).count(), 0);
    }

    #[test]
    fn upstream_detach_only_selected_numbered_gpio() {
        let mut bus = gpio_bus(3);
        bus.unregister_from_address(0x400).unwrap();
        let gpio = &bus.h7_gpio[0];
        assert_eq!(gpio.timer_endpoints(0).count(), 0);
        assert!(gpio.timer_endpoints(1).eq([(0x800, 2)]));
        assert!(gpio.timer_endpoints(2).eq([(0xc00, 3)]));
    }
}

#[cfg(test)]
mod gpio_connector_tests {
    use super::*;

    fn fixture() -> Bus {
        let mut bus = Bus::default();
        bus.map_nrf_peripheral(NrfPeripheral::gpio(0x50000500, "gpio0".into(), 32))
            .unwrap();
        bus.add_nrf_gpio_led(0x50000500, GpioLed::new("led".into(), 0, false), 0)
            .unwrap();
        bus.bridge_nrf_led("connector", "led").unwrap();
        assert_eq!(
            bus.nrf_led_routes
                .0
                .borrow()
                .lines(0x50000500)
                .next()
                .unwrap()
                .1
                .endpoints()
                .len(),
            1
        );
        assert_eq!(
            bus.nrf_gpio_connectors[0]
                .as_ref()
                .unwrap()
                .output
                .borrow()
                .endpoints()
                .len(),
            1
        );
        bus
    }

    #[test]
    fn upstream_gpio_source_machine_disposal_preserves_external_destination() {
        let mut source = crate::machine::Machine::without_default_ram();
        let mut destination = crate::machine::Machine::without_default_ram();
        source
            .bus
            .map_nrf_peripheral(NrfPeripheral::gpio(0, "sender".into(), 1))
            .unwrap();
        destination
            .bus
            .add_nrf_gpio_led(0x10, GpioLed::new("receiver".into(), 0, false), 0)
            .unwrap();

        let connector = destination
            .bus
            .bridge_nrf_led_from("connector", "receiver", Some((&mut source.bus, 0, 0)))
            .unwrap();
        let routes = source.bus.nrf_led_routes.0.clone();
        let source_endpoints = || {
            routes
                .borrow()
                .lines(0)
                .flat_map(|(_, line)| line.endpoints())
                .count()
        };
        assert_eq!(source_endpoints(), 1);
        assert_eq!(connector.output.borrow().endpoints().len(), 1);

        let mut output = std::io::sink();
        source.bus.write(24, 4, 1, &mut output).unwrap(); // DIRSET
        source.bus.write(8, 4, 1, &mut output).unwrap(); // OUTSET
        assert!(
            destination.bus.nrf_leds[0]
                .as_ref()
                .unwrap()
                .borrow()
                .is_on()
        );

        drop(source);

        assert_eq!(source_endpoints(), 0);
        assert_eq!(connector.output.borrow().endpoints().len(), 1);
        assert!(
            destination.bus.nrf_leds[0]
                .as_ref()
                .unwrap()
                .borrow()
                .is_on()
        );
        destination
            .bus
            .unregister_nrf_gpio_peripheral("receiver")
            .unwrap();
        assert!(connector.output.borrow().endpoints().is_empty());
    }

    #[test]
    fn upstream_gpio_external_removal_disconnects_source_endpoint() {
        let mut bus = fixture();
        bus.remove_nrf_gpio_connector("connector").unwrap();
        assert!(
            bus.nrf_led_routes
                .0
                .borrow()
                .lines(0x50000500)
                .next()
                .unwrap()
                .1
                .endpoints()
                .is_empty()
        );
    }

    #[test]
    fn upstream_gpio_source_unregister_preserves_external_destination() {
        let mut bus = fixture();
        bus.unregister_nrf_gpio_peripheral("gpio0").unwrap();
        assert!(
            bus.nrf_led_routes
                .0
                .borrow()
                .lines(0x50000500)
                .next()
                .unwrap()
                .1
                .endpoints()
                .is_empty()
        );
        assert_eq!(
            bus.nrf_gpio_connectors[0]
                .as_ref()
                .unwrap()
                .output
                .borrow()
                .endpoints()
                .len(),
            1
        );
    }
}
