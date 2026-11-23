use crate::stm32_i2c::Eeprom;

use super::{
    Adxl372, Spi, bme280::Bme280, bmp180::Bmp180, lis2ds12::Lis2ds12, lsm303::Lsm303,
    lsm330::Lsm330,
};

pub(super) enum Target {
    Eeprom(Eeprom),
    Adxl372(Box<Adxl372>),
    Bme280(Box<Bme280>),
    Lis2ds12(Box<Lis2ds12>),
    Lsm330(Lsm330),
    Lsm303(Lsm303),
    Bmp180(Bmp180),
}

impl Target {
    fn start_write(&mut self) {
        match self {
            Self::Eeprom(device) => device.start_write(),
            Self::Adxl372(device) => device.start_write(),
            Self::Bme280(device) => device.start_write(),
            Self::Lis2ds12(device) => device.start_write(),
            Self::Lsm330(device) => device.start_write(),
            Self::Lsm303(device) => device.start_write(),
            Self::Bmp180(device) => device.start_write(),
        }
    }

    fn write(&mut self, value: u8) {
        match self {
            Self::Eeprom(device) => device.write(value),
            Self::Adxl372(device) => device.write(value),
            Self::Bme280(device) => device.write(value),
            Self::Lis2ds12(device) => device.write(value),
            Self::Lsm330(device) => device.write(value),
            Self::Lsm303(device) => device.write(value),
            Self::Bmp180(device) => device.write(value),
        }
    }

    fn read(&mut self) -> u8 {
        match self {
            Self::Eeprom(device) => device.read(),
            Self::Adxl372(device) => device.read(),
            Self::Bme280(device) => device.read(),
            Self::Lis2ds12(device) => device.read(),
            Self::Lsm330(device) => device.read(),
            Self::Lsm303(device) => device.read(),
            Self::Bmp180(device) => device.read(),
        }
    }
}

const STOPPED: u32 = 1 << 1;
const RXDREADY: u32 = 1 << 2;
const TXDSENT: u32 = 1 << 7;
const ERROR: u32 = 1 << 9;
const BB: u32 = 1 << 14;
const SUSPENDED: u32 = 1 << 18;
const EVENTS: u32 = STOPPED | RXDREADY | TXDSENT | ERROR | BB | SUSPENDED;
const ANACK: u32 = 1 << 1;

#[derive(Clone, Copy, PartialEq)]
enum Direction {
    Read,
    Write,
}

pub(super) struct Twi {
    targets: Vec<(u32, Target)>,
    spi: Spi,
    spi_enabled: bool,
    active: Option<(usize, Direction)>,
    suspended: bool,
    rx: Option<u8>,
    pub(super) raised: u32,
    events: u32,
    interrupts: u32,
    errors: u32,
    shorts: u32,
    enabled: bool,
    pins: [u32; 2],
    frequency: u32,
    address: u32,
}

impl Default for Twi {
    fn default() -> Self {
        Self {
            targets: Vec::new(),
            spi: Spi::new(false),
            spi_enabled: false,
            active: None,
            suspended: false,
            rx: None,
            raised: 0,
            events: 0,
            interrupts: 0,
            errors: 0,
            shorts: 0,
            enabled: false,
            pins: [u32::MAX; 2],
            frequency: 0x0400_0000,
            address: 0,
        }
    }
}

impl Twi {
    pub(super) fn attach(&mut self, address: u32, target: Target) -> Result<(), String> {
        if address >= 128 || self.targets.iter().any(|(other, _)| *other == address) {
            return Err(format!(
                "invalid or duplicate TWI target address {address:#x}"
            ));
        }
        self.targets.push((address, target));
        Ok(())
    }

    pub(super) fn adxl(&mut self, name: &str) -> Option<&mut Adxl372> {
        self.targets
            .iter_mut()
            .find_map(|(_, target)| match target {
                Target::Adxl372(device) if device.name == name => Some(device.as_mut()),
                _ => None,
            })
    }

    pub(super) fn sensor_irqs(&self) -> impl Iterator<Item = (u32, u8, bool)> + '_ {
        self.targets.iter().flat_map(|(_, target)| {
            match target {
                Target::Lis2ds12(device) => [Some(device.irq()), None],
                Target::Lsm303(device) => device.irqs(),
                _ => [None; 2],
            }
            .into_iter()
            .flatten()
        })
    }

    fn finish_current_write(&mut self) {
        if let Some((index, Direction::Write)) = self.active {
            match &mut self.targets[index].1 {
                Target::Lis2ds12(device) => device.finish_write(),
                Target::Lsm303(device) => device.finish_write(),
                _ => {}
            }
        }
    }

    pub(super) fn reset(&mut self) {
        for (_, target) in &mut self.targets {
            match target {
                Target::Adxl372(device) => device.reset(),
                Target::Lis2ds12(device) => device.reset(),
                Target::Lsm330(device) => device.reset(),
                Target::Lsm303(device) => device.reset(),
                Target::Bmp180(device) => device.reset(),
                Target::Bme280(device) => **device = Bme280::default(),
                _ => {}
            }
        }
        *self = Self {
            targets: std::mem::take(&mut self.targets),
            ..Self::default()
        };
    }

    pub(super) fn pending(&self) -> bool {
        self.events & self.interrupts != 0
    }

    pub(super) fn event(offset: u32) -> Option<u32> {
        let bit = 1u32.checked_shl(offset.checked_sub(0x100)? / 4)?;
        (bit & EVENTS != 0).then_some(bit)
    }

    pub(super) fn is_task(offset: u32) -> bool {
        matches!(offset, 0x000 | 0x008 | 0x014 | 0x01c | 0x020)
    }

    fn raise(&mut self, events: u32) {
        self.events |= events;
        self.raised |= events;
    }

    fn start(&mut self, direction: Direction) {
        self.finish_current_write();
        self.suspended = false;
        self.rx = None;
        self.active = None;
        let Some(index) = self
            .targets
            .iter()
            .position(|(address, _)| *address == self.address)
        else {
            self.errors |= ANACK;
            self.raise(ERROR);
            return;
        };
        self.active = Some((index, direction));
        if direction == Direction::Write {
            self.targets[index].1.start_write();
        } else {
            self.receive();
        }
    }

    fn stop(&mut self) {
        self.finish_current_write();
        self.active = None;
        self.suspended = false;
        self.raise(STOPPED);
    }

    fn suspend(&mut self) {
        self.suspended = true;
        self.raise(SUSPENDED);
    }

    fn receive(&mut self) {
        if let Some((index, Direction::Read)) = self.active
            && !self.suspended
            && self.rx.is_none()
        {
            self.rx = Some(self.targets[index].1.read());
            self.raise(RXDREADY | BB);
            if self.shorts & 2 != 0 {
                self.stop();
            } else if self.shorts & 1 != 0 {
                self.suspend();
            }
        }
    }

    pub(super) fn read(&mut self, offset: u32) -> Option<u32> {
        if self.spi_enabled || matches!(offset, 0x510 | 0x554) {
            return self.spi.read(offset);
        }
        Some(match offset {
            offset if Self::is_task(offset) => 0,
            0x200 => self.shorts,
            0x304 | 0x308 => self.interrupts,
            0x4c4 => self.errors,
            0x500 => {
                if self.enabled {
                    5
                } else {
                    0
                }
            }
            0x508 | 0x50c => self.pins[((offset - 0x508) / 4) as usize],
            0x518 => {
                let value = self.rx.take().unwrap_or(0);
                self.receive();
                u32::from(value)
            }
            0x524 => self.frequency,
            0x588 => self.address,
            _ => u32::from(self.events & Self::event(offset)? != 0),
        })
    }

    pub(super) fn write(&mut self, offset: u32, value: u32) -> bool {
        if offset == 0x500 {
            if value == 1 && !self.enabled {
                self.spi_enabled = true;
            } else if value == 0 && self.spi_enabled {
                self.spi_enabled = false;
                return self.spi.write(offset, value);
            }
        }
        if self.spi_enabled || matches!(offset, 0x510 | 0x554) {
            return self.spi.write(offset, value);
        }
        if !self.enabled && matches!(offset, 0x508 | 0x50c | 0x524) {
            self.spi.write(offset, value);
        }
        let trigger = self.enabled && value & 1 != 0;
        match offset {
            0x000 if trigger => self.start(Direction::Read),
            0x008 if trigger => self.start(Direction::Write),
            0x014 if trigger => self.stop(),
            0x01c if trigger && self.active.is_some() => self.suspend(),
            0x020 if trigger => {
                self.suspended = false;
                self.receive();
            }
            offset if Self::is_task(offset) => {}
            0x200 => self.shorts = value & 3,
            0x304 => self.interrupts |= value & EVENTS,
            0x308 => self.interrupts &= !value,
            0x4c4 => self.errors &= !value,
            0x500 if matches!(value, 0 | 5) => {
                self.enabled = value != 0;
                if !self.enabled {
                    self.active = None;
                    self.suspended = false;
                    self.rx = None;
                }
            }
            0x508 | 0x50c => self.pins[((offset - 0x508) / 4) as usize] = value,
            0x51c => {
                if let Some((index, Direction::Write)) = self.active
                    && !self.suspended
                {
                    self.targets[index].1.write(value as u8);
                    self.raise(TXDSENT);
                }
            }
            0x524 => self.frequency = value,
            0x588 => self.address = value & 127,
            _ => {
                let Some(bit) = Self::event(offset) else {
                    return false;
                };
                super::set_event(&mut self.events, bit, value);
            }
        }
        true
    }
}
