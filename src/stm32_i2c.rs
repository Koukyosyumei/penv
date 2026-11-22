const ENABLE: u32 = 1;
const TX_READY: u32 = 1 << 1;
const RX_READY: u32 = 1 << 2;
const NACK: u32 = 1 << 4;
const STOPPED: u32 = 1 << 5;
const COMPLETE: u32 = 1 << 6;
const RELOAD_READY: u32 = 1 << 7;
const READ: u32 = 1 << 10;
const START: u32 = 1 << 13;
const STOP: u32 = 1 << 14;
const BUSY: u32 = 1 << 15;
const RELOAD: u32 = 1 << 24;
const AUTO_STOP: u32 = 1 << 25;

pub struct Stm32I2c {
    pub base: u32,
    pub irq: Option<u8>,
    pub dma_rx: u8,
    pub dma_tx: u8,
    registers: [u32; 11],
    status: u32,
    targets: Vec<(u32, Eeprom)>,
    addressed: Option<usize>,
    remaining: u32,
    reading: bool,
}

pub struct Eeprom {
    data: Vec<u8>,
    pointer: usize,
    address_bytes: usize,
    address_remaining: usize,
    address_latch: usize,
    page_size: usize,
    writable: bool,
}

impl Eeprom {
    pub fn new(size: usize, address_bits: usize, writable: bool, page_size: usize) -> Self {
        Self {
            data: vec![0xff; size],
            pointer: 0,
            address_bytes: address_bits / 8,
            address_remaining: 0,
            address_latch: 0,
            page_size,
            writable,
        }
    }

    fn start_write(&mut self) {
        self.address_remaining = self.address_bytes;
        self.address_latch = 0;
    }

    fn write(&mut self, value: u8) {
        if self.address_remaining != 0 {
            self.address_remaining -= 1;
            self.address_latch = (self.address_latch << 8) | usize::from(value);
            if self.address_remaining == 0 {
                self.pointer = self.address_latch % self.data.len();
            }
        } else if self.writable {
            self.data[self.pointer] = value;
            let page = self.pointer & !(self.page_size - 1);
            let end = (page + self.page_size).min(self.data.len());
            self.pointer = if self.pointer + 1 < end {
                self.pointer + 1
            } else {
                page
            };
        }
    }

    fn read(&mut self) -> u8 {
        let value = self.data[self.pointer];
        self.pointer = (self.pointer + 1) % self.data.len();
        value
    }
}

impl Stm32I2c {
    pub const SIZE: u32 = 0x400;

    pub fn new(base: u32) -> Self {
        Self {
            base,
            irq: None,
            dma_rx: 0,
            dma_tx: 0,
            registers: [0; 11],
            status: 0,
            targets: Vec::new(),
            addressed: None,
            remaining: 0,
            reading: false,
        }
    }

    pub fn attach(&mut self, address: u32, target: Eeprom) -> Result<(), String> {
        if address >= 1024 || self.targets.iter().any(|&(other, _)| other == address) {
            return Err(format!(
                "invalid or duplicate I²C target address {address:#x}"
            ));
        }
        self.targets.push((address, target));
        Ok(())
    }

    pub fn reset(&mut self) {
        self.registers = [0; 11];
        self.status = 0;
        self.addressed = None;
        self.remaining = 0;
        self.reading = false;
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    pub fn interrupt_pending(&self) -> bool {
        let irqs = self.registers[0];
        self.status & irqs & (TX_READY | RX_READY | NACK | STOPPED | COMPLETE) != 0
            || (self.status & RELOAD_READY != 0 && irqs & COMPLETE != 0)
    }

    pub fn pending_dma(&self) -> u8 {
        let control = self.registers[0];
        let rx = if control & (1 << 15) != 0 && self.status & RX_READY != 0 {
            self.dma_rx
        } else {
            0
        };
        let tx = if control & (1 << 14) != 0 && self.status & TX_READY != 0 {
            self.dma_tx
        } else {
            0
        };
        rx | tx
    }

    pub fn read(&mut self, address: u32) -> u32 {
        let offset = address - self.base;
        match offset {
            0x18 => self.status | 1,
            0x1c => 0,
            0x24 if self.status & RX_READY != 0 => {
                let value = self
                    .addressed
                    .map_or(0, |target| self.targets[target].1.read());
                self.advance_byte();
                value.into()
            }
            0x24 => 0,
            _ => self
                .registers
                .get((offset / 4) as usize)
                .copied()
                .unwrap_or(0),
        }
    }

    pub fn write(&mut self, address: u32, value: u32) {
        match address - self.base {
            0x00 => {
                self.registers[0] = value & 0x00ff_dfff;
                if value & ENABLE == 0 {
                    self.addressed = None;
                    self.remaining = 0;
                    self.status = 0;
                }
            }
            0x04 => {
                self.registers[1] = value & 0x07ff_9fff;
                if self.registers[0] & ENABLE == 0 {
                    return;
                }
                match value & (START | STOP) {
                    START => self.start(),
                    STOP => self.stop(),
                    0 if self.status & RELOAD_READY != 0 => self.load_count(),
                    _ => {}
                }
            }
            0x08 => self.registers[2] = value & 0x87ff,
            0x0c => self.registers[3] = value & 0x87fe,
            0x10 => self.registers[4] = value & 0xf0ff_ffff,
            0x14 => self.registers[5] = value & 0x8fff_9fff,
            0x1c => self.status &= !(value & 0x3f38),
            0x28 => {
                self.registers[10] = value & 0xff;
                if self.status & TX_READY != 0 {
                    if let Some(target) = self.addressed {
                        self.targets[target].1.write(value as u8);
                    }
                    self.advance_byte();
                }
            }
            _ => {}
        }
    }

    fn start(&mut self) {
        self.status &= NACK | STOPPED;
        self.reading = self.registers[1] & READ != 0;
        let control = self.registers[1];
        let address = if control & (1 << 11) != 0 {
            control & 0x3ff
        } else {
            (control >> 1) & 0x7f
        };
        self.addressed = self
            .targets
            .iter()
            .position(|&(target, _)| address == target);
        if let Some(target) = self.addressed {
            if !self.reading {
                self.targets[target].1.start_write();
            }
            self.status |= BUSY;
            self.load_count();
        } else {
            self.status |= NACK;
            self.stop();
        }
    }

    fn load_count(&mut self) {
        self.remaining = (self.registers[1] >> 16) & 0xff;
        self.set_transfer_state();
    }

    fn advance_byte(&mut self) {
        self.remaining -= 1;
        self.set_transfer_state();
    }

    fn set_transfer_state(&mut self) {
        self.status &= !(RX_READY | TX_READY | COMPLETE | RELOAD_READY);
        if self.remaining != 0 {
            self.status |= if self.reading { RX_READY } else { TX_READY };
        } else if self.registers[1] & RELOAD != 0 {
            self.status |= RELOAD_READY;
        } else if self.registers[1] & AUTO_STOP != 0 {
            self.stop();
        } else {
            self.status |= COMPLETE;
        }
    }

    fn stop(&mut self) {
        self.status &= !(BUSY | RX_READY | TX_READY | COMPLETE | RELOAD_READY);
        self.status |= STOPPED;
        self.addressed = None;
        self.remaining = 0;
    }
}
