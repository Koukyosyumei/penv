use std::{
    collections::VecDeque,
    io::{self, Write},
};

pub struct Ns16550 {
    pub base: u32,
    pub source: Option<u32>,
    pub wide: bool,
    receive: VecDeque<u8>,
    ier: u8,
    lcr: u8,
    mcr: u8,
    scratch: u8,
    divisor: [u8; 2],
    fifo: bool,
    transmit_interrupt: bool,
}

impl Ns16550 {
    pub(crate) fn reset(&mut self) {
        *self = Self::new(self.base, self.source, self.wide);
    }

    pub fn new(base: u32, source: Option<u32>, wide: bool) -> Self {
        Self {
            base,
            source,
            wide,
            receive: VecDeque::new(),
            ier: 0,
            lcr: 0,
            mcr: 0,
            scratch: 0,
            divisor: [0; 2],
            fifo: false,
            transmit_interrupt: false,
        }
    }

    pub(crate) fn save_snapshot(&self, output: &mut impl Write) -> io::Result<()> {
        use crate::snapshot::write_words;
        write_words(output, &[
            u64::from(self.base), self.source.map_or(u64::MAX, u64::from), u64::from(self.wide),
            u64::from(self.fifo), u64::from(self.transmit_interrupt), self.receive.len() as u64,
        ])?;
        output.write_all(&[self.ier, self.lcr, self.mcr, self.scratch, self.divisor[0], self.divisor[1]])?;
        let (first, second) = self.receive.as_slices();
        output.write_all(first)?;
        output.write_all(second)
    }

    pub(crate) fn read_snapshot(&self, input: &mut impl io::Read) -> io::Result<Self> {
        use crate::snapshot::{invalid, read_bool, read_words};
        let [base, source, wide, fifo, transmit_interrupt, length] = read_words(input)?;
        if base != u64::from(self.base) || source != self.source.map_or(u64::MAX, u64::from)
            || read_bool(wide)? != self.wide || length > 1 << 20
        {
            return Err(invalid("invalid NS16550 snapshot configuration or receive buffer"));
        }
        let mut registers = [0; 6];
        input.read_exact(&mut registers)?;
        let [ier, lcr, mcr, scratch, low, high] = registers;
        let mut receive = vec![0; length as usize];
        input.read_exact(&mut receive)?;
        Ok(Self {
            base: self.base, source: self.source, wide: self.wide, receive: receive.into(),
            ier, lcr, mcr, scratch, divisor: [low, high], fifo: read_bool(fifo)?,
            transmit_interrupt: read_bool(transmit_interrupt)?,
        })
    }

    pub fn size(&self) -> u64 {
        if self.wide { 32 } else { 8 }
    }

    pub fn register(&self, address: u32, width: usize) -> Option<u32> {
        let offset = address.wrapping_sub(self.base);
        if u64::from(offset) >= self.size() {
            return None;
        }
        if !matches!(width, 1 | 2 | 4) {
            return None;
        }
        Some(if self.wide && offset & 3 == 0 { offset >> 2 } else { offset })
    }

    pub fn queue_input(&mut self, bytes: Vec<u8>) {
        self.receive.extend(bytes);
    }

    fn receive_ready(&self) -> bool {
        !self.receive.is_empty()
    }

    fn interrupt_id(&self) -> u8 {
        if self.ier & 1 != 0 && self.receive_ready() {
            4
        } else if self.ier & 2 != 0 && self.transmit_interrupt {
            2
        } else {
            1
        }
    }

    pub fn interrupt_pending(&self) -> bool {
        self.interrupt_id() != 1
    }

    pub fn read(&mut self, register: u32) -> u32 {
        let value = match register {
            0 | 1 if self.lcr & 0x80 != 0 => self.divisor[register as usize],
            0 if self.receive_ready() => self.receive.pop_front().unwrap_or(0),
            1 => self.ier,
            2 => {
                let id = self.interrupt_id();
                if id == 2 {
                    self.transmit_interrupt = false;
                }
                id | if self.fifo { 0xc0 } else { 0 }
            }
            3 => self.lcr,
            4 => self.mcr,
            5 => 0x60 | u8::from(self.receive_ready()),
            6 => 0xb0,
            7 => self.scratch,
            _ => 0,
        };
        u32::from(value)
    }

    pub fn write(&mut self, register: u32, value: u32, output: &mut impl Write) -> io::Result<()> {
        let value = value as u8;
        match register {
            0 | 1 if self.lcr & 0x80 != 0 => self.divisor[register as usize] = value,
            0 => {
                output.write_all(&[value])?;
                self.transmit_interrupt = true;
            }
            1 => {
                if self.ier & 2 == 0 && value & 2 != 0 {
                    self.transmit_interrupt = true;
                }
                self.ier = value & 15;
            }
            2 => {
                self.fifo = value & 1 != 0;
                if value & 2 != 0 {
                    self.receive.clear();
                }
                if value & 4 != 0 {
                    self.transmit_interrupt = true;
                }
            }
            3 => self.lcr = value,
            4 => self.mcr = value,
            7 => self.scratch = value,
            _ => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod snapshot_tests {
    use super::*;

    #[test]
    fn ns16550_snapshot_preserves_receive_order_and_interrupt_enable() {
        let mut uart = Ns16550::new(0x1000, Some(4), true);
        uart.queue_input(vec![0x41, 0x42]);
        uart.write(1, 1, &mut io::sink()).unwrap();
        let mut snapshot = Vec::new();
        uart.save_snapshot(&mut snapshot).unwrap();

        let initial = Ns16550::new(0x1000, Some(4), true);
        let mut restored = initial.read_snapshot(&mut snapshot.as_slice()).unwrap();
        assert!(restored.interrupt_pending());
        assert_eq!((restored.read(0), restored.read(0)), (0x41, 0x42));
        assert!(!restored.interrupt_pending());
    }
}
