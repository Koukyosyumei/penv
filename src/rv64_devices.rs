use std::{
    collections::VecDeque,
    io::{self, Write},
};

use crate::{
    machine::MAX_HARTS,
    ns16550::Ns16550,
    rv64_ethernet::Ethernet,
    rv64_gem::Gem,
    rv64_mpfs::{DdrMock, Gpio, I2c, Qspi, Spi, Sysreg, Usb},
    rv64_mpfs_ihc::FireIhc,
    rv64_mpfs_sd,
    rv64_mpfs_wdog::Watchdog,
    rv64_starfive::{ResetGenerator, StarLightResetPin},
};

#[derive(Default)]
pub struct Devices {
    pub plic: Option<Plic>,
    pub uarts: Vec<Uart>,
    pub ethernet: Vec<Ethernet>,
    pub gems: Vec<Gem>,
    pub reset_generators: Vec<ResetGenerator>,
    pub mpfs_i2c: Vec<I2c>,
    pub mpfs_qspi: Vec<Qspi>,
    pub mpfs_gpio: Vec<Gpio>,
    pub mpfs_usb: Vec<Usb>,
    pub fire_ihc: Option<Box<FireIhc>>,
    pub fire_media_controller: Option<usize>,
    pub mpfs_sd: Vec<rv64_mpfs_sd::Controller>,
    pub mpfs_watchdogs: Vec<Watchdog>,
    pub mpfs_spi: Vec<Spi>,
    pub mpfs_ddr: Vec<DdrMock>,
    pub mpfs_sysreg: Option<Sysreg>,
    pub mpfs_cache_way_enable: Option<MpfsCacheWayEnable>,
    pub fallback_ranges: Vec<FallbackRange>,
    pub starlight_reset_pin: Option<StarLightResetPin>,
    pub reset_requested: bool,
    lines: [u64; MAX_HARTS],
    input_uart: usize,
}

// Backing value for the CacheConfig_WayEnable Python peripheral in polarfire-soc.repl.
pub struct MpfsCacheWayEnable {
    pub base: u32,
    value: u64,
    pub way_masks: Option<u32>,
}

impl MpfsCacheWayEnable {
    pub const SIZE: u64 = 8;

    pub const WAY_MASK_OFFSET: u32 = 0x800 - 8;
    pub const WAY_MASK_SIZE: u64 = 15 * 8;

    pub fn new(base: u32, way_masks: Option<u32>) -> Self {
        Self {
            base,
            value: 0,
            way_masks,
        }
    }

    fn mask(&self, address: u32, width: usize) -> Option<u64> {
        // The REPL script uses the same value for every offset in its window.
        let offset = u64::from(address.checked_sub(self.base)?);
        (offset + width as u64 <= Self::SIZE).then(|| u64::MAX >> (64 - width * 8))
    }

    pub fn read(&self, address: u32, width: usize) -> Option<u64> {
        Some(self.value & self.mask(address, width)?)
    }

    pub fn write(&mut self, address: u32, width: usize, value: u64) -> Option<()> {
        self.swap(address, width, value).map(|_| ())
    }

    pub fn swap(&mut self, address: u32, width: usize, value: u64) -> Option<u64> {
        // Renode leaves these cache way-mask registers in a silenced range.
        if width == 8
            && let Some(base) = self.way_masks
            && let Some(offset) = address.checked_sub(base)
            && u64::from(offset) + 8 <= Self::WAY_MASK_SIZE
        {
            return Some(0);
        }
        let mask = self.mask(address, width)?;
        let previous = std::mem::replace(&mut self.value, value & mask);
        Some(previous & mask)
    }
}

pub struct FallbackRange {
    pub start: u64,
    pub end: u64,
    pub value: u64,
    pub silent: bool,
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

    pub fn has_uart(&self, name: &str) -> bool {
        self.uart_index(name).is_some()
    }

    pub(crate) fn uart_index(&self, name: &str) -> Option<usize> {
        self.uarts.iter().position(|uart| uart.name == name)
    }

    pub(crate) fn uart_for_hart(&self, hart: usize, address: u32) -> Option<usize> {
        self.uarts.iter().position(|uart| {
            uart.contains(address) && (uart.harts == 0 || uart.harts & (1 << hart) != 0)
        })
    }

    pub fn select_uart(&mut self, name: &str) -> Result<(), String> {
        self.input_uart = self.uart_index(name)
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

    pub fn advance_watchdogs(&mut self, steps: u64) {
        for watchdog in &mut self.mpfs_watchdogs {
            self.reset_requested |= watchdog.advance(steps);
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
        if let Some(plic) = &mut self.plic {
            if let Some(ihc) = self.fire_ihc.as_ref() {
                for (source, pending) in ihc.interrupts() {
                    plic.line(source, pending);
                }
            }
            for i2c in &self.mpfs_i2c {
                plic.line(i2c.irq, i2c.pending());
            }
            for sd in &self.mpfs_sd {
                let (source, pending) = sd.interrupt();
                plic.line(source, pending);
            }
        }
        for watchdog in &self.mpfs_watchdogs {
            for (output, active) in watchdog.outputs.iter().zip(watchdog.levels()) {
                for &(hart, pin) in &output.harts {
                    self.lines[hart] |= u64::from(active) << pin;
                }
                if let Some(plic) = &mut self.plic {
                    plic.line(output.plic, active);
                }
            }
        }
        self.lines[hart] | self.plic.as_ref().map_or(0, |plic| plic.outputs(hart))
    }

    pub fn set_ethernet_irq(&mut self, source: usize, active: bool) {
        if let Some(plic) = &mut self.plic {
            plic.line(source, active);
        }
    }

    #[cfg(test)]
    pub fn read(&mut self, address: u32, width: usize) -> Option<u32> {
        self.read_for_hart(0, address, width)
    }

    pub fn read_for_hart(&mut self, hart: usize, address: u32, width: usize) -> Option<u32> {
        if width == 4
            && let Some(value) = self
                .starlight_reset_pin
                .as_ref()
                .and_then(|pin| pin.read(address))
        {
            return Some(value);
        }
        if let Some(uart) = self.uarts.iter_mut().find(|uart| {
            uart.contains(address) && (uart.harts == 0 || uart.harts & (1 << hart) != 0)
        }) {
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
        if width == 4
            && let Some(device) = self.ethernet.iter().find(|device| device.contains(address))
        {
            return Some(device.read(address));
        }
        if let Some(value) = self
            .mpfs_sysreg
            .as_ref()
            .and_then(|reg| reg.read(address, width))
        {
            return Some(value);
        }
        if let Some(ddr) = self
            .mpfs_ddr
            .iter_mut()
            .find(|ddr| ddr.accepts(address, width))
        {
            return Some(ddr.read(address, width));
        }
        if let Some(value) = self
            .mpfs_usb
            .iter()
            .find_map(|usb| usb.read(address, width))
        {
            return Some(value);
        }
        if let Some(value) = self
            .fire_ihc
            .as_ref()
            .and_then(|ihc| ihc.read(address, width))
        {
            return Some(value);
        }
        if width == 4
            && address == rv64_mpfs_sd::FIRE_MEDIA_SELECT
            && let Some(index) = self.fire_media_controller
        {
            return Some(u32::from(self.mpfs_sd[index].sd_selected()));
        }
        for sd in &mut self.mpfs_sd {
            if let Some(value) = sd.read(address, width) {
                return Some(value);
            }
        }
        if let Some(value) = self
            .mpfs_watchdogs
            .iter()
            .find_map(|watchdog| watchdog.read(address, width))
        {
            return Some(value);
        }
        if let Some(value) = self.gems.iter().find_map(|gem| gem.read(address, width)) {
            return Some(value);
        }
        if let Some(value) = self
            .mpfs_spi
            .iter()
            .find_map(|spi| spi.read(address, width))
        {
            return Some(value);
        }
        if let Some(value) = self
            .mpfs_gpio
            .iter()
            .find_map(|gpio| gpio.read(address, width))
        {
            return Some(value);
        }
        if let Some(value) = self
            .mpfs_i2c
            .iter()
            .find_map(|i2c| i2c.read(address, width))
            .or_else(|| {
                self.mpfs_qspi
                    .iter()
                    .find_map(|qspi| qspi.read(address, width))
            })
        {
            return Some(value);
        }
        let plic = self.plic.as_mut()?;
        (width == 4 && plic.contains(address)).then(|| plic.read(address - plic.base))
    }

    #[cfg(test)]
    pub fn write(
        &mut self,
        address: u32,
        width: usize,
        value: u32,
        output: &mut impl Write,
    ) -> Option<io::Result<()>> {
        self.write_for_hart(0, address, width, value, output)
    }

    pub fn write_for_hart(
        &mut self,
        hart: usize,
        address: u32,
        width: usize,
        value: u32,
        output: &mut impl Write,
    ) -> Option<io::Result<()>> {
        if width == 4
            && let Some(reset) = self
                .starlight_reset_pin
                .as_mut()
                .and_then(|pin| pin.write(address, value))
        {
            self.reset_requested |= reset;
            return Some(Ok(()));
        }
        if let Some(uart) = self.uarts.iter_mut().find(|uart| {
            uart.contains(address) && (uart.harts == 0 || uart.harts & (1 << hart) != 0)
        }) {
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
        if let Some(()) = self
            .mpfs_sysreg
            .as_mut()
            .and_then(|reg| reg.write(address, width, value))
        {
            return Some(Ok(()));
        }
        if self.mpfs_ddr.iter().any(|ddr| ddr.accepts(address, width)) {
            return Some(Ok(()));
        }
        if self
            .mpfs_usb
            .iter_mut()
            .any(|usb| usb.write(address, width, value).is_some())
        {
            return Some(Ok(()));
        }
        if self
            .fire_ihc
            .as_mut()
            .and_then(|ihc| ihc.write(address, width, value))
            .is_some()
        {
            return Some(Ok(()));
        }
        if width == 4
            && address == rv64_mpfs_sd::FIRE_MEDIA_SELECT
            && let Some(index) = self.fire_media_controller
        {
            if value <= 1 {
                self.mpfs_sd[index].select_sd(value == 1);
            }
            return Some(Ok(()));
        }
        if self
            .mpfs_watchdogs
            .iter_mut()
            .any(|watchdog| watchdog.write(address, width, value).is_some())
        {
            return Some(Ok(()));
        }
        if self
            .gems
            .iter_mut()
            .any(|gem| gem.write(address, width, value).is_some())
        {
            return Some(Ok(()));
        }
        if self
            .mpfs_spi
            .iter_mut()
            .any(|spi| spi.write(address, width, value).is_some())
        {
            return Some(Ok(()));
        }
        if self
            .mpfs_gpio
            .iter_mut()
            .any(|gpio| gpio.write(address, width, value).is_some())
        {
            return Some(Ok(()));
        }
        if self
            .mpfs_i2c
            .iter_mut()
            .any(|i2c| i2c.write(address, width, value).is_some())
            || self
                .mpfs_qspi
                .iter_mut()
                .any(|qspi| qspi.write(address, width, value).is_some())
        {
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
    pub(crate) harts: u64,
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
            UartKind::Ns16550(Ns16550::new(base, Some(source as u32), wide))
        };
        Self {
            harts: 0,
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
            UartKind::Ns16550(uart) => {
                let register = ns16550_register(uart, address, width)?;
                Some(read_ns16550(uart, register))
            }
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

fn read_ns16550(uart: &mut Ns16550, register: u32) -> u32 {
    match register {
        6 => {
            let control = uart.read(4);
            if control & 0x10 != 0 {
                // In loopback OUT2, OUT1, DTR, and RTS drive the modem inputs.
                return ((control & 0x0c) << 4) | ((control & 1) << 5) | ((control & 2) << 3);
            }
        }
        0x14 => return uart.read(5),
        _ => {}
    }
    uart.read(register)
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

    fn select(&self, context: &Context) -> usize {
        let mut selected = 0;
        let mut selected_priority = 0;
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
                if priority > selected_priority {
                    selected = source;
                    selected_priority = priority;
                }
            }
        }
        selected
    }

    fn outputs(&self, hart: usize) -> u64 {
        self.contexts.iter().fold(0, |bits, context| {
            if let Some((target, pin)) = context.route
                && target == hart
                && self.select(context) != 0
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
                    // The Renode REPL PLIC has a fixed zero priority threshold.
                    0 => 0,
                    4 => {
                        let source = self.select(context);
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
                if index < self.contexts.len() && offset % 0x1000 == 4 {
                    let source = value as usize;
                    if self.claimed_by.get(source) == Some(&Some(index)) {
                        self.claimed_by[source] = None;
                        let bit = 1 << (source % 32);
                        self.pending[source / 32] |= self.level[source / 32] & bit;
                    }
                }
            }
            _ => {}
        }
    }
}
