use std::{collections::VecDeque, io::{self, Read, Write}};

const RX_INTERRUPT: u16 = 1 << 4;
const TX_INTERRUPT: u16 = 1 << 5;

#[derive(Clone, Copy)]
pub(crate) enum InterruptRoute {
    Nvic(u8),
    Gic(u16),
}

#[derive(Clone)]
pub(crate) struct Pl011 {
    pub(crate) base: u32,
    pub(crate) name: String,
    pub(crate) irq: Option<InterruptRoute>,
    fifo_size: u32,
    receive: VecDeque<u8>,
    control: u16,
    line_control: u8,
    ir_low_power: u8,
    integer_baud: u16,
    fractional_baud: u8,
    fifo_levels: u8,
    dma_control: u8,
    irq_mask: u16,
    tx_pending: bool,
    rx_threshold: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uart_enable_fifo_threshold_and_mask_control_interrupts() {
        let mut uart = Pl011::new("uart".into(), 0x1000, 16, false, None);
        let base = uart.base;
        assert_eq!(uart.read(base + 0x18), 0x90);
        assert_eq!(uart.write(base, b'A'.into()), None);
        uart.queue_input([b'A']);
        assert_eq!(uart.read(base), 0);
        uart.write(base + 0x30, 0x301);
        uart.write(base + 0x2c, 0x70);
        uart.write(base + 0x38, u32::from(RX_INTERRUPT));
        uart.queue_input([b'A'; 7]);
        assert!(!uart.interrupt());
        uart.queue_input([b'B']);
        assert!(uart.interrupt());
        assert_eq!(uart.read(base), u32::from(b'A'));
        assert!(!uart.interrupt());
        assert_eq!(uart.write(base, b'C'.into()), Some(b'C'));
        uart.write(base + 0x38, u32::from(TX_INTERRUPT));
        assert!(uart.interrupt());
        uart.write(base + 0x44, u32::from(TX_INTERRUPT));
        assert!(!uart.interrupt());
    }
}

impl Pl011 {
    pub(crate) const SIZE: u32 = 0x1000;

    pub(crate) fn new(
        name: String,
        base: u32,
        fifo_size: u32,
        sbsa: bool,
        irq: Option<InterruptRoute>,
    ) -> Self {
        let mut uart = Self {
            base,
            name,
            irq,
            fifo_size,
            receive: VecDeque::new(),
            control: if sbsa { 0x301 } else { 0x300 },
            line_control: if sbsa { 0x70 } else { 0 },
            ir_low_power: 0,
            integer_baud: 0,
            fractional_baud: 0,
            fifo_levels: 0x12,
            dma_control: 0,
            irq_mask: 0,
            tx_pending: false,
            rx_threshold: 0,
        };
        uart.update_rx_threshold();
        uart
    }

    pub(crate) fn save_snapshot(&self, output: &mut impl Write) -> io::Result<()> {
        use crate::snapshot::{write_text, write_words};
        write_text(output, &self.name)?;
        write_words(output, &[
            u64::from(self.base), u64::from(self.fifo_size), u64::from(self.control),
            u64::from(self.line_control), u64::from(self.ir_low_power), u64::from(self.integer_baud),
            u64::from(self.fractional_baud), u64::from(self.fifo_levels), u64::from(self.dma_control),
            u64::from(self.irq_mask), u64::from(self.tx_pending), self.receive.len() as u64,
        ])?;
        let (first, second) = self.receive.as_slices();
        output.write_all(first)?;
        output.write_all(second)
    }

    pub(crate) fn load_snapshot(&mut self, input: &mut impl Read) -> io::Result<()> {
        use crate::snapshot::{invalid, read_bool, read_text, read_words};
        let name = read_text(input)?;
        let [base, fifo_size, control, line_control, ir_low_power, integer_baud, fractional_baud,
            fifo_levels, dma_control, irq_mask, tx_pending, receive] = read_words(input)?;
        if self.name != name || u64::from(self.base) != base || u64::from(self.fifo_size) != fifo_size {
            return Err(invalid("snapshot PL011 differs from the platform"));
        }
        // Host input may queue multiple bytes before firmware drains the UART.
        if receive > 16 * 1024 * 1024 {
            return Err(invalid("snapshot PL011 input exceeds size limit"));
        }
        let byte = |value| u8::try_from(value).map_err(io::Error::other);
        let halfword = |value| u16::try_from(value).map_err(io::Error::other);
        self.control = halfword(control)?;
        self.line_control = byte(line_control)?;
        self.ir_low_power = byte(ir_low_power)?;
        self.integer_baud = halfword(integer_baud)?;
        self.fractional_baud = byte(fractional_baud)?;
        self.fifo_levels = byte(fifo_levels)?;
        self.dma_control = byte(dma_control)?;
        self.irq_mask = halfword(irq_mask)?;
        self.tx_pending = read_bool(tx_pending)?;
        let mut bytes = vec![0; receive as usize];
        input.read_exact(&mut bytes)?;
        self.receive = bytes.into();
        self.update_rx_threshold();
        Ok(())
    }

    pub(crate) fn contains(&self, address: u32) -> bool {
        address & 3 == 0 && address.wrapping_sub(self.base) < Self::SIZE
    }

    fn capacity(&self) -> u32 {
        if self.line_control & 0x10 == 0 {
            1
        } else {
            self.fifo_size
        }
    }

    fn update_rx_threshold(&mut self) {
        if let Some(numerator) = [1, 2, 4, 6, 7].get(usize::from(self.fifo_levels >> 3)) {
            self.rx_threshold = (u64::from(self.capacity()) * numerator).div_ceil(8);
        }
    }

    fn raw_interrupts(&self) -> u16 {
        (u16::from(self.receive.len() as u64 >= self.rx_threshold) * RX_INTERRUPT)
            | (u16::from(self.tx_pending) * TX_INTERRUPT)
    }

    pub(crate) fn interrupt(&self) -> bool {
        self.raw_interrupts() & self.irq_mask != 0
    }

    pub(crate) fn queue_input(&mut self, bytes: impl IntoIterator<Item = u8>) {
        if self.control & 0x201 == 0x201 {
            self.receive.extend(bytes);
        }
    }

    pub(crate) fn read(&mut self, address: u32) -> u32 {
        let offset = address - self.base;
        match offset {
            0 => u32::from(self.receive.pop_front().unwrap_or(0)),
            0x18 => {
                0x80 | (u32::from(self.receive.is_empty()) << 4)
                    | (u32::from(self.receive.len() as u64 >= u64::from(self.capacity())) << 6)
            }
            0x20 => u32::from(self.ir_low_power),
            0x24 => u32::from(self.integer_baud),
            0x28 => u32::from(self.fractional_baud),
            0x2c => u32::from(self.line_control),
            0x30 => u32::from(self.control),
            0x34 => u32::from(self.fifo_levels),
            0x38 => u32::from(self.irq_mask),
            0x3c => u32::from(self.raw_interrupts()),
            0x40 => u32::from(self.raw_interrupts() & self.irq_mask),
            0x48 => u32::from(self.dma_control),
            0xfe0..=0xffc => {
                [0x11, 0x10, 0x34, 0, 0x0d, 0xf0, 0x05, 0xb1][((offset - 0xfe0) / 4) as usize]
            }
            _ => 0,
        }
    }

    pub(crate) fn write(&mut self, address: u32, value: u32) -> Option<u8> {
        match address - self.base {
            0 if self.control & 0x101 == 0x101 => {
                self.tx_pending = true;
                if self.control & 0x80 == 0 {
                    return Some(value as u8);
                }
            }
            0x20 => self.ir_low_power = value as u8,
            0x24 => self.integer_baud = value as u16,
            0x28 => self.fractional_baud = value as u8 & 0x3f,
            0x2c => {
                if self.line_control & 0x10 != 0 && value & 0x10 == 0 {
                    self.tx_pending = true;
                }
                self.line_control = value as u8;
                self.update_rx_threshold();
            }
            0x30 => {
                if self.control & 1 == 0 && value & 1 != 0 {
                    self.tx_pending = true;
                }
                self.control = value as u16;
            }
            0x34 => {
                self.fifo_levels = value as u8 & 0x3f;
                self.update_rx_threshold();
            }
            0x38 => self.irq_mask = value as u16 & 0x7ff,
            0x44 if value & u32::from(TX_INTERRUPT) != 0 => self.tx_pending = false,
            0x48 => self.dma_control = value as u8 & 7,
            _ => {}
        }
        None
    }
}
