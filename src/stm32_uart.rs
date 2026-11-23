use std::{collections::VecDeque, io};

const F7_UE: u32 = 1;
const F4_UE: u32 = 1 << 13;
const RE: u32 = 1 << 2;
const TE: u32 = 1 << 3;
const RXNE: u32 = 1 << 5;
const TC: u32 = 1 << 6;
const TXE: u32 = 1 << 7;

#[derive(Clone, Copy)]
pub enum Stm32Irq {
    Nvic(u8),
    Exti(u8),
}

#[derive(Clone, Copy)]
pub enum UsartLayout {
    F4,
    F7 { low_power: bool },
}

impl UsartLayout {
    pub fn size(self) -> u32 {
        match self {
            Self::F4 => 0x100,
            Self::F7 { .. } => 0x400,
        }
    }
}

pub struct Stm32Usart {
    pub name: String,
    pub dma_rx: u8,
    pub dma_tx: u8,
    pub base: u32,
    pub irq: Option<Stm32Irq>,
    receive: VecDeque<u8>,
    registers: [u32; 12],
    complete: bool,
    layout: UsartLayout,
}

impl Stm32Usart {
    pub fn new(name: String, base: u32, irq: Option<Stm32Irq>, low_power: bool) -> Self {
        Self::with_layout(name, base, irq, UsartLayout::F7 { low_power })
    }

    pub fn with_layout(
        name: String,
        base: u32,
        irq: Option<Stm32Irq>,
        layout: UsartLayout,
    ) -> Self {
        Self {
            name,
            dma_rx: 0,
            dma_tx: 0,
            base,
            irq,
            receive: VecDeque::new(),
            registers: [0; 12],
            complete: true,
            layout,
        }
    }

    pub fn reset(&mut self) {
        self.receive.clear();
        self.registers.fill(0);
        self.complete = true;
    }

    pub fn size(&self) -> u32 {
        self.layout.size()
    }

    fn enable_bit(&self) -> u32 {
        match self.layout {
            UsartLayout::F4 => F4_UE,
            UsartLayout::F7 { .. } => F7_UE,
        }
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < self.size()
    }

    pub fn queue_input(&mut self, bytes: impl IntoIterator<Item = u8>) {
        let enables = self.enable_bit() | RE;
        if self.registers[0] & enables == enables {
            self.receive.extend(bytes);
        }
    }

    fn status(&self) -> u32 {
        let flags =
            TXE | (u32::from(self.complete) * TC) | (u32::from(!self.receive.is_empty()) * RXNE);
        let control = self.registers[0];
        match self.layout {
            UsartLayout::F4 => flags,
            UsartLayout::F7 { low_power } => {
                flags
                    | (if low_power { 0 } else { 1 << 29 })
                    | (u32::from(control & TE != 0) << 21)
                    | (u32::from(control & RE != 0) << 22)
            }
        }
    }

    pub fn interrupt_pending(&self) -> bool {
        let control = self.registers[0];
        if matches!(self.layout, UsartLayout::F4) && control & F4_UE == 0 {
            return false;
        }
        self.status() & control & (RXNE | TXE | TC) != 0
    }

    fn f4_register(offset: u32) -> Option<usize> {
        // Share CR1, CR2, CR3, BRR and GTPR storage with F7.
        Some(match offset {
            0x08 => 3,
            0x0c => 0,
            0x10 => 1,
            0x14 => 2,
            0x18 => 11,
            _ => return None,
        })
    }

    fn receive_byte(&mut self) -> u32 {
        u32::from(self.receive.pop_front().unwrap_or(0))
    }

    fn transmit(&mut self, value: u32, send: impl FnOnce(u8) -> io::Result<()>) -> io::Result<()> {
        self.registers[10] = value & 0xff;
        let enables = self.enable_bit() | TE;
        if self.registers[0] & enables == enables {
            send(value as u8)?;
            self.complete = true;
        }
        Ok(())
    }

    fn write_register(&mut self, index: usize, mask: u32, value: u32) {
        let register = &mut self.registers[index];
        *register = (*register & !mask) | value;
    }

    pub fn pending_dma(&self) -> u8 {
        let control = self.registers[0];
        let dma = self.registers[2];
        let rx = if control & (self.enable_bit() | RE) == self.enable_bit() | RE
            && dma & (1 << 6) != 0
            && !self.receive.is_empty()
        {
            self.dma_rx
        } else {
            0
        };
        let tx = if control & (self.enable_bit() | TE) == self.enable_bit() | TE
            && dma & (1 << 7) != 0
        {
            self.dma_tx
        } else {
            0
        };
        rx | tx
    }

    pub fn read(&mut self, address: u32, width: usize) -> u32 {
        let offset = address - self.base;
        let value = if matches!(self.layout, UsartLayout::F4) {
            match offset & !3 {
                0 => self.status(),
                4 if offset == 4 => self.receive_byte(),
                _ => Self::f4_register(offset & !3).map_or(0, |index| self.registers[index]),
            }
        } else {
            match offset & !3 {
                0x00 | 0x04 | 0x08 | 0x0c | 0x14 | 0x28 | 0x2c => {
                    self.registers[(offset / 4) as usize]
                }
                0x1c => self.status(),
                0x24 if offset & 3 == 0 => self.receive_byte(),
                _ => 0,
            }
        };
        (value >> ((offset & 3) * 8)) & width_mask(width)
    }

    pub fn write(
        &mut self,
        address: u32,
        width: usize,
        value: u32,
        send: impl FnOnce(u8) -> io::Result<()>,
    ) -> io::Result<()> {
        let offset = address - self.base;
        let shift = (offset & 3) * 8;
        let mask = width_mask(width) << shift;
        let value = (value << shift) & mask;
        if matches!(self.layout, UsartLayout::F4) {
            match offset & !3 {
                0 => {
                    if mask & !value & TC != 0 {
                        self.complete = false;
                    }
                    if mask & !value & RXNE != 0 {
                        self.receive_byte();
                    }
                }
                4 if offset == 4 => self.transmit(value, send)?,
                _ => {
                    if let Some(index) = Self::f4_register(offset & !3) {
                        self.write_register(index, mask, value);
                    }
                }
            }
            return Ok(());
        }
        match offset & !3 {
            0x00 | 0x04 | 0x08 | 0x0c | 0x14 | 0x2c => {
                self.write_register((offset / 4) as usize, mask, value);
            }
            0x18 if value & (1 << 3) != 0 => self.receive.clear(),
            0x20 if value & TC != 0 => self.complete = false,
            0x28 if offset & 3 == 0 => self.transmit(value, send)?,
            _ => {}
        }
        Ok(())
    }
}

fn width_mask(width: usize) -> u32 {
    u32::MAX >> (32 - width * 8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn f4_sr_and_dr_follow_cr1_enable_bits() {
        let mut uart = Stm32Usart::with_layout("uart".into(), 0, None, UsartLayout::F4);
        let mut output = Vec::new();
        assert_eq!(uart.read(0, 4), TXE | TC);
        uart.write(0x0c, 4, F7_UE | RE | TE | TXE, |byte| {
            output.write_all(&[byte])
        })
        .unwrap();
        uart.queue_input(b"X".to_vec());
        uart.write(4, 1, u32::from(b'X'), |byte| output.write_all(&[byte]))
            .unwrap();
        assert!(!uart.interrupt_pending());
        assert!(output.is_empty());

        uart.write(0x0c, 4, F4_UE | RE | TE | RXNE, |byte| {
            output.write_all(&[byte])
        })
        .unwrap();
        uart.queue_input(b"Y".to_vec());
        assert!(uart.interrupt_pending());
        assert_eq!(uart.read(0, 1), TXE | TC | RXNE);
        assert_eq!(uart.read(4, 2), u32::from(b'Y'));
        assert!(!uart.interrupt_pending());
        uart.write(0, 1, !TC, |byte| output.write_all(&[byte]))
            .unwrap();
        assert_eq!(uart.read(0, 4), TXE);
        uart.write(4, 4, u32::from(b'Y'), |byte| output.write_all(&[byte]))
            .unwrap();
        assert_eq!(output, b"Y");
        assert_eq!(uart.read(0, 4), TXE | TC);
    }

    #[test]
    fn receive_irq_and_transmit_follow_control_and_status_registers() {
        let mut uart = Stm32Usart::with_layout(
            "uart".into(),
            0,
            Some(Stm32Irq::Nvic(3)),
            UsartLayout::F7 { low_power: false },
        );
        let mut output = Vec::new();
        uart.queue_input(b"X".to_vec());
        uart.write(0, 1, F7_UE | RE | TE | RXNE, |byte| {
            output.write_all(&[byte])
        })
        .unwrap();
        uart.queue_input(b"Y".to_vec());
        assert!(uart.interrupt_pending());
        assert_ne!(uart.read(0x1c, 4) & RXNE, 0);
        assert_eq!(uart.read(0x24, 1), u32::from(b'Y'));
        assert!(!uart.interrupt_pending());
        uart.write(0x28, 1, u32::from(b'Y'), |byte| output.write_all(&[byte]))
            .unwrap();
        assert_eq!(output, b"Y");
        uart.write(0x20, 1, TC, |byte| output.write_all(&[byte]))
            .unwrap();
        assert_eq!(uart.read(0x1c, 1) & TC, 0);
    }
}
