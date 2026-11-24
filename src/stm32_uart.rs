use std::{collections::VecDeque, io};

use crate::bus::CORTEX_M_STEP_HZ;

const UE: u32 = 1;
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

#[derive(Default)]
pub struct UartObservation {
    pub bytes: Vec<u8>,
    pub failing: Vec<String>,
}

pub struct Stm32Usart {
    pub name: String,
    pub dma_rx: u8,
    pub dma_tx: u8,
    pub base: u32,
    pub observed: Option<UartObservation>,
    pub irq: Option<Stm32Irq>,
    pub clock: Option<Stm32UsartClock>,
    receive: VecDeque<u8>,
    pending_receive: VecDeque<u8>,
    // Counts down to delivery of the next byte, or IDLE when the queue is empty.
    receive_deadline: Option<u64>,
    idle: bool,
    pub frequency: Option<u32>,
    registers: [u32; 12],
    complete: bool,
    layout: UsartLayout,
    pub legacy_layout: bool,
    legacy_rxne_cleared: bool,
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
            observed: None,
            irq,
            clock: None,
            receive: VecDeque::new(),
            pending_receive: VecDeque::new(),
            receive_deadline: None,
            idle: false,
            frequency: None,
            registers: [0; 12],
            complete: true,
            layout,
            legacy_layout: false,
            legacy_rxne_cleared: false,
        }
    }

    pub fn reset(&mut self) {
        self.observed
            .as_mut()
            .into_iter()
            .for_each(|observation| observation.bytes.clear());
        self.receive.clear();
        self.cancel_receive();
        self.idle = false;
        self.registers.fill(0);
        self.complete = true;
        self.legacy_rxne_cleared = false;
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

    pub fn receiver_enabled(&self) -> bool {
        let enables = self.enable_bit() | RE;
        self.registers[0] & enables == enables
    }

    fn frame_ticks(&self) -> Option<u64> {
        let low_power = matches!(self.layout, UsartLayout::F7 { low_power: true });
        const PRESCALERS: [u64; 16] = [
            1, 2, 4, 6, 8, 10, 12, 16, 32, 64, 128, 256, 256, 256, 256, 256,
        ];
        let frequency = u64::from(self.frequency?);
        let prescaler = PRESCALERS[(self.registers[11] & 0xf) as usize];
        let control = self.registers[0];
        let brr = if low_power {
            self.registers[3] & 0x000f_ffff
        } else {
            self.registers[3] & 0xffff
        };
        if brr == 0 {
            return None;
        }
        let (divisor, multiplier) = if low_power {
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
        let ticks = (CORTEX_M_STEP_HZ * u64::from(divisor) * prescaler * frame_half_bits)
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
            self.legacy_rxne_cleared = false;
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
            self.legacy_rxne_cleared = false;
            // The same interval measures the next queued frame or the
            // idle period that starts after the last byte is delivered.
            self.receive_deadline = self.frame_ticks();
        } else {
            self.idle = true;
            self.receive_deadline = None;
        }
    }

    pub fn tick_receive(&mut self) {
        self.tick();
    }

    fn status(&self) -> u32 {
        let flags = TXE
            | (u32::from(self.complete) * TC)
            | (u32::from(self.idle) * IDLE)
            | (u32::from(!self.receive.is_empty() && !self.legacy_rxne_cleared) * RXNE);
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
            || (self.receiver_enabled() && self.idle && control & IDLE != 0)
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
            if let Some(observation) = &mut self.observed {
                observation.bytes.push(value as u8);
                if let Some(pattern) = observation
                    .failing
                    .iter()
                    .find(|pattern| observation.bytes.ends_with(pattern.as_bytes()))
                {
                    return Err(io::Error::other(format!(
                        "UART {} matched failing text {pattern:?}",
                        self.name
                    )));
                }
            }
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

    fn legacy_register(&self, offset: u32) -> u32 {
        match offset {
            0x08 => self.registers[3],
            0x0c => (self.registers[0] & !(UE | (1 << 13))) | ((self.registers[0] & UE) << 13),
            0x10 => self.registers[1],
            0x14 => self.registers[2],
            0x18 => self.registers[5],
            _ => 0,
        }
    }

    pub fn read(&mut self, address: u32, width: usize) -> u32 {
        let offset = address - self.base;
        if self.legacy_layout {
            let value = match offset & !3 {
                0 => self.status() & 0xff,
                4 if offset & 3 == 0 => {
                    self.idle = false;
                    self.legacy_rxne_cleared = false;
                    u32::from(self.receive.pop_front().unwrap_or(0))
                }
                _ => self.legacy_register(offset & !3),
            };
            return crate::access_translation::little_lane(value.into(), offset & 3, width) as u32;
        }
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
        crate::access_translation::little_lane(value.into(), offset & 3, width) as u32
    }

    pub fn write(
        &mut self,
        address: u32,
        width: usize,
        value: u32,
        transmit: impl FnOnce(u8) -> io::Result<()>,
    ) -> io::Result<()> {
        let offset = address - self.base;
        if self.legacy_layout {
            let shift = (offset & 3) * 8;
            let mask = width_mask(width) << shift;
            let value = (value << shift) & mask;
            let aligned = offset & !3;
            match aligned {
                0 => {
                    self.complete &= mask & !value & TC == 0;
                    self.legacy_rxne_cleared |= mask & !value & RXNE != 0;
                    return Ok(());
                }
                4 => {
                    if offset & 3 == 0 && self.registers[0] & (UE | TE) != 0 {
                        transmit(value as u8)?;
                        self.complete = true;
                    }
                    return Ok(());
                }
                _ => {}
            }
            let merged = (self.legacy_register(aligned) & !mask) | value;
            let (offset, value) = match aligned {
                0x08 => (0x0c, merged & 0xffff),
                0x0c => (0x00, (merged & 0x9ffc) | ((merged >> 13) & UE)),
                0x10 => (0x04, merged),
                0x14 => (0x08, merged),
                0x18 => (0x14, merged & 0xffff),
                _ => return Ok(()),
            };
            return self.write_modern(offset, 4, value, transmit);
        }
        self.write_modern(offset, width, value, transmit)
    }

    fn write_modern(
        &mut self,
        offset: u32,
        width: usize,
        value: u32,
        transmit: impl FnOnce(u8) -> io::Result<()>,
    ) -> io::Result<()> {
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
                4 if offset == 4 => self.transmit(value, transmit)?,
                _ => {
                    if let Some(index) = Self::f4_register(offset & !3) {
                        self.write_register(index, mask, value);
                    }
                }
            }
            return Ok(());
        }
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
            0x28 if offset & 3 == 0 => self.transmit(value, transmit)?,
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

    const CR1: u32 = 0;
    const BRR: u32 = 0x0c;
    const ISR: u32 = 0x1c;
    const ICR: u32 = 0x20;
    const RDR: u32 = 0x24;
    const US: u64 = CORTEX_M_STEP_HZ / 1_000_000;

    #[test]
    fn renode_neonkey_legacy_uart_data_and_status() {
        let mut uart = Stm32Usart::new("usart1".into(), 0, None, false);
        uart.legacy_layout = true;
        assert_eq!(uart.read(0, 4), 0xc0);
        write(&mut uart, 8, 0x8b);
        write(&mut uart, 0x0c, (1 << 13) | RE | TE | RXNE);
        assert_eq!(uart.read(0x0c, 4), (1 << 13) | RE | TE | RXNE);
        assert_eq!(uart.read(8, 4), 0x8b);
        uart.queue_input([b'a', b'b']);
        assert!(uart.interrupt_pending());
        write(&mut uart, 0, !RXNE);
        assert!(!uart.interrupt_pending());
        assert_eq!(uart.read(4, 4), u32::from(b'a'));
        assert!(uart.interrupt_pending());
        assert_eq!(uart.read(4, 4), u32::from(b'b'));
        assert!(!uart.interrupt_pending());
        uart.write(4, 1, b'!' as u32, |byte| {
            assert_eq!(byte, b'!');
            Ok(())
        })
        .unwrap();
        assert_eq!(uart.read(0, 4), 0xc0);
    }

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
        let mut uart = Stm32Usart::new("usart3".into(), BASE, Some(Stm32Irq::Exti(28)), false);
        uart.clock = Some(Stm32UsartClock::from_alias("USART3").unwrap());
        bus.map_stm32_usart(uart).unwrap();
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
        bus.sample_nvic(&ActiveExceptions::default(), false);
        assert_eq!(bus.read::<4>(0xe000e204).unwrap(), 1 << (39 - 32));
        assert_eq!(bus.read::<4>(BASE + 0x24).unwrap(), u32::from(b'A'));
        write(&mut bus, RCC + 0x90, 1 << 18);
        write(&mut bus, RCC + 0x90, 0);
        assert_eq!(bus.read::<4>(BASE).unwrap(), 0);
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
