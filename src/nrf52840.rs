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
}

#[derive(Default)]
struct Clock {
    running: u32,
    events: u32,
    interrupts: u32,
    lf_source: u32,
}

struct Uarte {
    enabled: u32,
    events: u32,
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

    pub fn tick(&mut self) {
        if let Device::Rtc(rtc) = &mut self.device {
            rtc.tick();
        }
    }

    pub fn is_uart(&self) -> bool {
        matches!(self.device, Device::Uarte(_))
    }

    pub fn reset(&mut self) {
        match &mut self.device {
            Device::Clock(clock) => *clock = Clock::default(),
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
        address.wrapping_sub(self.base) < Self::SIZE
    }

    pub fn pending(&self) -> bool {
        match &self.device {
            Device::Clock(clock) => clock.events & clock.interrupts != 0,
            Device::Uarte(uart) => uart.events & uart.interrupts != 0,
            Device::Rtc(rtc) => rtc.events & rtc.interrupts != 0,
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

    pub fn read(&self, address: u32) -> Option<u32> {
        let offset = address - self.base;
        match &self.device {
            Device::Clock(clock) => clock.read(offset),
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
            Device::Uarte(uart) => uart.write(offset, value, bus, output)?,
            Device::Rtc(rtc) => rtc.write(offset, value),
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
            _ => return false,
        }
        true
    }
}

impl Uarte {
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
                self.events |= Self::RX_STARTED;
                self.receive(bus)?;
            }
            0x004 if trigger => {
                self.rx_active = None;
                self.events |= Self::END_RX | Self::RX_TIMEOUT;
            }
            0x008 if trigger => {
                if self.tx_count != 0 {
                    let length = self.tx_count as usize;
                    let address = Self::dma_address(self.tx_pointer, length)?;
                    let bytes = bus.memory_slice(address, length)?;
                    output.write_all(bytes).map_err(BusError::Output)?;
                }
                self.tx_amount = self.tx_count;
                self.events |= Self::TX_STARTED | Self::END_TX | Self::TX_READY | Self::TX_STOPPED;
            }
            0x00c if trigger => self.events |= Self::TX_STOPPED,
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
            self.events |= Self::RX_READY;
        }
        if self.rx_amount == count {
            self.rx_active = None;
            self.events |= Self::END_RX;
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
        bus.sample_nvic(0);
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
        bus.sample_nvic(0);
        assert_eq!(bus.nvic.as_ref().unwrap().next_irq(0), None);
        bus.queue_uart_input(b"XY".to_vec()).unwrap();
        assert_eq!(
            bus.read::<2>(alias + 8).unwrap(),
            u32::from(u16::from_le_bytes(*b"XY"))
        );
        assert_eq!(bus.read::<4>(uart + 0x53c).unwrap(), 2);
        bus.sample_nvic(0);
        assert_eq!(bus.nvic.as_ref().unwrap().next_irq(0), Some((40, 0)));
        assert!(bus.read::<4>(uart + 0x600).is_err());
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
