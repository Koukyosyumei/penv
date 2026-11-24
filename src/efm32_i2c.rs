use crate::bus_translation::NativePeripheral;
use std::collections::VecDeque;

const START: u32 = 1;
const MASTER_STOP: u32 = 1 << 8;
const TX_OVERFLOW: u32 = 1 << 12;
const RX_UNDERFLOW: u32 = 1 << 13;

#[derive(Default)]
pub struct Efm32I2c {
    control: u32,
    clock_divider: u32,
    slave_address: u32,
    route: u32,
    flags: u32,
    enabled_flags: u32,
    rx: VecDeque<u8>,
    tx: VecDeque<u8>,
}

impl Efm32I2c {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn irq(&self) -> bool {
        self.flags & self.enabled_flags != 0
    }

    pub fn read_register(&mut self, offset: u64) -> u32 {
        match offset {
            0x00 => self.control,
            0x10 => self.clock_divider,
            0x14 => self.slave_address,
            0x1c => match self.rx.pop_front() {
                Some(data) => u32::from(data),
                None => {
                    self.raise(RX_UNDERFLOW);
                    0
                }
            },
            0x20 => self.rx.front().copied().map_or(0, u32::from),
            0x28 => self.flags,
            0x34 => self.enabled_flags,
            0x38 => self.route,
            _ => 0,
        }
    }

    pub fn write_register(&mut self, offset: u64, value: u32) {
        match offset {
            0x00 => {
                if self.control & 1 != 0 && value & 1 == 0 {
                    self.reset();
                }
                self.control = value;
            }
            0x04 => {
                if value & 1 != 0 {
                    self.raise(START);
                }
                if value & 2 != 0 {
                    self.raise(MASTER_STOP);
                }
                if value & 0x40 != 0 {
                    self.tx.clear();
                }
            }
            0x10 => self.clock_divider = value,
            0x14 => self.slave_address = value,
            0x24 if self.tx.len() < 2 => self.tx.push_back(value as u8),
            0x24 => self.raise(TX_OVERFLOW),
            0x2c => self.raise(value),
            0x30 => self.flags &= !(value & self.enabled_flags),
            0x34 => {
                self.enabled_flags |= value;
                self.flags &= value;
            }
            0x38 => self.route = value,
            _ => {}
        }
    }

    fn raise(&mut self, mask: u32) {
        self.flags |= self.enabled_flags & mask;
    }
}

impl NativePeripheral for Efm32I2c {
    fn read(&mut self, _: Option<&str>, offset: u64) -> u64 {
        u64::from(self.read_register(offset))
    }

    fn write(&mut self, _: Option<&str>, offset: u64, value: u64) {
        self.write_register(offset, value as u32);
    }
}
