use std::{
    collections::VecDeque,
    io::{self, Write},
    sync::mpsc::{Receiver, TryRecvError},
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

pub struct LiteXTxTiming {
    pub capacity: usize,
    pub flush_steps: u64,
    pub timeout_steps: u64,
}

impl Default for LiteXTxTiming {
    fn default() -> Self {
        Self {
            capacity: 8,
            flush_steps: 10,
            timeout_steps: 20_000_000,
        }
    }
}

struct LiteXTransmit {
    timing: LiteXTxTiming,
    fifo: Vec<u8>,
    deadline: u64,
    step: u64,
}

struct LiteXLiveInput {
    receiver: Receiver<io::Result<Vec<u8>>>,
    bytes: std::vec::IntoIter<u8>,
    next_step: u64,
}

pub struct LiteXUart {
    pub bases: Vec<u32>,
    pub vex_irq: Option<(usize, u8)>,
    receive: VecDeque<u8>,
    pending: u32,
    enabled: u32,
    irq: bool,
    legacy_console_events: bool,
    live_input: Option<LiteXLiveInput>,
    transmit: Option<LiteXTransmit>,
}

impl LiteXUart {
    pub const SIZE: u32 = 0x100;

    pub fn new(bases: Vec<u32>, vex_irq: Option<(usize, u8)>) -> Self {
        Self {
            bases,
            vex_irq,
            receive: VecDeque::new(),
            pending: 1,
            enabled: 0,
            irq: false,
            legacy_console_events: false,
            live_input: None,
            transmit: None,
        }
    }

    pub fn legacy_console_events(&mut self) {
        self.legacy_console_events = true;
    }

    pub fn with_timing(mut self, timing: Option<LiteXTxTiming>) -> Self {
        if let Some(timing) = timing.filter(|timing| timing.capacity > 0) {
            self.transmit = Some(LiteXTransmit {
                timing,
                fifo: Vec::new(),
                deadline: u64::MAX,
                step: 0,
            });
        }
        self
    }

    pub fn attach_live_input(&mut self, receiver: Receiver<io::Result<Vec<u8>>>) {
        self.live_input = Some(LiteXLiveInput {
            receiver,
            bytes: Vec::new().into_iter(),
            next_step: 0,
        });
    }

    fn tick_live_input(&mut self, step: u64) -> io::Result<()> {
        // LiteX reports 115200 baud. A byte occupies ten bits on the wire.
        const BYTE_STEPS: u64 = (100_000_000u64 * 10).div_ceil(115_200);
        const IDLE_POLL_STEPS: u64 = 1024;
        let Some(input) = self.live_input.as_mut() else {
            return Ok(());
        };
        if step < input.next_step {
            return Ok(());
        }
        if input.bytes.len() == 0 {
            match input.receiver.try_recv() {
                Ok(bytes) => input.bytes = bytes?.into_iter(),
                Err(TryRecvError::Empty) => {
                    input.next_step = step.saturating_add(IDLE_POLL_STEPS);
                    return Ok(());
                }
                Err(TryRecvError::Disconnected) => {
                    self.live_input = None;
                    return Ok(());
                }
            }
        }
        if let Some(byte) = input.bytes.next() {
            input.next_step = step.saturating_add(BYTE_STEPS);
            self.receive.push_back(byte);
            self.pending |= 2;
            self.update_irq();
        }
        Ok(())
    }

    pub fn tick(&mut self, step: u64, output: &mut impl Write) -> io::Result<()> {
        self.tick_live_input(step)?;
        let Some(tx) = self.transmit.as_mut() else {
            return Ok(());
        };
        tx.step = step;
        if step >= tx.deadline && !tx.fifo.is_empty() {
            output.write_all(&tx.fifo)?;
            tx.fifo.clear();
            tx.deadline = u64::MAX;
            self.pending |= 1;
            self.update_irq();
        }
        Ok(())
    }

    fn update_irq(&mut self) {
        self.irq = self.enabled & self.pending != 0;
    }

    pub fn offset(&self, address: u32, width: usize) -> Option<u32> {
        if !matches!(width, 1 | 4) {
            return None;
        }
        self.bases.iter().find_map(|base| {
            let offset = address.wrapping_sub(*base);
            (offset < Self::SIZE).then_some(offset)
        })
    }

    pub fn queue_input(&mut self, bytes: Vec<u8>) {
        if !bytes.is_empty() {
            self.receive.extend(bytes);
            self.pending |= 2;
            self.update_irq();
        }
    }

    pub fn interrupt_pending(&self) -> bool {
        self.irq
    }

    pub fn read(&mut self, offset: u32) -> u32 {
        match offset {
            0 => {
                let byte = self.receive.pop_front().unwrap_or(0);
                if self.receive.is_empty() {
                    self.update_irq();
                }
                u32::from(byte)
            }
            4 => u32::from(
                self.transmit
                    .as_ref()
                    .is_some_and(|tx| tx.fifo.len() >= tx.timing.capacity),
            ),
            8 => u32::from(self.receive.is_empty()),
            // Legacy interrupt handlers may poll this until the IRQ deasserts.
            0x10 if self.legacy_console_events => self.pending & self.enabled,
            0x10 => self.pending,
            0x14 => self.enabled,
            _ => 0,
        }
    }

    pub fn write(&mut self, offset: u32, value: u32, output: &mut impl Write) -> io::Result<()> {
        match offset {
            0 if self.transmit.is_some() => {
                let tx = self.transmit.as_mut().unwrap();
                if tx.fifo.len() < tx.timing.capacity {
                    tx.fifo.push(value as u8);
                    let full = tx.fifo.len() >= tx.timing.capacity;
                    self.pending = (self.pending & !1) | u32::from(!full);
                    let delay = if full {
                        tx.timing.flush_steps
                    } else {
                        tx.timing.timeout_steps
                    };
                    tx.deadline = tx.step.saturating_add(delay);
                }
            }
            0 => {
                output.write_all(&[value as u8])?;
                self.pending |= 1;
                self.update_irq();
            }
            0x10 => {
                self.pending &= !value;
                // The legacy ISR acknowledges RX after reading a single byte.
                // Keep the remaining queued bytes visible to that handler.
                if self.legacy_console_events && !self.receive.is_empty() {
                    self.pending |= 2;
                }
                self.update_irq();
            }
            0x14 => {
                self.enabled = value & 3;
                self.update_irq();
            }
            _ => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn litex_receives_boot_abort_and_shell_command_in_separate_events() {
        // Renode LiteX_Ibex.robot: Should Boot / Should Display Help (UART input).
        const RX: u32 = 2;
        let mut uart = LiteXUart::new(vec![0], Some((0, 2)));
        let mut output = std::io::sink();
        uart.write(0x14, RX, &mut output).unwrap();
        for input in [b"Q".as_slice(), b"help\r"] {
            assert_eq!(uart.read(8), 1);
            assert!(!uart.interrupt_pending());
            uart.queue_input(input.to_vec());
            assert!(uart.interrupt_pending());
            for &byte in input {
                assert_eq!(uart.read(8), 0);
                assert_eq!(uart.read(0), u32::from(byte));
            }
            assert_eq!(uart.read(8), 1);
            assert_ne!(uart.read(0x10) & RX, 0);
            uart.write(0x10, RX, &mut output).unwrap();
            assert_eq!(uart.read(0x10) & RX, 0);
            assert!(!uart.interrupt_pending());
        }
    }
}
