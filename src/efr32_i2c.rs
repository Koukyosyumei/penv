use crate::bus_translation::NativePeripheral;
use std::{
    cell::RefCell,
    collections::{BTreeMap, VecDeque},
    rc::Rc,
};

pub trait Slave {
    fn write(&mut self, bytes: &[u8]);
    fn read(&mut self, count: usize) -> Vec<u8>;
}

const START: u32 = 1;
const RESTART: u32 = 2;
const TX_COMPLETE: u32 = 1 << 3;
const TX_LEVEL: u32 = 1 << 4;
const RX_VALID: u32 = 1 << 5;
const ACK: u32 = 1 << 6;
const NACK: u32 = 1 << 7;
const STOP: u32 = 1 << 8;
const BUS_HOLD: u32 = 1 << 11;
const RX_UNDERFLOW: u32 = 1 << 13;

#[derive(Default)]
pub struct Efr32I2c {
    slaves: BTreeMap<u8, Rc<RefCell<dyn Slave>>>,
    control: u32,
    interrupt_enable: u32,
    flags: u32,
    tx: VecDeque<u8>,
    rx: VecDeque<u8>,
    waiting_address: bool,
    writing: bool,
    address: Option<u8>,
}

impl Efr32I2c {
    pub fn reset(&mut self) {
        *self = Self {
            slaves: std::mem::take(&mut self.slaves),
            ..Self::default()
        };
    }

    pub fn register(&mut self, address: u8, slave: Rc<RefCell<dyn Slave>>) {
        self.slaves.insert(address, slave);
    }

    pub fn irq(&self) -> bool {
        self.flags & self.interrupt_enable != 0
    }

    fn slave(&self) -> Option<&Rc<RefCell<dyn Slave>>> {
        self.address.and_then(|address| self.slaves.get(&address))
    }

    fn refill(&mut self, count: usize) {
        let count = count.min(2 - self.rx.len());
        if count != 0 {
            if let Some(slave) = self.slave() {
                let values = slave.borrow_mut().read(count);
                self.rx.extend(values.into_iter().take(count));
            }
        }
        self.flags &= !RX_VALID;
        if !self.rx.is_empty() {
            self.flags |= RX_VALID;
        }
    }

    fn receive(&mut self) -> u8 {
        let Some(byte) = self.rx.pop_front() else {
            self.flags |= RX_UNDERFLOW;
            return 0;
        };
        self.refill(usize::from(self.control & 4 != 0));
        byte
    }

    fn stop(&mut self) {
        self.flags |= STOP;
        if self.writing {
            if let Some(slave) = self.slave().cloned() {
                slave.borrow_mut().write(self.tx.make_contiguous());
            }
            self.tx.clear();
            self.flags |= TX_LEVEL | TX_COMPLETE;
            self.writing = false;
        }
        self.address = None;
        self.waiting_address = false;
    }

    fn handle_address(&mut self, byte: u8) {
        self.waiting_address = false;
        self.address = Some(byte >> 1);
        self.writing = byte & 1 == 0;
        if self.slave().is_some() {
            self.flags |= ACK;
            if !self.writing {
                self.refill(if self.control & 4 != 0 { 2 } else { 1 });
            }
        } else {
            self.flags |= NACK;
        }
    }

    fn start(&mut self) {
        if self.writing {
            self.flags |= RESTART;
            self.stop();
        }
        self.flags |= START;
        if let Some(byte) = self.tx.pop_front() {
            self.handle_address(byte);
        } else {
            self.waiting_address = true;
            self.flags |= BUS_HOLD;
        }
    }

    fn transmit(&mut self, byte: u8) {
        self.flags &= !TX_LEVEL;
        if self.waiting_address {
            self.handle_address(byte);
        } else {
            self.tx.push_back(byte);
            if self.writing {
                self.flags |= if self.slave().is_some() { ACK } else { NACK };
            }
        }
    }
}

impl NativePeripheral for Efr32I2c {
    fn read(&mut self, _: Option<&str>, offset: u64) -> u64 {
        match offset {
            0 => self.control.into(),
            0x0c => (u64::from(!self.rx.is_empty()) << 8) | (u64::from(self.rx.len() == 2) << 9),
            0x1c => self.receive().into(),
            0x20 => u16::from_le_bytes([self.receive(), self.receive()]).into(),
            0x24 => self.rx.front().copied().unwrap_or_default().into(),
            0x28 => u16::from_le_bytes([
                self.rx.front().copied().unwrap_or_default(),
                self.rx.get(1).copied().unwrap_or_default(),
            ])
            .into(),
            0x34 => self.flags.into(),
            0x40 => self.interrupt_enable.into(),
            _ => 0,
        }
    }

    fn write(&mut self, _: Option<&str>, offset: u64, value: u64) {
        match offset {
            0 => self.control = value as u32,
            4 => {
                if value & 1 != 0 {
                    self.start();
                }
                if value & 2 != 0 {
                    self.stop();
                }
                if value & 4 != 0 && !self.writing {
                    self.refill(1);
                }
                if value & 0x40 != 0 {
                    self.tx.clear();
                }
            }
            0x2c => self.transmit(value as u8),
            0x30 => {
                self.transmit(value as u8);
                self.transmit((value >> 8) as u8);
            }
            0x38 => self.flags |= value as u32 & 0x7ffff,
            0x3c => self.flags &= !(value as u32),
            0x40 => self.interrupt_enable = value as u32 & 0x7ffff,
            _ => {}
        }
    }
}
