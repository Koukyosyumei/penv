use std::{
    collections::VecDeque,
    io::{self, Write},
};

const UE: u32 = 1;
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
pub enum Stm32UsartClock {
    Apb1Low(u8),
    Apb2(u8),
    Apb4(u8),
}

impl Stm32UsartClock {
    pub fn from_alias(alias: &str) -> Result<Self, String> {
        Ok(match alias {
            "USART1" => Self::Apb2(4),
            "USART6" => Self::Apb2(5),
            "USART2" => Self::Apb1Low(17),
            "USART3" => Self::Apb1Low(18),
            "UART4" => Self::Apb1Low(19),
            "UART5" => Self::Apb1Low(20),
            "UART7" => Self::Apb1Low(30),
            "UART8" => Self::Apb1Low(31),
            "LPUART1" => Self::Apb4(3),
            _ => return Err(format!("unsupported STM32 USART RCC alias {alias}")),
        })
    }
}

pub struct Stm32Usart {
    pub base: u32,
    pub name: String,
    pub observed: Option<Vec<u8>>,
    pub irq: Option<Stm32Irq>,
    pub clock: Option<Stm32UsartClock>,
    receive: VecDeque<u8>,
    registers: [u32; 12],
    complete: bool,
    low_power: bool,
}

impl Stm32Usart {
    pub const SIZE: u32 = 0x400;

    pub fn new(
        base: u32,
        irq: Option<Stm32Irq>,
        low_power: bool,
        clock: Option<Stm32UsartClock>,
    ) -> Self {
        Self {
            base,
            name: String::new(),
            observed: None,
            irq,
            clock,
            receive: VecDeque::new(),
            registers: [0; 12],
            complete: true,
            low_power,
        }
    }

    pub fn reset(&mut self) {
        let mut reset = Self::new(self.base, self.irq, self.low_power, self.clock);
        reset.name = std::mem::take(&mut self.name);
        reset.observed = self.observed.take();
        *self = reset;
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    pub fn receiver_enabled(&self) -> bool {
        self.registers[0] & (UE | RE) == UE | RE
    }

    pub fn queue_input(&mut self, bytes: Vec<u8>) {
        if self.receiver_enabled() {
            self.receive.extend(bytes);
        }
    }

    fn status(&self) -> u32 {
        let control = self.registers[0];
        (if self.low_power { 0 } else { 1 << 29 })
            | TXE
            | (u32::from(self.complete) * TC)
            | (u32::from(!self.receive.is_empty()) * RXNE)
            | (u32::from(control & TE != 0) << 21)
            | (u32::from(control & RE != 0) << 22)
    }

    pub fn interrupt_pending(&self) -> bool {
        self.status() & self.registers[0] & (RXNE | TXE | TC) != 0
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
        output: &mut impl Write,
    ) -> io::Result<()> {
        let offset = address - self.base;
        let shift = (offset & 3) * 8;
        let mask = width_mask(width) << shift;
        let value = (value << shift) & mask;
        match offset & !3 {
            0x00 | 0x04 | 0x08 | 0x0c | 0x14 | 0x2c => {
                let register = &mut self.registers[(offset / 4) as usize];
                *register = (*register & !mask) | value;
            }
            0x18 if value & (1 << 3) != 0 => self.receive.clear(),
            0x20 if value & TC != 0 => self.complete = false,
            0x28 if offset & 3 == 0 => {
                self.registers[10] = value & 0xff;
                if self.registers[0] & (UE | TE) == UE | TE {
                    output.write_all(&[value as u8])?;
                    if let Some(bytes) = &mut self.observed {
                        bytes.push(value as u8);
                    }
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

    #[test]
    fn registered_usart_uses_rcc_gate_reset_and_exti_route() {
        use crate::{
            bus::Bus, nvic::ActiveExceptions, stm32h7_exti::H7Exti,
            stm32h7_net_control::NetworkControl,
        };

        const RCC: u32 = 0x58024400;
        const BASE: u32 = 0x40004800;
        let mut bus = Bus::default();
        bus.map_h7_network_control(NetworkControl::new(RCC, None, None))
            .unwrap();
        bus.map_nvic(0xf0, None).unwrap();
        let mut lines = [None; H7Exti::LINES];
        lines[28] = Some(39);
        bus.map_h7_exti(0x58000000, lines).unwrap();
        bus.map_stm32_usart(
            BASE,
            Some(Stm32Irq::Exti(28)),
            false,
            Some(Stm32UsartClock::from_alias("USART3").unwrap()),
        )
        .unwrap();
        let write = |bus: &mut Bus, address, value| {
            bus.write(address, 4, value, &mut io::sink()).unwrap();
        };
        write(&mut bus, BASE, UE | RE | RXNE);
        write(&mut bus, RCC + 0xf0, 1 << 18);
        assert_eq!(bus.read::<4>(BASE + 0x1c).unwrap(), 0);
        write(&mut bus, RCC + 0xe8, 1 << 18);
        assert_eq!(bus.read::<4>(BASE).unwrap(), 0);
        write(&mut bus, BASE, UE | RE | RXNE);
        bus.queue_uart_input(b"A".to_vec());
        bus.sample_nvic(&ActiveExceptions::default());
        assert_eq!(bus.read::<4>(0xe000e204).unwrap(), 1 << (39 - 32));
        assert_eq!(bus.read::<4>(BASE + 0x24).unwrap(), u32::from(b'A'));
        write(&mut bus, RCC + 0x90, 1 << 18);
        write(&mut bus, RCC + 0x90, 0);
        assert_eq!(bus.read::<4>(BASE).unwrap(), 0);
    }

    #[test]
    fn receive_irq_and_transmit_follow_control_and_status_registers() {
        let mut uart = Stm32Usart::new(0, Some(Stm32Irq::Nvic(3)), false, None);
        let mut output = Vec::new();
        uart.queue_input(b"X".to_vec());
        uart.write(0, 1, UE | RE | TE | RXNE, &mut output).unwrap();
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
