use std::{collections::VecDeque, io};

use crate::bus::CORTEX_M_STEP_HZ;

const UE: u32 = 1;
const RE: u32 = 1 << 2;
const TE: u32 = 1 << 3;
const IDLE: u32 = 1 << 4;
const RXNE: u32 = 1 << 5;
const TC: u32 = 1 << 6;
const TXE: u32 = 1 << 7;

#[derive(Clone, Copy)]
pub enum Stm32Irq {
    Nvic(u8),
    Exti(u8),
}

pub struct Stm32Usart {
    pub name: String,
    pub base: u32,
    pub irq: Option<Stm32Irq>,
    pub dma_rx: u8,
    pub dma_tx: u8,
    receive: VecDeque<u8>,
    pending_receive: VecDeque<u8>,
    // Counts down to delivery of the next byte, or IDLE when the queue is empty.
    receive_deadline: Option<u64>,
    idle: bool,
    pub frequency: Option<u32>,
    registers: [u32; 12],
    complete: bool,
    low_power: bool,
}

impl Stm32Usart {
    pub const SIZE: u32 = 0x400;

    pub fn new(name: String, base: u32, irq: Option<Stm32Irq>, low_power: bool) -> Self {
        Self {
            name,
            base,
            irq,
            dma_rx: 0,
            dma_tx: 0,
            receive: VecDeque::new(),
            pending_receive: VecDeque::new(),
            receive_deadline: None,
            idle: false,
            frequency: None,
            registers: [0; 12],
            complete: true,
            low_power,
        }
    }

    pub fn reset(&mut self) {
        self.receive.clear();
        self.cancel_receive();
        self.idle = false;
        self.registers.fill(0);
        self.complete = true;
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    fn receiver_enabled(&self) -> bool {
        self.registers[0] & (UE | RE) == UE | RE
    }

    fn frame_ticks(&self) -> Option<u64> {
        let frequency = u64::from(self.frequency?);
        let control = self.registers[0];
        let brr = if self.low_power {
            self.registers[3] & 0x000f_ffff
        } else {
            self.registers[3] & 0xffff
        };
        if brr == 0 {
            return None;
        }
        let (divisor, multiplier) = if self.low_power {
            (brr, 256)
        } else if control & (1 << 15) != 0 {
            ((brr & !0xf) | ((brr & 7) << 1), 2)
        } else {
            (brr, 1)
        };
        if divisor == 0 {
            return None;
        }
        let word_bits = match (control & (1 << 28) != 0, control & (1 << 12) != 0) {
            (false, true) => 9,
            (true, false) => 7,
            _ => 8,
        };
        // The programmed word length includes parity. Use half-bit units
        // so the calculation also covers fractional stop-bit settings.
        let stop_half_bits = [2, 1, 4, 3][(self.registers[1] >> 12 & 3) as usize];
        let frame_half_bits = 2 + 2 * word_bits + stop_half_bits;
        let ticks = (CORTEX_M_STEP_HZ * u64::from(divisor) * frame_half_bits)
            .div_ceil(2 * frequency * multiplier);
        Some(ticks)
    }

    fn cancel_receive(&mut self) {
        self.pending_receive.clear();
        self.receive_deadline = None;
    }

    pub fn queue_input(&mut self, bytes: impl IntoIterator<Item = u8>) {
        if !self.receiver_enabled() {
            return;
        }
        let mut bytes = bytes.into_iter().peekable();
        if bytes.peek().is_none() {
            return;
        }
        if let Some(ticks) = self.frame_ticks() {
            if self.pending_receive.is_empty() {
                self.receive_deadline = Some(ticks);
            }
            self.pending_receive.extend(bytes);
        } else {
            self.receive.extend(bytes);
        }
    }

    pub fn tick(&mut self) {
        let Some(remaining) = &mut self.receive_deadline else {
            return;
        };
        *remaining -= 1;
        if *remaining != 0 {
            return;
        }
        if let Some(byte) = self.pending_receive.pop_front() {
            self.receive.push_back(byte);
            // The same interval measures the next queued frame or the
            // idle period that starts after the last byte is delivered.
            self.receive_deadline = self.frame_ticks();
        } else {
            self.idle = true;
            self.receive_deadline = None;
        }
    }

    fn status(&self) -> u32 {
        let control = self.registers[0];
        (if self.low_power { 0 } else { 1 << 29 })
            | TXE
            | (u32::from(self.idle) * IDLE)
            | (u32::from(self.complete) * TC)
            | (u32::from(!self.receive.is_empty()) * RXNE)
            | (u32::from(control & TE != 0) << 21)
            | (u32::from(control & RE != 0) << 22)
    }

    pub fn interrupt_pending(&self) -> bool {
        let enabled = self.registers[0];
        enabled & TXE != 0
            || (enabled & TC != 0 && self.complete)
            || (enabled & RXNE != 0 && !self.receive.is_empty())
            || (self.receiver_enabled() && enabled & IDLE != 0 && self.idle)
    }

    pub fn pending_dma(&self) -> u8 {
        let control = self.registers[0];
        let dma = self.registers[2];
        let rx =
            if control & (UE | RE) == UE | RE && dma & (1 << 6) != 0 && !self.receive.is_empty() {
                self.dma_rx
            } else {
                0
            };
        let tx = if control & (UE | TE) == UE | TE && dma & (1 << 7) != 0 {
            self.dma_tx
        } else {
            0
        };
        rx | tx
    }

    pub fn read(&mut self, address: u32, width: usize) -> u32 {
        let offset = address - self.base;
        let value = match offset & !3 {
            0x00 | 0x04 | 0x08 | 0x0c | 0x14 | 0x28 | 0x2c => self.registers[(offset / 4) as usize],
            0x1c => self.status(),
            0x24 if offset & 3 == 0 => u32::from(self.receive.pop_front().unwrap_or(0)),
            _ => 0,
        };
        (value >> ((offset & 3) * 8)) & width_mask(width)
    }

    pub fn write(
        &mut self,
        address: u32,
        width: usize,
        value: u32,
        transmit: impl FnOnce(u8) -> io::Result<()>,
    ) -> io::Result<()> {
        let offset = address - self.base;
        let shift = (offset & 3) * 8;
        let mask = width_mask(width) << shift;
        let value = (value << shift) & mask;
        match offset & !3 {
            0x00 | 0x04 | 0x0c => {
                let old_ticks = self.frame_ticks();
                let register = &mut self.registers[(offset / 4) as usize];
                *register = (*register & !mask) | value;
                if !self.receiver_enabled() {
                    self.cancel_receive();
                } else {
                    let ticks = self.frame_ticks();
                    if ticks != old_ticks && self.receive_deadline.is_some() {
                        self.receive_deadline = ticks;
                        if ticks.is_none() {
                            self.receive.append(&mut self.pending_receive);
                        }
                    }
                }
            }
            0x08 | 0x14 | 0x2c => {
                let register = &mut self.registers[(offset / 4) as usize];
                *register = (*register & !mask) | value;
            }
            0x18 if value & (1 << 3) != 0 => self.receive.clear(),
            0x20 => {
                if value & TC != 0 {
                    self.complete = false;
                }
                if value & IDLE != 0 {
                    self.idle = false;
                }
            }
            0x28 if offset & 3 == 0 => {
                self.registers[10] = value & 0xff;
                if self.registers[0] & (UE | TE) == UE | TE {
                    transmit(value as u8)?;
                    self.complete = true;
                }
            }
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

    const CR1: u32 = 0;
    const BRR: u32 = 0x0c;
    const ISR: u32 = 0x1c;
    const ICR: u32 = 0x20;
    const RDR: u32 = 0x24;
    const US: u64 = CORTEX_M_STEP_HZ / 1_000_000;

    fn configured_uart(control: u32) -> Stm32Usart {
        let mut uart = Stm32Usart::new("uart".into(), 0, None, false);
        uart.frequency = Some(16_000_000);
        write(&mut uart, CR1, control);
        write(&mut uart, BRR, 0x10);
        uart
    }

    fn write(uart: &mut Stm32Usart, addr: u32, value: u32) {
        uart.write(addr, 4, value, |_| Ok(())).unwrap();
    }

    fn run_us(uart: &mut Stm32Usart, us: u64) {
        for _ in 0..us * US {
            uart.tick();
        }
    }

    fn assert_empty(uart: &mut Stm32Usart) {
        assert_eq!(uart.read(ISR, 4) & RXNE, 0);
    }

    // STM32_UART.robot, with CR1/ISR/RDR translated to the F072 layout.
    #[test]
    fn renode_stm32_usart_receives_at_configured_baud_rate_and_detects_idle() {
        let mut uart = configured_uart(UE | RE);
        uart.queue_input([0x12, 0x34]);
        for byte in [0x12, 0x34] {
            assert_empty(&mut uart);
            run_us(&mut uart, 9);
            assert_empty(&mut uart);
            run_us(&mut uart, 1);
            assert_ne!(uart.read(ISR, 4) & RXNE, 0);
            assert_eq!(uart.read(RDR, 4), byte);
        }
        assert_eq!(uart.read(ISR, 4) & IDLE, 0);
        run_us(&mut uart, 9);
        assert_eq!(uart.read(ISR, 4) & IDLE, 0);
        run_us(&mut uart, 1);
        assert_ne!(uart.read(ISR, 4) & IDLE, 0);
        assert!(!uart.interrupt_pending());
        write(&mut uart, CR1, UE | RE | IDLE);
        assert!(uart.interrupt_pending());
        write(&mut uart, ICR, IDLE);
        assert!(!uart.interrupt_pending());
        run_us(&mut uart, 10);
        assert_eq!(uart.read(ISR, 4) & IDLE, 0);
    }

    #[test]
    fn renode_stm32_usart_parity_is_part_of_the_programmed_word_length() {
        let mut uart = configured_uart(UE | RE | (1 << 10));
        uart.queue_input([0x12]);
        run_us(&mut uart, 9);
        assert_empty(&mut uart);
        run_us(&mut uart, 1);
        assert_eq!(uart.read(RDR, 4), 0x12);
        assert_eq!(uart.read(ISR, 4) & IDLE, 0);
        run_us(&mut uart, 9);
        assert_eq!(uart.read(ISR, 4) & IDLE, 0);
        run_us(&mut uart, 1);
        assert_ne!(uart.read(ISR, 4) & IDLE, 0);
    }

    #[test]
    fn renode_stm32_usart_cancels_idle_when_the_next_frame_starts() {
        let mut uart = configured_uart(UE | RE);
        uart.queue_input([0x12]);
        run_us(&mut uart, 10);
        assert_eq!(uart.read(RDR, 4), 0x12);
        run_us(&mut uart, 9);
        assert_eq!(uart.read(ISR, 4) & IDLE, 0);
        uart.queue_input([0x34]);
        run_us(&mut uart, 1);
        assert_eq!(uart.read(ISR, 4) & IDLE, 0);
        run_us(&mut uart, 9);
        assert_eq!(uart.read(RDR, 4), 0x34);
        run_us(&mut uart, 10);
        assert_ne!(uart.read(ISR, 4) & IDLE, 0);
    }

    #[test]
    fn renode_stm32_usart_drops_input_if_either_enable_is_clear() {
        let mut uart = configured_uart(UE | RE);
        for (control, byte) in [(UE, 0x12), (RE, 0x34)] {
            write(&mut uart, CR1, control);
            uart.queue_input([byte]);
            run_us(&mut uart, 10);
            assert_empty(&mut uart);
            write(&mut uart, CR1, UE | RE);
            run_us(&mut uart, 10);
            assert_empty(&mut uart);
            assert_eq!(uart.read(ISR, 4) & IDLE, 0);
        }
    }

    #[test]
    fn renode_stm32_usart_drops_an_in_flight_byte_when_re_is_cleared() {
        let mut uart = configured_uart(UE | RE);
        uart.queue_input([0x12]);
        write(&mut uart, CR1, UE);
        run_us(&mut uart, 10);
        assert_empty(&mut uart);
        write(&mut uart, CR1, UE | RE);
        run_us(&mut uart, 10);
        assert_empty(&mut uart);
        assert_eq!(uart.read(ISR, 4) & IDLE, 0);
    }

    #[test]
    fn renode_stm32_usart_preserves_receive_order_when_brr_becomes_zero() {
        let mut uart = configured_uart(UE | RE);
        uart.queue_input([0x12]);
        assert_empty(&mut uart);
        write(&mut uart, BRR, 0);
        uart.queue_input([0x34]);
        assert_eq!(uart.read(RDR, 4), 0x12);
        assert_eq!(uart.read(RDR, 4), 0x34);
        assert_empty(&mut uart);
    }

    #[test]
    fn receive_irq_and_transmit_follow_control_and_status_registers() {
        let mut uart = Stm32Usart::new("uart".into(), 0, Some(Stm32Irq::Nvic(3)), false);
        let mut output = Vec::new();
        uart.queue_input(b"X".to_vec());
        uart.write(0, 1, UE | RE | TE | RXNE, |_| Ok(())).unwrap();
        uart.queue_input(b"Y".to_vec());
        assert!(uart.interrupt_pending());
        assert_ne!(uart.read(0x1c, 4) & RXNE, 0);
        assert_eq!(uart.read(0x24, 1), u32::from(b'Y'));
        assert!(!uart.interrupt_pending());
        uart.write(0x28, 1, u32::from(b'Y'), |byte| {
            output.push(byte);
            Ok(())
        })
        .unwrap();
        assert_eq!(output, b"Y");
        uart.write(0x20, 1, TC, |_| Ok(())).unwrap();
        assert_eq!(uart.read(0x1c, 1) & TC, 0);
    }
}
