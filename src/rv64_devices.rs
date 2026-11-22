use std::{
    collections::VecDeque,
    io::{self, Write},
};

use crate::{machine::MAX_HARTS, ns16550::Ns16550, rv64_starfive::ResetGenerator};

#[derive(Default)]
pub struct Devices {
    pub plic: Option<Plic>,
    pub uarts: Vec<Uart>,
    pub reset_generators: Vec<ResetGenerator>,
    pub fallback_ranges: Vec<FallbackRange>,
    lines: [u64; MAX_HARTS],
    input_uart: usize,
}

pub struct FallbackRange {
    pub start: u64,
    pub end: u64,
    pub value: u64,
}

impl Devices {
    pub fn fallback(&self, address: u64, width: usize) -> Option<u64> {
        let end = address.checked_add(width as u64 - 1)?;
        let range = self
            .fallback_ranges
            .iter()
            .rev()
            .find(|range| address >= range.start && end <= range.end)?;
        Some(range.value & (u64::MAX >> (64 - width * 8)))
    }

    pub fn select_uart(&mut self, name: &str) -> Result<(), String> {
        self.input_uart = self
            .uarts
            .iter()
            .position(|uart| uart.name == name)
            .ok_or_else(|| format!("unknown RV64 UART {name:?}"))?;
        Ok(())
    }

    pub fn queue_input(&mut self, bytes: Vec<u8>) {
        if let Some(uart) = self.uarts.get_mut(self.input_uart) {
            match &mut uart.kind {
                UartKind::Ns16550(uart) => uart.queue_input(bytes),
                UartKind::SiFive {
                    receive,
                    rx_control,
                    ..
                } if *rx_control & 1 != 0 => receive.extend(bytes),
                UartKind::SiFive { .. } => {}
            }
        }
    }

    pub fn tick(&mut self, hart: usize) -> u64 {
        self.lines.fill(0);
        for uart in &self.uarts {
            let active = uart.pending();
            for &(target, pin) in &uart.cpu_lines {
                self.lines[target] |= u64::from(active) << pin;
            }
            if let Some(plic) = &mut self.plic {
                plic.line(uart.source, active);
            }
        }
        self.lines[hart] | self.plic.as_ref().map_or(0, |plic| plic.outputs(hart))
    }

    pub fn read(&mut self, address: u32, width: usize) -> Option<u32> {
        if let Some(uart) = self.uarts.iter_mut().find(|uart| uart.contains(address)) {
            return uart.read(address, width);
        }
        if width == 4
            && let Some(device) = self
                .reset_generators
                .iter()
                .find(|device| device.contains(address))
        {
            return Some(device.read(address));
        }
        let plic = self.plic.as_mut()?;
        (width == 4 && plic.contains(address)).then(|| plic.read(address - plic.base))
    }

    pub fn write(
        &mut self,
        address: u32,
        width: usize,
        value: u32,
        output: &mut impl Write,
    ) -> Option<io::Result<()>> {
        if let Some(uart) = self.uarts.iter_mut().find(|uart| uart.contains(address)) {
            return uart.write(address, width, value, output);
        }
        if width == 4
            && let Some(device) = self
                .reset_generators
                .iter_mut()
                .find(|device| device.contains(address))
        {
            device.write(address, value);
            return Some(Ok(()));
        }
        let plic = self.plic.as_mut()?;
        (width == 4 && plic.contains(address)).then(|| {
            plic.write(address - plic.base, value);
            Ok(())
        })
    }
}

enum UartKind {
    Ns16550(Ns16550),
    SiFive {
        receive: VecDeque<u8>,
        tx_control: u32,
        rx_control: u32,
        enable: u32,
        divisor: u32,
    },
}

pub struct Uart {
    name: String,
    pub base: u32,
    source: usize,
    cpu_lines: Vec<(usize, u32)>,
    kind: UartKind,
}

impl Uart {
    pub fn new(
        name: String,
        base: u32,
        source: usize,
        cpu_lines: Vec<(usize, u32)>,
        sifive: bool,
        wide: bool,
    ) -> Self {
        let kind = if sifive {
            UartKind::SiFive {
                receive: VecDeque::new(),
                tx_control: 0,
                rx_control: 0,
                enable: 0,
                divisor: 0xffff,
            }
        } else {
            UartKind::Ns16550(Ns16550::new(base, source as u32, wide))
        };
        Self {
            name,
            base,
            source,
            cpu_lines,
            kind,
        }
    }

    pub fn size(&self) -> u64 {
        0x100
    }

    fn contains(&self, address: u32) -> bool {
        u64::from(address.wrapping_sub(self.base)) < self.size()
    }

    fn pending(&self) -> bool {
        match &self.kind {
            UartKind::Ns16550(uart) => uart.interrupt_pending(),
            UartKind::SiFive {
                receive,
                tx_control,
                rx_control,
                enable,
                ..
            } => sifive_pending(receive.len(), *tx_control, *rx_control) & enable != 0,
        }
    }

    fn read(&mut self, address: u32, width: usize) -> Option<u32> {
        match &mut self.kind {
            UartKind::Ns16550(uart) => Some(uart.read(ns16550_register(uart, address, width)?)),
            UartKind::SiFive {
                receive,
                tx_control,
                rx_control,
                enable,
                divisor,
            } if width == 4 => Some(match address - self.base {
                4 => receive.pop_front().map_or(1 << 31, u32::from),
                8 => *tx_control,
                12 => *rx_control,
                16 => *enable,
                20 => sifive_pending(receive.len(), *tx_control, *rx_control),
                24 => *divisor,
                _ => 0,
            }),
            _ => None,
        }
    }

    fn write(
        &mut self,
        address: u32,
        width: usize,
        value: u32,
        output: &mut impl Write,
    ) -> Option<io::Result<()>> {
        match &mut self.kind {
            UartKind::Ns16550(uart) => {
                Some(uart.write(ns16550_register(uart, address, width)?, value, output))
            }
            UartKind::SiFive {
                receive,
                tx_control,
                rx_control,
                enable,
                divisor,
                ..
            } if width == 4 => {
                match address - self.base {
                    0 if *tx_control & 1 != 0 => return Some(output.write_all(&[value as u8])),
                    8 => *tx_control = value & 0x70003,
                    12 => {
                        *rx_control = value & 0x70001;
                        if *rx_control & 1 == 0 {
                            receive.clear();
                        }
                    }
                    16 => *enable = value & 3,
                    24 => *divisor = value & 0xffff,
                    _ => {}
                }
                Some(Ok(()))
            }
            _ => None,
        }
    }
}

fn ns16550_register(uart: &Ns16550, address: u32, width: usize) -> Option<u32> {
    if !matches!(width, 1 | 2 | 4) {
        return None;
    }
    let offset = address - uart.base;
    // Wide Renode UARTs accept both byte and word access to each spaced register.
    Some(if uart.wide && offset & 3 == 0 {
        offset >> 2
    } else {
        offset
    })
}

fn sifive_pending(receive: usize, tx_control: u32, rx_control: u32) -> u32 {
    u32::from(tx_control >> 16 != 0) | (u32::from(receive > (rx_control >> 16) as usize) << 1)
}

pub struct Plic {
    pub base: u32,
    priorities: bool,
    priority: Vec<u32>,
    level: Vec<u32>,
    pending: Vec<u32>,
    claimed_by: Vec<Option<usize>>,
    contexts: Vec<Context>,
}

struct Context {
    route: Option<(usize, u32)>,
    threshold: u32,
    enable: Vec<u32>,
}

impl Plic {
    pub const SIZE: u64 = 0x0400_0000;

    pub fn new(
        base: u32,
        sources: usize,
        routes: Vec<Option<(usize, u32)>>,
        priorities: bool,
    ) -> Self {
        let words = (sources + 1).div_ceil(32);
        Self {
            base,
            priorities,
            priority: vec![0; sources + 1],
            level: vec![0; words],
            pending: vec![0; words],
            claimed_by: vec![None; sources + 1],
            contexts: routes
                .into_iter()
                .map(|route| Context {
                    route,
                    threshold: 0,
                    enable: vec![0; words],
                })
                .collect(),
        }
    }

    fn contains(&self, address: u32) -> bool {
        u64::from(address.wrapping_sub(self.base)) < Self::SIZE
    }

    fn line(&mut self, source: usize, active: bool) {
        if source == 0 || source >= self.priority.len() {
            return;
        }
        let word = source / 32;
        let bit = 1 << (source % 32);
        if active {
            self.level[word] |= bit;
            if self.claimed_by[source].is_none() {
                self.pending[word] |= bit;
            }
        } else {
            self.level[word] &= !bit;
        }
    }

    fn select(&self, context: &Context, mut threshold: u32) -> usize {
        let mut selected = 0;
        for (word, (&pending, &enabled)) in self.pending.iter().zip(&context.enable).enumerate() {
            let mut candidates = pending & enabled;
            while candidates != 0 {
                let source = word * 32 + candidates.trailing_zeros() as usize;
                candidates &= candidates - 1;
                let priority = if self.priorities {
                    self.priority[source]
                } else {
                    1
                };
                if priority > threshold {
                    selected = source;
                    threshold = priority;
                }
            }
        }
        selected
    }

    fn outputs(&self, hart: usize) -> u64 {
        self.contexts.iter().fold(0, |bits, context| {
            if let Some((target, pin)) = context.route
                && target == hart
                && self.select(context, context.threshold) != 0
            {
                bits | (1 << pin)
            } else {
                bits
            }
        })
    }

    fn read(&mut self, offset: u32) -> u32 {
        match offset {
            0..0x1000 => self
                .priority
                .get((offset / 4) as usize)
                .copied()
                .unwrap_or(0),
            0x1000..0x1080 => self
                .pending
                .get(((offset - 0x1000) / 4) as usize)
                .copied()
                .unwrap_or(0),
            0x2000..0x20_0000 => {
                let offset = offset - 0x2000;
                self.contexts
                    .get((offset / 0x80) as usize)
                    .and_then(|context| context.enable.get(((offset % 0x80) / 4) as usize))
                    .copied()
                    .unwrap_or(0)
            }
            0x20_0000.. => {
                let offset = offset - 0x20_0000;
                let index = (offset / 0x1000) as usize;
                let Some(context) = self.contexts.get(index) else {
                    return 0;
                };
                match offset % 0x1000 {
                    0 => context.threshold,
                    4 => {
                        let source = self.select(context, 0);
                        if source != 0 {
                            self.pending[source / 32] &= !(1 << (source % 32));
                            self.claimed_by[source] = Some(index);
                        }
                        source as u32
                    }
                    _ => 0,
                }
            }
            _ => 0,
        }
    }

    fn write(&mut self, offset: u32, value: u32) {
        match offset {
            4..0x1000 => {
                if let Some(priority) = self.priority.get_mut((offset / 4) as usize) {
                    *priority = value;
                }
            }
            0x2000..0x20_0000 => {
                let offset = offset - 0x2000;
                let word = ((offset % 0x80) / 4) as usize;
                if let Some(slot) = self
                    .contexts
                    .get_mut((offset / 0x80) as usize)
                    .and_then(|context| context.enable.get_mut(word))
                {
                    let valid = (self.priority.len() - word * 32).min(32);
                    let mask = u32::MAX >> (32 - valid);
                    *slot = value & mask;
                    if word == 0 {
                        *slot &= !1;
                    }
                }
            }
            0x20_0000.. => {
                let offset = offset - 0x20_0000;
                let index = (offset / 0x1000) as usize;
                let Some(context) = self.contexts.get_mut(index) else {
                    return;
                };
                match offset % 0x1000 {
                    0 if self.priorities => context.threshold = value,
                    4 => {
                        let source = value as usize;
                        if self.claimed_by.get(source) == Some(&Some(index)) {
                            self.claimed_by[source] = None;
                            let bit = 1 << (source % 32);
                            self.pending[source / 32] |= self.level[source / 32] & bit;
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}
