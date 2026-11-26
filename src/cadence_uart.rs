use std::collections::VecDeque;

use crate::bus_translation::NativePeripheral;

const RX_OVERFLOW: u32 = 1 << 5;
const RX_TIMEOUT: u32 = 1 << 8;
const TX_OVERFLOW: u32 = 1 << 12;
const IRQ_FLAGS: u32 = 0x1d3f;

pub struct CadenceUart {
    capacity: usize,
    clear_on_read: bool,
    pub enable_rx_overflow: bool,
    control: u32,
    mode: u32,
    mask: u32,
    sticky: u32,
    rx_trigger: usize,
    tx_trigger: usize,
    rx: VecDeque<u8>,
    tx: VecDeque<u8>,
    transmitted: VecDeque<u8>,
}

impl CadenceUart {
    pub fn new(clear_on_read: bool, capacity: usize) -> Self {
        let mut uart = Self {
            capacity,
            clear_on_read,
            enable_rx_overflow: false,
            control: 0,
            mode: 0,
            mask: 0,
            sticky: 0,
            rx_trigger: 0,
            tx_trigger: 0,
            rx: VecDeque::new(),
            tx: VecDeque::new(),
            transmitted: VecDeque::new(),
        };
        uart.reset();
        uart
    }

    pub fn reset(&mut self) {
        self.control = 0x128;
        self.mode = 0;
        self.mask = 0;
        self.sticky = 0;
        self.rx_trigger = 32;
        self.tx_trigger = 32;
        self.rx.clear();
        self.tx.clear();
        self.transmitted.clear();
        self.update_sticky();
    }

    pub fn write_char(&mut self, byte: u8) {
        if self.control & 0xc != 4 {
            return;
        }
        if self.rx.len() >= self.capacity && self.enable_rx_overflow {
            self.sticky |= RX_OVERFLOW;
        } else {
            self.rx.push_back(byte);
            self.sticky |= RX_TIMEOUT;
        }
        self.update_sticky();
    }

    pub fn transmit_character(&mut self, byte: u8) {
        if self.tx.len() < self.capacity {
            self.tx.push_back(byte);
            self.flush_tx();
        } else {
            self.sticky |= TX_OVERFLOW;
        }
        self.update_sticky();
    }

    pub fn take_transmitted(&mut self) -> Option<u8> {
        self.transmitted.pop_front()
    }

    fn flush_tx(&mut self) {
        if self.control & 0x30 == 0x10 {
            self.transmitted.append(&mut self.tx);
        }
    }

    fn levels(&self) -> u32 {
        let rx = self.rx.len();
        let tx = self.tx.len();
        u32::from(self.rx_trigger != 0 && rx >= self.rx_trigger)
            | (u32::from(rx == 0) << 1)
            | (u32::from(rx == self.capacity) << 2)
            | (u32::from(tx == 0) << 3)
            | (u32::from(tx == self.capacity) << 4)
            | (u32::from(self.tx_trigger != 0 && tx >= self.tx_trigger) << 10)
            | (u32::from(tx + 1 == self.capacity) << 11)
    }

    fn update_sticky(&mut self) {
        self.sticky |= self.levels();
    }

    pub fn irq(&self) -> bool {
        self.mask & self.sticky != 0
    }

    pub fn read_register(&mut self, offset: u64) -> u32 {
        match offset {
            0x00 => self.control,
            0x04 => self.mode,
            0x10 => self.mask,
            0x14 => {
                let flags = self.sticky;
                if self.clear_on_read {
                    self.sticky = self.levels();
                }
                flags
            }
            0x20 => self.rx_trigger as u32,
            0x2c => {
                let levels = self.levels();
                (levels & 0x1f) | ((levels & (1 << 10)) << 4)
            }
            0x30 => {
                let byte = self.rx.pop_front().unwrap_or_default();
                self.update_sticky();
                byte.into()
            }
            0x44 => self.tx_trigger as u32,
            _ => 0,
        }
    }

    pub fn write_register(&mut self, offset: u64, value: u32) {
        match offset {
            0x00 => {
                self.control = value & 0x1bc;
                if value & 1 != 0 {
                    self.rx.clear();
                }
                if value & 2 != 0 {
                    self.tx.clear();
                }
                if value & 0x40 != 0 {
                    self.sticky |= RX_TIMEOUT;
                }
                self.flush_tx();
                self.update_sticky();
            }
            0x04 => self.mode = value & 0x33ff,
            0x08 => self.mask |= value & IRQ_FLAGS,
            0x0c => self.mask &= !value,
            0x14 if !self.clear_on_read => {
                self.sticky &= !value;
                self.update_sticky();
            }
            0x20 => {
                self.rx_trigger = (value & 0x3f) as usize;
                self.update_sticky();
            }
            0x30 => self.transmit_character(value as u8),
            0x44 => {
                self.tx_trigger = (value & 0x3f) as usize;
                self.update_sticky();
            }
            _ => {}
        }
    }
}

impl NativePeripheral for CadenceUart {
    fn read(&mut self, _: Option<&str>, offset: u64) -> u64 {
        self.read_register(offset).into()
    }

    fn write(&mut self, _: Option<&str>, offset: u64, value: u64) {
        self.write_register(offset, value as u32);
    }
}
