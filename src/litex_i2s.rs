use std::{
    collections::VecDeque,
    fs::File,
    io::{self, BufWriter, Write},
};

const FIFO_DEPTH: usize = 512;
const READY_THRESHOLD: usize = 256;
const PUSH_INTERVAL: u64 = 1_000_000; // 10 ms at 100 MHz

pub struct I2s {
    pub name: String,
    pub csr: u32,
    pub buffer: u32,
    pub buffer_size: u32,
    pub irq: (usize, u8),
    pub transmit: bool,
    pub sample_bytes: usize,
    pub rate: u32,
    pub channels: u32,
    fifo: VecDeque<u32>,
    enabled: bool,
    events: u32,
    step: u64,
    tx_irq_after: u64,
    error: bool,
    input: Vec<u8>,
    position: usize,
    next_push: Option<u64>,
    output: Option<BufWriter<File>>,
}

impl I2s {
    pub const SIZE: u32 = 0x100;

    pub fn new(
        name: String,
        csr: u32,
        buffer: (u32, u32),
        irq: (usize, u8),
        transmit: bool,
    ) -> Self {
        Self {
            name,
            csr,
            buffer: buffer.0,
            buffer_size: buffer.1,
            irq,
            transmit,
            sample_bytes: 3,
            rate: 44_100,
            channels: 2,
            fifo: VecDeque::new(),
            enabled: false,
            events: 0,
            step: 0,
            tx_irq_after: 0,
            error: false,
            input: Vec::new(),
            position: 0,
            next_push: None,
            output: None,
        }
    }

    pub fn load_pcm(&mut self, input: Vec<u8>) -> Result<(), String> {
        if !input.len().is_multiple_of(self.sample_bytes) {
            return Err("LiteX I2S input ends with a partial PCM sample".into());
        }
        self.input = input;
        self.position = 0;
        Ok(())
    }

    pub fn set_output(&mut self, file: File) {
        self.output = Some(BufWriter::new(file));
    }

    pub fn flush(&mut self) -> io::Result<()> {
        if let Some(output) = &mut self.output {
            output.flush()?;
        }
        Ok(())
    }

    pub fn tick(&mut self, step: u64) {
        self.step = step;
        if self.transmit || !self.enabled || self.position == self.input.len() {
            return;
        }
        let deadline = self
            .next_push
            .get_or_insert(step.saturating_add(PUSH_INTERVAL));
        if step < *deadline {
            return;
        }
        *deadline = deadline.saturating_add(PUSH_INTERVAL);
        // The upstream pusher delivers 10 ms batches with a whole samples/ms rate.
        for _ in 0..10 * (self.rate / 1000) * self.channels {
            let Some(sample) = self
                .input
                .get(self.position..self.position + self.sample_bytes)
            else {
                break;
            };
            let mut bytes = [0; 4];
            bytes[4 - self.sample_bytes..].copy_from_slice(sample);
            self.position += self.sample_bytes;
            self.fifo.push_back(u32::from_be_bytes(bytes));
        }
    }

    fn pending(&self) -> u32 {
        let ready = if self.transmit {
            self.fifo.len() < READY_THRESHOLD
        } else {
            self.fifo.len() >= READY_THRESHOLD
        };
        u32::from(ready) | (u32::from(self.error) << 1)
    }

    pub fn interrupt_pending(&self) -> bool {
        self.enabled && self.step >= self.tx_irq_after && self.pending() & self.events != 0
    }

    pub fn read(&mut self, address: u32) -> Option<u32> {
        if address.wrapping_sub(self.buffer) < self.buffer_size {
            return Some(self.fifo.pop_front().unwrap_or_else(|| {
                self.error = true;
                0
            }));
        }
        let offset = address.wrapping_sub(self.csr);
        if offset >= Self::SIZE {
            return None;
        }
        Some(match offset {
            4 => self.pending(),
            8 => self.events,
            12 => u32::from(self.enabled),
            0x20 => (self.rate >> 16) & 0xff,
            0x24 => (self.rate >> 8) & 0xff,
            0x28 => self.rate & 0xff,
            0x2c => (self.sample_bytes as u32 * 8) << 2 | 1,
            _ => 0,
        })
    }

    pub fn write(&mut self, address: u32, value: u32) -> io::Result<bool> {
        if address.wrapping_sub(self.buffer) < self.buffer_size {
            if self.transmit {
                if let Some(output) = &mut self.output {
                    output.write_all(&value.to_be_bytes()[4 - self.sample_bytes..])?;
                }
            } else if self.fifo.len() < FIFO_DEPTH {
                self.fifo.push_back(value);
            } else {
                self.error = true;
            }
            return Ok(true);
        }
        let offset = address.wrapping_sub(self.csr);
        if offset >= Self::SIZE {
            return Ok(false);
        }
        match offset {
            4 => self.error &= value & 2 == 0,
            8 => {
                if self.transmit && self.events & 1 == 0 && value & 1 != 0 {
                    // Allow the CSR write to propagate to the transmitter IRQ.
                    self.tx_irq_after = self.step.saturating_add(3);
                }
                self.events = value & 3;
            }
            12 => {
                let enabled = value & 1 != 0;
                if self.enabled != enabled {
                    self.next_push = None;
                }
                self.enabled = enabled;
                if value & 2 != 0 {
                    self.fifo.clear();
                }
            }
            _ => {}
        }
        Ok(true)
    }
}
