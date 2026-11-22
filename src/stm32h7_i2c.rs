use crate::stm32_uart::Stm32Irq;

pub const I2C: &str = "I2C.STM32F7_I2C";

const PE: u32 = 1;
const NACK: u32 = 1 << 4;
const STOPPED: u32 = 1 << 5;
const START: u32 = 1 << 13;
const STOP: u32 = 1 << 14;

/// Master register interface for a bus with no modeled I2C targets.
pub struct I2c {
    pub base: u32,
    pub clock: Option<u8>,
    pub event: Option<Stm32Irq>,
    pub error: Option<Stm32Irq>,
    registers: [u32; 11],
    status: u32,
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
        }
    }

    pub fn reset(&mut self) {
        self.registers = [0; 11];
        self.status = 0;
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    pub fn interrupt(&self) -> Option<Stm32Irq> {
        self.event
            .filter(|_| self.registers[0] & self.status & (NACK | STOPPED) != 0)
    }

    pub fn read(&self, address: u32) -> u32 {
        match address - self.base {
            0x18 => self.status | 1,
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
                    self.status = 0;
                }
            }
            0x04 => {
                self.registers[1] = value & 0x07ff_9fff;
                if self.registers[0] & PE != 0 {
                    if value & START != 0 {
                        self.status |= NACK | STOPPED;
                    } else if value & STOP != 0 {
                        self.status |= STOPPED;
                    }
                }
            }
            0x08 => self.registers[2] = value & 0x87ff,
            0x0c => self.registers[3] = value & 0x87fe,
            0x10 => self.registers[4] = value & 0xf0ff_ffff,
            0x14 => self.registers[5] = value & 0x8fff_9fff,
            0x1c => self.status &= !(value & (NACK | STOPPED)),
            0x28 => self.registers[10] = value & 0xff,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{bus::Bus, nvic::ActiveExceptions, stm32h7_net_control::NetworkControl};

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
