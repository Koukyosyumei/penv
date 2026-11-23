const COMMAND_SENT: u32 = 1 << 7;
const WAIT_RESPONSE: u32 = 3 << 8;
const COMMAND_TIMEOUT: u32 = 1 << 2;
const START_COMMAND: u32 = 1 << 12;

pub struct Sdmmc {
    pub base: u32,
    pub irq: Option<u8>,
    registers: [u32; 24],
    status: u32,
}

impl Sdmmc {
    pub const MODEL: &str = "SD.STM32HSDMMC";
    pub const SIZE: u32 = 0x2000;

    pub fn new(base: u32, irq: Option<u8>) -> Self {
        Self {
            base,
            irq,
            registers: [0; 24],
            status: 0,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new(self.base, self.irq);
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    pub fn interrupt(&self) -> Option<u8> {
        self.irq
            .filter(|_| self.status & self.registers[0x3c / 4] != 0)
    }

    pub fn read(&self, address: u32) -> u32 {
        let offset = address - self.base;
        match offset {
            0x34 => self.status,
            _ => self
                .registers
                .get((offset / 4) as usize)
                .copied()
                .unwrap_or(0),
        }
    }

    pub fn write(&mut self, address: u32, value: u32) {
        let offset = address - self.base;
        let register = (offset / 4) as usize;
        match offset {
            0x00 => {
                self.registers[register] = value & 3;
                if value & 3 == 0 {
                    self.status = 0;
                }
            }
            0x0c => {
                self.registers[register] = value & (0x1ffff & !START_COMMAND);
                if value & START_COMMAND != 0 {
                    self.status |= if value & WAIT_RESPONSE == 0 {
                        COMMAND_SENT
                    } else {
                        // An empty slot cannot return a response.
                        COMMAND_TIMEOUT
                    };
                }
            }
            0x38 => self.status &= !value,
            0x04 | 0x08 | 0x24 | 0x28 | 0x2c | 0x3c | 0x50..=0x5c => {
                self.registers[register] = value;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_slot_times_out_commands_and_clears_the_masked_irq() {
        let mut controller = Sdmmc::new(0, Some(49));
        controller.write(0, 3);
        controller.write(0x0c, START_COMMAND);
        assert_eq!(controller.read(0x34), COMMAND_SENT);
        controller.write(0x38, COMMAND_SENT);
        controller.write(0x0c, START_COMMAND | (1 << 8) | 8);
        assert_eq!(controller.read(0x34), COMMAND_TIMEOUT);
        assert_eq!(controller.read(0x0c) & START_COMMAND, 0);
        assert_eq!(controller.interrupt(), None);
        controller.write(0x3c, COMMAND_TIMEOUT);
        assert_eq!(controller.interrupt(), Some(49));
        controller.write(0x38, COMMAND_TIMEOUT);
        assert_eq!(controller.read(0x34), 0);
        assert_eq!(controller.interrupt(), None);
    }
}
