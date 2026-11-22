use std::{
    collections::VecDeque,
    io::{self, Write},
};

pub struct MuraxUart {
    pub base: u32,
    irq_connected: bool,
    receive: VecDeque<u8>,
    receive_irq_enabled: bool,
    frame: u32,
}

impl MuraxUart {
    pub const SIZE: u64 = 16;

    pub fn new(base: u32, irq_connected: bool) -> Self {
        Self {
            base,
            irq_connected,
            receive: VecDeque::new(),
            receive_irq_enabled: false,
            frame: 0,
        }
    }

    pub fn contains(&self, address: u32) -> bool {
        u64::from(address.wrapping_sub(self.base)) < Self::SIZE
    }

    pub fn queue_input(&mut self, bytes: Vec<u8>) {
        self.receive.extend(bytes);
    }

    fn data_ready(&self) -> bool {
        !self.receive.is_empty()
    }

    pub fn interrupt_pending(&self) -> bool {
        self.irq_connected && self.receive_irq_enabled && self.data_ready()
    }

    pub fn read(&mut self, address: u32) -> u32 {
        match address - self.base {
            0 if self.data_ready() => u32::from(self.receive.pop_front().unwrap_or(0)),
            4 => (u32::from(self.data_ready()) << 9) | (u32::from(self.receive_irq_enabled) << 1),
            12 => self.frame,
            _ => 0,
        }
    }

    pub fn write(&mut self, address: u32, value: u32, output: &mut impl Write) -> io::Result<()> {
        match address - self.base {
            0 => output.write_all(&[value as u8])?,
            4 => self.receive_irq_enabled = value & 2 != 0,
            12 => self.frame = value & 0x1_0300,
            _ => {}
        }
        Ok(())
    }
}

pub struct LiteXUart {
    pub base: u32,
    receive: VecDeque<u8>,
    pending: u32,
    enabled: u32,
}

impl LiteXUart {
    pub const SIZE: u32 = 0x100;

    pub fn new(base: u32) -> Self {
        Self {
            base,
            receive: VecDeque::new(),
            pending: 1,
            enabled: 0,
        }
    }

    pub fn offset(&self, address: u32, width: usize) -> Option<u32> {
        let offset = address.wrapping_sub(self.base);
        (offset < Self::SIZE && matches!(width, 1 | 4)).then_some(offset)
    }

    pub fn queue_input(&mut self, bytes: Vec<u8>) {
        if !bytes.is_empty() {
            self.receive.extend(bytes);
            self.pending |= 2;
        }
    }

    pub fn interrupt_pending(&self) -> bool {
        self.enabled & self.pending != 0
    }

    pub fn read(&mut self, offset: u32) -> u32 {
        match offset {
            0 => u32::from(self.receive.pop_front().unwrap_or(0)),
            8 => u32::from(self.receive.is_empty()),
            0x10 => self.pending,
            0x14 => self.enabled,
            _ => 0,
        }
    }

    pub fn write(&mut self, offset: u32, value: u32, output: &mut impl Write) -> io::Result<()> {
        match offset {
            0 => {
                output.write_all(&[value as u8])?;
                self.pending |= 1;
            }
            0x10 => self.pending &= !value,
            0x14 => self.enabled = value & 3,
            _ => {}
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
pub enum UartLineEnd {
    Lf,
    Cr,
}

pub struct UartLineMatch {
    end: u8,
    pattern: Vec<u8>,
    tail: VecDeque<u8>,
    contains: bool,
    pub matched: bool,
}

impl UartLineMatch {
    pub fn new(pattern: &str, end: UartLineEnd) -> Self {
        Self {
            end: match end {
                UartLineEnd::Lf => b'\n',
                UartLineEnd::Cr => b'\r',
            },
            pattern: pattern.as_bytes().to_vec(),
            tail: VecDeque::new(),
            contains: false,
            matched: false,
        }
    }

    pub fn receive(&mut self, byte: u8) {
        if self.matched {
            return;
        }
        if byte == self.end {
            self.matched = self.contains;
            self.tail.clear();
            self.contains = false;
        } else if !self.contains {
            if self.tail.len() == self.pattern.len() {
                self.tail.pop_front();
            }
            self.tail.push_back(byte);
            self.contains = self.tail.iter().eq(self.pattern.iter());
        }
    }
}
