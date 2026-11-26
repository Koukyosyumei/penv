use std::{cell::RefCell, rc::Rc};

use crate::nrf52840::bme280::Bme280;

const ENABLE: u32 = 1;
const TX_READY: u32 = 1 << 1;
const RX_READY: u32 = 1 << 2;
const NACK: u32 = 1 << 4;
const STOPPED: u32 = 1 << 5;
const COMPLETE: u32 = 1 << 6;
const RELOAD_READY: u32 = 1 << 7;
const READ: u32 = 1 << 10;
const START: u32 = 1 << 13;
const STOP: u32 = 1 << 14;
const BUSY: u32 = 1 << 15;
const RELOAD: u32 = 1 << 24;
const AUTO_STOP: u32 = 1 << 25;

pub struct Stm32I2c {
    pub base: u32,
    pub(crate) name: Option<String>,
    pub irq: Option<crate::stm32_uart::Stm32Irq>,
    pub dma_rx: u8,
    pub dma_tx: u8,
    registers: [u32; 11],
    status: u32,
    targets: Vec<(u32, Target)>,
    addressed: Option<usize>,
    remaining: u32,
    reading: bool,
}

pub struct Eeprom {
    data: Vec<u8>,
    pointer: usize,
    address_bytes: usize,
    address_remaining: usize,
    address_latch: usize,
    page_size: usize,
    writable: bool,
}

impl Eeprom {
    pub fn new(size: usize, address_bits: usize, writable: bool, page_size: usize) -> Self {
        Self {
            data: vec![0xff; size],
            pointer: 0,
            address_bytes: address_bits / 8,
            address_remaining: 0,
            address_latch: 0,
            page_size,
            writable,
        }
    }

    pub(crate) fn start_write(&mut self) {
        self.address_remaining = self.address_bytes;
        self.address_latch = 0;
    }

    pub(crate) fn write(&mut self, value: u8) {
        if self.address_remaining != 0 {
            self.address_remaining -= 1;
            self.address_latch = (self.address_latch << 8) | usize::from(value);
            if self.address_remaining == 0 {
                self.pointer = self.address_latch % self.data.len();
            }
        } else if self.writable {
            self.data[self.pointer] = value;
            let page = self.pointer & !(self.page_size - 1);
            let end = (page + self.page_size).min(self.data.len());
            self.pointer = if self.pointer + 1 < end {
                self.pointer + 1
            } else {
                page
            };
        }
    }

    pub(crate) fn read(&mut self) -> u8 {
        let value = self.data[self.pointer];
        self.pointer = (self.pointer + 1) % self.data.len();
        value
    }
}

enum Target {
    Eeprom(Eeprom),
    Fram(Rc<RefCell<Fram>>, bool),
    Sht45(String, Sht45),
    Bme280(String, Box<Bme280>),
    Lis2dw12(String, Box<crate::lis2dw12::Lis2dw12>),
}

pub(crate) struct Fram(Eeprom);

impl Fram {
    pub(crate) fn new() -> Self {
        const SIZE: usize = 128 * 1024;
        let mut memory = Eeprom::new(SIZE, 16, true, SIZE);
        memory.data.fill(0);
        Self(memory)
    }

    pub(crate) fn set_write_protected(&mut self, protected: bool) {
        self.0.writable = !protected;
    }
}

#[derive(Default)]
pub(crate) struct Sht45 {
    temperature: f64,
    humidity: f64,
    serial: u32,
    response: std::collections::VecDeque<u8>,
}

impl Sht45 {
    fn write(&mut self, command: u8) {
        let words = match command {
            0x15 | 0x1e | 0x24 | 0x2f | 0x32 | 0x39 | 0xe0 | 0xf6 | 0xfd => [
                ((self.temperature + 45.0) * 65535.0 / 175.0)
                    .round_ties_even().clamp(0.0, 65535.0) as u16,
                ((self.humidity + 6.0) * 65535.0 / 125.0)
                    .round_ties_even().clamp(0.0, 65535.0) as u16,
            ],
            0x89 => [(self.serial >> 16) as u16, self.serial as u16],
            0x94 => {
                self.response.clear();
                return;
            }
            _ => return,
        };
        self.response.clear();
        for word in words {
            let bytes = word.to_be_bytes();
            let crc = bytes.into_iter().fold(0xffu8, |crc, byte| {
                (0..8).fold(crc ^ byte, |crc, _| {
                    (crc << 1) ^ if crc & 0x80 != 0 { 0x31 } else { 0 }
                })
            });
            self.response.extend([bytes[0], bytes[1], crc]);
        }
    }

    pub(crate) fn set_property(&mut self, property: &str, value: &str) -> Result<(), String> {
        let measurement = || {
            value.parse::<f64>().ok().filter(|value| value.is_finite())
                .ok_or_else(|| format!("invalid SHT45 {property} value {value:?}"))
        };
        match property {
            "Temperature" => self.temperature = measurement()?,
            "Humidity" => self.humidity = measurement()?,
            "SerialNumber" => self.serial = crate::parse_address(value)?,
            _ => return Err(format!("unknown SHT45 property {property:?}")),
        }
        Ok(())
    }
}

impl Target {
    fn start_write(&mut self) {
        match self {
            Self::Eeprom(memory) => memory.start_write(),
            Self::Fram(memory, high) => {
                let mut memory = memory.borrow_mut();
                memory.0.start_write();
                memory.0.address_latch = usize::from(*high);
            }
            Self::Sht45(..) => {}
            Self::Bme280(_, sensor) => sensor.start_write(),
            Self::Lis2dw12(_, sensor) => sensor.start_write(),
        }
    }

    fn read(&mut self, now_ns: u64) -> u8 {
        match self {
            Self::Eeprom(memory) => memory.read(),
            Self::Fram(memory, _) => memory.borrow_mut().0.read(),
            Self::Sht45(_, sensor) => sensor.response.pop_front().unwrap_or(0),
            Self::Bme280(_, sensor) => sensor.read(),
            Self::Lis2dw12(name, sensor) => sensor.read(name, now_ns),
        }
    }

    fn write(&mut self, value: u8) {
        match self {
            Self::Eeprom(memory) => memory.write(value),
            Self::Fram(memory, _) => memory.borrow_mut().0.write(value),
            Self::Sht45(_, sensor) => sensor.write(value),
            Self::Bme280(_, sensor) => sensor.write(value),
            Self::Lis2dw12(_, sensor) => sensor.write(value),
        }
    }
}

impl Stm32I2c {
    pub const SIZE: u32 = 0x400;

    pub fn new(base: u32) -> Self {
        Self {
            base,
            name: None,
            irq: None,
            dma_rx: 0,
            dma_tx: 0,
            registers: [0; 11],
            status: 0,
            targets: Vec::new(),
            addressed: None,
            remaining: 0,
            reading: false,
        }
    }

    pub fn attach(&mut self, address: u32, target: Eeprom) -> Result<(), String> {
        self.attach_target(address, Target::Eeprom(target))
    }

    pub(crate) fn attach_fram(&mut self, address: u32, fram: Rc<RefCell<Fram>>, high: bool) -> Result<(), String> {
        self.attach_target(address, Target::Fram(fram, high))
    }

    pub(crate) fn attach_sensor(&mut self, address: u32, name: String, model: &str) -> Result<(), String> {
        let target = match model {
            "I2C.SHT45" => Target::Sht45(name, Sht45::default()),
            "I2C.BME280" => Target::Bme280(name, Box::default()),
            "Sensors.LIS2DW12" => Target::Lis2dw12(format!("{}.{}", self.name.as_deref().unwrap_or_default(), name), Box::default()),
            _ => return Err(format!("unsupported STM32 I²C sensor {model:?}")),
        };
        self.attach_target(address, target)
    }

    pub(crate) fn lis2dw12(&mut self, path: &str) -> Option<&mut crate::lis2dw12::Lis2dw12> {
        self.targets.iter_mut().find_map(|(_, target)| match target {
            Target::Lis2dw12(name, sensor) if name == path => Some(sensor.as_mut()),
            _ => None,
        })
    }

    pub(crate) fn set_sensor_property(&mut self, name: &str, property: &str, value: &str) -> Result<(), String> {
        for (_, target) in &mut self.targets {
            match target {
                Target::Sht45(sensor_name, sensor) if sensor_name == name => return sensor.set_property(property, value),
                Target::Bme280(sensor_name, sensor) if sensor_name == name => return sensor.set_property(property, value),
                _ => {}
            }
        }
        Err(format!("unknown I²C sensor {name:?}"))
    }

    fn attach_target(&mut self, address: u32, target: Target) -> Result<(), String> {
        if address >= 1024 || self.targets.iter().any(|&(other, _)| other == address) {
            return Err(format!(
                "invalid or duplicate I²C target address {address:#x}"
            ));
        }
        self.targets.push((address, target));
        Ok(())
    }

    pub fn reset(&mut self) {
        self.registers = [0; 11];
        self.status = 0;
        self.addressed = None;
        self.remaining = 0;
        self.reading = false;
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    pub fn interrupt_pending(&self) -> bool {
        let irqs = self.registers[0];
        self.status & irqs & (TX_READY | RX_READY | NACK | STOPPED | COMPLETE) != 0
            || (self.status & RELOAD_READY != 0 && irqs & COMPLETE != 0)
    }

    pub fn pending_dma(&self) -> u8 {
        let control = self.registers[0];
        let rx = if control & (1 << 15) != 0 && self.status & RX_READY != 0 {
            self.dma_rx
        } else {
            0
        };
        let tx = if control & (1 << 14) != 0 && self.status & TX_READY != 0 {
            self.dma_tx
        } else {
            0
        };
        rx | tx
    }

    pub fn read(&mut self, address: u32, now_ns: u64) -> u32 {
        let offset = address - self.base;
        match offset {
            0x18 => self.status | 1,
            0x1c => 0,
            0x24 if self.status & RX_READY != 0 => {
                let value = self
                    .addressed
                    .map_or(0, |target| self.targets[target].1.read(now_ns));
                self.advance_byte();
                value.into()
            }
            0x24 => 0,
            _ => self
                .registers
                .get((offset / 4) as usize)
                .copied()
                .unwrap_or(0),
        }
    }

    pub fn write(&mut self, address: u32, value: u32) {
        match address - self.base {
            0x00 => {
                self.registers[0] = value & 0x00ff_dfff;
                if value & ENABLE == 0 {
                    self.addressed = None;
                    self.remaining = 0;
                    self.status = 0;
                }
            }
            0x04 => {
                self.registers[1] = value & 0x07ff_9fff;
                if self.registers[0] & ENABLE == 0 {
                    return;
                }
                match value & (START | STOP) {
                    START => self.start(),
                    STOP => self.stop(),
                    0 if self.status & RELOAD_READY != 0 => self.load_count(),
                    _ => {}
                }
            }
            0x08 => self.registers[2] = value & 0x87ff,
            0x0c => self.registers[3] = value & 0x87fe,
            0x10 => self.registers[4] = value & 0xf0ff_ffff,
            0x14 => self.registers[5] = value & 0x8fff_9fff,
            0x1c => self.status &= !(value & 0x3f38),
            0x28 => {
                self.registers[10] = value & 0xff;
                if self.status & TX_READY != 0 {
                    if let Some(target) = self.addressed {
                        self.targets[target].1.write(value as u8);
                    }
                    self.advance_byte();
                }
            }
            _ => {}
        }
    }

    fn start(&mut self) {
        self.status &= NACK | STOPPED;
        self.reading = self.registers[1] & READ != 0;
        let control = self.registers[1];
        let address = if control & (1 << 11) != 0 {
            control & 0x3ff
        } else {
            (control >> 1) & 0x7f
        };
        self.addressed = self
            .targets
            .iter()
            .position(|&(target, _)| address == target);
        if let Some(target) = self.addressed {
            if !self.reading {
                self.targets[target].1.start_write();
            }
            self.status |= BUSY;
            self.load_count();
        } else {
            self.status |= NACK;
            self.stop();
        }
    }

    fn load_count(&mut self) {
        self.remaining = (self.registers[1] >> 16) & 0xff;
        self.set_transfer_state();
    }

    fn advance_byte(&mut self) {
        self.remaining -= 1;
        self.set_transfer_state();
    }

    fn set_transfer_state(&mut self) {
        self.status &= !(RX_READY | TX_READY | COMPLETE | RELOAD_READY);
        if self.remaining != 0 {
            self.status |= if self.reading { RX_READY } else { TX_READY };
        } else if self.registers[1] & RELOAD != 0 {
            self.status |= RELOAD_READY;
        } else if self.registers[1] & AUTO_STOP != 0 {
            self.stop();
        } else {
            self.status |= COMPLETE;
        }
    }

    fn stop(&mut self) {
        self.status &= !(BUSY | RX_READY | TX_READY | COMPLETE | RELOAD_READY);
        self.status |= STOPPED;
        self.addressed = None;
        self.remaining = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mb85rc1mt_relays_share_address_and_write_protection() {
        let memory = Rc::new(RefCell::new(Fram::new()));
        let mut low = Target::Fram(Rc::clone(&memory), false);
        let mut high = Target::Fram(Rc::clone(&memory), true);

        low.start_write();
        for byte in [0xff, 0xff, 0xab, 0xcd] {
            low.write(byte);
        }
        low.start_write();
        for byte in [0xff, 0xff] {
            low.write(byte);
        }
        assert_eq!(low.read(0), 0xab);
        assert_eq!(high.read(0), 0xcd);

        memory.borrow_mut().set_write_protected(true);
        high.start_write();
        for byte in [0, 0, 0xef] {
            high.write(byte);
        }
        assert_eq!(high.read(0), 0xcd);
    }
}
