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
    stm32_adc::Stm32F0Adc,
    stm32_can::Stm32Can,
    stm32_crc::Stm32F0Crc,
    stm32_dma::Stm32Dma,
    stm32_exti::Exti,
    stm32_flash::Stm32F0Flash,
    stm32_gpio::Stm32Gpio,
    stm32_gpio_board::{GpioButton, GpioLed},
    stm32_i2c::Stm32I2c,
    stm32_rcc::Stm32F0Rcc,
    stm32_rtc::Stm32Rtc,
    stm32_spi::Stm32F0Spi,
    stm32_timer::Stm32Timer,
    stm32_uart::{Stm32Irq, Stm32Usart},
    systick::SysTick,
    uart::{LiteXUart, MuraxUart, UartHistory, UartWaitMode},
};

// Cortex-M peripheral time advances by 10 ns per execution step.
pub(crate) const CORTEX_M_STEP_HZ: u64 = 100_000_000;

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
    #[cfg(test)]
    pub faulting_accesses: Option<std::ops::RangeInclusive<u32>>,
    #[cfg(test)]
    pub faulting_stack_writes: Option<std::ops::RangeInclusive<u32>>,
    regions: Vec<Region>,
    reservations: [Option<u32>; crate::machine::MAX_HARTS],
    uart_tx: Option<u32>,
    pub uart: Option<MuraxUart>,
    ns16550: Option<Ns16550>,
    litex_uart: Option<LiteXUart>,
    stm32_uarts: Vec<Stm32Usart>,
    stm32_uart_history: Vec<Option<UartHistory>>,
    exti: Option<Exti>,
    stm32_gpios: Vec<Stm32Gpio>,
    gpio_exti_levels: u32,
    gpio_leds: Vec<(u32, GpioLed)>,
    gpio_buttons: Vec<GpioButton>,
    gpio_timer_inputs: Vec<(u32, u8, u32, u8)>,
    led_events: bool,
    stm32_timers: Vec<Stm32Timer>,
    stm32_spis: Vec<Stm32F0Spi>,
    stm32_i2cs: Vec<Stm32I2c>,
    stm32_crc: Option<Stm32F0Crc>,
    stm32_cans: Vec<Stm32Can>,
    stm32_adc: Option<Stm32F0Adc>,
    stm32_adc_name: Option<String>,
    stm32_dma: Option<Stm32Dma>,
    stm32f0_tags: Vec<std::ops::RangeInclusive<u32>>,
    stm32_rcc: Option<Stm32F0Rcc>,
    stm32_flash: Option<Stm32F0Flash>,
    stm32_rtc: Option<Stm32Rtc>,
    plic: Option<Plic>,
    pub systick: Option<SysTick>,
    pub nvic: Option<Nvic>,
    nvic_name: Option<String>,
    external_nvic_levels: u32,
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
            || self.stm32_cans.iter().any(|can| {
                let other = u64::from(can.base);
                start < other + u64::from(can.size) && other < end
            })
            || self.stm32_i2cs.iter().any(|i2c| {
                let other = u64::from(i2c.base);
                start < other + u64::from(Stm32I2c::SIZE) && other < end
            })
            || self.stm32_spis.iter().any(|spi| {
                let other = u64::from(spi.base);
                start < other + u64::from(Stm32F0Spi::SIZE) && other < end
            })
            || self.stm32_timers.iter().any(|timer| {
                let other = u64::from(timer.base);
                start < other + u64::from(Stm32Timer::SIZE) && other < end
            })
            || self.stm32_gpios.iter().any(|gpio| {
                let other = u64::from(gpio.base);
                start < other + u64::from(gpio.size) && other < end
            })
            || self.stm32_dma.as_ref().is_some_and(|dma| {
                let other = u64::from(dma.base);
                start < other + u64::from(Stm32Dma::SIZE) && other < end
            })
            || self.stm32_adc.as_ref().is_some_and(|adc| {
                let other = u64::from(adc.base);
                start < other + u64::from(Stm32F0Adc::SIZE) && other < end
            })
            || self.stm32_crc.as_ref().is_some_and(|crc| {
                let other = u64::from(crc.base);
                start < other + u64::from(Stm32F0Crc::SIZE) && other < end
            })
            || self.stm32_rtc.as_ref().is_some_and(|rtc| {
                let other = u64::from(rtc.base);
                start < other + u64::from(Stm32Rtc::SIZE) && other < end
            })
            || self.stm32_flash.as_ref().is_some_and(|flash| {
                let other = u64::from(flash.base);
                let options = u64::from(Stm32F0Flash::OPTION_BASE);
                (start < other + u64::from(Stm32F0Flash::SIZE) && other < end)
                    || (start < options + u64::from(Stm32F0Flash::OPTION_SIZE) && options < end)
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

    pub fn set_nvic_name(&mut self, name: String) {
        self.nvic_name = Some(name);
    }

    pub fn drive_nvic_line(&mut self, target: &str, line: u8, high: bool) -> Result<(), String> {
        self.validate_nvic_name(target)?;
        let mask = 1u32 << line;
        if high {
            self.external_nvic_levels |= mask;
            // Latch a pulse even when both commands occur before the next CPU step.
            self.nvic.as_mut().unwrap().set_line(line, true, 0);
        } else {
            self.external_nvic_levels &= !mask;
        }
        Ok(())
    }

    fn validate_nvic_name(&self, target: &str) -> Result<(), String> {
        let name = target.strip_prefix("sysbus.").unwrap_or(target);
        if self.nvic.is_none() || self.nvic_name.as_deref() != Some(name) {
            return Err(format!("unknown Cortex-M NVIC {target:?}"));
        }
        Ok(())
    }

    pub fn pend_nvic_exception(&mut self, target: &str, exception: u32) -> Result<(), String> {
        use crate::systick::{ICSR, NMI, PENDSV, SYSTICK};

        self.validate_nvic_name(target)?;
        let pending = match u8::try_from(exception) {
            Ok(irq @ 16..=47) => {
                self.nvic.as_mut().unwrap().set_line(irq - 16, true, 0);
                return Ok(());
            }
            Ok(crate::nvic::HARDFAULT) => {
                self.nvic.as_mut().unwrap().set_hardfault_pending(true);
                return Ok(());
            }
            Ok(NMI) => 1 << 31,
            Ok(PENDSV) => 1 << 28,
            Ok(SYSTICK) => 1 << 26,
            _ => return Err(format!("unsupported Cortex-M exception {exception}")),
        };
        self.systick
            .as_mut()
            .ok_or("system exception pending bits require SCB ICSR")?
            .write(ICSR, pending);
        Ok(())
    }

    pub fn set_nvic_clocked(&mut self, target: &str, clocked: bool) -> Result<(), String> {
        self.validate_nvic_name(target)?;
        self.systick
            .as_mut()
            .ok_or("NVIC Clocked requires a SysTick counter")?
            .set_clocked(clocked);
        Ok(())
    }

    pub fn tick_stm32_uarts(&mut self) {
        for uart in &mut self.stm32_uarts {
            uart.tick();
        }
    }

    pub fn advance_stm32_dma(&mut self, output: &mut impl Write) -> Result<(), BusError> {
        let Some(dma) = self
            .stm32_dma
            .as_ref()
            .filter(|dma| dma.has_enabled_channels())
        else {
            return Ok(());
        };
        let mut requests = self.stm32_adc.as_ref().map_or(0, Stm32F0Adc::pending_dma);
        for uart in &self.stm32_uarts {
            requests |= uart.pending_dma();
        }
        for spi in &self.stm32_spis {
            requests |= spi.pending_dma();
        }
        for i2c in &self.stm32_i2cs {
            requests |= i2c.pending_dma();
        }
        let Some(transfer) = dma.next_transfer(requests) else {
            return Ok(());
        };
        let value = match transfer.read_width {
            1 => self.read::<1>(transfer.from),
            2 => self.read::<2>(transfer.from),
            4 => self.read::<4>(transfer.from),
            _ => Err(BusError::Unmapped(transfer.from, transfer.read_width)),
        };
        let result = value.and_then(|value| match transfer.write_width {
            width @ (1 | 2 | 4) => self.write(transfer.to, width, value, output),
            width => Err(BusError::Unmapped(transfer.to, width)),
        });
        self.stm32_dma
            .as_mut()
            .unwrap()
            .complete(transfer, result.is_ok());
        match result {
            Err(error @ BusError::Output(_)) => Err(error),
            _ => Ok(()),
        }
    }

    pub fn reset_cortex_m_if_requested(&mut self) -> bool {
        let system = self.nvic.as_mut().is_some_and(Nvic::take_reset_request);
        let reload = self
            .stm32_flash
            .as_mut()
            .is_some_and(Stm32F0Flash::take_reload_request);
        if !system && !reload {
            return false;
        }
        self.reset_cortex_m_devices(reload);
        true
    }

    pub fn reset_cortex_m_devices(&mut self, reload: bool) {
        if let Some(nvic) = self.nvic.as_mut() {
            nvic.reset();
        }
        if let Some(systick) = self.systick.as_mut() {
            systick.reset();
        }
        if let Some(uart) = self.litex_uart.as_mut() {
            *uart = LiteXUart::new(uart.base);
        }
        if let Some(flash) = self.stm32_flash.as_mut() {
            flash.reset(reload);
        }
        if let Some(rcc) = self.stm32_rcc.as_mut() {
            rcc.reset(reload);
        }
        if let Some(adc) = self.stm32_adc.as_mut() {
            adc.reset();
        }
        if let Some(dma) = self.stm32_dma.as_mut() {
            dma.reset();
        }
        if let Some(crc) = self.stm32_crc.as_mut() {
            crc.reset();
        }
        for uart in &mut self.stm32_uarts {
            uart.reset();
        }
        for port in &mut self.stm32_gpios {
            port.reset();
        }
        for timer in &mut self.stm32_timers {
            timer.reset();
        }
        for spi in &mut self.stm32_spis {
            spi.reset();
        }
        for i2c in &mut self.stm32_i2cs {
            i2c.reset();
        }
        for can in &mut self.stm32_cans {
            can.reset();
        }
        // SRAM, flash contents, external devices and the RTC backup domain stay powered.
        self.refresh_gpio_wiring();
        if let Some(exti) = self.exti.as_mut() {
            exti.reset(self.gpio_exti_levels);
        }
    }

    pub fn sample_nvic(&mut self, active: u64) {
        let (timer_irqs, timer_dma) = self.tick_stm32_timers();
        if let Some(nvic) = self.nvic.as_mut() {
            nvic.set_uart_level(
                self.litex_uart
                    .as_ref()
                    .is_some_and(LiteXUart::interrupt_pending),
                active,
            );
            let mut exti_inputs = self.gpio_exti_levels;
            for uart in &self.stm32_uarts {
                if uart.interrupt_pending() {
                    match uart.irq {
                        Some(Stm32Irq::Nvic(line)) => nvic.set_line(line, true, active),
                        Some(Stm32Irq::Exti(line)) => exti_inputs |= 1 << line,
                        None => {}
                    }
                }
            }
            if let Some(rtc) = self.stm32_rtc.as_mut() {
                let mut signals = rtc.tick();
                while signals != 0 {
                    let signal = signals.trailing_zeros() as usize;
                    signals &= signals - 1;
                    match rtc.routes[signal] {
                        Some(Stm32Irq::Nvic(line)) => nvic.set_line(line, true, active),
                        Some(Stm32Irq::Exti(line)) => exti_inputs |= 1 << line,
                        None => {}
                    }
                }
            }
            let mut irqs = self
                .exti
                .as_mut()
                .map_or(0, |exti| exti.sample(exti_inputs));
            irqs |= timer_irqs | self.external_nvic_levels;
            if self
                .stm32_flash
                .as_ref()
                .is_some_and(Stm32F0Flash::interrupt_pending)
            {
                irqs |= 1 << Stm32F0Flash::IRQ;
            }
            if let Some(adc) = self.stm32_adc.as_mut()
                && adc.tick()
                && let Some(line) = adc.irq
            {
                irqs |= 1 << line;
            }
            for can in &self.stm32_cans {
                irqs |= can.interrupts();
            }
            for i2c in &self.stm32_i2cs {
                if i2c.interrupt_pending()
                    && let Some(line) = i2c.irq
                {
                    irqs |= 1 << line;
                }
            }
            for spi in &self.stm32_spis {
                if spi.interrupt_pending()
                    && let Some(line) = spi.irq
                {
                    irqs |= 1 << line;
                }
            }
            if let Some(dma) = self.stm32_dma.as_mut() {
                dma.queue_requests(timer_dma);
                irqs |= dma.interrupts();
            }
            while irqs != 0 {
                nvic.set_line(irqs.trailing_zeros() as u8, true, active);
                irqs &= irqs - 1;
            }
        }
    }

    fn tick_stm32_timers(&mut self) -> (u32, u8) {
        let mut irqs = 0;
        let mut dma_requests = 0;
        let mut gpio_changed = false;
        for timer in &mut self.stm32_timers {
            irqs |= timer.tick();
            let changed = timer.take_output_changes();
            if changed != 0 {
                for (signal, pin) in &timer.gpio_outputs {
                    if changed & (1 << signal) != 0
                        && let Some(port) = self
                            .stm32_gpios
                            .iter_mut()
                            .find(|port| port.base == pin.port)
                    {
                        port.set_alternate_output(pin.pin, pin.function, timer.pin_output(*signal));
                        gpio_changed = true;
                    }
                }
            }
        }
        if gpio_changed {
            self.refresh_gpio_wiring();
        }
        for timer in &mut self.stm32_timers {
            dma_requests |= timer.take_dma_requests();
        }
        (irqs, dma_requests)
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

    pub fn map_stm32_can(&mut self, can: Stm32Can) -> Result<(), String> {
        let start = u64::from(can.base);
        let end = start + u64::from(can.size);
        if can.base & 3 != 0
            || can.size < 0x320
            || can.size & 3 != 0
            || end > 1 << 32
            || self.overlaps(start, end)
        {
            return Err(format!(
                "invalid or overlapping STM32 CAN range at {:#010x}",
                can.base
            ));
        }
        self.stm32_cans.push(can);
        Ok(())
    }

    pub fn map_stm32_i2c(&mut self, i2c: Stm32I2c) -> Result<(), String> {
        let start = u64::from(i2c.base);
        let end = start + u64::from(Stm32I2c::SIZE);
        if i2c.base & 3 != 0 || end > 1 << 32 || self.overlaps(start, end) {
            return Err(format!(
                "invalid or overlapping STM32 I²C range at {:#010x}",
                i2c.base
            ));
        }
        self.stm32_i2cs.push(i2c);
        Ok(())
    }

    pub fn map_stm32f0_spi(&mut self, spi: Stm32F0Spi) -> Result<(), String> {
        let start = u64::from(spi.base);
        let end = start + u64::from(Stm32F0Spi::SIZE);
        if spi.base & 3 != 0 || end > 1 << 32 || self.overlaps(start, end) {
            return Err(format!(
                "invalid or overlapping STM32F0 SPI range at {:#010x}",
                spi.base
            ));
        }
        self.stm32_spis.push(spi);
        Ok(())
    }

    pub fn map_stm32_timer(&mut self, timer: Stm32Timer) -> Result<(), String> {
        let start = u64::from(timer.base);
        let end = start + u64::from(Stm32Timer::SIZE);
        if timer.base & 3 != 0 || end > 1 << 32 || self.overlaps(start, end) {
            return Err(format!(
                "invalid or overlapping STM32 timer range at {:#010x}",
                timer.base
            ));
        }
        self.stm32_timers.push(timer);
        Ok(())
    }

    pub fn route_gpio_timer_input(&mut self, port: u32, pin: u8, timer: u32, channel: u8) {
        self.gpio_timer_inputs.push((port, pin, timer, channel));
    }

    pub fn add_gpio_led(&mut self, base: u32, led: GpioLed) {
        self.gpio_leds.push((base, led));
        self.refresh_gpio_wiring();
    }

    pub fn gpio_led_index(&self, target: &str) -> Result<usize, String> {
        let path = target.strip_prefix("sysbus.").unwrap_or(target);
        self.gpio_leds
            .iter()
            .position(|(_, led)| match path.split_once('.') {
                Some((parent, name)) => led.parent.as_deref() == Some(parent) && led.name == name,
                None => led.name == path,
            })
            .ok_or_else(|| format!("unknown STM32 GPIO LED {target:?}"))
    }

    pub fn gpio_led(&self, index: usize) -> &GpioLed {
        &self.gpio_leds[index].1
    }

    pub fn add_gpio_button(&mut self, button: GpioButton) {
        self.gpio_buttons.push(button);
        self.set_button(self.gpio_buttons.len() - 1, false);
    }

    pub fn report_led_events(&mut self, enabled: bool) {
        self.led_events = enabled;
    }

    pub fn board_button(&self, name: &str) -> Option<usize> {
        self.gpio_buttons
            .iter()
            .position(|button| button.name == name)
    }

    pub fn set_named_button(&mut self, target: &str, pressed: bool) -> Result<(), String> {
        let target = target.strip_prefix("sysbus.").unwrap_or(target);
        let index = match target.split_once('.') {
            Some((parent, name)) => self
                .gpio_buttons
                .iter()
                .position(|button| button.parent.as_deref() == Some(parent) && button.name == name),
            None => self.board_button(target),
        }
        .ok_or_else(|| format!("unknown STM32 board button {target:?}"))?;
        self.set_button(index, pressed);
        Ok(())
    }

    pub fn set_button(&mut self, index: usize, pressed: bool) {
        let button = &self.gpio_buttons[index];
        if let Some(port) = self
            .stm32_gpios
            .iter_mut()
            .find(|port| port.base == button.port)
        {
            port.set_external(button.pin, pressed ^ button.invert);
            self.refresh_gpio_wiring();
        }
    }

    fn refresh_gpio_wiring(&mut self) {
        let previous = self.gpio_exti_levels;
        self.gpio_exti_levels = self
            .stm32_gpios
            .iter()
            .fold(0, |levels, port| levels | port.exti_levels());
        if let Some(exti) = &mut self.exti {
            exti.update_gpio(previous, self.gpio_exti_levels);
        }
        if !self.gpio_timer_inputs.is_empty() {
            for timer in &mut self.stm32_timers {
                timer.capture_next = 0;
            }
            for &(port, pin, timer, channel) in &self.gpio_timer_inputs {
                if self
                    .stm32_gpios
                    .iter()
                    .any(|gpio| gpio.base == port && gpio.pins() & (1 << pin) != 0)
                    && let Some(timer) = self
                        .stm32_timers
                        .iter_mut()
                        .find(|device| device.base == timer)
                {
                    timer.capture_next |= 1 << channel;
                }
            }
            for timer in &mut self.stm32_timers {
                timer.apply_capture_inputs();
            }
        }
        for (base, led) in &mut self.gpio_leds {
            if let Some(port) = self.stm32_gpios.iter().find(|port| port.base == *base) {
                led.sample(port.pins(), self.led_events);
            }
        }
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

    pub fn map_stm32_dma(&mut self, dma: Stm32Dma) -> Result<(), String> {
        let start = u64::from(dma.base);
        let end = start + u64::from(Stm32Dma::SIZE);
        if self.stm32_dma.is_some()
            || dma.base & 3 != 0
            || end > 1 << 32
            || self.overlaps(start, end)
        {
            return Err(format!(
                "invalid or overlapping STM32 DMA range at {:#010x}",
                dma.base
            ));
        }
        self.stm32_dma = Some(dma);
        Ok(())
    }

    pub fn map_stm32f0_adc(&mut self, name: String, adc: Stm32F0Adc) -> Result<(), String> {
        let start = u64::from(adc.base);
        let end = start + u64::from(Stm32F0Adc::SIZE);
        if self.stm32_adc.is_some()
            || adc.base & 3 != 0
            || end > 1 << 32
            || self.overlaps(start, end)
        {
            return Err(format!(
                "invalid or overlapping STM32F0 ADC range at {:#010x}",
                adc.base
            ));
        }
        self.stm32_adc = Some(adc);
        self.stm32_adc_name = Some(name);
        Ok(())
    }

    pub fn set_named_adc_input(
        &mut self,
        name: &str,
        channel: usize,
        volts: f64,
    ) -> Result<(), String> {
        if self.stm32_adc_name.as_deref() != Some(name) {
            return Err(format!("unknown STM32 ADC {name:?}"));
        }
        self.set_stm32f0_adc_input(channel, volts)
    }

    pub fn validate_stm32f0_adc_input(&self, channel: usize, volts: f64) -> Result<(), String> {
        self.stm32_adc
            .as_ref()
            .ok_or("ADC input requires an STM32F0 ADC")?
            .validate_input(channel, volts)
    }

    pub fn set_stm32f0_adc_input(&mut self, channel: usize, volts: f64) -> Result<(), String> {
        self.stm32_adc
            .as_mut()
            .ok_or("ADC input requires an STM32F0 ADC")?
            .set_input(channel, volts)
    }

    pub fn map_stm32f0_crc(&mut self, base: u32, configurable_poly: bool) -> Result<(), String> {
        let start = u64::from(base);
        let end = start + u64::from(Stm32F0Crc::SIZE);
        if self.stm32_crc.is_some() || base & 3 != 0 || end > 1 << 32 || self.overlaps(start, end) {
            return Err(format!(
                "invalid or overlapping STM32F0 CRC range at {base:#010x}"
            ));
        }
        self.stm32_crc = Some(Stm32F0Crc::new(base, configurable_poly));
        Ok(())
    }

    pub fn map_stm32_rtc(&mut self, rtc: Stm32Rtc) -> Result<(), String> {
        let start = u64::from(rtc.base);
        let end = start + u64::from(Stm32Rtc::SIZE);
        if self.stm32_rtc.is_some()
            || rtc.base & 3 != 0
            || end > 1 << 32
            || self.overlaps(start, end)
        {
            return Err(format!(
                "invalid or overlapping STM32 RTC range at {:#010x}",
                rtc.base
            ));
        }
        self.stm32_rtc = Some(rtc);
        Ok(())
    }

    pub fn map_stm32f072_flash(&mut self) -> Result<(), String> {
        const BANK: std::ops::Range<u32> = 0x0800_0000..0x0802_0000;
        if !self
            .stm32f0_tags
            .contains(&(Stm32F0Flash::BASE..=0x4002_23ff))
        {
            return Ok(());
        }
        let Some(index) = self.regions.iter().position(|region| {
            region.base == BANK.start && region.bytes.len() == (BANK.end - BANK.start) as usize
        }) else {
            return Ok(());
        };
        let start = u64::from(Stm32F0Flash::BASE);
        let options = u64::from(Stm32F0Flash::OPTION_BASE);
        if self.overlaps(start, start + u64::from(Stm32F0Flash::SIZE))
            || self.overlaps(options, options + u64::from(Stm32F0Flash::OPTION_SIZE))
        {
            return Err("STM32F072 flash registers or option bytes overlap another mapping".into());
        }
        self.regions[index].bytes.fill(0xff);
        self.stm32_flash = Some(Stm32F0Flash::new(BANK));
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

    pub fn map_stm32_usart(&mut self, uart: Stm32Usart) -> Result<(), String> {
        let base = uart.base;
        let start = u64::from(base);
        let end = start + u64::from(Stm32Usart::SIZE);
        if base & 3 != 0 || end > 1 << 32 || self.overlaps(start, end) {
            return Err(format!(
                "invalid or overlapping STM32 USART range at {base:#010x}"
            ));
        }
        self.stm32_uarts.push(uart);
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

    fn stm32_usart_index(&self, target: &str) -> Result<usize, String> {
        let name = target.strip_prefix("sysbus.").unwrap_or(target);
        self.stm32_uarts
            .iter()
            .position(|uart| uart.name == name)
            .ok_or_else(|| format!("unknown STM32 USART {target:?}"))
    }

    pub fn write_stm32_uart_line(
        &mut self,
        target: &str,
        text: &str,
        ending: &[u8],
    ) -> Result<(), String> {
        let index = self.stm32_usart_index(target)?;
        // Renode writes the low byte of each UTF-16 code unit before the ending.
        self.stm32_uarts[index].queue_input(
            text.encode_utf16()
                .map(|unit| unit as u8)
                .chain(ending.iter().copied()),
        );
        Ok(())
    }

    pub fn record_stm32_uart(&mut self, target: &str) {
        if let Ok(index) = self.stm32_usart_index(target) {
            self.stm32_uart_history
                .resize_with(self.stm32_uarts.len(), || None);
            self.stm32_uart_history[index].get_or_insert_with(UartHistory::default);
        }
    }

    pub fn watch_stm32_uart(
        &mut self,
        target: &str,
        pattern: &str,
        end: UartWaitMode,
    ) -> Result<usize, String> {
        let index = self.stm32_usart_index(target)?;
        self.stm32_uart_history[index]
            .as_mut()
            .expect("a script wait enables history before execution")
            .begin_wait(pattern, end);
        Ok(index)
    }

    pub fn stm32_uart_matched(&self, index: usize) -> bool {
        self.stm32_uart_history[index]
            .as_ref()
            .is_some_and(UartHistory::matched)
    }

    pub fn finish_stm32_uart_watch(&mut self, index: usize) {
        if let Some(history) = &mut self.stm32_uart_history[index] {
            history.finish_wait();
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
            && let Some(rtc) = self.stm32_rtc.as_mut()
            && rtc.contains(address)
        {
            return Ok(rtc.read(address));
        }
        if address as usize & (N - 1) == 0
            && let Some(flash) = self.stm32_flash.as_ref()
            && flash.contains_options(address, N)
        {
            return Ok(flash.read_options(address, N));
        }
        if N == 4
            && address & 3 == 0
            && let Some(flash) = self.stm32_flash.as_ref()
            && flash.contains(address)
        {
            return Ok(flash.read(address));
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
            && let Some(can) = self.stm32_cans.iter().find(|can| can.contains(address))
        {
            return Ok(can.read(address));
        }
        if address & 3 == 0
            && let Some(i2c) = self.stm32_i2cs.iter_mut().find(|i2c| i2c.contains(address))
        {
            let mask = u32::MAX >> (32 - N * 8);
            return Ok(i2c.read(address) & mask);
        }
        if address as usize & (N - 1) == 0
            && let Some(spi) = self.stm32_spis.iter_mut().find(|spi| spi.contains(address))
        {
            let mask = u32::MAX >> (32 - N * 8);
            return Ok(spi.read(address, N) & mask);
        }
        if matches!(N, 2 | 4)
            && address as usize & (N - 1) == 0
            && let Some(timer) = self
                .stm32_timers
                .iter()
                .find(|timer| timer.contains(address))
        {
            let value = timer.read(address & !3);
            return Ok(if N == 2 {
                (value >> ((address & 2) * 8)) & 0xffff
            } else {
                value
            });
        }
        if matches!(N, 2 | 4)
            && address as usize & (N - 1) == 0
            && let Some(gpio) = self
                .stm32_gpios
                .iter_mut()
                .find(|gpio| gpio.contains(address))
        {
            return Ok(if N == 2 {
                gpio.read_halfword(address)
            } else {
                gpio.read(address)
            });
        }
        if N == 4
            && address & 3 == 0
            && let Some(dma) = &self.stm32_dma
            && dma.contains(address)
        {
            return Ok(dma.read(address));
        }
        if matches!(N, 2 | 4)
            && address & 3 == 0
            && let Some(adc) = self.stm32_adc.as_mut()
            && adc.contains(address)
        {
            return Ok(adc.read(address));
        }
        if address as usize & (N - 1) == 0
            && let Some(crc) = self.stm32_crc.as_mut()
            && crc.contains(address)
        {
            return Ok(crc.read(address, N));
        }
        match self.read_ram::<N>(address) {
            Err(BusError::Unmapped(..)) if self.stm32f0_tagged(address, N) => Ok(0),
            result => result,
        }
    }

    pub fn read_ram<const N: usize>(&self, address: u32) -> Result<u32, BusError> {
        #[cfg(test)]
        if self
            .faulting_accesses
            .as_ref()
            .is_some_and(|range| range.contains(&address))
        {
            return Err(BusError::Unmapped(address, N));
        }
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
        if let Some((index, device)) = self
            .stm32_uarts
            .iter_mut()
            .enumerate()
            .find(|(_, device)| device.contains(address))
        {
            return device
                .write(address, width, value, |byte| {
                    uart.write_all(&[byte])?;
                    if let Some(Some(history)) = self.stm32_uart_history.get_mut(index) {
                        history.receive(byte);
                    }
                    Ok(())
                })
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
            && let Some(rtc) = self.stm32_rtc.as_mut()
            && rtc.contains(address)
        {
            rtc.write(address, value);
            return Ok(());
        }
        if let Some(flash) = self.stm32_flash.as_mut()
            && flash.contains_options(address, width)
        {
            flash.program_option(address, width, value);
            return Ok(());
        }
        if width == 4
            && let Some(flash) = self.stm32_flash.as_mut()
            && flash.contains(address)
        {
            if let Some(range) = flash.write(address, value) {
                let result = self.memory(range.start, (range.end - range.start) as usize);
                let success = result.map(|bytes| bytes.fill(0xff)).is_ok();
                self.stm32_flash.as_mut().unwrap().finish(success);
            }
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
            && let Some(can) = self.stm32_cans.iter_mut().find(|can| can.contains(address))
        {
            can.write(address, value);
            return Ok(());
        }
        if address & 3 == 0
            && let Some(i2c) = self.stm32_i2cs.iter_mut().find(|i2c| i2c.contains(address))
        {
            let mask = u32::MAX >> (32 - width * 8);
            i2c.write(address, value & mask);
            return Ok(());
        }
        if let Some(spi) = self.stm32_spis.iter_mut().find(|spi| spi.contains(address)) {
            let mask = u32::MAX >> (32 - width * 8);
            spi.write(address, width, value & mask, self.led_events);
            return Ok(());
        }
        if matches!(width, 2 | 4)
            && let Some(timer) = self
                .stm32_timers
                .iter_mut()
                .find(|timer| timer.contains(address))
        {
            if width == 2 {
                timer.write_halfword(address, value);
            } else {
                timer.write(address, value);
            }
            return Ok(());
        }
        if matches!(width, 2 | 4)
            && let Some(gpio) = self
                .stm32_gpios
                .iter_mut()
                .find(|gpio| gpio.contains(address))
        {
            if width == 2 {
                gpio.write_halfword(address, value);
            } else {
                gpio.write(address, value);
            }
            self.refresh_gpio_wiring();
            return Ok(());
        }
        if width == 4
            && let Some(dma) = self.stm32_dma.as_mut()
            && dma.contains(address)
        {
            dma.write(address, value);
            return Ok(());
        }
        if width == 4
            && let Some(adc) = self.stm32_adc.as_mut()
            && adc.contains(address)
        {
            adc.write(address, value);
            return Ok(());
        }
        if let Some(crc) = self.stm32_crc.as_mut()
            && crc.contains(address)
        {
            crc.write(address, width, value);
            return Ok(());
        }
        if self
            .stm32_flash
            .as_ref()
            .is_some_and(|flash| flash.contains_bank(address))
        {
            let old = self.read_ram::<2>(address & !1)? as u16;
            let flash = self.stm32_flash.as_mut().unwrap();
            if let Some(value) = flash.program(address, width, old, value as u16) {
                self.memory(address, 2)?
                    .copy_from_slice(&value.to_le_bytes());
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_a_line_to_the_named_stm32_usart() {
        let mut bus = Bus::default();
        let usart1 = 0x4001_3800;
        let usart2 = 0x4000_4400;
        bus.map_stm32_usart(Stm32Usart::new("usart1".into(), usart1, None, false))
            .unwrap();
        bus.map_stm32_usart(Stm32Usart::new("usart2".into(), usart2, None, false))
            .unwrap();
        for base in [usart1, usart2] {
            bus.write(base, 4, 0b101, &mut std::io::sink()).unwrap();
        }
        bus.write_stm32_uart_line("sysbus.usart2", "A", b"\r")
            .unwrap();
        assert_eq!(bus.read::<4>(usart1 + 0x1c).unwrap() & (1 << 5), 0);
        assert_eq!(bus.read::<4>(usart2 + 0x24).unwrap(), u32::from(b'A'));
        assert_eq!(bus.read::<4>(usart2 + 0x24).unwrap(), u32::from(b'\r'));
        assert!(
            bus.write_stm32_uart_line("sysbus.usart3", "A", b"\r")
                .is_err()
        );
    }

    #[test]
    fn uart_watch_ignores_other_ports_without_filtering_stdout() {
        let mut bus = Bus::default();
        let usart1 = 0x4001_3800;
        let usart2 = 0x4000_4400;
        let mut output = Vec::new();
        for (name, base) in [("usart1", usart1), ("usart2", usart2)] {
            bus.map_stm32_usart(Stm32Usart::new(name.into(), base, None, false))
                .unwrap();
            bus.write(base, 4, 0b1001, &mut output).unwrap();
        }
        assert!(
            bus.watch_stm32_uart("sysbus.usart3", "OK", UartWaitMode::Lf)
                .is_err()
        );
        bus.record_stm32_uart("sysbus.usart2");
        let index = bus
            .watch_stm32_uart("sysbus.usart2", "OK", UartWaitMode::Lf)
            .unwrap();
        for (base, bytes) in [
            (usart2, b"O".as_slice()),
            (usart1, b"OK\n"),
            (usart2, b"K\r"),
        ] {
            for &byte in bytes {
                bus.write(base + 0x28, 1, byte.into(), &mut output).unwrap();
            }
            assert!(!bus.stm32_uart_matched(index));
        }
        bus.write(usart2 + 0x28, 1, b'\n'.into(), &mut output)
            .unwrap();
        assert!(bus.stm32_uart_matched(index));
        assert_eq!(output, b"OOK\nK\r\n");
        bus.finish_stm32_uart_watch(index);
        assert!(!bus.stm32_uart_matched(index));
    }

    #[test]
    fn renode_stm32_pwm_duty_reaches_gpio_for_all_four_upstream_pulses() {
        use crate::stm32_timer::TimerPin;

        // STM32L072.robot: PWM Should Support GPIO Output, adapted to F072 TIM2.
        const GPIO: u32 = 0x4800_0000;
        const TIM2: u32 = 0x4000_0000;
        for (pulse, expected_percent) in [(5, 0), (85, 33), (127, 50), (250, 100)] {
            let mut bus = Bus::default();
            bus.map_nvic(0xc0, None).unwrap();
            bus.map_stm32_gpio(Stm32Gpio::new(GPIO, 0x400, [2, 0, 0], 8))
                .unwrap();
            let mut timer = Stm32Timer::new(TIM2, CORTEX_M_STEP_HZ as u32, u32::MAX, [0; 8]);
            timer.gpio_outputs.push((
                2,
                TimerPin {
                    port: GPIO,
                    pin: 0,
                    function: 0,
                },
            ));
            bus.map_stm32_timer(timer).unwrap();
            let mut output = io::sink();
            for (offset, value) in [
                (0x2c, 255),
                (0x3c, pulse),
                (0x1c, 6 << 4),
                (0x20, 1 << 8),
                (0, 1),
            ] {
                bus.write(TIM2 + offset, 4, value, &mut output).unwrap();
            }
            let mut high = 0u32;
            for _ in 0..256 {
                bus.sample_nvic(0);
                high += bus.read::<4>(GPIO + 0x10).unwrap() & 1;
            }
            assert_eq!(high, pulse);
            assert!((high * 100).abs_diff(expected_percent * 256) <= 10 * 256);
        }
    }

    #[test]
    fn renode_stm32_pwm_input_capture_routes_period_and_pulse_to_gpio() {
        use crate::stm32_timer::TimerPin;

        // STM32F4_Discovery.robot: both PWM Capture cases use 1 kHz / 33%.
        const GPIO: u32 = 0x4800_0000;
        const SOURCE: u32 = 0x4000_0000;
        const CAPTURE: u32 = 0x4000_0400;
        for pair in [0, 2] {
            let mut bus = Bus::default();
            bus.map_nvic(0xc0, None).unwrap();
            bus.map_stm32_gpio(Stm32Gpio::new(GPIO, 0x400, [2, 0, 0], 8))
                .unwrap();
            let mut source = Stm32Timer::new(SOURCE, 1_000_000, u32::MAX, [0; 8]);
            source.gpio_outputs.push((
                0,
                TimerPin {
                    port: GPIO,
                    pin: 0,
                    function: 0,
                },
            ));
            bus.map_stm32_timer(source).unwrap();
            bus.map_stm32_timer(Stm32Timer::new(CAPTURE, 1_000_000, 0xffff, [0; 8]))
                .unwrap();
            bus.route_gpio_timer_input(GPIO, 0, CAPTURE, pair);
            let flags = 6 << pair;
            let first_ccr = CAPTURE + 0x34 + 4 * u32::from(pair);
            let mut output = io::sink();
            for (base, offset, value) in [
                (CAPTURE, 0x18 + 2 * u32::from(pair), 0x0201), // direct and paired indirect input
                (CAPTURE, 0x20, 0x31 << (pair * 4)),           // rising and falling
                (CAPTURE, 0, 1),
                (SOURCE, 0x2c, 999),
                (SOURCE, 0x34, 330),
                (SOURCE, 0x18, 0x60),
                (SOURCE, 0x20, 1),
                (SOURCE, 0, 1),
            ] {
                bus.write(base + offset, 4, value, &mut output).unwrap();
            }
            let mut rising = Vec::new();
            let mut falling = Vec::new();
            for _ in 0..135_000 {
                bus.sample_nvic(0);
                let status = bus.read::<4>(CAPTURE + 0x10).unwrap();
                if status & (2 << pair) != 0 {
                    rising.push(bus.read::<4>(first_ccr).unwrap());
                }
                if status & (4 << pair) != 0 {
                    falling.push(bus.read::<4>(first_ccr + 4).unwrap());
                }
                if status & flags != 0 {
                    bus.write(CAPTURE + 0x10, 4, !flags, &mut output).unwrap();
                }
            }
            assert_eq!(rising.len(), 2);
            assert_eq!(falling.len(), 2);
            let period = rising[1] - rising[0];
            let pulse = falling[1] - rising[1];
            assert_eq!((period, pulse), (1000, 330));
            assert!((1_000_000u32 / period).abs_diff(1000) <= 25);
            assert!((pulse * 100 / period).abs_diff(33) <= 1);
        }
    }

    #[test]
    fn renode_stm32_flash_erase_modes_preserve_the_expected_ranges() {
        // Adapt stm32f4-erase.robot and STM32L072.robot to F072 page geometry.
        const FLASH: u32 = 0x0800_0000;
        const PAGE_SIZE: usize = 2048;
        const BANK_SIZE: usize = 0x20000;
        let mut bus = Bus::default();
        let controller = Stm32F0Flash::BASE;
        bus.map_ram(FLASH, BANK_SIZE).unwrap();
        bus.set_stm32f0_tags(vec![controller..=0x4002_23ff]);
        bus.map_stm32f072_flash().unwrap();
        let write = |bus: &mut Bus, offset, value| {
            bus.write(controller + offset, 4, value, &mut std::io::sink())
                .unwrap();
        };
        // Seed flash as an image loader, equivalent to the upstream fixture setup.
        let page = FLASH + PAGE_SIZE as u32;
        let markers = [
            FLASH,
            FLASH + 2 * PAGE_SIZE as u32,
            FLASH + BANK_SIZE as u32 - 4,
        ];
        for (address, word) in markers
            .into_iter()
            .zip([0xdead_beefu32, 0xcafe_babe, 0x4433_2211])
        {
            bus.memory(address, 4)
                .unwrap()
                .copy_from_slice(&word.to_le_bytes());
        }
        bus.memory(page, PAGE_SIZE).unwrap().fill(0x12);
        write(&mut bus, 4, 0x4567_0123);
        write(&mut bus, 4, 0xcdef_89ab);

        write(&mut bus, 0x14, page + 4);
        write(&mut bus, 0x10, 1 << 6); // STRT alone
        assert_ne!(bus.read::<4>(controller + 0x0c).unwrap() & (1 << 2), 0);
        assert!(
            bus.memory(page, PAGE_SIZE)
                .unwrap()
                .iter()
                .all(|&byte| byte == 0x12)
        );

        write(&mut bus, 0x0c, 1 << 2); // clear PGERR
        write(&mut bus, 0x10, (1 << 1) | (1 << 6)); // PER | STRT
        assert!(
            bus.memory(page, PAGE_SIZE)
                .unwrap()
                .iter()
                .all(|&byte| byte == 0xff)
        );
        for (address, word) in markers
            .into_iter()
            .zip([0xdead_beef, 0xcafe_babe, 0x4433_2211])
        {
            assert_eq!(bus.read::<4>(address).unwrap(), word);
        }

        write(&mut bus, 0x10, (1 << 2) | (1 << 6)); // MER | STRT
        assert!(
            bus.memory(FLASH, BANK_SIZE)
                .unwrap()
                .iter()
                .all(|&byte| byte == 0xff)
        );
    }

    #[test]
    fn stm32f072_flash_preserves_programmed_data_on_reload_and_erases_a_page() {
        let mut bus = Bus::default();
        let flash = 0x0800_0000;
        let controller = Stm32F0Flash::BASE;
        bus.map_ram(flash, 0x20000).unwrap();
        bus.set_stm32f0_tags(vec![controller..=0x4002_23ff]);
        bus.map_stm32f072_flash().unwrap();
        let write = |bus: &mut Bus, offset, value| {
            bus.write(controller + offset, 4, value, &mut std::io::sink())
                .unwrap();
        };
        let unlock = |bus: &mut Bus| {
            write(bus, 4, 0x4567_0123);
            write(bus, 4, 0xcdef_89ab);
        };

        bus.write(flash, 2, 0, &mut std::io::sink()).unwrap();
        assert_eq!(bus.read::<2>(flash).unwrap(), 0xffff);
        unlock(&mut bus);
        write(&mut bus, 0x10, 1);
        bus.write(flash, 2, 0xabcd, &mut std::io::sink()).unwrap();
        write(&mut bus, 0x10, 1 << 13);
        assert!(bus.reset_cortex_m_if_requested());
        assert_eq!(bus.read::<2>(flash).unwrap(), 0xabcd);
        assert_ne!(bus.read::<4>(controller + 0x10).unwrap() & (1 << 7), 0);

        unlock(&mut bus);
        write(&mut bus, 0x14, flash);
        write(&mut bus, 0x10, (1 << 1) | (1 << 6));
        assert_eq!(bus.read::<2>(flash).unwrap(), 0xffff);
    }
}
