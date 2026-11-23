use std::{
    alloc::{Layout, alloc_zeroed},
    collections::{BTreeMap, BTreeSet},
    fmt,
    io::{self, Write},
};

use crate::{
    clint::Clint,
    murax::{Gpio, Timer},
    nrf52840::Peripheral as NrfPeripheral,
    ns16550::Ns16550,
    nvic::{Nvic, VTOR},
    plic::Plic,
    stm32_adc::Stm32F0Adc,
    stm32_can::Stm32Can,
    stm32_crc::Stm32F0Crc,
    stm32_dma::Stm32Dma,
    stm32_exti::Exti,
    stm32_flash::Stm32F0Flash,
    stm32_gpio::Stm32Gpio,
    stm32_gpio_board::{GpioButton, GpioLed},
    stm32_i2c::Stm32I2c,
    stm32_rcc::Stm32F0Rcc,
    stm32_rtc::Stm32Rtc,
    stm32_spi::Stm32F0Spi,
    stm32_timer::Stm32Timer,
    stm32_uart::{Stm32Irq, Stm32Usart},
    systick::SysTick,
    uart::{LiteXUart, MuraxUart},
};

// Cortex-M peripheral clocks use a 10 ns time quantum.
pub(crate) const CORTEX_M_STEP_HZ: u64 = 100_000_000;

pub const RAM_BASE: u32 = 0x8000_0000;
const TRIVIAL_PERIPHERAL_SIZE: u32 = 0x1000;

pub const UART_TX: u32 = 0x1000_0000;

#[derive(Debug)]
pub enum BusError {
    Unmapped(u32, usize),
    Denied(u32),
    SlaveFault(u32),
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
        }
    }
}

struct Region {
    base: u32,
    bytes: Vec<u8>,
    aliases: Vec<u32>,
    privileged: bool,
    name: Option<String>,
    log_access: bool,
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

#[derive(Default)]
pub struct Bus {
    pub cpu_hooks: BTreeMap<u32, Vec<CpuHook>>,
    pub itstate_log_hook: Option<crate::thumb::opcode_counter::HookPhase>,
    pub thumb_trace: Option<crate::thumb::trace::Trace>,
    pub thumb_opcodes: crate::thumb::opcode_counter::OpcodeCounters,
    pub thumb_opcode_hooks: crate::thumb::opcode_counter::OpcodeLogHooks,
    pub semihosting: Option<crate::semihosting::Semihosting>,
    regions: Vec<Region>,
    ram_io_pages: BTreeSet<u32>,
    ram_io_logs: Vec<RamIoRead>,
    faulting_peripherals: Vec<FaultingPeripheral>,
    thumb_data_fault: Option<u32>,
    pub thumb_negative_priority: bool,
    pub thumb_unprivileged: bool,
    pub fault_on_unmapped_thumb_data: bool,
    pub report_unmapped_thumb_data: bool,
    trivial_peripherals: Vec<u32>,
    nrf_peripherals: Vec<NrfPeripheral>,
    nrf_tags: Vec<(u32, u32, u32)>,
    nrf_cpu_hz: Option<u64>,
    nrf_clock_remainder: u64,
    reservations: [Option<(u32, usize)>; crate::machine::MAX_HARTS],
    uart_tx: Option<u32>,
    trivial_uart: Option<u32>,
    pub uart: Option<MuraxUart>,
    ns16550: Option<Ns16550>,
    litex_uart: Option<LiteXUart>,
    stm32_uarts: Vec<Stm32Usart>,
    exti: Option<Exti>,
    stm32_gpios: Vec<Stm32Gpio>,
    gpio_exti_levels: u32,
    gpio_leds: Vec<(u32, GpioLed)>,
    nrf_led_routes: crate::nrf52840::gpio_line::GpioRoutes<usize>,
    nrf_leds: Vec<Option<GpioLed>>,
    gpio_buttons: Vec<GpioButton>,
    gpio_timer_inputs: Vec<(u32, u8, u32, u8)>,
    led_events: bool,
    stm32_timers: Vec<Stm32Timer>,
    stm32_spis: Vec<Stm32F0Spi>,
    stm32_i2cs: Vec<Stm32I2c>,
    stm32_crc: Option<Stm32F0Crc>,
    stm32_cans: Vec<Stm32Can>,
    stm32_adc: Option<Stm32F0Adc>,
    stm32_dma: Option<Stm32Dma>,
    stm32f0_tags: Vec<std::ops::RangeInclusive<u32>>,
    stm32_rcc: Option<Stm32F0Rcc>,
    stm32_flash: Option<Stm32F0Flash>,
    stm32_rtc: Option<Stm32Rtc>,
    plic: Option<Plic>,
    pub systick: Option<SysTick>,
    pub nvic: Option<Nvic>,
    pub clint: Option<Clint>,
    gpio: Option<(u32, Gpio)>,
    pub timer: Option<(u32, Timer)>,
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
            bytes,
            aliases: Vec::new(),
            privileged: false,
            name: None,
            log_access: false,
        })
    }

    fn offset(&self, address: u32, width: usize) -> Option<usize> {
        std::iter::once(self.base)
            .chain(self.aliases.iter().copied())
            .find_map(|base| {
                let offset = address.checked_sub(base)? as usize;
                (width <= self.bytes.len().checked_sub(offset)?).then_some(offset)
            })
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
                Some((region.name.as_deref()?, region.offset(address, width)?))
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

    pub fn map_ram_alias(&mut self, base: u32, alias: u32, size: usize) -> Result<(), String> {
        let start = u64::from(alias);
        let end = start.saturating_add(size as u64);
        if end > 1 << 32 || self.overlaps(start, end) {
            return Err(format!("invalid or overlapping RAM alias at {alias:#010x}"));
        }
        let region = self
            .regions
            .iter_mut()
            .find(|region| region.base == base && region.bytes.len() == size)
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
        for (pin, line) in self.nrf_led_routes.lines(gpio) {
            for endpoint in line.endpoints() {
                let led = self.nrf_leds[endpoint.receiver].as_mut().unwrap();
                led.sample_level(pins & (1 << pin) != 0, self.led_events);
            }
        }
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
                let offset = region.offset(address, length)?;
                Some(&region.bytes[offset..offset + length])
            })
            .ok_or(BusError::Unmapped(address, length))
    }

    fn overlaps(&self, start: u64, end: u64) -> bool {
        self.trivial_peripherals.iter().any(|&base| {
            let base = u64::from(base);
            start < base + u64::from(TRIVIAL_PERIPHERAL_SIZE) && base < end
        }) || self.faulting_peripherals.iter().any(|device| {
            let base = u64::from(device.base);
            start < base + FAULTING_PERIPHERAL_SIZE as u64 && base < end
        }) || self.nrf_peripherals.iter().any(|device| {
            let other = u64::from(device.base);
            start < other + u64::from(device.size()) && other < end
        }) || self
            .nvic
            .as_ref()
            .is_some_and(|nvic| nvic.overlaps(start, end))
            || (self.systick.is_some() && SysTick::overlaps(start, end))
            || self
                .uart_tx
                .is_some_and(|address| (start..end).contains(&u64::from(address)))
            || self.trivial_uart.is_some_and(|base| {
                let base = u64::from(base);
                start < base + 4 && base < end
            })
            || self.uart.as_ref().is_some_and(|uart| {
                let other = u64::from(uart.base);
                start < other + MuraxUart::SIZE && other < end
            })
            || self.litex_uart.as_ref().is_some_and(|uart| {
                let other = u64::from(uart.base);
                start < other + u64::from(LiteXUart::SIZE) && other < end
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
            || self.exti.as_ref().is_some_and(|exti| {
                let other = u64::from(exti.base);
                start < other + u64::from(Exti::SIZE) && other < end
            })
            || self.stm32_uarts.iter().any(|uart| {
                let other = u64::from(uart.base);
                start < other + u64::from(Stm32Usart::SIZE) && other < end
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
            || self.regions.iter().any(|region| {
                std::iter::once(region.base)
                    .chain(region.aliases.iter().copied())
                    .any(|base| {
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

    pub fn map_trivial_uart(&mut self, base: u32) -> Result<(), String> {
        let start = u64::from(base);
        if self.trivial_uart.is_some() || base & 3 != 0 || self.overlaps(start, start + 4) {
            return Err("invalid, duplicate, or overlapping TrivialUart".into());
        }
        self.trivial_uart = Some(base);
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

    pub fn map_nvic(
        &mut self,
        priority_mask: u8,
        uart_irq: Option<u8>,
        cortex_m4: bool,
    ) -> Result<(), String> {
        if Nvic::ranges(cortex_m4)
            .any(|(start, end)| self.overlaps(u64::from(start), u64::from(end)))
        {
            return Err("Cortex-M NVIC registers overlap an existing mapping".into());
        }
        self.nvic = Some(Nvic::new(priority_mask, uart_irq, cortex_m4));
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
        if let Some(uart) = self.litex_uart.as_mut() {
            *uart = LiteXUart::new(uart.base);
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

    pub fn sample_nvic(&mut self, active: u128, sleeping: bool) {
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
            let mut exti_inputs = self.gpio_exti_levels;
            for uart in &self.stm32_uarts {
                if uart.interrupt_pending() {
                    match uart.irq {
                        Some(Stm32Irq::Nvic(line)) => nvic.set_line(line, true, active),
                        Some(Stm32Irq::Exti(line)) => exti_inputs |= 1 << line,
                        None => {}
                    }
                }
            }
            if let Some(rtc) = self.stm32_rtc.as_mut() {
                let mut signals = rtc.tick();
                while signals != 0 {
                    let signal = signals.trailing_zeros() as usize;
                    signals &= signals - 1;
                    match rtc.routes[signal] {
                        Some(Stm32Irq::Nvic(line)) => nvic.set_line(line, true, active),
                        Some(Stm32Irq::Exti(line)) => exti_inputs |= 1 << line,
                        None => {}
                    }
                }
            }
            let mut irqs = self
                .exti
                .as_mut()
                .map_or(0, |exti| exti.sample(exti_inputs));
            irqs |= timer_irqs;
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

    pub fn add_nrf_gpio_led(
        &mut self,
        base: u32,
        mut led: GpioLed,
        input: u32,
    ) -> Result<(), String> {
        self.nrf_led_routes.line_mut(base, led.pin).connect(
            self.nrf_leds.len(),
            &led.name,
            1,
            input,
        )?;
        let driven = self.nrf_gpio_driven_high(base);
        led.sample_level(driven & (1 << led.pin) != 0, self.led_events);
        self.nrf_leds.push(Some(led));
        Ok(())
    }

    pub fn unregister_nrf_led(&mut self, name: &str) -> Result<(), String> {
        let receiver = self
            .nrf_leds
            .iter()
            .position(|led| led.as_ref().is_some_and(|led| led.name == name))
            .ok_or_else(|| format!("unknown nRF GPIO LED {name:?}"))?;
        self.nrf_led_routes.unregister_receiver(&receiver);
        self.nrf_leds[receiver] = None;
        Ok(())
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

    pub fn set_button(&mut self, index: usize, pressed: bool) {
        let button = &self.gpio_buttons[index];
        if let Some(port) = self
            .stm32_gpios
            .iter_mut()
            .find(|port| port.base == button.port)
        {
            port.set_external(button.pin, pressed ^ button.invert);
            self.refresh_gpio_wiring();
        } else if let Some(port) = self
            .nrf_peripherals
            .iter_mut()
            .find(|port| port.base == button.port)
        {
            port.set_gpio_input(button.pin, pressed ^ button.invert);
        }
    }

    fn refresh_gpio_wiring(&mut self) {
        self.gpio_exti_levels = self
            .stm32_gpios
            .iter()
            .fold(0, |levels, port| levels | port.exti_levels());
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
                for (pin, line) in self.nrf_led_routes.lines(port.base) {
                    for endpoint in line.endpoints() {
                        let led = self.nrf_leds[endpoint.receiver].as_mut().unwrap();
                        led.sample_level(high & (1 << pin) != 0, self.led_events);
                    }
                }
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

    pub fn map_stm32f0_adc(&mut self, adc: Stm32F0Adc) -> Result<(), String> {
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
        Ok(())
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
            region.base == BANK.start && region.bytes.len() == (BANK.end - BANK.start) as usize
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
        let end = start + u64::from(Stm32Usart::SIZE);
        if base & 3 != 0 || end > 1 << 32 || self.overlaps(start, end) {
            return Err(format!(
                "invalid or overlapping STM32 USART range at {base:#010x}"
            ));
        }
        self.stm32_uarts.push(uart);
        Ok(())
    }

    pub fn has_uart_input(&self) -> bool {
        self.uart.is_some()
            || self.ns16550.is_some()
            || self.litex_uart.is_some()
            || !self.stm32_uarts.is_empty()
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
        } else if let Some(uart) = self.stm32_uarts.first_mut() {
            uart.queue_input(bytes);
        } else if let Some(index) = self.nrf_peripherals.iter().position(NrfPeripheral::is_uart) {
            let mut device = self.nrf_peripherals.remove(index);
            device.queue_input(bytes);
            let result = device.receive(self);
            self.nrf_peripherals.insert(index, device);
            result?;
        } else if let Some(handler) = self.semihosting.as_mut().filter(|handler| handler.console) {
            handler.queue_input(bytes);
        }
        Ok(())
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
        for region in &mut self.regions {
            if let Some(offset) = region.offset(address, width) {
                if width != 0 {
                    let address = region.base + offset as u32;
                    for reservation in &mut self.reservations {
                        if reservation.is_some_and(|(reserved, _)| {
                            let reserved = reserved & !3;
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

    fn canonical_ram_address(&self, address: u32, width: usize) -> Option<u32> {
        self.regions.iter().find_map(|region| {
            region
                .offset(address, width)
                .map(|offset| region.base + offset as u32)
        })
    }

    pub fn clear_reservation(&mut self, hart: usize) {
        self.reservations[hart] = None;
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
        let value = self.read_ram::<N>(address).map_err(|e| e.to_string())?;
        self.reservations[0] = self
            .canonical_ram_address(address, N)
            .map(|address| (address, N));
        Ok(value)
    }

    pub fn thumb_store_exclusive<const N: usize>(
        &mut self,
        address: u32,
        value: u32,
    ) -> Result<u32, String> {
        self.check_thumb_access(address, N, true, false)?;
        if address as usize % N != 0 {
            return Err(BusError::Misaligned(address, N).to_string());
        }
        let canonical = self
            .canonical_ram_address(address, N)
            .ok_or_else(|| BusError::Unmapped(address, N).to_string())?;
        if self.reservations[0].take() != Some((canonical, N)) {
            return Ok(1);
        }
        self.memory(address, N)
            .map_err(|e| e.to_string())?
            .copy_from_slice(&value.to_le_bytes()[..N]);
        Ok(0)
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
                            .offset(address.wrapping_add(offset as u32), 1)
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
                CpuHook::Log(message) => writeln!(output, "{message}").map_err(BusError::Output)?,
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
        if N == 4 && self.trivial_peripheral_at(address) {
            return Ok(0);
        }
        if let Some(result) = self.read_faulting_peripheral::<N>(address) {
            return result;
        }
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
        match self.read_ram::<N>(address) {
            Err(BusError::Unmapped(..)) if self.stm32f0_tagged(address, N) => Ok(0),
            Err(error @ BusError::Unmapped(..)) => self
                .nrf_tag(address, N)
                .ok_or(error)
                .map(|value| value & (u32::MAX >> (32 - N * 8))),
            result => result,
        }
    }

    pub fn read_ram<const N: usize>(&self, address: u32) -> Result<u32, BusError> {
        if address as usize & (N - 1) != 0 {
            return Err(BusError::Misaligned(address, N));
        }
        for region in &self.regions {
            if let Some(offset) = region.offset(address, N) {
                let mut bytes = [0; 4];
                bytes[..N].copy_from_slice(&region.bytes[offset..offset + N]);
                return Ok(u32::from_le_bytes(bytes));
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
        if width == 4 && self.trivial_peripheral_at(address) {
            return Ok(());
        }
        if let Some(result) = self.write_faulting_peripheral(address, width, value) {
            return result;
        }
        if self.trivial_uart == Some(address) && matches!(width, 1 | 2 | 4) {
            return uart.write_all(&[value as u8]).map_err(BusError::Output);
        }
        if self.uart_tx == Some(address) && width == 1 {
            return uart.write_all(&[value as u8]).map_err(BusError::Output);
        }
        if address as usize & (width - 1) != 0 {
            return Err(BusError::Misaligned(address, width));
        }
        if width == 4
            && let Some(index) = self
                .nrf_peripherals
                .iter()
                .position(|device| device.contains(address))
        {
            let mut device = self.nrf_peripherals.remove(index);
            let result = device.write(address, value, self, uart);
            for (gpio, pin, high) in device.sensor_irqs() {
                self.apply_nrf_input(gpio, pin, high);
            }
            self.nrf_peripherals.insert(index, device);
            return result;
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
        match self.memory(address, width) {
            Ok(bytes) => bytes.copy_from_slice(&value.to_le_bytes()[..width]),
            Err(error @ BusError::Unmapped(..)) => {
                if !self.stm32f0_tagged(address, width) && self.nrf_tag(address, width).is_none() {
                    return Err(error);
                }
            }
            Err(error) => return Err(error),
        }
        Ok(())
    }
}
