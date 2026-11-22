use std::{
    alloc::{Layout, alloc_zeroed},
    fmt,
    io::{self, Write},
};

use crate::{
    clint::Clint,
    murax::{Gpio, Timer},
    ns16550::Ns16550,
    nvic::{Nvic, VTOR},
    plic::Plic,
    stm32_exti::Exti,
    stm32_gpio::Stm32Gpio,
    stm32_rcc::Stm32F0Rcc,
    stm32_uart::{Stm32Irq, Stm32Usart},
    systick::SysTick,
    uart::{LiteXUart, MuraxUart},
};

pub const RAM_BASE: u32 = 0x8000_0000;
pub const UART_TX: u32 = 0x1000_0000;

#[derive(Debug)]
pub enum BusError {
    Unmapped(u32, usize),
    Denied(u32),
    Page(u32),
    Misaligned(u32, usize),
    Output(io::Error),
}

impl BusError {
    pub fn at_virtual(self, address: u32) -> Self {
        match self {
            Self::Unmapped(_, width) => Self::Unmapped(address, width),
            _ => self,
        }
    }
}

impl fmt::Display for BusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unmapped(address, width) => {
                write!(f, "unmapped RAM access at {address:#010x} ({width} bytes)")
            }
            Self::Page(address) => write!(f, "page fault at {address:#010x}"),
            Self::Denied(address) => write!(
                f,
                "physical memory protection denied access at {address:#010x}"
            ),
            Self::Misaligned(address, width) => {
                write!(f, "misaligned {width}-byte access at {address:#010x}")
            }
            Self::Output(error) => write!(f, "UART: {error}"),
        }
    }
}

struct Region {
    base: u32,
    bytes: Vec<u8>,
}

#[derive(Default)]
pub struct Bus {
    regions: Vec<Region>,
    reservations: [Option<u32>; crate::machine::MAX_HARTS],
    uart_tx: Option<u32>,
    pub uart: Option<MuraxUart>,
    ns16550: Option<Ns16550>,
    litex_uart: Option<LiteXUart>,
    stm32_uarts: Vec<Stm32Usart>,
    exti: Option<Exti>,
    stm32_gpios: Vec<Stm32Gpio>,
    stm32f0_tags: Vec<std::ops::RangeInclusive<u32>>,
    stm32_rcc: Option<Stm32F0Rcc>,
    plic: Option<Plic>,
    pub systick: Option<SysTick>,
    pub nvic: Option<Nvic>,
    pub clint: Option<Clint>,
    gpio: Option<(u32, Gpio)>,
    pub timer: Option<(u32, Timer)>,
}

impl Region {
    fn new(base: u32, size: usize) -> Result<Self, String> {
        let end = u64::from(base).saturating_add(size as u64);
        if size == 0 || end > 1 << 32 {
            return Err(format!("invalid RAM range at {base:#010x} ({size} bytes)"));
        }
        let layout = Layout::array::<u8>(size).map_err(|error| error.to_string())?;
        // A zeroed allocation allows demand-zero pages on hosts that support them.
        // SAFETY: The layout is valid and nonzero. The pointer is checked below.
        let allocation = unsafe { alloc_zeroed(layout) };
        if allocation.is_null() {
            return Err(format!("could not allocate {size} bytes of guest RAM"));
        }
        // SAFETY: Global allocation with u8 alignment and size bytes, all initialized.
        // Vec takes ownership and deallocates with the same layout.
        let bytes = unsafe { Vec::from_raw_parts(allocation, size, size) };
        Ok(Self { base, bytes })
    }

    fn offset(&self, address: u32, width: usize) -> Option<usize> {
        let offset = address.checked_sub(self.base)? as usize;
        (width <= self.bytes.len().checked_sub(offset)?).then_some(offset)
    }
}

impl Bus {
    pub fn new(size: usize) -> Result<Self, String> {
        let mut bus = Self {
            uart_tx: Some(UART_TX),
            ..Self::default()
        };
        bus.map_ram(RAM_BASE, size)?;
        Ok(bus)
    }

    pub fn map_ram(&mut self, base: u32, size: usize) -> Result<(), String> {
        let start = u64::from(base);
        let end = start.saturating_add(size as u64);
        if self.overlaps(start, end) {
            return Err(format!(
                "overlapping RAM range at {base:#010x} ({size} bytes)"
            ));
        }
        self.regions.push(Region::new(base, size)?);
        Ok(())
    }

    fn overlaps(&self, start: u64, end: u64) -> bool {
        (self.nvic.is_some() && Nvic::overlaps(start, end))
            || (self.systick.is_some() && SysTick::overlaps(start, end))
            || self
                .uart_tx
                .is_some_and(|address| (start..end).contains(&u64::from(address)))
            || self.uart.as_ref().is_some_and(|uart| {
                let other = u64::from(uart.base);
                start < other + MuraxUart::SIZE && other < end
            })
            || self.litex_uart.as_ref().is_some_and(|uart| {
                let other = u64::from(uart.base);
                start < other + u64::from(LiteXUart::SIZE) && other < end
            })
            || self.stm32_gpios.iter().any(|gpio| {
                let other = u64::from(gpio.base);
                start < other + u64::from(gpio.size) && other < end
            })
            || self.stm32_rcc.as_ref().is_some_and(|rcc| {
                let other = u64::from(rcc.base);
                start < other + u64::from(Stm32F0Rcc::SIZE) && other < end
            })
            || self.exti.as_ref().is_some_and(|exti| {
                let other = u64::from(exti.base);
                start < other + u64::from(Exti::SIZE) && other < end
            })
            || self.stm32_uarts.iter().any(|uart| {
                let other = u64::from(uart.base);
                start < other + u64::from(Stm32Usart::SIZE) && other < end
            })
            || self.ns16550.as_ref().is_some_and(|uart| {
                let other = u64::from(uart.base);
                start < other + uart.size() && other < end
            })
            || self.plic.as_ref().is_some_and(|plic| {
                let other = u64::from(plic.base);
                start < other + Plic::SIZE && other < end
            })
            || self.clint.as_ref().is_some_and(|clint| {
                let other = u64::from(clint.base);
                start < other + Clint::SIZE && other < end
            })
            || [
                (self.gpio.as_ref().map(|(base, _)| *base), 0xc),
                (self.timer.as_ref().map(|(base, _)| *base), 0x5c),
            ]
            .into_iter()
            .any(|(base, size)| {
                base.is_some_and(|base| {
                    let other = u64::from(base);
                    start < other + size && other < end
                })
            })
            || self.regions.iter().any(|region| {
                let other = u64::from(region.base);
                start < other + region.bytes.len() as u64 && other < end
            })
    }

    pub fn map_clint(&mut self, base: u32, targets: usize) -> Result<(), String> {
        let start = u64::from(base);
        let end = start + Clint::SIZE;
        if base & 3 != 0 || end > 1 << 32 || self.clint.is_some() || self.overlaps(start, end) {
            return Err(format!(
                "invalid or overlapping CLINT range at {base:#010x}"
            ));
        }
        self.clint = Some(Clint::new(base, targets));
        Ok(())
    }

    pub fn map_uart(&mut self, base: u32, irq_connected: bool) -> Result<(), String> {
        let start = u64::from(base);
        let end = start + MuraxUart::SIZE;
        if base & 3 != 0 || end > 1 << 32 || self.uart.is_some() || self.overlaps(start, end) {
            return Err(format!("invalid or overlapping UART range at {base:#010x}"));
        }
        self.uart = Some(MuraxUart::new(base, irq_connected));
        Ok(())
    }

    pub fn map_plic(&mut self, base: u32, sources: u32, outputs: &[u32]) -> Result<(), String> {
        if !(1..=63).contains(&sources) {
            return Err("PLIC requires 1–63 interrupt sources".into());
        }
        let start = u64::from(base);
        let end = start + Plic::SIZE;
        if base & 3 != 0 || end > 1 << 32 || self.plic.is_some() || self.overlaps(start, end) {
            return Err(format!("invalid or overlapping PLIC range at {base:#010x}"));
        }
        self.plic = Some(Plic::new(base, sources, outputs));
        Ok(())
    }

    pub fn map_ns16550(&mut self, base: u32, source: u32, wide: bool) -> Result<(), String> {
        let uart = Ns16550::new(base, source, wide);
        let start = u64::from(base);
        let end = start + uart.size();
        if (wide && base & 3 != 0)
            || end > 1 << 32
            || self.uart.is_some()
            || self.ns16550.is_some()
            || self.overlaps(start, end)
        {
            return Err(format!(
                "invalid or overlapping NS16550 range at {base:#010x}"
            ));
        }
        self.ns16550 = Some(uart);
        Ok(())
    }

    pub fn map_nvic(&mut self, priority_mask: u8, uart_irq: Option<u8>) -> Result<(), String> {
        if Nvic::RANGES
            .into_iter()
            .any(|(start, end)| self.overlaps(u64::from(start), u64::from(end)))
        {
            return Err("Cortex-M NVIC registers overlap an existing mapping".into());
        }
        self.nvic = Some(Nvic::new(priority_mask, uart_irq));
        Ok(())
    }

    pub fn sample_nvic(&mut self, active: u64) {
        if let Some(nvic) = self.nvic.as_mut() {
            nvic.set_uart_level(
                self.litex_uart
                    .as_ref()
                    .is_some_and(LiteXUart::interrupt_pending),
                active,
            );
            let mut exti_inputs = 0;
            for uart in &self.stm32_uarts {
                if uart.interrupt_pending() {
                    match uart.irq {
                        Some(Stm32Irq::Nvic(line)) => nvic.set_line(line, true, active),
                        Some(Stm32Irq::Exti(line)) => exti_inputs |= 1 << line,
                        None => {}
                    }
                }
            }
            if let Some(exti) = self.exti.as_mut() {
                let mut mask = exti.sample(exti_inputs);
                while mask != 0 {
                    let line = mask.trailing_zeros() as u8;
                    mask &= mask - 1;
                    nvic.set_line(line, true, active);
                }
            }
        }
    }

    pub fn map_exti(
        &mut self,
        base: u32,
        outputs: Vec<Option<u8>>,
        first_direct: u32,
    ) -> Result<(), String> {
        let start = u64::from(base);
        let end = start + u64::from(Exti::SIZE);
        if base & 3 != 0 || end > 1 << 32 || self.overlaps(start, end) {
            return Err(format!(
                "invalid or overlapping STM32 EXTI range at {base:#010x}"
            ));
        }
        self.exti = Some(Exti::new(base, outputs, first_direct));
        Ok(())
    }

    pub fn map_systick(&mut self) -> Result<(), String> {
        if SysTick::RANGES
            .into_iter()
            .any(|(start, end)| self.overlaps(u64::from(start), u64::from(end)))
        {
            return Err("Cortex-M SysTick registers overlap an existing mapping".into());
        }
        self.systick = Some(SysTick::default());
        Ok(())
    }

    pub fn map_litex_uart(&mut self, base: u32) -> Result<(), String> {
        let start = u64::from(base);
        let end = start + u64::from(LiteXUart::SIZE);
        if base & 3 != 0 || end > 1 << 32 || self.overlaps(start, end) {
            return Err(format!(
                "invalid or overlapping LiteX UART range at {base:#010x}"
            ));
        }
        self.litex_uart = Some(LiteXUart::new(base));
        Ok(())
    }

    pub fn set_stm32f0_tags(&mut self, tags: Vec<std::ops::RangeInclusive<u32>>) {
        self.stm32f0_tags = tags;
    }

    fn stm32f0_tagged(&self, address: u32, width: usize) -> bool {
        address.checked_add(width as u32 - 1).is_some_and(|end| {
            self.stm32f0_tags
                .iter()
                .any(|range| range.contains(&address) && range.contains(&end))
        })
    }

    pub fn map_stm32_gpio(&mut self, gpio: Stm32Gpio) -> Result<(), String> {
        let start = u64::from(gpio.base);
        let end = start + u64::from(gpio.size);
        if gpio.base & 3 != 0
            || gpio.size < 0x2c
            || gpio.size & 3 != 0
            || end > 1 << 32
            || self.overlaps(start, end)
        {
            return Err(format!(
                "invalid or overlapping STM32 GPIO range at {:#010x}",
                gpio.base
            ));
        }
        self.stm32_gpios.push(gpio);
        Ok(())
    }

    pub fn map_stm32f0_rcc(&mut self, base: u32) -> Result<(), String> {
        let start = u64::from(base);
        let end = start + u64::from(Stm32F0Rcc::SIZE);
        if self.stm32_rcc.is_some() || base & 3 != 0 || end > 1 << 32 || self.overlaps(start, end) {
            return Err(format!(
                "invalid or overlapping STM32F0 RCC range at {base:#010x}"
            ));
        }
        self.stm32_rcc = Some(Stm32F0Rcc::new(base));
        Ok(())
    }

    pub fn map_stm32_usart(
        &mut self,
        base: u32,
        irq: Option<Stm32Irq>,
        low_power: bool,
    ) -> Result<(), String> {
        let start = u64::from(base);
        let end = start + u64::from(Stm32Usart::SIZE);
        if base & 3 != 0 || end > 1 << 32 || self.overlaps(start, end) {
            return Err(format!(
                "invalid or overlapping STM32 USART range at {base:#010x}"
            ));
        }
        self.stm32_uarts.push(Stm32Usart::new(base, irq, low_power));
        Ok(())
    }

    pub fn has_uart_input(&self) -> bool {
        self.uart.is_some()
            || self.ns16550.is_some()
            || self.litex_uart.is_some()
            || !self.stm32_uarts.is_empty()
    }

    pub fn queue_uart_input(&mut self, bytes: Vec<u8>) {
        if let Some(uart) = self.uart.as_mut() {
            uart.queue_input(bytes);
        } else if let Some(uart) = self.ns16550.as_mut() {
            uart.queue_input(bytes);
        } else if let Some(uart) = self.litex_uart.as_mut() {
            uart.queue_input(bytes);
        } else if let Some(uart) = self.stm32_uarts.first_mut() {
            uart.queue_input(bytes);
        }
    }

    pub fn tick_plic(&mut self) -> u32 {
        if let Some(uart) = self.ns16550.as_ref()
            && let Some(plic) = self.plic.as_mut()
        {
            plic.set_line(uart.source, uart.interrupt_pending());
        }
        self.plic.as_ref().map_or(0, Plic::irq_outputs)
    }

    pub fn has_plic(&self) -> bool {
        self.plic.is_some()
    }

    pub fn map_murax_gpio(&mut self, base: u32) -> Result<(), String> {
        self.check_murax_range(base, 0xc, self.gpio.is_some())?;
        self.gpio = Some((base, Gpio::default()));
        Ok(())
    }

    pub fn map_murax_timer(&mut self, base: u32, connected: bool) -> Result<(), String> {
        self.check_murax_range(base, 0x5c, self.timer.is_some())?;
        let mut timer = Timer::default();
        timer.connected = connected;
        self.timer = Some((base, timer));
        Ok(())
    }

    fn check_murax_range(&self, base: u32, size: u64, already_mapped: bool) -> Result<(), String> {
        let start = u64::from(base);
        let end = start + size;
        if base & 3 != 0 || end > 1 << 32 || already_mapped || self.overlaps(start, end) {
            return Err(format!(
                "invalid or overlapping Murax peripheral range at {base:#010x}"
            ));
        }
        Ok(())
    }

    pub fn memory(&mut self, address: u32, width: usize) -> Result<&mut [u8], BusError> {
        for region in &mut self.regions {
            if let Some(offset) = region.offset(address, width) {
                if width != 0 {
                    for reservation in &mut self.reservations {
                        if reservation.is_some_and(|reserved| {
                            u64::from(address) < u64::from(reserved) + 4
                                && u64::from(reserved) < u64::from(address) + width as u64
                        }) {
                            *reservation = None;
                        }
                    }
                }
                return Ok(&mut region.bytes[offset..offset + width]);
            }
        }
        Err(BusError::Unmapped(address, width))
    }

    pub fn load_reserved(&mut self, hart: usize, address: u32) -> Result<u32, BusError> {
        let value = self.read_ram::<4>(address)?;
        self.reservations[hart] = Some(address);
        Ok(value)
    }

    pub fn store_conditional(
        &mut self,
        hart: usize,
        address: u32,
        value: u32,
    ) -> Result<u32, BusError> {
        let reserved = self.reservations[hart].take() == Some(address);
        if reserved {
            self.atomic_word(address)?
                .copy_from_slice(&value.to_le_bytes());
            Ok(0)
        } else {
            self.read_ram::<4>(address)?;
            Ok(1)
        }
    }

    pub fn atomic_update(
        &mut self,
        address: u32,
        update: impl FnOnce(u32) -> u32,
    ) -> Result<u32, BusError> {
        let word = self.atomic_word(address)?;
        let previous = u32::from_le_bytes(*word);
        *word = update(previous).to_le_bytes();
        Ok(previous)
    }

    fn atomic_word(&mut self, address: u32) -> Result<&mut [u8; 4], BusError> {
        if address & 3 != 0 {
            return Err(BusError::Misaligned(address, 4));
        }
        Ok(self.memory(address, 4)?.try_into().unwrap())
    }

    pub fn read<const N: usize>(&mut self, address: u32) -> Result<u32, BusError> {
        if N == 4
            && address & 3 == 0
            && let Some(uart) = self.uart.as_mut()
            && uart.contains(address)
        {
            return Ok(uart.read(address));
        }
        if N == 4
            && let Some(clint) = self.clint.as_ref()
            && address & 3 == 0
            && clint.contains(address)
        {
            return Ok(clint.read(address));
        }
        if N == 4 && address & 3 == 0 {
            if let Some((base, gpio)) = &self.gpio
                && address.wrapping_sub(*base) < 0xc
            {
                return Ok(gpio.read(address - *base));
            }
            if let Some((base, timer)) = &self.timer
                && address.wrapping_sub(*base) < 0x5c
            {
                return Ok(timer.read(address - *base));
            }
        }
        if let Some(uart) = self.ns16550.as_mut()
            && let Some(register) = uart.register(address, N)
        {
            return Ok(uart.read(register));
        }
        if N == 4
            && address & 3 == 0
            && let Some(plic) = self.plic.as_mut()
            && plic.contains(address)
        {
            return Ok(plic.read(address));
        }
        if let Some(uart) = self.litex_uart.as_mut()
            && let Some(offset) = uart.offset(address, N)
            && (N == 1 || address & 3 == 0)
        {
            return Ok(uart.read(offset));
        }
        if N == 4
            && let Some(timer) = self.systick.as_mut()
            && let Some(value) = timer.read(address)
        {
            return Ok(value);
        }
        if address as usize & (N - 1) == 0
            && let Some(value) = self.nvic.as_ref().and_then(|nvic| nvic.read(address, N))
        {
            return Ok(value);
        }
        if address as usize & (N - 1) == 0
            && let Some(uart) = self
                .stm32_uarts
                .iter_mut()
                .find(|device| device.contains(address))
        {
            return Ok(uart.read(address, N));
        }
        if N == 4
            && address & 3 == 0
            && let Some(exti) = self.exti.as_ref()
            && exti.contains(address)
        {
            return Ok(exti.read(address));
        }
        if N == 4
            && address & 3 == 0
            && let Some(rcc) = self.stm32_rcc.as_ref()
            && rcc.contains(address)
        {
            return Ok(rcc.read(address));
        }
        if N == 4
            && address & 3 == 0
            && let Some(gpio) = self
                .stm32_gpios
                .iter_mut()
                .find(|gpio| gpio.contains(address))
        {
            return Ok(gpio.read(address));
        }
        match self.read_ram::<N>(address) {
            Err(BusError::Unmapped(..)) if self.stm32f0_tagged(address, N) => Ok(0),
            result => result,
        }
    }

    pub fn read_ram<const N: usize>(&self, address: u32) -> Result<u32, BusError> {
        if address as usize & (N - 1) != 0 {
            return Err(BusError::Misaligned(address, N));
        }
        for region in &self.regions {
            if let Some(offset) = region.offset(address, N) {
                let mut bytes = [0; 4];
                bytes[..N].copy_from_slice(&region.bytes[offset..offset + N]);
                return Ok(u32::from_le_bytes(bytes));
            }
        }
        Err(BusError::Unmapped(address, N))
    }

    pub fn write(
        &mut self,
        address: u32,
        width: usize,
        value: u32,
        uart: &mut impl Write,
    ) -> Result<(), BusError> {
        if self.uart_tx == Some(address) && width == 1 {
            return uart.write_all(&[value as u8]).map_err(BusError::Output);
        }
        if address as usize & (width - 1) != 0 {
            return Err(BusError::Misaligned(address, width));
        }
        if width == 4
            && let Some(device) = self.uart.as_mut()
            && device.contains(address)
        {
            return device.write(address, value, uart).map_err(BusError::Output);
        }
        if width == 4
            && let Some(clint) = self.clint.as_mut()
            && clint.contains(address)
        {
            clint.write(address, value);
            return Ok(());
        }
        if width == 4 {
            if let Some((base, gpio)) = &mut self.gpio
                && address.wrapping_sub(*base) < 0xc
            {
                gpio.write(address - *base, value);
                return Ok(());
            }
            if let Some((base, timer)) = &mut self.timer
                && address.wrapping_sub(*base) < 0x5c
            {
                timer.write(address - *base, value);
                return Ok(());
            }
        }
        if let Some(device) = self.ns16550.as_mut()
            && let Some(register) = device.register(address, width)
        {
            return device
                .write(register, value, uart)
                .map_err(BusError::Output);
        }
        if width == 4
            && let Some(plic) = self.plic.as_mut()
            && plic.contains(address)
        {
            plic.write(address, value);
            return Ok(());
        }
        if let Some(device) = self.litex_uart.as_mut()
            && let Some(offset) = device.offset(address, width)
        {
            return device.write(offset, value, uart).map_err(BusError::Output);
        }
        if width == 4
            && let Some(timer) = self.systick.as_mut()
            && timer.write(address, value)
        {
            return Ok(());
        }
        if address == VTOR && width == 4 {
            let base = value & !0x7f;
            let mapped = self.read_ram::<4>(base).is_ok();
            if let Some(nvic) = self.nvic.as_mut() {
                if mapped {
                    nvic.set_vector_base(base);
                }
                return Ok(());
            }
        }
        if self
            .nvic
            .as_mut()
            .is_some_and(|nvic| nvic.write(address, width, value))
        {
            return Ok(());
        }
        if let Some(device) = self
            .stm32_uarts
            .iter_mut()
            .find(|device| device.contains(address))
        {
            return device
                .write(address, width, value, uart)
                .map_err(BusError::Output);
        }
        if width == 4
            && let Some(exti) = self.exti.as_mut()
            && exti.contains(address)
        {
            exti.write(address, value);
            return Ok(());
        }
        if width == 4
            && let Some(rcc) = self.stm32_rcc.as_mut()
            && rcc.contains(address)
        {
            rcc.write(address, value);
            return Ok(());
        }
        if width == 4
            && let Some(gpio) = self
                .stm32_gpios
                .iter_mut()
                .find(|gpio| gpio.contains(address))
        {
            gpio.write(address, value);
            return Ok(());
        }
        match self.memory(address, width) {
            Ok(bytes) => bytes.copy_from_slice(&value.to_le_bytes()[..width]),
            Err(error @ BusError::Unmapped(..)) => {
                if !self.stm32f0_tagged(address, width) {
                    return Err(error);
                }
            }
            Err(error) => return Err(error),
        }
        Ok(())
    }
}
