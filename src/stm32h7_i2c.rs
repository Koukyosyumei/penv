use crate::{iis2mdc::Iis2mdc, stm32_uart::Stm32Irq, tmp108::Tmp108, veml7700::Veml7700};
use crate::{lps25hb::Lps25hb, lsm6dso::Lsm6dso};

pub const I2C: &str = "I2C.STM32F7_I2C";

const PE: u32 = 1;
const TXIS: u32 = 1 << 1;
const RXNE: u32 = 1 << 2;
const TC: u32 = 1 << 6;
const TCR: u32 = 1 << 7;
const BUSY: u32 = 1 << 15;
const READ: u32 = 1 << 10;
const RELOAD: u32 = 1 << 24;
const AUTOEND: u32 = 1 << 25;
const NACK: u32 = 1 << 4;
const STOPPED: u32 = 1 << 5;
const START: u32 = 1 << 13;
const STOP: u32 = 1 << 14;

pub enum Target {
    Light(Veml7700),
    Magnetometer(Iis2mdc),
    Temperature(Tmp108),
    Pressure(Lps25hb),
    Imu(String, Box<Lsm6dso>),
}

impl Target {
    fn stop(&mut self) {
        match self {
            Self::Light(sensor) => sensor.stop(),
            Self::Temperature(sensor) => sensor.stop(),
            Self::Pressure(sensor) => sensor.stop(),
            Self::Magnetometer(_) | Self::Imu(..) => {}
        }
    }
}

/// STM32F7 master register interface.
pub struct I2c {
    pub base: u32,
    pub clock: Option<u8>,
    pub event: Option<Stm32Irq>,
    pub error: Option<Stm32Irq>,
    registers: [u32; 11],
    status: u32,
    targets: Vec<(u16, Target)>,
    active: Option<usize>,
    remaining: u8,
    data: [u8; 256],
    position: usize,
    reading: bool,
    preloaded: Option<u8>,
}

impl I2c {
    pub const SIZE: u32 = 0x400;

    pub fn new(base: u32, clock: Option<u8>) -> Self {
        Self {
            base,
            clock,
            event: None,
            error: None,
            registers: [0; 11],
            status: 0,
            targets: Vec::new(),
            active: None,
            remaining: 0,
            data: [0; 256],
            position: 0,
            reading: false,
            preloaded: None,
        }
    }

    pub fn attach(&mut self, address: u16, target: Target) -> Result<(), &'static str> {
        if address > 0x7f
            || self
                .targets
                .iter()
                .any(|(candidate, _)| *candidate == address)
        {
            return Err("invalid or duplicate I2C target address");
        }
        self.targets.push((address, target));
        Ok(())
    }

    pub fn find_imu(&mut self, name: &str) -> Option<&mut Lsm6dso> {
        self.targets
            .iter_mut()
            .find_map(|(_, target)| match target {
                Target::Imu(registered, sensor) if registered == name => Some(sensor.as_mut()),
                _ => None,
            })
    }

    pub fn sample_sensor_outputs(
        &mut self,
        step: u64,
    ) -> impl Iterator<Item = (u32, u8, bool)> + '_ {
        self.targets
            .iter_mut()
            .filter_map(move |(_, target)| match target {
                Target::Magnetometer(sensor) => sensor.tick(step),
                Target::Imu(_, sensor) => sensor.tick(step),
                _ => None,
            })
    }

    pub fn reset(&mut self) {
        self.stop();
        self.registers = [0; 11];
        self.status = 0;
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    pub fn interrupt(&self) -> Option<Stm32Irq> {
        let pending = self.status & (TXIS | RXNE | NACK | STOPPED | TC)
            | if self.status & TCR != 0 { TC } else { 0 };
        self.event.filter(|_| self.registers[0] & pending != 0)
    }

    pub fn read(&mut self, address: u32) -> u32 {
        match address - self.base {
            0x18 => self.status | u32::from(self.status & TXIS == 0),
            0x24 => {
                if self.status & RXNE == 0 {
                    return 0;
                }
                let value = match self.active.map(|target| &mut self.targets[target].1) {
                    Some(Target::Magnetometer(sensor)) => sensor.read_byte(),
                    Some(Target::Pressure(sensor)) => sensor.read_byte(),
                    Some(Target::Imu(_, sensor)) => sensor.read_byte(),
                    Some(Target::Light(_) | Target::Temperature(_)) if self.position < 2 => {
                        self.data[self.position]
                    }
                    _ => 0xff,
                };
                self.position += 1;
                self.remaining -= 1;
                self.update_transfer();
                u32::from(value)
            }
            offset if offset & 3 == 0 => self
                .registers
                .get((offset / 4) as usize)
                .copied()
                .unwrap_or(0),
            _ => 0,
        }
    }

    pub fn write(&mut self, address: u32, value: u32) {
        match address - self.base {
            0x00 => {
                self.registers[0] = value & 0x00ff_dfff;
                if value & PE == 0 {
                    self.stop();
                    self.status = 0;
                }
            }
            0x04 => {
                self.registers[1] = value & 0x07ff_9fff;
                if self.registers[0] & PE != 0 {
                    if value & START != 0 {
                        self.start();
                    } else if value & STOP != 0 {
                        self.stop();
                    } else if self.status & TCR != 0 {
                        self.load_count();
                    }
                }
            }
            0x08 => self.registers[2] = value & 0x87ff,
            0x0c => self.registers[3] = value & 0x87fe,
            0x10 => self.registers[4] = value & 0xf0ff_ffff,
            0x14 => self.registers[5] = value & 0x8fff_9fff,
            0x18 if value & 1 != 0 => self.preloaded = None,
            0x1c => self.status &= !(value & (NACK | STOPPED)),
            0x28 => {
                self.registers[10] = value & 0xff;
                if self.status & TXIS != 0 {
                    self.transmit(value as u8);
                    self.update_transfer();
                } else if self.remaining == 0 {
                    self.preloaded = Some(value as u8);
                }
            }
            _ => {}
        }
    }

    fn transmit(&mut self, byte: u8) {
        if let Some(slot) = self.data.get_mut(self.position) {
            *slot = byte;
        }
        self.position += 1;
        self.remaining -= 1;
    }

    fn start(&mut self) {
        self.finish_write();
        let control = self.registers[1];
        let address = if control & (1 << 11) != 0 {
            (control & 0x3ff) as u16
        } else {
            ((control >> 1) & 0x7f) as u16
        };
        let selected = self
            .targets
            .iter()
            .position(|(candidate, _)| *candidate == address);
        if selected != self.active
            && let Some(old) = self.active
        {
            self.targets[old].1.stop();
        }
        self.active = selected;
        self.position = 0;
        self.reading = control & READ != 0;
        if let Some(target) = selected {
            self.status |= BUSY;
            if self.reading {
                let bytes = match &mut self.targets[target].1 {
                    Target::Light(sensor) => Some(sensor.read()),
                    Target::Temperature(sensor) => Some(sensor.read()),
                    Target::Magnetometer(_) | Target::Pressure(_) | Target::Imu(..) => None,
                };
                if let Some(bytes) = bytes {
                    self.data[..2].copy_from_slice(&bytes);
                }
            }
            self.load_count();
        } else {
            self.stop();
            self.status |= NACK;
        }
    }

    fn load_count(&mut self) {
        self.remaining = (self.registers[1] >> 16) as u8;
        if !self.reading
            && self.remaining != 0
            && let Some(byte) = self.preloaded.take()
        {
            self.transmit(byte);
        }
        self.update_transfer();
    }

    fn update_transfer(&mut self) {
        self.status &= !(TXIS | RXNE | TC | TCR);
        if self.remaining != 0 {
            self.status |= if self.reading { RXNE } else { TXIS };
        } else if self.registers[1] & RELOAD != 0 {
            self.status |= TCR;
        } else if self.registers[1] & AUTOEND != 0 {
            self.stop();
        } else {
            self.status |= TC;
        }
    }

    fn finish_write(&mut self) {
        if let Some(target) = self.active
            && !self.reading
        {
            match &mut self.targets[target].1 {
                Target::Light(sensor) if (1..=3).contains(&self.position) => {
                    sensor.write(&self.data[..self.position])
                }
                Target::Temperature(sensor) if self.position <= self.data.len() => {
                    sensor.write(&self.data[..self.position])
                }
                Target::Imu(_, sensor) if self.position <= self.data.len() => {
                    sensor.write(&self.data[..self.position])
                }
                Target::Pressure(sensor) if self.position <= self.data.len() => {
                    sensor.write(&self.data[..self.position])
                }
                Target::Magnetometer(sensor) if self.position <= self.data.len() => {
                    sensor.write(&self.data[..self.position])
                }
                _ => {}
            }
        }
    }

    fn stop(&mut self) {
        self.finish_write();
        if let Some(target) = self.active.take() {
            self.targets[target].1.stop();
        }
        self.remaining = 0;
        self.position = 0;
        self.preloaded = None;
        self.status = (self.status & NACK) | STOPPED;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{bus::Bus, nvic::ActiveExceptions, stm32h7_net_control::NetworkControl};

    #[test]
    fn tmp108_reads_signed_temperature_and_threshold_in_big_endian_order() {
        let mut i2c = I2c::new(0, None);
        i2c.attach(0x48, Target::Temperature(Tmp108::new(-12.5)))
            .unwrap();
        i2c.write(0, PE);
        let address = 0x48 << 1;
        let read_word = |i2c: &mut I2c| {
            i2c.write(4, address | START | READ | AUTOEND | (2 << 16));
            u16::from_be_bytes([i2c.read(0x24) as u8, i2c.read(0x24) as u8])
        };
        assert_eq!(read_word(&mut i2c), 0xf380);
        i2c.write(4, address | START | (3 << 16));
        for value in [2, 0x12, 0x3f] {
            i2c.write(0x28, value);
        }
        assert_eq!(read_word(&mut i2c), 0x1230);
        assert_eq!(read_word(&mut i2c), 0xf380);
    }

    #[test]
    fn preloaded_txdr_is_consumed_once_when_starting_write() {
        let mut i2c = I2c::new(0, None);
        i2c.attach(0x1e, Target::Magnetometer(Iis2mdc::new([0; 3])))
            .unwrap();
        i2c.write(0, PE);
        i2c.write(0x28, 0x4f);
        let address = 0x1e << 1;
        i2c.write(4, address | START | (1 << 16));
        assert_eq!(i2c.read(0x18) & (TC | TXIS), TC);
        i2c.write(4, address | START | READ | AUTOEND | (1 << 16));
        assert_eq!(i2c.read(0x24), 0x40);
    }

    #[test]
    fn sensor_register_read_uses_reload_repeated_start_and_event_irq() {
        let mut i2c = I2c::new(0, None);
        i2c.event = Some(Stm32Irq::Nvic(31));
        i2c.attach(0x10, Target::Light(Veml7700::new(500.0)))
            .unwrap();
        i2c.write(0, PE | TXIS | RXNE | TC | STOPPED);

        let address = 0x10 << 1;
        let byte = 1 << 16;
        i2c.write(4, address | START | RELOAD | byte);
        assert_ne!(i2c.read(0x18) & (BUSY | TXIS), 0);
        assert!(matches!(i2c.interrupt(), Some(Stm32Irq::Nvic(31))));
        i2c.write(0x28, 4); // ALS register pointer
        assert_eq!(i2c.read(0x18) & (TCR | TC), TCR);
        assert!(i2c.interrupt().is_some()); // TCIE enables both TC and TCR.
        i2c.write(4, address);
        assert_eq!(i2c.read(0x18) & (TCR | TC), TC);

        i2c.write(4, address | START | READ | AUTOEND | (2 << 16));
        assert_eq!(i2c.read(0x18) & RXNE, RXNE);
        let lo = i2c.read(0x24) as u8;
        let hi = i2c.read(0x24) as u8;
        assert_eq!(u16::from_le_bytes([lo, hi]), 8681);
        assert_eq!(i2c.read(0x18) & (STOPPED | BUSY | RXNE), STOPPED);
        assert!(i2c.interrupt().is_some());
        i2c.write(0x1c, STOPPED);
        assert!(i2c.interrupt().is_none());
    }

    #[test]
    fn unpopulated_i2c_bus_sets_nack_and_stop_when_clocked() {
        let base = 0x5800_1c00;
        let rcc = 0x5802_4400;
        let mut bus = Bus::default();
        bus.map_nvic(0xf0, None).unwrap();
        let mut i2c = I2c::new(base, Some(4));
        i2c.event = Some(Stm32Irq::Nvic(62));
        bus.map_h7_i2c(i2c).unwrap();
        bus.map_h7_network_control(NetworkControl::new(rcc, None, None))
            .unwrap();
        let write = |bus: &mut Bus, address, value| {
            bus.write(address, 4, value, &mut std::io::sink()).unwrap();
        };
        write(&mut bus, base, PE | NACK);
        assert_eq!(bus.read::<4>(base).unwrap(), 0);
        write(&mut bus, rcc + 0xf4, 1 << 7);
        write(&mut bus, base, PE | NACK);
        write(&mut bus, base + 4, START | (0x68 << 1));
        assert_eq!(bus.read::<4>(base + 0x18).unwrap(), 1 | NACK | STOPPED);
        bus.sample_nvic(&ActiveExceptions::default());
        assert_eq!(bus.read::<4>(0xe000_e204).unwrap(), 1 << 30);
        write(&mut bus, base + 0x1c, NACK);
        assert_eq!(bus.read::<4>(base + 0x18).unwrap(), 1 | STOPPED);
        write(&mut bus, rcc + 0x9c, 1 << 7);
        assert_eq!(bus.read::<4>(base + 0x18).unwrap(), 0);
        write(&mut bus, rcc + 0x9c, 0);
        assert_eq!(bus.read::<4>(base + 0x18).unwrap(), 1);
    }
}
