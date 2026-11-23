mod adxl372;
mod arducam;
mod bme280;
mod bmp180;
mod lis2ds12;
mod lsm303;
mod lsm330;
mod lsm9ds1;
use adxl372::Adxl372;

mod aar;
use aar::Aar;

mod egu;
use egu::Egu;

mod ecb;
use ecb::Ecb;

mod rng;
use rng::Rng;

mod radio;
pub(crate) use radio::AirFrame;
use radio::Radio;

mod pdm;
use pdm::Pdm;

mod i2s;
use i2s::I2s;

mod timer;
use timer::Timer;

mod twi;
use twi::Twi;

mod gpio;
mod ppi;
mod spi;
use spi::Spi;
pub(crate) use spi::SpiMuxPort;

use ppi::Ppi;
mod watchdog;

use gpio::{Gpio, Gpiote};

use watchdog::Watchdog;

use std::{collections::VecDeque, io::Write};

use crate::bus::{Bus, BusError};

const UART_EVENTS: u32 = 0x005a_0397;

pub struct Peripheral {
    pub base: u32,
    pub irq: u8,
    pub name: String,
    device: Device,
}

enum Device {
    Clock(Clock),
    Uarte(Uarte),
    Rtc(Rtc),
    Timer(Timer),
    I2s(I2s),
    Pdm(Pdm),
    Radio(Radio),
    Rng(Rng),
    Ecb(Ecb),
    Aar(Aar),
    Egu(Egu),
    Twi(Twi),
    Spi(Spi),
    Watchdog(Watchdog),
    Gpio(Gpio),
    Gpiote(Gpiote),
    Ppi(Box<Ppi>),
    Nvmc { cache_config: u32 },
    PwmConfig { output_pin: u32 },
    UsbdConfig { iso_in: u32 },
    Spim3Config([u32; 10]),
    Temperature { ready: bool, irq_enabled: bool },
}

#[derive(Default)]
struct Clock {
    running: u32,
    events: u32,
    interrupts: u32,
    lf_source: u32,
    lf_source_copy: u32,
    constant_latency: bool,
    dcdc: u32,
    dcdc0: u32,
    trace_config: u32,
    retention: u32,
    ram_priority: [u32; 14],
}

struct Uarte {
    enabled: u32,
    events: u32,
    raised: u32,
    interrupts: u32,
    irq_level: bool,
    shorts: u32,
    pins: [u32; 4],
    baud: u32,
    config: u32,
    rx_pointer: u32,
    rx_count: u32,
    rx_amount: u32,
    rx_active: Option<(u32, u32)>,
    legacy_rx: bool,
    legacy_tx: bool,
    rx_byte: Option<u8>,
    tx_pointer: u32,
    tx_count: u32,
    tx_amount: u32,
    input: VecDeque<u8>,
    link_output: Option<Vec<u8>>,
}

impl Default for Uarte {
    fn default() -> Self {
        Self {
            enabled: 0,
            events: 0,
            raised: 0,
            interrupts: 0,
            irq_level: false,
            shorts: 0,
            pins: [u32::MAX; 4],
            baud: 0x0400_0000,
            config: 0,
            rx_pointer: 0,
            rx_count: 0,
            rx_amount: 0,
            rx_active: None,
            legacy_rx: false,
            legacy_tx: false,
            rx_byte: None,
            tx_pointer: 0,
            tx_count: 0,
            tx_amount: 0,
            input: VecDeque::new(),
            link_output: None,
        }
    }
}

impl Peripheral {
    pub const SIZE: u32 = 0x1000;

    pub fn temperature() -> Self {
        Self {
            base: 0x4000_c000,
            irq: 12,
            name: "temp".into(),
            device: Device::Temperature {
                ready: false,
                irq_enabled: false,
            },
        }
    }

    pub fn spim3_config() -> Self {
        Self {
            base: 0x4002_f000,
            irq: 47,
            name: "spim3".into(),
            device: Device::Spim3Config([0; 10]),
        }
    }

    fn spim3_config_index(offset: u32) -> Option<usize> {
        [
            0x500, 0x508, 0x50c, 0x510, 0x514, 0x524, 0x554, 0x560, 0x564, 0x5c0,
        ]
        .iter()
        .position(|&register| offset == register)
    }

    pub fn usbd_config() -> Self {
        Self {
            base: 0x4002_7000,
            irq: 39,
            name: "usbd".into(),
            device: Device::UsbdConfig { iso_in: 0 },
        }
    }

    pub fn pwm_config(base: u32, index: usize) -> Self {
        Self {
            base,
            irq: 0,
            name: format!("pwm{index}"),
            device: Device::PwmConfig {
                output_pin: u32::MAX,
            },
        }
    }

    pub fn clock(base: u32, irq: u8, name: String) -> Self {
        Self {
            base,
            irq,
            name,
            device: Device::Clock(Clock::default()),
        }
    }

    pub fn uarte(base: u32, irq: u8, name: String) -> Self {
        Self {
            base,
            irq,
            name,
            device: Device::Uarte(Uarte::default()),
        }
    }

    pub fn i2s(base: u32, irq: u8, name: String, loopback: bool) -> Self {
        Self {
            base,
            irq,
            name,
            device: Device::I2s(I2s::new(loopback)),
        }
    }

    pub fn egu(base: u32, irq: u8, name: String) -> Self {
        Self {
            base,
            irq,
            name,
            device: Device::Egu(Egu::default()),
        }
    }

    pub fn aar() -> Self {
        Self {
            base: 0x4000_f000,
            irq: 15,
            name: "aar".into(),
            device: Device::Aar(Aar::default()),
        }
    }

    pub fn ecb(base: u32, irq: u8, name: String) -> Self {
        Self {
            base,
            irq,
            name,
            device: Device::Ecb(Ecb::default()),
        }
    }

    pub fn rng(base: u32, irq: u8, name: String) -> Self {
        Self {
            base,
            irq,
            name,
            device: Device::Rng(Rng::default()),
        }
    }

    pub fn is_radio(&self) -> bool {
        matches!(self.device, Device::Radio(_))
    }

    pub fn attach_radio_medium(&mut self) {
        if let Device::Radio(radio) = &mut self.device {
            radio.attach_medium();
        }
    }

    pub fn tick_radio_air(&mut self) -> Option<AirFrame> {
        match &mut self.device {
            Device::Radio(radio) => radio.tick_air(),
            _ => None,
        }
    }

    pub fn receive_radio_air(&mut self, frame: AirFrame, bus: &mut Bus) -> Result<(), BusError> {
        match &mut self.device {
            Device::Radio(radio) => radio.receive_air(frame, bus),
            _ => Ok(()),
        }
    }

    pub fn radio(base: u32, irq: u8, name: String, loopback: bool) -> Self {
        Self {
            base,
            irq,
            name,
            device: Device::Radio(Radio::new(loopback)),
        }
    }

    pub fn pdm(base: u32, irq: u8, name: String) -> Self {
        Self {
            base,
            irq,
            name,
            device: Device::Pdm(Pdm::default()),
        }
    }

    pub fn is_pdm(&self) -> bool {
        matches!(self.device, Device::Pdm(_))
    }

    pub fn is_i2s(&self) -> bool {
        matches!(self.device, Device::I2s(_))
    }

    pub fn configure_i2s_pcm(&mut self, input: Vec<u8>, capture: bool) {
        if let Device::I2s(i2s) = &mut self.device {
            i2s.configure_pcm(input, capture);
        }
    }

    pub fn i2s_pcm_output(&self) -> Option<&[u8]> {
        match &self.device {
            Device::I2s(i2s) => i2s.pcm_output(),
            _ => None,
        }
    }

    pub fn set_pdm_input(&mut self, input: Vec<u8>) {
        if let Device::Pdm(pdm) = &mut self.device {
            pdm.set_input(input);
        }
    }

    pub fn audio_transfer_due(&mut self) -> bool {
        match &mut self.device {
            Device::I2s(i2s) => i2s.tick(),
            Device::Pdm(pdm) => pdm.tick(),
            _ => false,
        }
    }

    pub fn complete_audio(&mut self, bus: &mut Bus) -> Result<(), BusError> {
        match &mut self.device {
            Device::I2s(i2s) => i2s.complete(bus),
            Device::Pdm(pdm) => pdm.complete(bus),
            _ => Ok(()),
        }
    }

    pub fn timer(base: u32, irq: u8, name: String, channels: usize) -> Self {
        Self {
            base,
            irq,
            name,
            device: Device::Timer(Timer::new(channels)),
        }
    }

    pub fn rtc(base: u32, irq: u8, name: String, channels: usize) -> Self {
        Self {
            base,
            irq,
            name,
            device: Device::Rtc(Rtc {
                channels,
                ..Rtc::default()
            }),
        }
    }

    pub fn spi(base: u32, irq: u8, name: String, loopback: bool) -> Self {
        Self {
            base,
            irq,
            name,
            device: Device::Spi(Spi::new(loopback)),
        }
    }

    pub fn twi(base: u32, irq: u8, name: String) -> Self {
        Self {
            base,
            irq,
            name,
            device: Device::Twi(Twi::default()),
        }
    }

    pub fn attach_ov2640(&mut self, address: u32) -> Result<(), String> {
        let Device::Twi(twi) = &mut self.device else {
            return Err("OV2640 requires TWI".into());
        };
        twi.attach(address, twi::Target::Ov2640(arducam::Ov2640::default()))
    }

    pub fn attach_lsm9ds1(&mut self, address: u32, magnetic: bool) -> Result<(), String> {
        let Device::Twi(twi) = &mut self.device else {
            return Err("LSM9DS1 requires TWI".into());
        };
        twi.attach(
            address,
            twi::Target::Lsm9ds1(lsm9ds1::Lsm9ds1::new(magnetic)),
        )
    }

    pub fn lsm9ds1_samples(&mut self) -> Option<&mut VecDeque<[f64; 3]>> {
        match &mut self.device {
            Device::Twi(twi) => twi.lsm9ds1_samples(),
            _ => None,
        }
    }

    pub fn attach_camera(&mut self, gpio: u32, pin: u32) {
        if let Device::Spi(spi) = &mut self.device {
            spi.attach_camera(gpio, pin);
        }
    }

    pub fn set_camera_image(&mut self, image: &mut Option<Vec<u8>>) -> bool {
        match &mut self.device {
            Device::Spi(spi) => spi.set_camera_image(image),
            _ => false,
        }
    }

    pub fn attach_lsm303(
        &mut self,
        address: u32,
        values: [f64; 3],
        irq: Option<(u32, [u8; 2])>,
    ) -> Result<(), String> {
        let Device::Twi(twi) = &mut self.device else {
            return Err("LSM303DLHC requires TWI".into());
        };
        twi.attach(
            address,
            twi::Target::Lsm303(lsm303::Lsm303::new(values, irq)),
        )
    }

    pub fn attach_lsm330(
        &mut self,
        address: u32,
        values: [f64; 3],
        gyroscope: bool,
    ) -> Result<(), String> {
        let Device::Twi(twi) = &mut self.device else {
            return Err("LSM330 requires TWI".into());
        };
        twi.attach(
            address,
            twi::Target::Lsm330(lsm330::Lsm330::new(values, gyroscope)),
        )
    }

    pub fn attach_bmp180(
        &mut self,
        address: u32,
        temperature: f64,
        pressure: u32,
    ) -> Result<(), String> {
        let Device::Twi(twi) = &mut self.device else {
            return Err("BMP180 requires TWI".into());
        };
        twi.attach(
            address,
            twi::Target::Bmp180(bmp180::Bmp180::new(temperature, pressure)),
        )
    }

    pub fn attach_lis2ds12(
        &mut self,
        address: u32,
        acceleration: [f64; 3],
        gpio: u32,
        pin: u8,
    ) -> Result<(), String> {
        let Device::Twi(twi) = &mut self.device else {
            return Err("LIS2DS12 requires TWI".into());
        };
        twi.attach(
            address,
            twi::Target::Lis2ds12(Box::new(lis2ds12::Lis2ds12::new(acceleration, gpio, pin))),
        )
    }

    pub fn sensor_irqs(&self) -> impl Iterator<Item = (u32, u8, bool)> + '_ {
        match &self.device {
            Device::Twi(twi) => Some(twi.sensor_irqs()),
            _ => None,
        }
        .into_iter()
        .flatten()
    }

    pub fn attach_bme280(&mut self, address: u32) -> Result<(), String> {
        let Device::Twi(twi) = &mut self.device else {
            return Err("BME280 requires TWI".into());
        };
        twi.attach(address, twi::Target::Bme280(Box::default()))
    }

    pub fn attach_adxl_i2c(
        &mut self,
        address: u32,
        name: String,
        acceleration: [i16; 3],
    ) -> Result<(), String> {
        let Device::Twi(twi) = &mut self.device else {
            return Err("ADXL372 requires TWI".into());
        };
        twi.attach(
            address,
            twi::Target::Adxl372(Box::new(Adxl372::new(name, acceleration))),
        )
    }

    pub fn attach_adxl_spi(&mut self, gpio: u32, pin: u32, name: String, acceleration: [i16; 3]) {
        if let Device::Spi(spi) = &mut self.device {
            spi.attach_adxl(gpio, pin, Adxl372::new(name, acceleration));
        }
    }

    pub fn attach_spi_mux(&mut self, ports: Vec<SpiMuxPort>) {
        if let Device::Spi(spi) = &mut self.device {
            spi.attach_mux(ports);
        }
    }

    pub fn adxl_input(&mut self, name: &str) -> Option<&mut [i16; 3]> {
        let device = match &mut self.device {
            Device::Twi(twi) => twi.adxl(name),
            Device::Spi(spi) => spi.adxl(name),
            _ => None,
        }?;
        Some(&mut device.acceleration)
    }

    pub fn release_spi_cs(&mut self, gpio: u32, pins: u32) {
        if let Device::Spi(spi) = &mut self.device {
            spi.release_cs(gpio, pins);
        }
    }

    pub fn attach_eeprom(
        &mut self,
        address: u32,
        target: crate::stm32_i2c::Eeprom,
    ) -> Result<(), String> {
        let Device::Twi(twi) = &mut self.device else {
            return Err("expected an nRF52840 TWI controller".into());
        };
        twi.attach(address, twi::Target::Eeprom(target))
    }

    pub fn ppi(base: u32, name: String) -> Self {
        Self {
            base,
            irq: 0,
            name,
            device: Device::Ppi(Box::default()),
        }
    }

    pub fn is_ppi(&self) -> bool {
        matches!(self.device, Device::Ppi(_))
    }

    pub fn ppi_event_supported(&self, address: u32) -> bool {
        self.contains(address)
            && address & 3 == 0
            && match &self.device {
                Device::Uarte(_) => Uarte::event(address - self.base).is_some(),
                Device::Twi(_) => Twi::event(address - self.base).is_some(),
                Device::Spi(_) => address - self.base == 0x108,
                Device::Timer(timer) => timer.event(address - self.base),
                Device::Rtc(rtc) => rtc.event_bit(address - self.base).is_some(),
                Device::I2s(_) => I2s::event(address - self.base).is_some(),
                Device::Pdm(_) => Pdm::event(address - self.base).is_some(),
                Device::Radio(_) => Radio::event(address - self.base).is_some(),
                Device::Rng(_) => address - self.base == rng::VALRDY,
                Device::Ecb(_) => matches!(address - self.base, 0x100 | 0x104),
                Device::Aar(_) => matches!(address - self.base, 0x100 | 0x104 | 0x108),
                Device::Egu(_) => Egu::event(address - self.base),
                _ => false,
            }
    }

    pub fn ppi_task_supported(&self, address: u32) -> bool {
        self.contains(address)
            && match &self.device {
                Device::Uarte(_) => matches!(address - self.base, 0 | 4 | 8 | 12),
                Device::Twi(_) => Twi::is_task(address - self.base),
                Device::Timer(timer) => timer.task(address - self.base),
                Device::I2s(_) => matches!(address - self.base, 0 | 4),
                Device::Pdm(_) => matches!(address - self.base, 0 | 4),
                Device::Radio(_) => Radio::task(address - self.base),
                Device::Rng(_) => matches!(address - self.base, 0 | 4),
                Device::Ecb(_) => matches!(address - self.base, 0 | 4),
                Device::Aar(_) => matches!(address - self.base, 0 | 8),
                Device::Egu(_) => Egu::task(address - self.base),
                Device::Ppi(_) => address - self.base <= 0x2c && address & 3 == 0,
                _ => false,
            }
    }

    pub fn take_raised_events(&mut self, events: &mut Vec<u32>) {
        let mut raised = match &mut self.device {
            Device::Uarte(uart) => std::mem::take(&mut uart.raised),
            Device::Twi(twi) => std::mem::take(&mut twi.raised),
            Device::Spi(spi) => std::mem::take(&mut spi.raised),
            Device::Timer(timer) => std::mem::take(&mut timer.raised),
            Device::Rtc(rtc) => std::mem::take(&mut rtc.raised),
            Device::I2s(i2s) => std::mem::take(&mut i2s.raised),
            Device::Pdm(pdm) => std::mem::take(&mut pdm.raised),
            Device::Radio(radio) => std::mem::take(&mut radio.raised),
            Device::Rng(rng) => std::mem::take(&mut rng.raised),
            Device::Ecb(ecb) => std::mem::take(&mut ecb.raised),
            Device::Aar(aar) => std::mem::take(&mut aar.raised),
            Device::Egu(egu) => std::mem::take(&mut egu.raised),
            _ => return,
        };
        while raised != 0 {
            let index = raised.trailing_zeros();
            raised &= raised - 1;
            events.push(self.base + 0x100 + index * 4);
        }
    }

    pub fn ppi_tasks(&self, event: u32, tasks: &mut Vec<u32>) {
        if let Device::Ppi(ppi) = &self.device {
            ppi.tasks(event, tasks);
        }
    }

    pub fn gpiote(base: u32, irq: u8, name: String, ports: [u32; 2]) -> Self {
        let mut gpiote = Gpiote::default();
        gpiote.ports = ports;
        Self {
            base,
            irq,
            name,
            device: Device::Gpiote(gpiote),
        }
    }

    pub fn set_gpio_input(&mut self, pin: u8, high: bool) {
        if let Device::Gpio(gpio) = &mut self.device {
            gpio.set_external(pin, high);
        }
    }

    pub fn gpio_driven_high(&self) -> Option<u32> {
        match &self.device {
            Device::Gpio(gpio) => Some(gpio.driven_high()),
            _ => None,
        }
    }

    pub fn gpio_sample(&self) -> Option<(u32, bool)> {
        if let Device::Gpio(gpio) = &self.device {
            Some(gpio.sample())
        } else {
            None
        }
    }

    pub fn gpio_task(&mut self, pin: u32, action: u32) {
        if let Device::Gpio(gpio) = &mut self.device {
            gpio.task_pin(pin, action);
        }
    }

    pub fn gpiote_ports(&self) -> Option<[u32; 2]> {
        if let Device::Gpiote(gpiote) = &self.device {
            Some(gpiote.ports)
        } else {
            None
        }
    }

    pub fn gpiote_sample(&mut self, samples: [(u32, bool); 2]) {
        if let Device::Gpiote(gpiote) = &mut self.device {
            gpiote.sample(samples);
        }
    }

    pub fn gpio(base: u32, name: String, pins: usize) -> Self {
        Self {
            base,
            irq: 0,
            name,
            device: Device::Gpio(Gpio::new(pins)),
        }
    }

    pub fn size(&self) -> u32 {
        if matches!(self.device, Device::Gpio(_)) {
            0x300
        } else {
            Self::SIZE
        }
    }

    pub fn nvmc() -> Self {
        Self {
            base: 0x4001_e000,
            irq: 0,
            name: "nvmc".into(),
            device: Device::Nvmc { cache_config: 0 },
        }
    }

    pub fn watchdog(base: u32, irq: u8, name: String) -> Self {
        Self {
            base,
            irq,
            name,
            device: Device::Watchdog(Watchdog::default()),
        }
    }

    pub fn tick(&mut self, sleeping: bool) {
        match &mut self.device {
            Device::Rtc(rtc) => rtc.tick(),
            Device::Timer(timer) => timer.tick(),
            Device::Rng(rng) => rng.tick(),
            Device::Watchdog(watchdog) => watchdog.tick(sleeping),
            _ => {}
        }
    }

    pub fn reset_requested(&self) -> bool {
        matches!(&self.device, Device::Watchdog(watchdog) if watchdog.reset_requested)
    }

    pub fn link_uart(&mut self) {
        if let Device::Uarte(uart) = &mut self.device {
            uart.link_output = Some(Vec::new());
        }
    }

    pub fn take_uart_link_output(&mut self) -> Vec<u8> {
        match &mut self.device {
            Device::Uarte(uart) => uart
                .link_output
                .as_mut()
                .map(std::mem::take)
                .unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    pub fn is_uart(&self) -> bool {
        matches!(self.device, Device::Uarte(_))
    }

    pub fn set_uart_irq(&mut self, level: bool) {
        if let Device::Uarte(uart) = &mut self.device {
            uart.irq_level = level;
        }
    }

    pub fn reset(&mut self) {
        match &mut self.device {
            Device::Gpio(gpio) => gpio.reset(),
            Device::Twi(twi) => twi.reset(),
            Device::Spi(spi) => spi.reset(),
            Device::Timer(timer) => timer.reset(),
            Device::I2s(i2s) => i2s.reset(),
            Device::Pdm(pdm) => pdm.reset(),
            Device::Radio(radio) => radio.reset(),
            Device::Rng(rng) => *rng = Rng::default(),
            Device::Ecb(ecb) => *ecb = Ecb::default(),
            Device::Aar(aar) => *aar = Aar::default(),
            Device::Egu(egu) => *egu = Egu::default(),
            Device::Gpiote(gpiote) => gpiote.reset(),
            Device::Ppi(ppi) => **ppi = Ppi::default(),
            Device::Clock(clock) => {
                *clock = Clock {
                    retention: clock.retention,
                    ..Clock::default()
                };
            }
            Device::Nvmc { cache_config } => *cache_config = 0,
            Device::PwmConfig { output_pin } => *output_pin = u32::MAX,
            Device::UsbdConfig { iso_in } => *iso_in = 0,
            Device::Spim3Config(registers) => registers.fill(0),
            Device::Temperature { ready, irq_enabled } => {
                *ready = false;
                *irq_enabled = false;
            }
            Device::Watchdog(watchdog) => *watchdog = Watchdog::default(),
            Device::Uarte(uart) => {
                let linked = uart.link_output.is_some();
                *uart = Uarte::default();
                uart.link_output = linked.then(Vec::new);
            }
            Device::Rtc(rtc) => {
                *rtc = Rtc {
                    channels: rtc.channels,
                    ..Rtc::default()
                }
            }
        }
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < self.size()
    }

    pub fn pending(&self) -> bool {
        match &self.device {
            Device::Clock(clock) => clock.events & clock.interrupts != 0,
            Device::Temperature { ready, irq_enabled } => *ready && *irq_enabled,
            Device::Nvmc { .. }
            | Device::PwmConfig { .. }
            | Device::UsbdConfig { .. }
            | Device::Spim3Config(_)
            | Device::Gpio(_)
            | Device::Ppi(_) => false,
            Device::Watchdog(watchdog) => watchdog.pending(),
            Device::Uarte(uart) => uart.irq_level,
            Device::Gpiote(gpiote) => gpiote.pending(),
            Device::Rtc(rtc) => rtc.events & rtc.interrupts != 0,
            Device::Twi(twi) => twi.pending(),
            Device::Spi(spi) => spi.pending(),
            Device::Timer(timer) => timer.pending(),
            Device::I2s(i2s) => i2s.pending(),
            Device::Pdm(pdm) => pdm.pending(),
            Device::Radio(radio) => radio.pending(),
            Device::Rng(rng) => rng.pending(),
            Device::Ecb(ecb) => ecb.pending(),
            Device::Aar(aar) => aar.pending(),
            Device::Egu(egu) => egu.pending(),
        }
    }

    pub fn queue_uart_link_input(&mut self, bytes: Vec<u8>) {
        if let Device::Uarte(uart) = &mut self.device
            && uart.enabled != 0
        {
            // Bytes already received by the link can wait for the next DMA buffer.
            uart.input.extend(bytes);
        }
    }

    pub fn queue_input(&mut self, bytes: Vec<u8>) {
        if let Device::Uarte(uart) = &mut self.device
            && (uart.rx_active.is_some() || uart.legacy_rx)
        {
            uart.input.extend(bytes);
        }
    }

    pub fn receive(&mut self, bus: &mut Bus) -> Result<(), BusError> {
        if let Device::Uarte(uart) = &mut self.device {
            uart.receive(bus)?;
        }
        Ok(())
    }

    pub fn read(&mut self, address: u32) -> Option<u32> {
        let offset = address - self.base;
        match &mut self.device {
            Device::UsbdConfig { iso_in } => (offset == 0x530).then_some(*iso_in),
            Device::Temperature { ready, irq_enabled } => match offset {
                0 | 4 => Some(0),
                0x100 => Some(u32::from(*ready)),
                0x304 | 0x308 => Some(u32::from(*irq_enabled)),
                0x508 => Some(25 * 4),
                _ => None,
            },
            Device::Spim3Config(registers) => Some(registers[Self::spim3_config_index(offset)?]),
            Device::PwmConfig { output_pin } => match offset {
                0x500 => Some(0),
                0x560 => Some(*output_pin),
                _ => None,
            },
            Device::Clock(clock) => clock.read(offset),
            Device::Twi(twi) => twi.read(offset),
            Device::Spi(spi) => spi.read(offset),
            Device::Timer(timer) => timer.read(offset),
            Device::I2s(i2s) => i2s.read(offset),
            Device::Pdm(pdm) => pdm.read(offset),
            Device::Radio(radio) => radio.read(offset),
            Device::Rng(rng) => rng.read(offset),
            Device::Ecb(ecb) => ecb.read(offset),
            Device::Aar(aar) => aar.read(offset),
            Device::Egu(egu) => egu.read(offset),
            Device::Nvmc { cache_config } => (offset == 0x540).then_some(*cache_config),
            Device::Gpio(gpio) => gpio.read(offset),
            Device::Gpiote(gpiote) => gpiote.read(offset),
            Device::Ppi(ppi) => ppi.read(offset),
            Device::Watchdog(watchdog) => watchdog.read(offset),
            Device::Uarte(uart) => uart.read(offset),
            Device::Rtc(rtc) => rtc.read(offset),
        }
    }

    pub fn write(
        &mut self,
        address: u32,
        value: u32,
        bus: &mut Bus,
        output: &mut impl Write,
    ) -> Result<(), BusError> {
        let offset = address - self.base;
        let supported = match &mut self.device {
            Device::Temperature { ready, irq_enabled } => {
                match offset {
                    0 => *ready |= value & 1 != 0,
                    4 => {}
                    0x100 => *ready = value & 1 != 0,
                    0x304 => *irq_enabled |= value & 1 != 0,
                    0x308 => *irq_enabled &= value & 1 == 0,
                    _ => return Err(BusError::Unmapped(address, 4)),
                }
                true
            }
            Device::Spim3Config(registers) => {
                if let Some(index) = Self::spim3_config_index(offset)
                    && (offset != 0x500 || matches!(value, 0 | 7))
                {
                    registers[index] = value;
                    true
                } else {
                    false
                }
            }
            Device::UsbdConfig { iso_in } if offset == 0x530 => {
                *iso_in = value & 1;
                true
            }
            Device::UsbdConfig { .. } => false,
            Device::PwmConfig { output_pin } => match offset {
                0x500 => value == 0,
                0x560 => {
                    *output_pin = value;
                    true
                }
                _ => false,
            },
            Device::Clock(clock) => clock.write(offset, value),
            Device::Nvmc { cache_config } => {
                if offset == 0x540 && value & !1 == 0 {
                    *cache_config = value;
                    true
                } else {
                    false
                }
            }
            Device::Gpio(gpio) => {
                let supported = gpio.write(offset, value);
                if supported {
                    bus.sample_nrf_gpio_outputs(self.base, gpio.driven_high());
                }
                supported
            }
            Device::Gpiote(gpiote) => gpiote.write(offset, value, bus),
            Device::Ppi(ppi) => ppi.write(
                offset,
                value,
                |event| bus.nrf_ppi_event_supported(event),
                |task| {
                    (self.base..=self.base + 0x2c).contains(&task) && task & 3 == 0
                        || bus.nrf_ppi_task_supported(task)
                },
            ),
            Device::Watchdog(watchdog) => watchdog.write(offset, value),
            Device::Uarte(uart) => uart.write(offset, value, bus, output)?,
            Device::Rtc(rtc) => rtc.write(offset, value),
            Device::Twi(twi) => twi.write(offset, value),
            Device::Spi(spi) => {
                if offset == 0x51c {
                    spi.sample_cs(bus);
                }
                spi.write(offset, value)
            }
            Device::Timer(timer) => timer.write(offset, value),
            Device::I2s(i2s) => i2s.write(offset, value),
            Device::Pdm(pdm) => pdm.write(offset, value),
            Device::Radio(radio) => radio.write(offset, value, bus)?,
            Device::Rng(rng) => rng.write(offset, value),
            Device::Ecb(ecb) => ecb.write(offset, value, bus),
            Device::Aar(aar) => aar.write(offset, value, bus)?,
            Device::Egu(egu) => egu.write(offset, value),
        };
        if supported {
            Ok(())
        } else {
            Err(BusError::Unmapped(address, 4))
        }
    }
}

fn set_event(events: &mut u32, bit: u32, value: u32) {
    *events = (*events & !bit) | if value & 1 != 0 { bit } else { 0 };
}

impl Clock {
    fn read(&self, offset: u32) -> Option<u32> {
        Some(match offset {
            0x000..=0x010 | 0x018 | 0x078 | 0x07c => 0,
            0x100 | 0x104 | 0x10c => (self.events >> ((offset - 0x100) / 4)) & 1,
            0x304 | 0x308 => self.interrupts,
            0x40c => (self.running & 1) * 0x10001,
            0x414 => self.lf_source_copy,
            0x418 => ((self.running & 2) << 15) | (self.lf_source_copy & 3),
            0x438 => 0, // USB VBUS is disconnected.
            0x518 => self.lf_source,
            0x51c => self.retention,
            0x55c => self.trace_config,
            0x578 => self.dcdc,
            0x580 => self.dcdc0,
            0xe00..=0xe34 => self.ram_priority[((offset - 0xe00) / 4) as usize],
            _ => return None,
        })
    }

    fn write(&mut self, offset: u32, value: u32) -> bool {
        let trigger = value & 1 != 0;
        match offset {
            0x000 | 0x004 | 0x008 | 0x00c => {
                if trigger {
                    let bit = 1 << (offset / 8);
                    let start = u32::from(offset & 4 == 0);
                    if offset == 8 {
                        self.lf_source_copy = self.lf_source;
                    }
                    set_event(&mut self.running, bit, start);
                    set_event(&mut self.events, bit, start);
                }
            }
            0x010 | 0x018 if trigger => set_event(&mut self.events, 8, u32::from(offset == 0x010)),
            0x010 | 0x018 => {}
            0x078 | 0x07c if trigger => self.constant_latency = offset == 0x078,
            0x078 | 0x07c => {}
            0x100 | 0x104 | 0x10c => {
                set_event(&mut self.events, 1 << ((offset - 0x100) / 4), value)
            }
            0x304 => self.interrupts |= value & 0xb,
            0x308 => self.interrupts &= !value,
            0x518 => self.lf_source = value & 0x30003,
            0x51c => self.retention = value & 0xff,
            0x55c => self.trace_config = value & 0x30003,
            0x578 => self.dcdc = value & 1,
            0x580 => self.dcdc0 = value & 1,
            // AMLI RAMPRI shares the POWER/CLOCK page. Bus accesses are serialized.
            0xe00..=0xe34 => self.ram_priority[((offset - 0xe00) / 4) as usize] = value,
            _ => return false,
        }
        true
    }
}

fn dma_address(address: u32, length: usize) -> Result<u32, BusError> {
    const RAM_BASE: u32 = 0x2000_0000;
    const RAM_END: u32 = 0x2004_0000;
    if address < RAM_BASE || u64::from(address) + length as u64 > u64::from(RAM_END) {
        return Err(BusError::Unmapped(address, length));
    }
    Ok(address)
}

impl Uarte {
    fn update_irq(&mut self) {
        self.irq_level = self.events & self.interrupts != 0;
    }

    fn raise(&mut self, events: u32) {
        self.events |= events;
        self.raised |= events;
        self.update_irq();
    }

    const RX_READY: u32 = 1 << 2;
    const END_RX: u32 = 1 << 4;
    const TX_READY: u32 = 1 << 7;
    const END_TX: u32 = 1 << 8;
    const RX_TIMEOUT: u32 = 1 << 17;
    const RX_STARTED: u32 = 1 << 19;
    const TX_STARTED: u32 = 1 << 20;
    const TX_STOPPED: u32 = 1 << 22;

    fn event(offset: u32) -> Option<u32> {
        let bit = 1u32.checked_shl(offset.checked_sub(0x100)? / 4)?;
        (bit & UART_EVENTS != 0).then_some(bit)
    }

    fn read(&mut self, offset: u32) -> Option<u32> {
        Some(match offset {
            0x000 | 0x004 | 0x008 | 0x00c | 0x02c | 0x480 => 0,
            0x200 => self.shorts,
            0x300 | 0x304 | 0x308 => self.interrupts,
            0x500 => self.enabled,
            0x508..=0x514 => self.pins[((offset - 0x508) / 4) as usize],
            0x518 if self.enabled == 4 => {
                let data = self.rx_byte.take().unwrap_or(0);
                self.receive_legacy();
                u32::from(data)
            }
            0x524 => self.baud,
            0x534 => self.rx_pointer,
            0x538 => self.rx_count,
            0x53c => self.rx_amount,
            0x544 => self.tx_pointer,
            0x548 => self.tx_count,
            0x54c => self.tx_amount,
            0x56c => self.config,
            _ => u32::from(self.events & Self::event(offset)? != 0),
        })
    }

    fn write(
        &mut self,
        offset: u32,
        value: u32,
        bus: &mut Bus,
        output: &mut impl Write,
    ) -> Result<bool, BusError> {
        let trigger = value & 1 != 0 && self.enabled == 8;
        match offset {
            0x000 if self.enabled == 4 && value & 1 != 0 => {
                self.legacy_rx = true;
                self.receive(bus)?;
            }
            0x000 if trigger => {
                self.rx_amount = 0;
                self.rx_active = Some((self.rx_pointer, self.rx_count));
                self.raise(Self::RX_STARTED);
                self.receive(bus)?;
            }
            0x004 if value & 1 != 0 => {
                self.rx_active = None;
                self.legacy_rx = false;
                self.raise(Self::RX_TIMEOUT);
                if self.rx_amount < self.rx_count {
                    self.raise(Self::END_RX);
                }
            }
            0x008 if self.enabled == 4 && value & 1 != 0 => self.legacy_tx = true,
            0x00c if self.enabled == 4 && value & 1 != 0 => self.legacy_tx = false,
            0x51c if self.enabled == 4 => {
                if self.legacy_tx {
                    self.transmit(&[value as u8], output)?;
                    self.raise(Self::TX_READY);
                }
            }
            0x008 if trigger => {
                if self.tx_count != 0 {
                    let length = self.tx_count as usize;
                    let address = dma_address(self.tx_pointer, length)?;
                    let bytes = bus.memory_slice(address, length)?;
                    self.transmit(bytes, output)?;
                }
                self.tx_amount = self.tx_count;
                self.raise(Self::TX_STARTED | Self::END_TX | Self::TX_READY);
                if self.tx_count == 0 {
                    self.raise(Self::TX_STOPPED);
                }
            }
            0x00c if trigger => self.raise(Self::TX_STOPPED),
            0x000 | 0x004 | 0x008 | 0x00c | 0x480 => {}
            0x02c if trigger => {
                self.receive(bus)?;
                self.raise(Self::END_RX);
            }
            0x02c => {}
            0x200 => self.shorts = value & 0x60,
            0x300 | 0x304 | 0x308 => {
                self.interrupts = match offset {
                    0x300 => value & UART_EVENTS,
                    0x304 => self.interrupts | (value & UART_EVENTS),
                    _ => self.interrupts & !value,
                };
                self.update_irq();
            }
            0x500 if matches!(value, 0 | 4 | 8) => {
                self.enabled = value;
                if value == 0 {
                    self.rx_active = None;
                    self.legacy_rx = false;
                    self.legacy_tx = false;
                    self.rx_byte = None;
                    self.input.clear();
                }
            }
            0x508..=0x514 => self.pins[((offset - 0x508) / 4) as usize] = value,
            0x524 => self.baud = value,
            0x534 => self.rx_pointer = value,
            0x538 => self.rx_count = value & 0xffff,
            0x544 => self.tx_pointer = value,
            0x548 => self.tx_count = value & 0xffff,
            0x56c => self.config = value,
            _ => match Self::event(offset) {
                Some(bit) => {
                    set_event(&mut self.events, bit, value);
                    self.update_irq();
                    if bit == Self::RX_READY && self.legacy_rx && value == 0 {
                        self.receive(bus)?;
                    }
                }
                None => return Ok(false),
            },
        }
        Ok(true)
    }

    fn transmit(&mut self, bytes: &[u8], output: &mut impl Write) -> Result<(), BusError> {
        if let Some(link) = &mut self.link_output {
            link.extend_from_slice(bytes);
            Ok(())
        } else {
            output.write_all(bytes).map_err(BusError::Output)
        }
    }

    fn receive_legacy(&mut self) {
        if self.legacy_rx
            && self.rx_byte.is_none()
            && self.events & Self::RX_READY == 0
            && let Some(byte) = self.input.pop_front()
        {
            self.rx_byte = Some(byte);
            self.raise(Self::RX_READY);
        }
    }

    fn receive(&mut self, bus: &mut Bus) -> Result<(), BusError> {
        self.receive_legacy();
        let Some((pointer, count)) = self.rx_active else {
            return Ok(());
        };
        let length = ((count - self.rx_amount) as usize).min(self.input.len());
        if length != 0 {
            let target = pointer
                .checked_add(self.rx_amount)
                .ok_or(BusError::Unmapped(pointer, length))?;
            let target = dma_address(target, length)?;
            for (destination, byte) in bus
                .memory(target, length)?
                .iter_mut()
                .zip(self.input.drain(..length))
            {
                *destination = byte;
            }
            self.rx_amount += length as u32;
            self.raise(Self::RX_READY);
        }
        if self.rx_amount == count {
            self.rx_active = None;
            self.raise(Self::END_RX);
            if self.shorts & 0x40 != 0 {
                self.raise(Self::RX_TIMEOUT);
            } else if self.shorts & 0x20 != 0 && self.rx_count != 0 {
                self.rx_amount = 0;
                self.rx_active = Some((self.rx_pointer, self.rx_count));
                self.raise(Self::RX_STARTED);
            }
        }
        Ok(())
    }
}

struct Rtc {
    channels: usize,
    running: bool,
    phase: u64,
    counter: u32,
    prescaler: u32,
    events: u32,
    interrupts: u32,
    compare: [u32; 4],
    event_enabled: u32,
    raised: u32,
}

impl Default for Rtc {
    fn default() -> Self {
        Self {
            channels: 0,
            running: false,
            phase: 0,
            counter: 0,
            prescaler: 0,
            events: 0,
            interrupts: 0,
            compare: [Self::MASK; 4],
            event_enabled: 0,
            raised: 0,
        }
    }
}

impl Rtc {
    const MASK: u32 = 0x00ff_ffff;

    fn tick(&mut self) {
        if !self.running {
            return;
        }
        let period = crate::bus::CORTEX_M_STEP_HZ * (u64::from(self.prescaler) + 1);
        self.phase += 32_768;
        if self.phase >= period {
            self.phase -= period;
            self.counter = (self.counter + 1) & Self::MASK;
            let mut reached = 1 | (u32::from(self.counter == 0) << 1);
            for (index, compare) in self.compare[..self.channels].iter().enumerate() {
                if self.counter == *compare {
                    reached |= 1 << (16 + index);
                }
            }
            self.events |= reached;
            self.raised |= reached & self.event_enabled;
        }
    }

    fn event_bit(&self, offset: u32) -> Option<u32> {
        match offset {
            0x100 => Some(1),
            0x104 => Some(2),
            0x140..=0x14c if ((offset - 0x140) / 4) < self.channels as u32 => {
                Some(1 << (16 + (offset - 0x140) / 4))
            }
            _ => None,
        }
    }

    fn read(&self, offset: u32) -> Option<u32> {
        Some(match offset {
            0x000 | 0x004 | 0x008 | 0x00c => 0,
            0x304 | 0x308 => self.interrupts,
            0x340 | 0x344 | 0x348 => self.event_enabled,
            0x504 => self.counter,
            0x508 => self.prescaler,
            0x540..=0x54c if ((offset - 0x540) / 4) < self.channels as u32 => {
                self.compare[((offset - 0x540) / 4) as usize]
            }
            _ => u32::from(self.events & self.event_bit(offset)? != 0),
        })
    }

    fn write(&mut self, offset: u32, value: u32) -> bool {
        let mask = ((1 << self.channels) - 1) << 16 | 3;
        match offset {
            0x000 if value & 1 != 0 => self.running = true,
            0x004 if value & 1 != 0 => self.running = false,
            0x008 | 0x00c if value & 1 != 0 => {
                self.counter = if offset == 0x008 { 0 } else { Self::MASK - 15 };
                self.phase = 0;
            }
            0x000 | 0x004 | 0x008 | 0x00c => {}
            0x304 => self.interrupts |= value & mask,
            0x308 => self.interrupts &= !value,
            0x340 => self.event_enabled = value & mask,
            0x344 => self.event_enabled |= value & mask,
            0x348 => self.event_enabled &= !value,
            0x508 => {
                self.prescaler = value & 0xfff;
                self.phase = 0;
            }
            0x540..=0x54c if ((offset - 0x540) / 4) < self.channels as u32 => {
                let index = ((offset - 0x540) / 4) as usize;
                self.compare[index] = value & Self::MASK;
                self.events &= !(1 << (16 + index));
            }
            _ => match self.event_bit(offset) {
                Some(bit) => set_event(&mut self.events, bit, value),
                None => return false,
            },
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn easy_dma_transfers_and_signals_uart1_on_the_second_nvic_bank() {
        let mut bus = Bus::default();
        let ram = 0x2000_0000;
        let alias = 0x0080_0000;
        let uart = 0x4002_8000;
        let nvic = crate::nvic::BASE;
        let mut output = Vec::new();
        bus.map_ram(ram, 0x40000).unwrap();
        bus.map_ram_alias(ram, alias, 0x40000).unwrap();
        assert!(bus.map_ram(alias, 4).is_err());
        bus.map_nrf_peripheral(Peripheral::uarte(uart, 40, "uart1".into()))
            .unwrap();
        bus.map_nvic(0xe0, None, true).unwrap();
        bus.memory(alias, 3).unwrap().copy_from_slice(b"abc");
        for (address, value) in [
            (nvic + 0x104, 1 << 8),
            (uart + 0x500, 8),
            (uart + 0x544, ram),
            (uart + 0x548, 3),
            (uart + 0x304, Uarte::END_TX | Uarte::END_RX),
            (uart + 0x008, 1),
        ] {
            bus.write(address, 4, value, &mut output).unwrap();
        }
        assert_eq!(output, b"abc");
        assert_eq!(bus.read::<4>(uart + 0x54c).unwrap(), 3);
        bus.sample_nvic(0, false);
        assert_eq!(bus.nvic.as_ref().unwrap().next_irq(0), Some((40, 0)));
        for (address, value) in [
            (uart + 0x120, 0),
            (nvic + 0x284, 1 << 8),
            (uart + 0x534, ram + 8),
            (uart + 0x538, 2),
            (uart, 1),
        ] {
            bus.write(address, 4, value, &mut output).unwrap();
        }
        bus.sample_nvic(0, false);
        assert_eq!(bus.nvic.as_ref().unwrap().next_irq(0), None);
        bus.queue_uart_input(b"XY".to_vec()).unwrap();
        assert_eq!(
            bus.read::<2>(alias + 8).unwrap(),
            u32::from(u16::from_le_bytes(*b"XY"))
        );
        assert_eq!(bus.read::<4>(uart + 0x53c).unwrap(), 2);
        bus.sample_nvic(0, false);
        assert_eq!(bus.nvic.as_ref().unwrap().next_irq(0), Some((40, 0)));
        assert!(bus.read::<4>(uart + 0x600).is_err());
    }

    #[test]
    fn gpiote_tasks_drive_the_selected_port_and_a_sense_event_sets_irq() {
        let mut bus = Bus::default();
        let p0 = 0x5000_0500;
        let p1 = 0x5000_0800;
        let gpiote = 0x4000_6000;
        bus.map_nrf_peripheral(Peripheral::gpio(p0, "gpio0".into(), 32))
            .unwrap();
        bus.map_nrf_peripheral(Peripheral::gpio(p1, "gpio1".into(), 16))
            .unwrap();
        bus.map_nrf_peripheral(Peripheral::gpiote(gpiote, 6, "gpiote".into(), [p0, p1]))
            .unwrap();
        bus.map_nvic(0xe0, None, true).unwrap();
        let mut output = Vec::new();
        for (address, value) in [
            (crate::nvic::BASE + 0x100, 1 << 6),
            (gpiote + 0x304, 1 << 31),
            (gpiote + 0x510, (1 << 13) | (2 << 8) | 3),
            (p1 + 0x208, (2 << 16) | 1),
            (gpiote + 0x030, 1),
        ] {
            bus.write(address, 4, value, &mut output).unwrap();
        }
        assert_eq!(bus.read::<4>(p0 + 4).unwrap(), 0);
        assert_eq!(bus.read::<4>(p1 + 4).unwrap(), 4);
        bus.sample_nvic(0, false);
        assert_eq!(bus.read::<4>(gpiote + 0x17c).unwrap(), 1);
        assert_eq!(bus.nvic.as_ref().unwrap().next_irq(0), Some((6, 0)));
    }

    #[test]
    fn rtc_event_starts_timer_and_fixed_ppi_channel_starts_radio_rx() {
        let mut bus = Bus::default();
        let rtc = 0x4000_b000;
        let timer = 0x4000_8000;
        let ppi = 0x4001_f000;
        let radio = 0x4000_1000;
        let mut output = Vec::new();
        bus.map_nvic(0xe0, None, true).unwrap();
        bus.map_nrf_peripheral(Peripheral::rtc(rtc, 11, "rtc0".into(), 3))
            .unwrap();
        bus.map_nrf_peripheral(Peripheral::timer(timer, 8, "timer0".into(), 4))
            .unwrap();
        bus.map_nrf_peripheral(Peripheral::ppi(ppi, "ppi".into()))
            .unwrap();
        bus.map_nrf_peripheral(Peripheral::radio(radio, 1, "radio".into(), false))
            .unwrap();
        for (address, value) in [
            (rtc + 0x540, 1),
            (rtc + 0x344, 1 << 16),
            (ppi + 0x510, rtc + 0x140),
            (ppi + 0x514, timer),
            (ppi + 0x504, 1 | (1 << 21)),
            (timer + 0x510, 0),
            (timer + 0x540, 1),
            (rtc, 1),
        ] {
            bus.write(address, 4, value, &mut output).unwrap();
        }
        assert!(bus.write(ppi + 0x504, 4, 1 << 23, &mut output).is_err());
        for _ in 0..3100 {
            bus.advance_nrf_ppi(&mut output).unwrap();
            bus.sample_nvic(0, false);
        }
        assert_eq!(bus.read::<4>(radio + 0x158).unwrap(), 1);
    }

    #[test]
    fn twi_nack_can_trigger_stop_through_ppi() {
        let mut bus = Bus::default();
        let twi = 0x4000_3000;
        let ppi = 0x4001_f000;
        let mut output = Vec::new();
        bus.map_nrf_peripheral(Peripheral::twi(twi, 3, "twi0".into()))
            .unwrap();
        bus.map_nrf_peripheral(Peripheral::ppi(ppi, "ppi".into()))
            .unwrap();
        for (address, value) in [
            (ppi + 0x510, twi + 0x124),
            (ppi + 0x514, twi + 0x014),
            (ppi + 0x504, 1),
            (twi + 0x500, 5),
            (twi + 0x008, 1),
        ] {
            bus.write(address, 4, value, &mut output).unwrap();
        }
        assert_eq!(bus.read::<4>(twi + 0x4c4).unwrap(), 2);
        assert_eq!(bus.read::<4>(twi + 0x104).unwrap(), 0);
        bus.advance_nrf_ppi(&mut output).unwrap();
        assert_eq!(bus.read::<4>(twi + 0x104).unwrap(), 1);
        assert!(bus.write(twi + 0x500, 4, 6, &mut output).is_err());
    }

    #[test]
    fn ppi_routes_a_fresh_dma_event_to_stop_tx() {
        let mut bus = Bus::default();
        let uart = 0x4000_2000;
        let ppi = 0x4001_f000;
        let ram = 0x2000_0000;
        let mut output = Vec::new();
        bus.map_ram(ram, 8).unwrap();
        bus.memory(ram, 1).unwrap()[0] = b'A';
        bus.map_nrf_peripheral(Peripheral::uarte(uart, 2, "uart0".into()))
            .unwrap();
        bus.map_nrf_peripheral(Peripheral::ppi(ppi, "ppi".into()))
            .unwrap();
        assert!(bus.write(ppi + 0x510, 4, 0x4000_b140, &mut output).is_err());
        for (address, value) in [
            (ppi + 0x510, uart + 0x120),
            (ppi + 0x514, uart + 0x00c),
            (ppi + 0x504, 1),
            (uart + 0x500, 8),
            (uart + 0x544, ram),
            (uart + 0x548, 1),
        ] {
            bus.write(address, 4, value, &mut output).unwrap();
        }
        for _ in 0..2 {
            bus.write(uart + 0x008, 4, 1, &mut output).unwrap();
            assert_eq!(bus.read::<4>(uart + 0x158).unwrap(), 0);
            bus.advance_nrf_ppi(&mut output).unwrap();
            assert_eq!(bus.read::<4>(uart + 0x158).unwrap(), 1);
            bus.write(uart + 0x158, 4, 0, &mut output).unwrap();
        }
        assert_eq!(output, b"AA");
    }

    #[test]
    fn watchdog_reset_clears_peripheral_state_and_preserves_ram() {
        let mut bus = Bus::default();
        let wdt = 0x4001_0000;
        let ram = 0x2000_0000;
        bus.map_ram(ram, 4).unwrap();
        bus.map_nvic(0xe0, None, true).unwrap();
        bus.map_nrf_peripheral(Peripheral::watchdog(wdt, 16, "wdt".into()))
            .unwrap();
        bus.memory(ram, 1).unwrap()[0] = 42;
        let mut output = Vec::new();
        bus.write(wdt + 0x504, 4, 15, &mut output).unwrap();
        bus.write(wdt, 4, 1, &mut output).unwrap();
        for _ in 0..48_828 {
            bus.sample_nvic(0, false);
        }
        assert!(!bus.reset_cortex_m_if_requested(false));
        bus.sample_nvic(0, false);
        assert!(bus.reset_cortex_m_if_requested(false));
        assert_eq!(bus.read::<4>(wdt + 0x400).unwrap(), 0);
        assert_eq!(bus.read::<1>(ram).unwrap(), 42);
        assert!(!bus.reset_cortex_m_if_requested(false));
    }

    #[test]
    fn clock_and_rtc_latch_events_and_gate_interrupts() {
        let mut clock = Clock::default();
        assert!(clock.write(0x008, 0));
        assert_eq!(clock.read(0x104), Some(0));
        assert!(clock.write(0x518, 1));
        assert!(clock.write(0x008, 1));
        assert_eq!(clock.read(0x418), Some(0x10001));
        assert_eq!(clock.read(0x104), Some(1));
        assert_eq!(clock.events & clock.interrupts, 0);
        assert!(clock.write(0x304, 2));
        assert_eq!(clock.events & clock.interrupts, 2);
        assert!(clock.write(0x104, 0));
        assert_eq!(clock.read(0x418), Some(0x10001));
        assert_eq!(clock.events & clock.interrupts, 0);

        let mut rtc = Rtc {
            channels: 3,
            ..Rtc::default()
        };
        assert!(rtc.write(0x508, 1));
        assert!(rtc.write(0x540, 1));
        assert!(rtc.write(0x304, 1 << 16));
        assert!(rtc.write(0, 1));
        for _ in 0..6103 {
            rtc.tick();
        }
        assert_eq!(rtc.read(0x504), Some(0));
        rtc.tick();
        assert_eq!(rtc.read(0x504), Some(1));
        assert_eq!(rtc.events & rtc.interrupts, 1 << 16);
        assert!(rtc.write(0x140, 0));
        assert_eq!(rtc.events & rtc.interrupts, 0);
        assert_eq!(rtc.read(0x54c), None);
        assert!(!rtc.write(0x14c, 1));
    }
}
