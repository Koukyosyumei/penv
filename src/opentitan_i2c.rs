use std::collections::{BTreeMap, VecDeque};

use crate::{bus_translation::NativePeripheral, efr32_i2c::Slave};

const DEPTH: usize = 64;
const START: u32 = 0x100;
const STOP: u32 = 0x200;
const READ: u32 = 0xc00;
const ACQUIRE_OVERFLOW: u32 = 1 << 13;
const TRANSMIT_OVERFLOW: u32 = 1 << 12;
const TRANSACTION_COMPLETE: u32 = 1 << 9;

#[derive(Default)]
enum Transfer {
    #[default]
    Idle,
    Select(u8),
    Active(u8, u8),
    Missing,
}

#[derive(Default)]
pub struct OpenTitanI2c {
    slaves: BTreeMap<u8, Box<dyn Slave>>,
    mode: u32,
    interrupts: u32,
    interrupt_enable: u32,
    fifo_control: u32,
    format: VecDeque<u32>,
    rx: VecDeque<u8>,
    tx: VecDeque<u8>,
    acquired: VecDeque<u32>,
    host_transfer: Transfer,
    target_active: bool,
}

impl OpenTitanI2c {
    pub fn register(&mut self, address: u8, slave: impl Slave + 'static) {
        self.slaves.insert(address, Box::new(slave));
    }

    pub fn irq(&self, mask: u32) -> bool {
        self.interrupts & self.interrupt_enable & mask != 0
    }

    fn enqueue<T>(
        queue: &mut VecDeque<T>,
        item: T,
        flags: &mut u32,
        overflow: u32,
        watermark: Option<(usize, u32)>,
    ) {
        if queue.len() == DEPTH {
            *flags |= overflow;
            return;
        }
        queue.push_back(item);
        if let Some((level, flag)) = watermark {
            if queue.len() == level {
                *flags |= flag;
            }
        }
    }

    pub fn finish_transmission(&mut self) -> Result<(), &'static str> {
        if self.mode == 1 {
            return Err("host cannot receive a target STOP");
        }
        Self::enqueue(
            &mut self.acquired,
            STOP,
            &mut self.interrupts,
            ACQUIRE_OVERFLOW,
            None,
        );
        self.target_active = false;
        self.interrupts |= TRANSACTION_COMPLETE;
        Ok(())
    }

    fn run_commands(&mut self) {
        while let Some(command) = self.format.pop_front() {
            let data = command as u8;
            let flags = command & (START | STOP | READ);
            match self.host_transfer {
                Transfer::Idle if flags == START => {
                    self.host_transfer = if self.slaves.contains_key(&data) {
                        Transfer::Select(data)
                    } else {
                        self.interrupts |= 1 << 4;
                        Transfer::Missing
                    };
                }
                Transfer::Select(slave) if flags == 0 => {
                    self.host_transfer = Transfer::Active(slave, data);
                }
                Transfer::Active(slave, register) => {
                    let device = self.slaves.get_mut(&slave).unwrap();
                    if flags & READ != 0 {
                        device.write(&[register]);
                        let length = if data == 0 { 256 } else { usize::from(data) };
                        let level =
                            [1, 4, 8, 16, 30, 64, 64, 64][((self.fifo_control >> 2) & 7) as usize];
                        for byte in device.read(length) {
                            Self::enqueue(
                                &mut self.rx,
                                byte,
                                &mut self.interrupts,
                                1 << 3,
                                Some((level, 2)),
                            );
                        }
                    } else if flags == 0 {
                        device.write(&[register, data]);
                    } else {
                        continue;
                    }
                    if flags & STOP != 0 {
                        self.host_transfer = Transfer::Idle;
                    }
                }
                Transfer::Missing if flags & STOP != 0 => self.host_transfer = Transfer::Idle,
                _ => {}
            }
        }
    }
}

impl Slave for OpenTitanI2c {
    fn write(&mut self, bytes: &[u8]) {
        if self.mode == 1 || bytes.is_empty() {
            return;
        }
        let flags = START | if self.target_active { STOP } else { 0 };
        self.target_active = true;
        for (index, byte) in bytes.iter().enumerate() {
            let value = u32::from(*byte) | if index == 0 { flags } else { 0 };
            Self::enqueue(
                &mut self.acquired,
                value,
                &mut self.interrupts,
                ACQUIRE_OVERFLOW,
                None,
            );
        }
    }

    fn read(&mut self, count: usize) -> Vec<u8> {
        let count = count.min(self.tx.len());
        self.tx.drain(..count).collect()
    }
}

impl NativePeripheral for OpenTitanI2c {
    fn read(&mut self, _: Option<&str>, offset: u64) -> u64 {
        u64::from(match offset {
            0 => self.interrupts,
            4 => self.interrupt_enable,
            0x10 => self.mode,
            0x14 => {
                let flags = [
                    (0, self.format.len() == DEPTH),
                    (1, self.rx.len() == DEPTH),
                    (2, self.format.is_empty()),
                    (3, true),
                    (4, true),
                    (5, self.rx.is_empty()),
                    (6, self.tx.len() == DEPTH),
                    (7, self.acquired.len() == DEPTH),
                    (8, self.tx.is_empty()),
                    (9, self.acquired.is_empty()),
                ];
                flags
                    .into_iter()
                    .fold(0, |status, (bit, set)| status | (u32::from(set) << bit))
            }
            0x18 => self.rx.pop_front().unwrap_or_default().into(),
            0x20 => self.fifo_control,
            0x24 => {
                self.format.len() as u32
                    | ((self.tx.len() as u32) << 8)
                    | ((self.rx.len() as u32) << 16)
                    | ((self.acquired.len() as u32) << 24)
            }
            0x4c => self.acquired.pop_front().unwrap_or_default(),
            _ => 0,
        })
    }

    fn write(&mut self, _: Option<&str>, offset: u64, value: u64) {
        let value = value as u32;
        match offset {
            0 => self.interrupts &= !value,
            4 => self.interrupt_enable = value,
            8 => self.interrupts |= value,
            0x10 => {
                self.mode = match value & 3 {
                    3 => 1,
                    mode => mode,
                };
                if self.mode == 1 {
                    self.run_commands();
                }
            }
            0x1c if self.mode != 2 => {
                let level = [1, 4, 8, 16][((self.fifo_control >> 5) & 3) as usize];
                Self::enqueue(
                    &mut self.format,
                    value,
                    &mut self.interrupts,
                    4,
                    Some((level, 1)),
                );
                if self.mode == 1 {
                    self.run_commands();
                }
            }
            0x20 => {
                self.fifo_control = value & 0x7c;
                if value & 1 != 0 {
                    self.rx.clear();
                }
                if value & 2 != 0 {
                    self.format.clear();
                }
                if value & 0x80 != 0 {
                    self.acquired.clear();
                }
                if value & 0x100 != 0 {
                    self.tx.clear();
                }
            }
            0x50 if self.mode != 1 => {
                Self::enqueue(
                    &mut self.tx,
                    value as u8,
                    &mut self.interrupts,
                    TRANSMIT_OVERFLOW,
                    None,
                );
            }
            _ => {}
        }
    }
}
