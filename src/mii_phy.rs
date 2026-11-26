const CONTROL: usize = 0;
const STATUS: usize = 1;
const ADVERTISEMENT: usize = 4;
const RESET: u16 = 1 << 15;
const RESTART_NEGOTIATION: u16 = 1 << 9;
const POWER_DOWN: u16 = 1 << 11;
const ISOLATE: u16 = 1 << 10;
const LINK_AND_NEGOTIATION: u16 = (1 << 2) | (1 << 5);

pub struct MiiPhy {
    pub address: u8,
    defaults: [u16; 32],
    control: u16,
    advertisement: u16,
}

impl MiiPhy {
    pub fn new(address: u8, registers: [u16; 32]) -> Self {
        Self {
            address,
            defaults: registers,
            control: registers[CONTROL],
            advertisement: registers[ADVERTISEMENT],
        }
    }

    pub fn read(&self, index: usize) -> u16 {
        match index {
            CONTROL => self.control,
            STATUS if self.control & (POWER_DOWN | ISOLATE) != 0 => {
                self.defaults[STATUS] & !LINK_AND_NEGOTIATION
            }
            ADVERTISEMENT => self.advertisement,
            _ => self.defaults[index],
        }
    }

    pub fn write(&mut self, index: usize, value: u16) {
        match index {
            CONTROL if value & RESET != 0 => {
                self.control = self.defaults[CONTROL] & !(RESET | RESTART_NEGOTIATION);
                self.advertisement = self.defaults[ADVERTISEMENT];
            }
            CONTROL => self.control = value & !RESTART_NEGOTIATION,
            ADVERTISEMENT => self.advertisement = value,
            _ => {}
        }
    }
}
