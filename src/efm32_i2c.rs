use crate::bus_translation::NativePeripheral;
use std::{
    cell::RefCell,
    collections::{BTreeMap, VecDeque},
    rc::Rc,
};

const START: u32 = 1;
const RX_AVAILABLE: u32 = 1 << 5;
const ACK: u32 = 1 << 6;

pub trait I2cPacketSlave {
    fn write_packet(&mut self, packet: &[u8]);
    fn read_packet(&self) -> Vec<u8>;
}

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
    slaves: BTreeMap<u8, Rc<RefCell<dyn I2cPacketSlave>>>,
    started: bool,
    write_address: Option<u8>,
    packet: Vec<u8>,
    register: u8,
}

impl Efm32I2c {
    pub fn reset(&mut self) {
        *self = Self {
            slaves: std::mem::take(&mut self.slaves),
            ..Self::default()
        };
    }

    pub fn register_slave(&mut self, address: u8, slave: Rc<RefCell<dyn I2cPacketSlave>>) {
        self.slaves.insert(address & 0xfe, slave);
    }

    fn finish_write(&mut self) {
        if let Some(slave) = self
            .write_address
            .take()
            .and_then(|address| self.slaves.get(&address))
        {
            slave.borrow_mut().write_packet(&self.packet);
        }
        self.packet.clear();
    }

    fn transmit(&mut self, data: u8) {
        if let Some(address) = self.write_address {
            if self.packet.len() < 3 {
                if self.packet.len() == 1 {
                    self.register = data;
                }
                self.packet.push(data);
                if self.slaves.contains_key(&address) {
                    self.raise(ACK);
                }
            } else {
                self.raise(TX_OVERFLOW);
            }
            return;
        }
        let address = data & 0xfe;
        if data & 1 == 0 {
            self.write_address = Some(address);
            self.packet.push(0xfd);
            if self.slaves.contains_key(&address) {
                self.raise(ACK);
            }
        } else {
            self.started = false;
            if let Some(slave) = self.slaves.get(&address) {
                let mut slave = slave.borrow_mut();
                slave.write_packet(&[0xfc, self.register]);
                let mut response = slave.read_packet();
                response.pop(); // The final byte is the peripheral packet checksum.
                self.rx.extend(response);
                drop(slave);
                self.raise(ACK);
                if !self.rx.is_empty() {
                    self.raise(RX_AVAILABLE);
                }
            }
        }
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
                    self.finish_write();
                    self.started = true;
                    while self.started {
                        let Some(byte) = self.tx.pop_front() else {
                            break;
                        };
                        self.transmit(byte);
                    }
                }
                if value & 2 != 0 {
                    self.raise(MASTER_STOP);
                    self.finish_write();
                    self.started = false;
                }
                if value & 0x40 != 0 {
                    self.tx.clear();
                }
            }
            0x10 => self.clock_divider = value,
            0x14 => self.slave_address = value,
            0x24 if self.started => self.transmit(value as u8),
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
