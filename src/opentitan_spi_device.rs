use crate::{bus_translation::NativePeripheral, dma_engine::Width};

const CAPACITY: usize = 2048;
const BUFFER: u64 = 0x1000;

#[derive(Default)]
struct Fifo {
    read: usize,
    write: usize,
}

impl Fifo {
    fn depth(&self) -> usize {
        (self.write + 2 * CAPACITY - self.read) % (2 * CAPACITY)
    }

    fn pointers(&self) -> u64 {
        self.read as u64 | (self.write as u64) << 16
    }

    fn push(&mut self, storage: &mut [u8], value: u8) -> bool {
        if self.depth() == CAPACITY {
            return false;
        }
        storage[self.write % CAPACITY] = value;
        self.write = (self.write + 1) % (2 * CAPACITY);
        true
    }

    fn pop(&mut self, storage: &[u8]) -> Option<u8> {
        if self.depth() == 0 {
            return None;
        }
        let value = storage[self.read % CAPACITY];
        self.read = (self.read + 1) % (2 * CAPACITY);
        Some(value)
    }
}

pub struct SpiDevice {
    sram: [u8; 2 * CAPACITY],
    rx: Fifo,
    tx: Fifo,
    control: u32,
    watermark: u32,
}

impl Default for SpiDevice {
    fn default() -> Self {
        Self {
            sram: [0; 2 * CAPACITY],
            rx: Fifo::default(),
            tx: Fifo::default(),
            control: 0x8000_0010,
            watermark: 0x80,
        }
    }
}

impl SpiDevice {
    pub fn transmit(&mut self, incoming: u8) -> u8 {
        let (rx, tx) = self.sram.split_at_mut(CAPACITY);
        let output = self.tx.pop(tx).unwrap_or(0);
        self.rx.push(rx, incoming);
        output
    }

    pub fn finish_transmission(&mut self) {}
}

impl NativePeripheral for SpiDevice {
    fn read(&mut self, _: Option<&str>, offset: u64) -> u64 {
        match offset {
            0x10 => self.control.into(),
            0x18 => self.watermark.into(),
            0x1c => (self.rx.depth() & 0xff) as u64 | ((self.tx.depth() & 0xff) as u64) << 16,
            0x24 => self.rx.pointers(),
            0x28 => self.tx.pointers(),
            0x2c => 0x01fc_0000,
            0x30 => 0x03fc_0200,
            offset if offset >= BUFFER => {
                let index = (offset - BUFFER) as usize;
                self.sram
                    .get(index..index.saturating_add(4))
                    .map_or(0, |data| {
                        u32::from_le_bytes(data.try_into().unwrap()).into()
                    })
            }
            _ => 0,
        }
    }

    fn write(&mut self, _: Option<&str>, offset: u64, value: u64) {
        match offset {
            0x10 => {
                self.control = value as u32;
                if value & (1 << 16) != 0 {
                    self.tx = Fifo::default();
                }
                if value & (1 << 17) != 0 {
                    self.rx = Fifo::default();
                }
            }
            0x18 => self.watermark = value as u32,
            0x24 => self.rx.read = value as usize & (2 * CAPACITY - 1),
            0x28 => self.tx.write = (value >> 16) as usize & (2 * CAPACITY - 1),
            offset if offset >= BUFFER => {
                let index = (offset - BUFFER) as usize;
                if (CAPACITY..2 * CAPACITY).contains(&index) {
                    for byte in (value as u32).to_be_bytes() {
                        self.tx.push(&mut self.sram[CAPACITY..], byte);
                    }
                } else if let Some(data) = self.sram.get_mut(index..index.saturating_add(4)) {
                    data.copy_from_slice(&(value as u32).to_le_bytes());
                }
            }
            _ => {}
        }
    }

    fn read_width(&mut self, region: Option<&str>, offset: u64, width: Width) -> u64 {
        if width == Width::Byte {
            return offset
                .checked_sub(BUFFER)
                .and_then(|index| self.sram.get(index as usize))
                .copied()
                .unwrap_or_default()
                .into();
        }
        self.read(region, offset)
    }

    fn write_width(&mut self, region: Option<&str>, offset: u64, width: Width, value: u64) {
        if width == Width::Byte {
            if let Some(byte) = offset
                .checked_sub(BUFFER)
                .and_then(|index| self.sram.get_mut(index as usize))
            {
                *byte = value as u8;
            }
        } else {
            self.write(region, offset, value);
        }
    }
}
