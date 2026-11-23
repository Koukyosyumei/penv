use std::{
    alloc::{Layout, alloc_zeroed},
    fmt,
    fs::File,
    io::{self, Write},
    path::Path,
};

use crate::{
    clint::Clint,
    litex,
    litex_eth::{Ethernet, Phy, Registration},
    litex_i2s::I2s,
    litex_panel::{ButtonAction, Panel},
    litex_sd::{Card, Controller as SdController},
    litex_video::Framebuffer,
    murax::{Gpio, Timer},
    ns16550::Ns16550,
    nvic::{Nvic, VTOR},
    plic::Plic,
    stm32_exti::Exti,
    stm32_uart::{Stm32Irq, Stm32Usart},
    systick::SysTick,
    uart::{LiteXTxTiming, LiteXUart, MuraxUart},
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
    AudioOutput(io::Error),
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
            Self::AudioOutput(error) => write!(f, "LiteX I2S: {error}"),
        }
    }
}

pub(crate) struct Region {
    base: u32,
    aliases: Vec<u32>,
    bytes: Vec<u8>,
    flash_pages: Option<Vec<bool>>,
}

struct LiteXTimer {
    bases: Vec<u32>,
    timer: litex::Timer,
    vex_irq: Option<(usize, u8)>,
}

#[derive(Default)]
pub struct Bus {
    regions: Vec<Region>,
    silencers: Vec<(u32, u32)>,
    tags: Vec<(u32, u32)>,
    renode_unmapped_data: bool,
    reservations: [Option<u32>; crate::machine::MAX_HARTS],
    uart_tx: Option<u32>,
    pub uart: Option<MuraxUart>,
    ns16550: Option<Ns16550>,
    litex_uart: Option<LiteXUart>,
    litex_timer: Option<LiteXTimer>,
    pub litex_cpu_timer: Option<litex::CpuTimer>,
    litex_soc: Option<litex::SocController>,
    litex_io: Vec<(Vec<u32>, litex::IoDevice)>,
    litex_gpio_irqs: Vec<(usize, usize, u8)>,
    tohost: Option<(u32, bool)>,
    pub tohost_exit: Option<u64>,
    litex_eth: Option<Ethernet>,
    litex_panel: Option<Panel>,
    pub litex_i2s: Vec<I2s>,
    litex_sd: Option<SdController>,
    pub litex_video: Option<Framebuffer>,
    litex_flash: Option<(Vec<u32>, usize, crate::litex_flash::Flash)>,
    stm32_uarts: Vec<Stm32Usart>,
    exti: Option<Exti>,
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
        Ok(Self {
            base,
            aliases: Vec::new(),
            bytes,
            flash_pages: None,
        })
    }

    fn bytes_mut(&mut self, offset: usize, width: usize) -> &mut [u8] {
        if let Some(pages) = self.flash_pages.as_mut()
            && width != 0
        {
            for (page, initialized) in pages
                .iter_mut()
                .enumerate()
                .take((offset + width - 1) / 4096 + 1)
                .skip(offset / 4096)
            {
                if !*initialized {
                    self.bytes[page * 4096..(page + 1) * 4096].fill(0xff);
                    *initialized = true;
                }
            }
        }
        &mut self.bytes[offset..offset + width]
    }

    pub(crate) fn flash_size(&self) -> usize {
        self.bytes.len()
    }

    pub(crate) fn flash_read(&self, offset: usize) -> u8 {
        if self
            .flash_pages
            .as_ref()
            .is_some_and(|pages| !pages[offset / 4096])
        {
            0xff
        } else {
            self.bytes[offset]
        }
    }

    pub(crate) fn flash_program(&mut self, offset: usize, byte: u8) {
        if offset < self.bytes.len() {
            self.bytes_mut(offset, 1)[0] &= byte;
        }
    }

    pub(crate) fn flash_erase(&mut self, offset: usize, size: usize) {
        let end = (offset + size).min(self.bytes.len());
        if offset < end
            && let Some(pages) = self.flash_pages.as_mut()
        {
            pages[offset / 4096..end / 4096].fill(false);
        }
    }

    fn offset(&self, address: u32, width: usize) -> Option<usize> {
        let at = |base| {
            let offset = address.checked_sub(base)? as usize;
            (width <= self.bytes.len().checked_sub(offset)?).then_some(offset)
        };
        at(self.base).or_else(|| self.aliases.iter().find_map(|&base| at(base)))
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

    pub fn watch_tohost(&mut self, address: u32, act: bool) -> Result<(), String> {
        if address & 7 != 0
            || self.read_ram::<4>(address).is_err()
            || self.read_ram::<4>(address + 4).is_err()
        {
            return Err("--tohost requires an aligned 8-byte address in RAM".into());
        }
        self.tohost = Some((address, act));
        Ok(())
    }

    fn observe_tohost(
        &mut self,
        address: u32,
        width: usize,
        uart: &mut impl Write,
    ) -> Result<(), BusError> {
        let Some((base, act)) = self.tohost else {
            return Ok(());
        };
        // ACT's RV32 Sail model commits each packet with a separate upper-word store.
        if self.tohost_exit.is_some()
            || address != if act { base + 4 } else { base }
            || (act && width != 4)
        {
            return Ok(());
        }
        let low = self.read_ram::<4>(base)?;
        let high = self.read_ram::<4>(base + 4)?;
        if act && high == 0x0101_0000 {
            uart.write_all(&[low as u8]).map_err(BusError::Output)?;
        } else if (!act || high == 0) && low & 1 != 0 {
            self.tohost_exit = Some(((u64::from(high) << 32) | u64::from(low)) >> 1);
        }
        Ok(())
    }

    pub fn use_renode_unmapped_data(&mut self) {
        self.renode_unmapped_data = true;
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

    pub fn map_ram_views(&mut self, bases: &[u32], size: usize) -> Result<(), String> {
        let Some((&base, aliases)) = bases.split_first() else {
            return Err("expected at least one RAM address".into());
        };
        for (index, &base) in bases.iter().enumerate() {
            let start = u64::from(base);
            let end = start.saturating_add(size as u64);
            if end > 1 << 32
                || self.overlaps(start, end)
                || bases[..index].iter().any(|&other| {
                    let other = u64::from(other);
                    start < other + size as u64 && other < end
                })
            {
                return Err(format!("invalid or overlapping RAM view at {base:#010x}"));
            }
        }
        let mut region = Region::new(base, size)?;
        region.aliases.extend_from_slice(aliases);
        self.regions.push(region);
        Ok(())
    }

    fn reservation_address(&self, address: u32) -> u32 {
        self.regions
            .iter()
            .find_map(|region| {
                region
                    .offset(address, 4)
                    .map(|offset| region.base + offset as u32)
            })
            .unwrap_or(address)
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
                uart.bases
                    .iter()
                    .any(|&base| overlaps_range(start, end, base, LiteXUart::SIZE))
            })
            || self.litex_timer.as_ref().is_some_and(|device| {
                device
                    .bases
                    .iter()
                    .any(|&base| overlaps_range(start, end, base, device.timer.size()))
            })
            || self
                .litex_cpu_timer
                .as_ref()
                .is_some_and(|timer| overlaps_range(start, end, timer.base, litex::CpuTimer::SIZE))
            || self.litex_eth.as_ref().is_some_and(|eth| {
                eth.registrations.iter().any(|registration| {
                    overlaps_range(start, end, registration.base, registration.size)
                })
            })
            || self.litex_flash.as_ref().is_some_and(|(bases, _, _)| {
                bases
                    .iter()
                    .any(|&base| overlaps_range(start, end, base, crate::litex_flash::Flash::SIZE))
            })
            || self.litex_panel.as_ref().is_some_and(|panel| {
                panel
                    .bases
                    .iter()
                    .any(|&base| overlaps_range(start, end, base, Panel::SIZE))
            })
            || self.litex_i2s.iter().any(|i2s| {
                overlaps_range(start, end, i2s.csr, I2s::SIZE)
                    || overlaps_range(start, end, i2s.buffer, i2s.buffer_size)
            })
            || self.litex_sd.as_ref().is_some_and(|sd| {
                sd.registrations.iter().any(|registration| {
                    overlaps_range(start, end, registration.base, registration.size)
                })
            })
            || self.litex_video.as_ref().is_some_and(|video| {
                overlaps_range(start, end, video.dma, Framebuffer::DMA_SIZE)
                    || overlaps_range(start, end, video.vtg, Framebuffer::VTG_SIZE)
            })
            || self.litex_io.iter().any(|(bases, device)| {
                bases
                    .iter()
                    .any(|&base| overlaps_range(start, end, base, device.size()))
            })
            || self
                .litex_soc
                .as_ref()
                .is_some_and(|soc| overlaps_range(start, end, soc.base, soc.size()))
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
            || self
                .silencers
                .iter()
                .any(|&(base, size)| overlaps_range(start, end, base, size))
            || self.regions.iter().any(|region| {
                std::iter::once(&region.base)
                    .chain(&region.aliases)
                    .any(|&base| {
                        let other = u64::from(base);
                        start < other + region.bytes.len() as u64 && other < end
                    })
            })
    }

    pub fn map_clint(&mut self, base: u32, targets: usize) -> Result<(), String> {
        self.map_clint_with_frequency(base, targets, Clint::STEP_HZ)
    }

    pub fn map_clint_with_frequency(
        &mut self,
        base: u32,
        targets: usize,
        frequency: u32,
    ) -> Result<(), String> {
        let start = u64::from(base);
        let end = start + Clint::SIZE;
        if base & 3 != 0 || end > 1 << 32 || self.clint.is_some() || self.overlaps(start, end) {
            return Err(format!(
                "invalid or overlapping CLINT range at {base:#010x}"
            ));
        }
        self.clint = Some(Clint::new(base, targets, frequency));
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

    pub fn map_plic(
        &mut self,
        base: u32,
        sources: u32,
        outputs: &[u32],
        priorities: bool,
    ) -> Result<(), String> {
        if !(1..=63).contains(&sources) {
            return Err("PLIC requires 1–63 interrupt sources".into());
        }
        let start = u64::from(base);
        let end = start + Plic::SIZE;
        if base & 3 != 0 || end > 1 << 32 || self.plic.is_some() || self.overlaps(start, end) {
            return Err(format!("invalid or overlapping PLIC range at {base:#010x}"));
        }
        self.plic = Some(Plic::new(base, sources, outputs).with_priorities(priorities));
        Ok(())
    }

    pub fn map_ns16550(
        &mut self,
        base: u32,
        source: Option<u32>,
        wide: bool,
    ) -> Result<(), String> {
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

    fn check_litex_ranges(&self, bases: &[u32], size: u32) -> Result<(), String> {
        for (index, &base) in bases.iter().enumerate() {
            let start = u64::from(base);
            let end = start + u64::from(size);
            if base & 3 != 0
                || end > 1 << 32
                || self.overlaps(start, end)
                || bases[..index]
                    .iter()
                    .any(|&base| overlaps_range(start, end, base, size))
            {
                return Err(format!(
                    "invalid or overlapping LiteX MMIO range at {base:#010x}"
                ));
            }
        }
        Ok(())
    }

    pub fn map_litex_uart(
        &mut self,
        bases: Vec<u32>,
        vex_irq: Option<(usize, u8)>,
    ) -> Result<(), String> {
        self.map_litex_uart_with_timing(bases, vex_irq, None)
    }

    pub fn map_litex_uart_with_timing(
        &mut self,
        bases: Vec<u32>,
        vex_irq: Option<(usize, u8)>,
        timing: Option<LiteXTxTiming>,
    ) -> Result<(), String> {
        self.check_litex_ranges(&bases, LiteXUart::SIZE)?;
        self.litex_uart = Some(LiteXUart::new(bases, vex_irq).with_timing(timing));
        Ok(())
    }

    pub fn tick_litex_uart(&mut self, step: u64, output: &mut impl Write) -> io::Result<()> {
        if let Some(uart) = self.litex_uart.as_mut() {
            uart.tick(step, output)?;
        }
        Ok(())
    }

    pub fn map_litex_timer(
        &mut self,
        bases: Vec<u32>,
        timer: litex::Timer,
        vex_irq: Option<(usize, u8)>,
    ) -> Result<(), String> {
        self.check_litex_ranges(&bases, timer.size())?;
        self.litex_timer = Some(LiteXTimer {
            bases,
            timer,
            vex_irq,
        });
        Ok(())
    }

    pub fn map_litex_timer_alias(&mut self, base: u32) -> Result<(), String> {
        let timer = self
            .litex_timer
            .as_ref()
            .ok_or("--litex-timer-alias requires a LiteX timer")?;
        self.check_litex_ranges(&[base], timer.timer.size())?;
        self.litex_timer.as_mut().unwrap().bases.push(base);
        Ok(())
    }

    pub fn map_litex_cpu_timer(&mut self, base: u32, frequency: u32) -> Result<(), String> {
        self.check_litex_ranges(&[base], litex::CpuTimer::SIZE)?;
        self.litex_cpu_timer = Some(litex::CpuTimer::new(base, frequency));
        Ok(())
    }

    pub fn map_silencer(&mut self, base: u32, size: u32) -> Result<(), String> {
        let start = u64::from(base);
        let end = start + u64::from(size);
        if size == 0 || end > 1 << 32 || self.overlaps(start, end) {
            return Err(format!(
                "invalid or overlapping SilenceRange at {base:#010x}"
            ));
        }
        self.silencers.push((base, size));
        Ok(())
    }

    pub fn map_tag(&mut self, base: u32, size: u32) -> Result<(), String> {
        if size == 0 || u64::from(base) + u64::from(size) > 1 << 32 {
            return Err(format!("invalid Tag range at {base:#010x}"));
        }
        self.tags.push((base, size));
        Ok(())
    }

    fn zero_mmio(&self, address: u32, width: usize) -> bool {
        self.silencers
            .iter()
            .chain(&self.tags)
            .any(|&(base, size)| {
                address >= base && u64::from(address - base) + width as u64 <= u64::from(size)
            })
    }

    pub fn map_litex_eth(
        &mut self,
        name: String,
        registrations: Vec<Registration>,
        irq: Option<(usize, u8)>,
        phy: Option<Phy>,
        wide_csr: bool,
    ) -> Result<(), String> {
        for (index, registration) in registrations.iter().enumerate() {
            let start = u64::from(registration.base);
            let end = start + u64::from(registration.size);
            self.check_litex_ranges(&[registration.base], registration.size)?;
            if registration.size == 0
                || registrations[..index]
                    .iter()
                    .any(|other| overlaps_range(start, end, other.base, other.size))
            {
                return Err(format!(
                    "invalid or overlapping Ethernet range at {start:#010x}"
                ));
            }
        }
        let mut ethernet = Ethernet::new(registrations, irq, phy).with_wide_csr(wide_csr);
        ethernet.name = Some(name);
        self.litex_eth = Some(ethernet);
        Ok(())
    }

    pub fn capture_litex_framebuffer(&self, path: &Path) -> Result<(), String> {
        let video = self.litex_video.as_ref().ok_or("no LiteX framebuffer")?;
        if video.active.is_none() {
            return Err("LiteX framebuffer DMA is disabled".into());
        }
        let file = File::create(path).map_err(|error| format!("{}: {error}", path.display()))?;
        video
            .render(self, io::BufWriter::new(file))
            .map_err(|error| format!("{}: {error}", path.display()))
    }

    pub fn litex_phy_mut(&mut self, name: &str) -> Result<&mut Phy, String> {
        let path = name.strip_prefix("sysbus.").unwrap_or(name);
        self.litex_eth
            .as_mut()
            .and_then(|ethernet| ethernet.phy_mut(path))
            .ok_or_else(|| format!("unknown LiteX Ethernet PHY {name:?}"))
    }

    pub fn attach_litex_tftp(
        &mut self,
        name: &str,
        server: crate::litex_tftp::Server,
    ) -> Result<(), String> {
        let name = name.strip_prefix("sysbus.").unwrap_or(name);
        let ethernet = self
            .litex_eth
            .as_mut()
            .filter(|ethernet| ethernet.name.as_deref() == Some(name))
            .ok_or_else(|| format!("unknown LiteX Ethernet controller {name:?}"))?;
        ethernet.tftp = Some(server);
        Ok(())
    }

    pub fn map_litex_flash(&mut self, bases: Vec<u32>, backing: u32) -> Result<(), String> {
        self.check_litex_ranges(&bases, crate::litex_flash::Flash::SIZE)?;
        let index = self
            .regions
            .iter()
            .position(|region| region.base == backing)
            .ok_or("flash backing must be mapped memory")?;
        let region = &mut self.regions[index];
        let size = region.bytes.len();
        if !size.is_power_of_two() || !(65536..=268435456).contains(&size) {
            return Err(
                "Micron flash requires power-of-two backing between 64 KiB and 256 MiB".into(),
            );
        }
        region.flash_pages = Some(vec![false; size / 4096]);
        self.litex_flash = Some((bases, index, crate::litex_flash::Flash::default()));
        Ok(())
    }

    pub fn map_litex_sd(&mut self, sd: SdController) -> Result<(), String> {
        for (index, region) in sd.registrations.iter().enumerate() {
            let start = u64::from(region.base);
            let end = start + u64::from(region.size);
            self.check_litex_ranges(&[region.base], region.size)?;
            if region.size == 0
                || sd.registrations[..index]
                    .iter()
                    .any(|other| overlaps_range(start, end, other.base, other.size))
            {
                return Err(format!("invalid or overlapping SD range at {start:#010x}"));
            }
        }
        self.litex_sd = Some(sd);
        Ok(())
    }

    pub fn attach_litex_sd(&mut self, name: &str, card: Card) -> Result<(), String> {
        let name = name.strip_prefix("sysbus.").unwrap_or(name);
        if let Some(controller) = self
            .litex_sd
            .as_mut()
            .filter(|controller| controller.name == name)
        {
            controller.card = Some(card);
            return Ok(());
        }
        for (_, device) in &mut self.litex_io {
            if let litex::IoDevice::Spi(spi) = device
                && spi.name.as_deref() == Some(name)
            {
                spi.card = Some(Box::new(crate::litex_sd::SpiCard::new(card)));
                return Ok(());
            }
        }
        Err(format!("unknown LiteX SD controller {name:?}"))
    }

    pub fn map_litex_panel(&mut self, panel: Panel) -> Result<(), String> {
        self.check_litex_ranges(&panel.bases, Panel::SIZE)?;
        self.litex_panel = Some(panel);
        Ok(())
    }

    pub fn litex_led_state(&self, name: &str) -> Result<bool, String> {
        self.litex_panel
            .as_ref()
            .and_then(|panel| panel.led_state(name))
            .or_else(|| {
                let path = name.strip_prefix("sysbus.").unwrap_or(name);
                self.litex_io.iter().find_map(|(_, device)| match device {
                    litex::IoDevice::Gpio(gpio) => gpio.led_state(path),
                    _ => None,
                })
            })
            .ok_or_else(|| format!("unknown wired LiteX LED {name:?}"))
    }

    pub fn litex_sensor_mut(
        &mut self,
        name: &str,
    ) -> Result<&mut crate::litex_i2c::Sensor, String> {
        let path = name.strip_prefix("sysbus.").unwrap_or(name);
        self.litex_io
            .iter_mut()
            .find_map(|(_, device)| match device {
                litex::IoDevice::I2c(controller) => controller.sensor_mut(path),
                _ => None,
            })
            .ok_or_else(|| format!("unknown LiteX I2C sensor {name:?}"))
    }

    pub fn has_litex_button(&self, name: &str) -> bool {
        let name = name.rsplit('.').next().unwrap_or(name);
        self.litex_panel
            .as_ref()
            .is_some_and(|panel| panel.has_button(name))
            || self.litex_io.iter().any(|(_, device)| {
                matches!(device, litex::IoDevice::Gpio(gpio) if gpio.has_button(name))
            })
    }

    pub fn set_litex_button(&mut self, name: &str, action: ButtonAction) -> Result<(), String> {
        let name = name.rsplit('.').next().unwrap_or(name);
        if let Some(panel) = self
            .litex_panel
            .as_mut()
            .filter(|panel| panel.has_button(name))
        {
            return panel.button_action(name, action);
        }
        for (_, device) in &mut self.litex_io {
            if let litex::IoDevice::Gpio(gpio) = device
                && gpio.has_button(name)
            {
                return gpio.button_action(name, action);
            }
        }
        Err(format!("unknown LiteX button {name:?}"))
    }

    pub fn map_litex_io(&mut self, bases: Vec<u32>, device: litex::IoDevice) -> Result<(), String> {
        self.check_litex_ranges(&bases, device.size())?;
        if let litex::IoDevice::Gpio(gpio) = &device
            && let Some((hart, line)) = gpio.irq
        {
            self.litex_gpio_irqs.push((self.litex_io.len(), hart, line));
        }
        self.litex_io.push((bases, device));
        Ok(())
    }

    pub fn map_litex_soc(&mut self, base: u32, wide_csr: bool) -> Result<(), String> {
        let soc = litex::SocController::new(base, wide_csr);
        self.check_litex_ranges(&[base], soc.size())?;
        self.litex_soc = Some(soc);
        Ok(())
    }

    pub fn map_litex_video(&mut self, framebuffer: Framebuffer) -> Result<(), String> {
        self.check_litex_ranges(&[framebuffer.dma], Framebuffer::DMA_SIZE)?;
        self.check_litex_ranges(&[framebuffer.vtg], Framebuffer::VTG_SIZE)?;
        let dma = u64::from(framebuffer.dma);
        if overlaps_range(
            dma,
            dma + u64::from(Framebuffer::DMA_SIZE),
            framebuffer.vtg,
            Framebuffer::VTG_SIZE,
        ) {
            return Err("overlapping LiteX framebuffer regions".into());
        }
        self.litex_video = Some(framebuffer);
        Ok(())
    }

    pub fn map_litex_i2s(&mut self, device: I2s) -> Result<(), String> {
        self.check_litex_ranges(&[device.csr], I2s::SIZE)?;
        self.check_litex_ranges(&[device.buffer], device.buffer_size)?;
        if overlaps_range(
            u64::from(device.csr),
            u64::from(device.csr) + u64::from(I2s::SIZE),
            device.buffer,
            device.buffer_size,
        ) {
            return Err("overlapping LiteX I2S regions".into());
        }
        self.litex_i2s.push(device);
        Ok(())
    }

    pub fn tick_litex(&mut self, hart: usize, step: u64) -> u64 {
        let mut lines = 0;
        if let Some(device) = self.litex_timer.as_mut() {
            device.timer.tick();
            if let Some((target, line)) = device.vex_irq
                && target == hart
                && device.timer.interrupt_pending()
            {
                lines |= 1 << line;
            }
        }
        if let Some(uart) = self.litex_uart.as_ref()
            && let Some((target, line)) = uart.vex_irq
            && target == hart
            && uart.interrupt_pending()
        {
            lines |= 1 << line;
        }
        if let Some(eth) = self.litex_eth.as_ref()
            && let Some((target, line)) = eth.vex_irq
            && target == hart
            && eth.interrupt_pending()
        {
            lines |= 1 << line;
        }
        if let Some(panel) = self.litex_panel.as_ref()
            && let Some((target, line)) = panel.irq
            && target == hart
            && panel.interrupt_pending()
        {
            lines |= 1 << line;
        }
        for i2s in &mut self.litex_i2s {
            i2s.tick(step);
            if i2s.irq.0 == hart && i2s.interrupt_pending() {
                lines |= 1 << i2s.irq.1;
            }
        }
        for &(index, target, line) in &self.litex_gpio_irqs {
            if target == hart
                && let litex::IoDevice::Gpio(gpio) = &self.litex_io[index].1
                && gpio.interrupt_pending()
            {
                lines |= 1 << line;
            }
        }
        lines
    }

    pub fn has_litex_irq(&self) -> bool {
        self.litex_timer
            .as_ref()
            .is_some_and(|device| device.vex_irq.is_some())
            || self
                .litex_uart
                .as_ref()
                .is_some_and(|uart| uart.vex_irq.is_some())
            || self
                .litex_eth
                .as_ref()
                .is_some_and(|eth| eth.vex_irq.is_some())
            || self
                .litex_panel
                .as_ref()
                .is_some_and(|panel| panel.irq.is_some())
            || !self.litex_gpio_irqs.is_empty()
            || !self.litex_i2s.is_empty()
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

    pub fn attach_litex_uart_input(
        &mut self,
        receiver: std::sync::mpsc::Receiver<io::Result<Vec<u8>>>,
    ) -> Result<(), String> {
        let uart = self
            .litex_uart
            .as_mut()
            .ok_or("--litex-uart-stdio requires a LiteX UART")?;
        uart.attach_live_input(receiver);
        Ok(())
    }

    pub fn enable_litex_uart_legacy_console(&mut self) -> Result<(), String> {
        let uart = self
            .litex_uart
            .as_mut()
            .ok_or("--litex-uart-legacy-console requires a LiteX UART")?;
        uart.legacy_console_events();
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
            && let Some(source) = uart.source
            && let Some(plic) = self.plic.as_mut()
        {
            plic.set_line(source, uart.interrupt_pending());
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
                    let address = u64::from(region.base) + offset as u64;
                    for reservation in &mut self.reservations {
                        if reservation.is_some_and(|reserved| {
                            address < u64::from(reserved) + 4
                                && u64::from(reserved) < address + width as u64
                        }) {
                            *reservation = None;
                        }
                    }
                }
                return Ok(region.bytes_mut(offset, width));
            }
        }
        Err(BusError::Unmapped(address, width))
    }

    pub fn load_reserved(&mut self, hart: usize, address: u32) -> Result<u32, BusError> {
        let value = self.read_ram::<4>(address)?;
        self.reservations[hart] = Some(self.reservation_address(address));
        Ok(value)
    }

    pub fn store_conditional(
        &mut self,
        hart: usize,
        address: u32,
        value: u32,
    ) -> Result<u32, BusError> {
        let canonical = self.reservation_address(address);
        let reserved = self.reservations[hart].take() == Some(canonical);
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
        // Mapped devices cannot overlap RAM. Try the common RAM access first.
        let ram_error = match self.read_ram::<N>(address) {
            Ok(value) => return Ok(value),
            Err(error) => error,
        };
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
        if (N == 1 || N == 4 && address & 3 == 0)
            && let Some(device) = self.litex_timer.as_ref()
            && let Some(offset) = litex_offset(&device.bases, device.timer.size(), address)
        {
            return Ok(device.timer.read(offset, N));
        }
        if N == 4
            && address & 3 == 0
            && let Some(timer) = self.litex_cpu_timer.as_ref()
            && timer.contains(address)
        {
            return Ok(timer.read(address));
        }
        if N == 4
            && address & 3 == 0
            && let Some(soc) = self.litex_soc.as_ref()
            && soc.contains(address)
        {
            return Ok(soc.read(address));
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
        if address as usize & (N - 1) == 0
            && let Some(value) = self.litex_io.iter().find_map(|(bases, device)| {
                let offset = device.offset(bases, address, N)?;
                Some(device.read(offset, N))
            })
        {
            return Ok(value);
        }
        if address as usize & (N - 1) == 0
            && let Some(eth) = self.litex_eth.as_mut()
            && let Some((region, offset)) = eth.locate(address, N)
        {
            return Ok(eth.read(region, offset, N));
        }
        if address as usize & (N - 1) == 0
            && let Some((bases, _, flash)) = self.litex_flash.as_ref()
            && let Some(offset) = litex_offset(bases, crate::litex_flash::Flash::SIZE, address)
        {
            let value = flash.read(offset & !3) >> ((offset & 3) * 8);
            return Ok(value & (u32::MAX >> ((4 - N) * 8)));
        }
        if N == 4
            && address & 3 == 0
            && let Some(panel) = self.litex_panel.as_ref()
            && let Some(offset) = litex_offset(&panel.bases, Panel::SIZE, address)
        {
            return Ok(panel.read(offset));
        }
        if N == 4
            && address & 3 == 0
            && let Some(sd) = self.litex_sd.as_ref()
            && let Some((region, offset)) = sd.locate(address)
        {
            return Ok(sd.read(region, offset));
        }
        if N == 4
            && address & 3 == 0
            && let Some(video) = self.litex_video.as_ref()
            && let Some(value) = video.read(address)
        {
            return Ok(value);
        }
        if (N == 1 || (N == 4 && address & 3 == 0))
            && let Some(value) = self
                .litex_i2s
                .iter_mut()
                .find_map(|i2s| i2s.read(address & !3))
        {
            return Ok(if N == 1 {
                (value >> ((address & 3) * 8)) & 0xff
            } else {
                value
            });
        }
        if matches!(ram_error, BusError::Unmapped(..))
            && (self.renode_unmapped_data || self.zero_mmio(address, N))
        {
            return Ok(0);
        }
        Err(ram_error)
    }

    pub fn read_ram<const N: usize>(&self, address: u32) -> Result<u32, BusError> {
        if address as usize & (N - 1) != 0 {
            return Err(BusError::Misaligned(address, N));
        }
        for region in &self.regions {
            if let Some(offset) = region.offset(address, N) {
                let mut bytes = [0; 4];
                if region.flash_pages.is_some() {
                    for (index, byte) in bytes[..N].iter_mut().enumerate() {
                        *byte = region.flash_read(offset + index);
                    }
                } else {
                    bytes[..N].copy_from_slice(&region.bytes[offset..offset + N]);
                }
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
        let ram_error = match self.memory(address, width) {
            Ok(bytes) => {
                bytes.copy_from_slice(&value.to_le_bytes()[..width]);
                return self.observe_tohost(address, width, uart);
            }
            Err(error) => error,
        };
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
        if matches!(width, 1 | 4)
            && let Some(device) = self.litex_timer.as_mut()
            && let Some(offset) = litex_offset(&device.bases, device.timer.size(), address)
        {
            device.timer.write(offset, width, value);
            return Ok(());
        }
        if width == 4
            && let Some(timer) = self.litex_cpu_timer.as_mut()
            && timer.contains(address)
        {
            timer.write(address, value);
            return Ok(());
        }
        if width == 4
            && let Some(soc) = self.litex_soc.as_mut()
            && soc.contains(address)
        {
            soc.write(address, value);
            return Ok(());
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
        for (bases, device) in &mut self.litex_io {
            if let Some(offset) = device.offset(bases, address, width) {
                device.write(offset, width, value);
                return Ok(());
            }
        }
        if let Some(eth) = self.litex_eth.as_mut()
            && let Some((region, offset)) = eth.locate(address, width)
        {
            eth.write(region, offset, width, value);
            return Ok(());
        }
        if let Some((bases, backing, flash)) = self.litex_flash.as_mut()
            && let Some(offset) = litex_offset(bases, crate::litex_flash::Flash::SIZE, address)
        {
            if offset & 3 == 0 {
                flash.write(offset, value, &mut self.regions[*backing]);
            }
            return Ok(());
        }
        if width == 4
            && let Some(panel) = self.litex_panel.as_mut()
            && let Some(offset) = litex_offset(&panel.bases, Panel::SIZE, address)
        {
            panel.write(offset, value);
            return Ok(());
        }
        if width == 4
            && let Some((region, offset)) = self.litex_sd.as_ref().and_then(|sd| sd.locate(address))
        {
            let mut sd = self.litex_sd.take().unwrap();
            sd.write(region, offset, value, self);
            self.litex_sd = Some(sd);
            return Ok(());
        }
        if width == 4
            && self
                .litex_video
                .as_mut()
                .is_some_and(|video| video.write(address, value))
        {
            return Ok(());
        }
        if matches!(width, 1 | 4) {
            for i2s in &mut self.litex_i2s {
                let value = if width == 1 {
                    (value & 0xff) << ((address & 3) * 8)
                } else {
                    value
                };
                if i2s
                    .write(address & !3, value)
                    .map_err(BusError::AudioOutput)?
                {
                    return Ok(());
                }
            }
        }
        if self.renode_unmapped_data || self.zero_mmio(address, width) {
            return Ok(());
        }
        Err(ram_error)
    }
}

fn overlaps_range(start: u64, end: u64, base: u32, size: u32) -> bool {
    let base = u64::from(base);
    start < base + u64::from(size) && base < end
}

fn litex_offset(bases: &[u32], size: u32, address: u32) -> Option<u32> {
    bases.iter().find_map(|&base| {
        let offset = address.wrapping_sub(base);
        (offset < size).then_some(offset)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn litex_spi_loopback_and_gpio_use_shared_registers() {
        let mut bus = Bus::default();
        let spi = 0x6000_2000;
        let spi_shadow = 0xe000_2000;
        let gpio = 0x6000_5800;
        let gpio_shadow = 0xe000_5800;
        bus.map_litex_io(
            vec![spi, spi_shadow],
            litex::IoDevice::Spi(litex::Spi::default()),
        )
        .unwrap();
        bus.map_litex_io(
            vec![gpio, gpio_shadow],
            litex::IoDevice::Gpio(litex::Gpio::new(
                litex::GpioMode::InOut,
                false,
                None,
                Vec::new(),
                Vec::new(),
            )),
        )
        .unwrap();

        bus.write(spi_shadow + 4, 4, 1, &mut io::sink()).unwrap();
        assert_eq!(bus.read::<4>(spi + 0x10).unwrap(), 0xff);
        bus.write(spi + 0x18, 4, 1, &mut io::sink()).unwrap();
        bus.write(spi + 0x0c, 4, 0x134, &mut io::sink()).unwrap();
        bus.write(spi_shadow + 4, 4, 1, &mut io::sink()).unwrap();
        assert_eq!(bus.read::<4>(spi + 8).unwrap(), 1);
        assert_eq!(bus.read::<4>(spi_shadow + 0x10).unwrap(), 0x34);

        bus.write(gpio, 4, u32::MAX, &mut io::sink()).unwrap();
        bus.write(gpio + 4, 4, 0x55aa, &mut io::sink()).unwrap();
        assert_eq!(bus.read::<4>(gpio_shadow).unwrap(), 0);
        assert_eq!(bus.read::<4>(gpio_shadow + 4).unwrap(), 0x55aa);
    }

    #[test]
    fn ram_views_share_contents_and_reservations() {
        let mut bus = Bus::default();
        let base = 0x4000_0000;
        let shadow = 0xc000_0000;
        bus.map_ram_views(&[base, shadow], 0x1000).unwrap();
        assert!(bus.map_ram(shadow + 0x800, 0x1000).is_err());
        assert!(bus.map_ram_views(&[0, 0x800], 0x1000).is_err());
        bus.map_ram(0, 0x1000).unwrap();

        bus.memory(base + 4, 4)
            .unwrap()
            .copy_from_slice(&[1, 2, 3, 4]);
        assert_eq!(bus.load_reserved(0, shadow + 4).unwrap(), 0x0403_0201);
        assert_eq!(bus.store_conditional(0, base + 4, 42).unwrap(), 0);
        assert_eq!(bus.load_reserved(0, base + 4).unwrap(), 42);
        bus.write(shadow + 5, 1, 1, &mut io::sink()).unwrap();
        assert_eq!(bus.store_conditional(0, base + 4, 0).unwrap(), 1);
        assert_eq!(bus.read_ram::<4>(shadow + 4).unwrap(), 0x12a);
    }
}
