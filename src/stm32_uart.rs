use std::{
    collections::VecDeque,
    io::{self, Write},
};

const F7_UE: u32 = 1;
const F4_UE: u32 = 1 << 13;
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

#[derive(Default)]
struct Reception {
    pending: VecDeque<u8>,
    progress: u64,
    idle_armed: bool,
    idle: bool,
}

pub struct Stm32Usart {
    pub base: u32,
    pub irq: Option<Stm32Irq>,
    receive: VecDeque<u8>,
    timing: Reception,
    frequency: u32,
    registers: [u32; 12],
    complete: bool,
    layout: UsartLayout,
}

impl Stm32Usart {
    pub fn new(base: u32, irq: Option<Stm32Irq>, layout: UsartLayout, frequency: u32) -> Self {
        Self {
            base,
            irq,
            receive: VecDeque::new(),
            timing: Reception::default(),
            frequency,
            registers: [0; 12],
            complete: true,
            layout,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new(self.base, self.irq, self.layout, self.frequency);
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

    fn receiver_enabled(&self) -> bool {
        let enables = self.enable_bit() | RE;
        self.registers[0] & enables == enables
    }

    fn baud_rate(&self) -> u64 {
        let divider = u64::from(self.registers[3] & 0xffff);
        if divider == 0 {
            return 0;
        }
        let frequency = u64::from(self.frequency);
        if self.registers[0] & (1 << 15) == 0 {
            frequency / divider
        } else {
            let divisor = divider & !8;
            if divisor == 0 {
                0
            } else {
                frequency * 2 / divisor
            }
        }
    }

    pub fn queue_input(&mut self, bytes: Vec<u8>) {
        if bytes.is_empty() || !self.receiver_enabled() {
            return;
        }
        if matches!(self.layout, UsartLayout::F4) && self.baud_rate() != 0 {
            if self.timing.pending.is_empty() {
                self.timing.progress = 0;
            }
            self.timing.idle_armed = false;
            self.timing.pending.extend(bytes);
        } else {
            self.receive.extend(bytes);
        }
    }

    pub fn tick(&mut self, step_hz: u32) {
        if !self.timing.idle_armed && self.timing.pending.is_empty() {
            return;
        }
        let baud = self.baud_rate();
        // CR1.M counts parity within the word. STOP is encoded as 1, 0.5,
        // 2 or 1.5 bits, so compare time in half-bit units.
        let stop = [2, 1, 4, 3][((self.registers[1] >> 12) & 3) as usize];
        let word = 8 + u64::from(self.registers[0] & (1 << 12) != 0);
        let frame = u64::from(step_hz) * (2 * (1 + word) + stop);
        self.timing.progress += 2 * baud;
        while self.timing.progress >= frame {
            self.timing.progress -= frame;
            if let Some(byte) = self.timing.pending.pop_front() {
                if self.receiver_enabled() {
                    self.receive.push_back(byte);
                    self.timing.idle_armed = self.timing.pending.is_empty();
                }
            } else {
                self.timing.idle = true;
                self.timing.idle_armed = false;
            }
            if !self.timing.idle_armed && self.timing.pending.is_empty() {
                self.timing.progress = 0;
                break;
            }
        }
    }

    fn status(&self) -> u32 {
        let flags =
            TXE | (u32::from(self.complete) * TC) | (u32::from(!self.receive.is_empty()) * RXNE);
        let control = self.registers[0];
        match self.layout {
            UsartLayout::F4 => flags | (u32::from(self.timing.idle) * IDLE),
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
        let interrupts = RXNE
            | TXE
            | TC
            | if matches!(self.layout, UsartLayout::F4) {
                IDLE
            } else {
                0
            };
        self.status() & control & interrupts != 0
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

    fn transmit(&mut self, value: u32, output: &mut impl Write) -> io::Result<()> {
        self.registers[10] = value & 0xff;
        let enables = self.enable_bit() | TE;
        if self.registers[0] & enables == enables {
            output.write_all(&[value as u8])?;
            self.complete = true;
        }
        Ok(())
    }

    fn write_register(&mut self, index: usize, mask: u32, value: u32) {
        let register = &mut self.registers[index];
        *register = (*register & !mask) | value;
    }

    pub fn read(&mut self, address: u32, width: usize) -> u32 {
        let offset = address - self.base;
        let value = if matches!(self.layout, UsartLayout::F4) {
            match offset & !3 {
                0 => self.status(),
                4 if offset == 4 => {
                    self.timing.idle = false;
                    self.receive_byte()
                }
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
        output: &mut impl Write,
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
                4 if offset == 4 => self.transmit(value, output)?,
                _ => {
                    if let Some(index) = Self::f4_register(offset & !3) {
                        let previous_baud = self.baud_rate();
                        self.write_register(index, mask, value);
                        if index == 0 && !self.receiver_enabled() {
                            self.timing.idle_armed = false;
                        }
                        let baud = self.baud_rate();
                        if baud != previous_baud {
                            self.timing.progress = 0;
                            if baud == 0 {
                                self.timing.idle_armed = false;
                                self.receive.append(&mut self.timing.pending);
                            }
                        }
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
            0x28 if offset & 3 == 0 => self.transmit(value, output)?,
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

    use crate::bus::Bus;

    // Tests port the CPU-less STM32_UART.robot fixture and its 1 us quantum.
    struct TimedUart(Bus);

    impl TimedUart {
        const BASE: u32 = 0x4000_0000;

        fn new(control: u32) -> Self {
            let mut bus = Bus::default();
            bus.map_stm32_usart("uart", Self::BASE, None, UsartLayout::F4, 16_000_000)
                .unwrap();
            bus.set_step_hz(1_000_000);
            let mut uart = Self(bus);
            uart.write(0x0c, control);
            uart.write(0x08, 0x10);
            uart
        }

        fn write(&mut self, offset: u32, value: u32) {
            self.0
                .write(Self::BASE + offset, 4, value, &mut io::sink())
                .unwrap();
        }

        fn read(&mut self, offset: u32) -> u32 {
            self.0.read::<4>(Self::BASE + offset).unwrap()
        }

        fn inject(&mut self, byte: u8) {
            self.0.queue_uart_input(vec![byte]);
        }

        fn run_for(&mut self, microseconds: u64) {
            for _ in 0..microseconds {
                self.0.tick_h7_peripherals();
            }
        }

        fn empty(&mut self) {
            assert_eq!(self.read(0) & RXNE, 0);
        }

        fn idle(&mut self, expected: bool) {
            assert_eq!(self.read(0) & IDLE != 0, expected);
        }
    }

    #[test]
    fn renode_uart_receives_at_configured_baud_rate() {
        let mut uart = TimedUart::new(0x2004);
        uart.inject(0x12);
        uart.inject(0x34);

        for byte in [0x12, 0x34] {
            uart.empty();
            uart.run_for(9);
            uart.empty();
            uart.run_for(1);
            assert_eq!(uart.read(4), byte);
        }
        uart.idle(false);
        uart.run_for(9);
        uart.idle(false);
        uart.run_for(1);
        uart.idle(true);
    }

    #[test]
    fn renode_uart_parity_is_within_word_length() {
        let mut uart = TimedUart::new(0x2404);
        uart.inject(0x12);
        uart.run_for(9);
        uart.empty();
        uart.run_for(1);
        assert_eq!(uart.read(4), 0x12);
        uart.idle(false);
        uart.run_for(9);
        uart.idle(false);
        uart.run_for(1);
        uart.idle(true);
    }

    #[test]
    fn renode_uart_preserves_order_when_baud_becomes_zero() {
        let mut uart = TimedUart::new(0x2004);
        uart.inject(0x12);
        uart.write(0x08, 0);
        uart.inject(0x34);
        assert_eq!(uart.read(4), 0x12);
        assert_eq!(uart.read(4), 0x34);
    }

    #[test]
    fn renode_uart_new_frame_cancels_idle_detection() {
        let mut uart = TimedUart::new(0x2004);
        uart.inject(0x12);
        uart.run_for(10);
        assert_eq!(uart.read(4), 0x12);
        uart.run_for(9);
        uart.idle(false);
        uart.inject(0x34);
        uart.run_for(1);
        uart.idle(false);
    }

    #[test]
    fn renode_uart_drops_queued_input_when_receiver_is_disabled() {
        let mut uart = TimedUart::new(0x2004);
        uart.inject(0x12);
        uart.write(0x0c, 0x2000);
        uart.run_for(10);
        uart.empty();
        uart.write(0x0c, 0x2004);
        uart.run_for(10);
        uart.empty();
    }

    #[test]
    fn renode_uart_drops_input_when_uart_or_receiver_is_disabled() {
        let mut uart = TimedUart::new(0x2004);
        for (control, byte) in [(0x2000, 0x12), (0x4, 0x34)] {
            uart.write(0x0c, control);
            uart.inject(byte);
            uart.run_for(10);
            uart.empty();
            uart.write(0x0c, 0x2004);
            uart.run_for(10);
            uart.empty();
        }
    }

    #[test]
    fn f4_sr_and_dr_follow_cr1_enable_bits() {
        let mut uart = Stm32Usart::new(0, None, UsartLayout::F4, 8_000_000);
        let mut output = Vec::new();
        assert_eq!(uart.read(0, 4), TXE | TC);
        uart.write(0x0c, 4, F7_UE | RE | TE | TXE, &mut output)
            .unwrap();
        uart.queue_input(b"X".to_vec());
        uart.write(4, 1, u32::from(b'X'), &mut output).unwrap();
        assert!(!uart.interrupt_pending());
        assert!(output.is_empty());

        uart.write(0x0c, 4, F4_UE | RE | TE | RXNE, &mut output)
            .unwrap();
        uart.queue_input(b"Y".to_vec());
        assert!(uart.interrupt_pending());
        assert_eq!(uart.read(0, 1), TXE | TC | RXNE);
        assert_eq!(uart.read(4, 2), u32::from(b'Y'));
        assert!(!uart.interrupt_pending());
        uart.write(0, 1, !TC, &mut output).unwrap();
        assert_eq!(uart.read(0, 4), TXE);
        uart.write(4, 4, u32::from(b'Y'), &mut output).unwrap();
        assert_eq!(output, b"Y");
        assert_eq!(uart.read(0, 4), TXE | TC);
    }

    #[test]
    fn receive_irq_and_transmit_follow_control_and_status_registers() {
        let mut uart = Stm32Usart::new(
            0,
            Some(Stm32Irq::Nvic(3)),
            UsartLayout::F7 { low_power: false },
            8_000_000,
        );
        let mut output = Vec::new();
        uart.queue_input(b"X".to_vec());
        uart.write(0, 1, F7_UE | RE | TE | RXNE, &mut output)
            .unwrap();
        uart.queue_input(b"Y".to_vec());
        assert!(uart.interrupt_pending());
        assert_ne!(uart.read(0x1c, 4) & RXNE, 0);
        assert_eq!(uart.read(0x24, 1), u32::from(b'Y'));
        assert!(!uart.interrupt_pending());
        uart.write(0x28, 1, u32::from(b'Y'), &mut output).unwrap();
        assert_eq!(output, b"Y");
        uart.write(0x20, 1, TC, &mut output).unwrap();
        assert_eq!(uart.read(0x1c, 1) & TC, 0);
    }
}
