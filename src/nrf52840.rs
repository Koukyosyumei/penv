mod twi;
use twi::Twi;

mod gpio;
mod ppi;

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
    Twi(Twi),
    Watchdog(Watchdog),
    Gpio(Gpio),
    Gpiote(Gpiote),
    Ppi(Box<Ppi>),
    Nvmc { cache_config: u32 },
}

#[derive(Default)]
struct Clock {
    running: u32,
    events: u32,
    interrupts: u32,
    lf_source: u32,
    dcdc: u32,
}

struct Uarte {
    enabled: u32,
    events: u32,
    raised: u32,
    interrupts: u32,
    pins: [u32; 4],
    baud: u32,
    config: u32,
    rx_pointer: u32,
    rx_count: u32,
    rx_amount: u32,
    rx_active: Option<(u32, u32)>,
    tx_pointer: u32,
    tx_count: u32,
    tx_amount: u32,
    input: VecDeque<u8>,
}

impl Default for Uarte {
    fn default() -> Self {
        Self {
            enabled: 0,
            events: 0,
            raised: 0,
            interrupts: 0,
            pins: [u32::MAX; 4],
            baud: 0x0400_0000,
            config: 0,
            rx_pointer: 0,
            rx_count: 0,
            rx_amount: 0,
            rx_active: None,
            tx_pointer: 0,
            tx_count: 0,
            tx_amount: 0,
            input: VecDeque::new(),
        }
    }
}

impl Peripheral {
    pub const SIZE: u32 = 0x1000;

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

    pub fn twi(base: u32, irq: u8, name: String) -> Self {
        Self {
            base,
            irq,
            name,
            device: Device::Twi(Twi::default()),
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
        twi.attach(address, target)
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
            && match self.device {
                Device::Uarte(_) => Uarte::event(address - self.base).is_some(),
                Device::Twi(_) => Twi::event(address - self.base).is_some(),
                _ => false,
            }
    }

    pub fn ppi_task_supported(&self, address: u32) -> bool {
        self.contains(address)
            && match self.device {
                Device::Uarte(_) => matches!(address - self.base, 0 | 4 | 8 | 12),
                Device::Twi(_) => Twi::is_task(address - self.base),
                Device::Ppi(_) => address - self.base <= 0x2c && address & 3 == 0,
                _ => false,
            }
    }

    pub fn take_raised_events(&mut self, events: &mut Vec<u32>) {
        let mut raised = match &mut self.device {
            Device::Uarte(uart) => std::mem::take(&mut uart.raised),
            Device::Twi(twi) => std::mem::take(&mut twi.raised),
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
            Device::Watchdog(watchdog) => watchdog.tick(sleeping),
            _ => {}
        }
    }

    pub fn reset_requested(&self) -> bool {
        matches!(&self.device, Device::Watchdog(watchdog) if watchdog.reset_requested)
    }

    pub fn is_uart(&self) -> bool {
        matches!(self.device, Device::Uarte(_))
    }

    pub fn reset(&mut self) {
        match &mut self.device {
            Device::Gpio(gpio) => gpio.reset(),
            Device::Twi(twi) => twi.reset(),
            Device::Gpiote(gpiote) => gpiote.reset(),
            Device::Ppi(ppi) => **ppi = Ppi::default(),
            Device::Clock(clock) => *clock = Clock::default(),
            Device::Nvmc { cache_config } => *cache_config = 0,
            Device::Watchdog(watchdog) => *watchdog = Watchdog::default(),
            Device::Uarte(uart) => *uart = Uarte::default(),
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
            Device::Nvmc { .. } | Device::Gpio(_) | Device::Ppi(_) => false,
            Device::Watchdog(watchdog) => watchdog.pending(),
            Device::Uarte(uart) => uart.events & uart.interrupts != 0,
            Device::Gpiote(gpiote) => gpiote.pending(),
            Device::Rtc(rtc) => rtc.events & rtc.interrupts != 0,
            Device::Twi(twi) => twi.pending(),
        }
    }

    pub fn queue_input(&mut self, bytes: Vec<u8>) {
        if let Device::Uarte(uart) = &mut self.device
            && uart.rx_active.is_some()
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
            Device::Clock(clock) => clock.read(offset),
            Device::Twi(twi) => twi.read(offset),
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
            Device::Clock(clock) => clock.write(offset, value),
            Device::Nvmc { cache_config } => {
                if offset == 0x540 && value & !1 == 0 {
                    *cache_config = value;
                    true
                } else {
                    false
                }
            }
            Device::Gpio(gpio) => gpio.write(offset, value),
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
            0x000..=0x010 | 0x018 => 0,
            0x100 | 0x104 | 0x10c => (self.events >> ((offset - 0x100) / 4)) & 1,
            0x304 | 0x308 => self.interrupts,
            0x40c => (self.running & 1) * 0x10001,
            0x418 => ((self.running & 2) << 15) | (self.lf_source & 3),
            0x518 => self.lf_source,
            0x578 => self.dcdc,
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
                    set_event(&mut self.running, bit, start);
                    set_event(&mut self.events, bit, start);
                }
            }
            0x010 | 0x018 if trigger => set_event(&mut self.events, 8, u32::from(offset == 0x010)),
            0x010 | 0x018 => {}
            0x100 | 0x104 | 0x10c => {
                set_event(&mut self.events, 1 << ((offset - 0x100) / 4), value)
            }
            0x304 => self.interrupts |= value & 0xb,
            0x308 => self.interrupts &= !value,
            0x518 => self.lf_source = value & 0x30003,
            0x578 => self.dcdc = value & 1,
            _ => return false,
        }
        true
    }
}

impl Uarte {
    fn raise(&mut self, events: u32) {
        self.events |= events;
        self.raised |= events;
    }

    fn dma_address(address: u32, length: usize) -> Result<u32, BusError> {
        const RAM_BASE: u32 = 0x2000_0000;
        const RAM_END: u32 = 0x2004_0000;
        if address < RAM_BASE || u64::from(address) + length as u64 > u64::from(RAM_END) {
            return Err(BusError::Unmapped(address, length));
        }
        Ok(address)
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

    fn read(&self, offset: u32) -> Option<u32> {
        Some(match offset {
            0x000 | 0x004 | 0x008 | 0x00c | 0x200 | 0x480 => 0,
            0x300 | 0x304 | 0x308 => self.interrupts,
            0x500 => self.enabled,
            0x508..=0x514 => self.pins[((offset - 0x508) / 4) as usize],
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
            0x000 if trigger => {
                self.rx_amount = 0;
                self.rx_active = Some((self.rx_pointer, self.rx_count));
                self.raise(Self::RX_STARTED);
                self.receive(bus)?;
            }
            0x004 if trigger => {
                self.rx_active = None;
                self.raise(Self::END_RX | Self::RX_TIMEOUT);
            }
            0x008 if trigger => {
                if self.tx_count != 0 {
                    let length = self.tx_count as usize;
                    let address = Self::dma_address(self.tx_pointer, length)?;
                    let bytes = bus.memory_slice(address, length)?;
                    output.write_all(bytes).map_err(BusError::Output)?;
                }
                self.tx_amount = self.tx_count;
                self.raise(Self::TX_STARTED | Self::END_TX | Self::TX_READY);
                if self.tx_count == 0 {
                    self.raise(Self::TX_STOPPED);
                }
            }
            0x00c if trigger => self.raise(Self::TX_STOPPED),
            0x000 | 0x004 | 0x008 | 0x00c | 0x480 => {}
            0x200 if value == 0 => {}
            0x300 => self.interrupts = value & UART_EVENTS,
            0x304 => self.interrupts |= value & UART_EVENTS,
            0x308 => self.interrupts &= !value,
            0x500 if matches!(value, 0 | 8) => {
                self.enabled = value;
                if value == 0 {
                    self.rx_active = None;
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
                Some(bit) => set_event(&mut self.events, bit, value),
                None => return Ok(false),
            },
        }
        Ok(true)
    }

    fn receive(&mut self, bus: &mut Bus) -> Result<(), BusError> {
        let Some((pointer, count)) = self.rx_active else {
            return Ok(());
        };
        let length = ((count - self.rx_amount) as usize).min(self.input.len());
        if length != 0 {
            let target = pointer
                .checked_add(self.rx_amount)
                .ok_or(BusError::Unmapped(pointer, length))?;
            let target = Self::dma_address(target, length)?;
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
            self.events |= 1 | (u32::from(self.counter == 0) << 1);
            for (index, compare) in self.compare[..self.channels].iter().enumerate() {
                if self.counter == *compare {
                    self.events |= 1 << (16 + index);
                }
            }
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
