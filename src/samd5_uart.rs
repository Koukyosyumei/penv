use std::collections::VecDeque;

const DRE: u8 = 1;
const TXC: u8 = 2;
const RXC: u8 = 4;
const RXS: u8 = 8;

pub(crate) struct Uart {
    pub(crate) base: u32,
    pub(crate) name: String,
    pub(crate) irq: Option<u8>,
    incoming: VecDeque<u8>,
    interrupt_enable: u8,
    flags: u8,
}

impl Uart {
    pub(crate) const SIZE: u32 = 0x100;

    pub(crate) fn new(base: u32, name: String, irq: Option<u8>) -> Self {
        Self {
            base,
            name,
            irq,
            incoming: VecDeque::new(),
            interrupt_enable: 0,
            flags: 0,
        }
    }

    pub(crate) fn reset(&mut self) {
        self.incoming.clear();
        self.interrupt_enable = 0;
        self.flags = 0;
    }

    pub(crate) fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    fn flags(&self) -> u8 {
        self.flags | if self.incoming.is_empty() { DRE } else { RXC }
    }

    pub(crate) fn interrupt(&self) -> bool {
        self.flags() & self.interrupt_enable != 0
    }

    pub(crate) fn queue_input(&mut self, bytes: impl Iterator<Item = u8>) {
        let original_len = self.incoming.len();
        self.incoming.extend(bytes);
        if self.incoming.len() > original_len {
            self.flags |= RXS;
        }
    }

    pub(crate) fn read(&mut self, address: u32) -> u8 {
        match address - self.base {
            0x14 | 0x16 => self.interrupt_enable,
            0x18 => self.flags(),
            0x28 => {
                let result = self.incoming.pop_front().unwrap_or(0);
                if self.incoming.is_empty() {
                    self.flags &= !RXS;
                }
                result
            }
            _ => 0,
        }
    }

    pub(crate) fn write(&mut self, address: u32, value: u8) -> Option<u8> {
        match address - self.base {
            0x14 => self.interrupt_enable &= !value,
            0x16 => self.interrupt_enable |= value & (DRE | TXC | RXC | RXS),
            0x18 => self.flags &= !(value & (TXC | RXS)),
            0x28 => {
                self.flags |= TXC;
                return Some(value);
            }
            _ => {}
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samd5_uart_updates_irq_from_receive_queue_and_tx_flags() {
        let mut uart = Uart::new(0, "sercom3".into(), Some(58));
        uart.write(0x16, RXC | TXC);
        uart.queue_input([b'A', b'B'].into_iter());
        assert!(uart.interrupt());
        assert_eq!(uart.read(0x18), RXC | RXS);
        assert_eq!(uart.read(0x28), b'A');
        assert_eq!(uart.read(0x28), b'B');
        assert!(!uart.interrupt());
        assert_eq!(uart.write(0x28, b'C'), Some(b'C'));
        assert!(uart.interrupt());
        uart.write(0x18, TXC);
        assert!(!uart.interrupt());
        assert_eq!(uart.read(0x18), DRE);
    }
}
