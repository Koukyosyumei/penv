mod ble;

use std::{collections::VecDeque, io::Write};

use crate::{
    bus::{Bus, BusError, RAM_BASE},
    csr::{Csr, EXTERNAL},
    sv32::TranslationCache,
    thumb::{Thumb, ThumbStep},
};

pub const MAX_HARTS: usize = 8;

pub struct Machine {
    pub bus: Bus,
    pub primary: Hart,
    secondary: Vec<Hart>,
    next_hart: usize,
    elapsed_steps: u64,
    cpu_paused: bool,
    cpu_halted: bool,
    cpu_clock_stopped: bool,
    pub reset_log: Option<String>,
    uart_step_prefix: bool,
    uart_line_start: bool,
    reset_steps: VecDeque<u64>,
    uart_input: VecDeque<(u64, Vec<u8>)>,
    button_events: VecDeque<(u64, usize, bool)>,
    adc_events: VecDeque<(u64, usize, f64)>,
    adxl_events: VecDeque<(u64, String, [i16; 3])>,
}

#[derive(Clone)]
pub struct Hart {
    pub name: Option<String>,
    csr: Csr,
    translations: TranslationCache,
    thumb: Option<Thumb>,
    pub pc: u32,
    pub registers: [u32; 32],
}

#[derive(Debug, PartialEq, Eq)]
pub enum Stop {
    Semihosting(crate::semihosting::Exit),
    Ecall,
    Ebreak,
    Paused,
    Limit,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CpuResetKind {
    Cpu,
    Machine,
}

enum Step {
    Retired,
    RetiredAndPaused,
    Trap,
    Stop(Stop),
}

#[derive(Clone, Copy)]
enum Access {
    Fetch,
    Load,
    Store,
}

enum Fault {
    Bus(Access, BusError),
    Illegal(u32),
    InvalidCsr(u32, String),
    MisalignedTarget(u32),
}

impl Fault {
    fn exception(&self) -> Option<(u32, u32)> {
        Some(match self {
            Self::Bus(access, error) => match error {
                BusError::Misaligned(address, _) => (
                    match access {
                        Access::Fetch => 0,
                        Access::Load => 4,
                        Access::Store => 6,
                    },
                    *address,
                ),
                BusError::Unmapped(address, _)
                | BusError::Denied(address)
                | BusError::SlaveFault(address) => (
                    match access {
                        Access::Fetch => 1,
                        Access::Load => 5,
                        Access::Store => 7,
                    },
                    *address,
                ),
                BusError::Page(address) => (
                    match access {
                        Access::Fetch => 12,
                        Access::Load => 13,
                        Access::Store => 15,
                    },
                    *address,
                ),
                BusError::Output(_) | BusError::Python(_) => return None,
            },
            Self::Illegal(insn) | Self::InvalidCsr(insn, _) => (2, *insn),
            Self::MisalignedTarget(address) => (0, *address),
        })
    }
}

impl std::fmt::Display for Fault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bus(_, error) => error.fmt(f),
            Self::Illegal(insn) => write!(f, "unsupported instruction {insn:#010x}"),
            Self::InvalidCsr(_, error) => f.write_str(error),
            Self::MisalignedTarget(address) => {
                write!(f, "misaligned instruction target {address:#010x}")
            }
        }
    }
}

fn sign(value: u32, bits: u32) -> u32 {
    (((value << (32 - bits)) as i32) >> (32 - bits)) as u32
}

fn multiply_divide(operation: u32, lhs: u32, rhs: u32) -> u32 {
    let signed_lhs = lhs as i32;
    let signed_rhs = rhs as i32;
    match operation {
        0 => lhs.wrapping_mul(rhs),
        1 => ((i64::from(signed_lhs) * i64::from(signed_rhs)) >> 32) as u32,
        2 => ((i64::from(signed_lhs) * i64::from(rhs)) >> 32) as u32,
        3 => ((u64::from(lhs) * u64::from(rhs)) >> 32) as u32,
        4 if rhs != 0 => signed_lhs.wrapping_div(signed_rhs) as u32,
        5 if rhs != 0 => lhs / rhs,
        4 | 5 => u32::MAX,
        6 if rhs != 0 => signed_lhs.wrapping_rem(signed_rhs) as u32,
        7 if rhs != 0 => lhs % rhs,
        6 | 7 => lhs,
        _ => unreachable!("funct3 is a three-bit field"),
    }
}

impl Machine {
    pub fn new(size: usize) -> Result<Self, String> {
        Ok(Self {
            bus: Bus::new(size)?,
            ..Self::without_default_ram()
        })
    }

    pub fn without_default_ram() -> Self {
        Self {
            bus: Bus::default(),
            primary: Hart {
                name: None,
                csr: Csr::default(),
                translations: TranslationCache::default(),
                thumb: None,
                pc: RAM_BASE,
                registers: [0; 32],
            },
            secondary: Vec::new(),
            next_hart: 0,
            elapsed_steps: 0,
            cpu_paused: false,
            cpu_halted: false,
            cpu_clock_stopped: false,
            reset_log: None,
            uart_step_prefix: false,
            uart_line_start: true,
            reset_steps: VecDeque::new(),
            uart_input: VecDeque::new(),
            button_events: VecDeque::new(),
            adc_events: VecDeque::new(),
            adxl_events: VecDeque::new(),
        }
    }

    pub fn configure_harts(&mut self, count: usize) {
        self.secondary = (1..count)
            .map(|id| {
                let mut hart = self.primary.clone();
                hart.csr.hart_id = id;
                hart.name = None;
                hart
            })
            .collect();
        self.next_hart = 0;
    }

    pub fn hart_count(&self) -> usize {
        1 + self.secondary.len()
    }

    pub fn hart_mut(&mut self, id: usize) -> &mut Hart {
        if id == 0 {
            &mut self.primary
        } else {
            &mut self.secondary[id - 1]
        }
    }

    pub fn set_named_entry(&mut self, name: &str, entry: u32) -> Result<(), String> {
        let hart = std::iter::once(&mut self.primary)
            .chain(&mut self.secondary)
            .find(|hart| hart.name.as_deref() == Some(name))
            .ok_or_else(|| format!("unknown CPU {name:?}"))?;
        hart.pc = entry;
        Ok(())
    }

    pub fn set_cortex_m_monitor_pc(
        &mut self,
        name: &str,
        entry: u32,
        output: &mut impl Write,
    ) -> Result<(), String> {
        if self.primary.name.as_deref() != Some(name) {
            return Err(format!("unknown CPU {name:?}"));
        }
        let thumb = self
            .primary
            .thumb
            .as_mut()
            .ok_or("expected a Cortex-M CPU")?;
        self.primary.pc = thumb.patch_monitor_pc(entry, name, output)?;
        Ok(())
    }

    pub fn set_entry(&mut self, entry: u32) {
        for hart in std::iter::once(&mut self.primary).chain(&mut self.secondary) {
            hart.pc = entry;
        }
    }

    pub fn raw_load_address(&self) -> u32 {
        self.primary
            .thumb
            .as_ref()
            .map_or(RAM_BASE, Thumb::vector_base)
    }

    pub fn infer_cortex_m_vectors(&mut self, address: u32) {
        if let Some(thumb) = self.primary.thumb.as_mut() {
            thumb.infer_vector_base(address);
        }
    }

    pub fn is_cortex_m(&self) -> bool {
        self.primary.thumb.is_some()
    }

    pub fn has_cortex_m4(&self) -> bool {
        self.primary
            .thumb
            .as_ref()
            .is_some_and(|thumb| thumb.thumb2)
    }

    pub fn validate_entry(&mut self) -> Result<(), String> {
        for hart in std::iter::once(&mut self.primary).chain(&mut self.secondary) {
            if hart.thumb.is_some() {
                hart.reset_cortex_m(&mut self.bus)?;
                continue;
            }
            hart.fetch(&mut self.bus)
                .map_err(|error| format!("hart {}: {error}", hart.csr.hart_id))?;
        }
        Ok(())
    }

    pub fn schedule_reset(&mut self, step: u64) -> Result<(), String> {
        if !self.is_cortex_m() {
            return Err("--reset-at requires a Cortex-M platform".into());
        }
        let index = self
            .reset_steps
            .partition_point(|&scheduled| scheduled < step);
        if self.reset_steps.get(index) != Some(&step) {
            self.reset_steps.insert(index, step);
        }
        Ok(())
    }

    pub fn schedule_uart_input(&mut self, step: u64, bytes: Vec<u8>) -> Result<(), String> {
        if !self.bus.has_uart_input() {
            return Err("UART input requires a platform UART".into());
        }
        let index = self
            .uart_input
            .partition_point(|(scheduled, _)| *scheduled <= step);
        self.uart_input.insert(index, (step, bytes));
        Ok(())
    }

    pub fn schedule_button(&mut self, step: u64, name: &str, pressed: bool) -> Result<(), String> {
        let button = self
            .bus
            .board_button(name)
            .ok_or_else(|| format!("unknown board button {name:?}"))?;
        let index = self
            .button_events
            .partition_point(|(scheduled, _, _)| *scheduled <= step);
        self.button_events.insert(index, (step, button, pressed));
        Ok(())
    }

    pub fn schedule_adxl_input(
        &mut self,
        step: u64,
        name: String,
        acceleration: [i16; 3],
    ) -> Result<(), String> {
        if self.bus.nrf_adxl_input(&name).is_none() {
            return Err(format!("unknown nRF52840 ADXL372 {name:?}"));
        }
        let index = self
            .adxl_events
            .partition_point(|(scheduled, _, _)| *scheduled <= step);
        self.adxl_events.insert(index, (step, name, acceleration));
        Ok(())
    }

    pub fn schedule_adc_input(
        &mut self,
        step: u64,
        channel: usize,
        volts: f64,
    ) -> Result<(), String> {
        self.bus.validate_stm32f0_adc_input(channel, volts)?;
        let index = self
            .adc_events
            .partition_point(|(scheduled, _, _)| *scheduled <= step);
        self.adc_events.insert(index, (step, channel, volts));
        Ok(())
    }

    pub fn report_uart_steps(&mut self, enabled: bool) {
        self.uart_step_prefix = enabled;
    }

    pub fn pause_cpu(&mut self) {
        self.cpu_paused = true;
    }

    pub fn resume_cpu(&mut self) {
        self.cpu_paused = false;
    }

    pub fn set_cpu_halted(&mut self, halted: bool) {
        self.cpu_halted = halted;
    }

    pub fn set_cpu_clocked(&mut self, clocked: bool) {
        self.cpu_clock_stopped = !clocked;
    }

    pub fn reset_cpu_control(
        &mut self,
        kind: CpuResetKind,
        output: &mut impl Write,
    ) -> Result<(), String> {
        if !self.is_cortex_m() {
            return Err("CPU reset control requires a Cortex-M CPU".into());
        }
        if kind == CpuResetKind::Machine {
            self.bus.reset_cortex_m_if_requested(true);
        }
        self.primary.reset_cortex_m(&mut self.bus)?;
        self.cpu_halted = false;
        self.cpu_paused = kind == CpuResetKind::Cpu;
        if kind == CpuResetKind::Machine
            && let Some(message) = self.reset_log.as_deref()
        {
            writeln!(output, "{message}").map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    // Monitor steps count completed instructions. A precise fault redirects the
    // CPU without completing the access, so stepping continues in the handler.
    pub fn monitor_step_cortex_m(
        &mut self,
        count: u64,
        output: &mut impl Write,
    ) -> Result<(), String> {
        if !self.is_cortex_m() || count == 0 {
            return Err("Step requires a Cortex-M CPU and a positive instruction count".into());
        }
        self.resume_cpu();
        let result = (|| {
            let mut remaining = count;
            let mut missed = 0;
            while remaining != 0 {
                match self.run(1, output)? {
                    (Stop::Limit, 1) => {
                        remaining -= 1;
                        missed = 0;
                    }
                    (Stop::Limit, 0) => {
                        missed += 1;
                        if missed == 256 || self.cpu_halted || self.cpu_clock_stopped {
                            return Err("Cortex-M Step could not retire an instruction".into());
                        }
                    }
                    (Stop::Paused | Stop::Ebreak, _) => break,
                    (stop, _) => return Err(format!("Cortex-M Step stopped with {stop:?}")),
                }
            }
            Ok(())
        })();
        self.pause_cpu();
        result
    }

    pub fn run(&mut self, limit: u64, output: &mut impl Write) -> Result<(Stop, u64), String> {
        self.run_steps(limit, output, None)
    }

    fn run_steps(
        &mut self,
        limit: u64,
        output: &mut impl Write,
        mut radio_outbox: Option<&mut Vec<crate::nrf52840::AirFrame>>,
    ) -> Result<(Stop, u64), String> {
        if self.cpu_paused {
            return Ok((Stop::Paused, 0));
        }
        let mut retired = 0;
        for _ in 0..limit {
            let index = self.next_hart;
            if index == self.secondary.len() {
                self.next_hart = 0;
            } else {
                self.next_hart += 1;
            }
            let hart = if index == 0 {
                &mut self.primary
            } else {
                &mut self.secondary[index - 1]
            };
            self.elapsed_steps = self.elapsed_steps.wrapping_add(1);
            if self
                .reset_steps
                .front()
                .is_some_and(|&step| step <= self.elapsed_steps)
            {
                while self
                    .reset_steps
                    .front()
                    .is_some_and(|&step| step <= self.elapsed_steps)
                {
                    self.reset_steps.pop_front();
                }
                self.bus.reset_cortex_m_if_requested(true);
                hart.reset_cortex_m(&mut self.bus)?;
            }
            while self
                .uart_input
                .front()
                .is_some_and(|(step, _)| *step <= self.elapsed_steps)
            {
                let (_, bytes) = self.uart_input.pop_front().unwrap();
                self.bus
                    .queue_uart_input(bytes)
                    .map_err(|e| e.to_string())?;
            }
            while self
                .button_events
                .front()
                .is_some_and(|(step, _, _)| *step <= self.elapsed_steps)
            {
                let (_, button, pressed) = self.button_events.pop_front().unwrap();
                self.bus.set_button(button, pressed);
            }
            while self
                .adc_events
                .front()
                .is_some_and(|(step, _, _)| *step <= self.elapsed_steps)
            {
                let (_, channel, volts) = self.adc_events.pop_front().unwrap();
                self.bus.set_stm32f0_adc_input(channel, volts)?;
            }
            while self
                .adxl_events
                .front()
                .is_some_and(|(step, _, _)| *step <= self.elapsed_steps)
            {
                let (_, name, acceleration) = self.adxl_events.pop_front().unwrap();
                *self.bus.nrf_adxl_input(&name).unwrap() = acceleration;
            }
            let halted = index == 0 && (self.cpu_halted || self.cpu_clock_stopped);
            let result = if self.uart_step_prefix {
                let mut stamped = StepUart {
                    output,
                    step: self.elapsed_steps,
                    line_start: &mut self.uart_line_start,
                };
                hart.execute_step(&mut self.bus, self.elapsed_steps, halted, &mut stamped)
            } else {
                hart.execute_step(&mut self.bus, self.elapsed_steps, halted, output)
            };
            if let Some(outbox) = radio_outbox.as_deref_mut()
                && let Some(frame) = self.bus.tick_radio_air()
            {
                outbox.push(frame);
            }
            match result? {
                Step::Retired => retired += 1,
                Step::RetiredAndPaused => {
                    self.pause_cpu();
                    return Ok((Stop::Paused, retired + 1));
                }
                Step::Trap => {}
                Step::Stop(stop) => {
                    if stop == Stop::Paused {
                        self.pause_cpu();
                    }
                    return Ok((stop, retired));
                }
            }
        }
        Ok((Stop::Limit, retired))
    }
}

struct StepUart<'a, W> {
    output: &'a mut W,
    step: u64,
    line_start: &'a mut bool,
}

impl<W: Write> Write for StepUart<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        for line in bytes.split_inclusive(|&byte| byte == b'\n') {
            if *self.line_start {
                write!(self.output, "[step {}] ", self.step)?;
            }
            self.output.write_all(line)?;
            *self.line_start = line.last() == Some(&b'\n');
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.output.flush()
    }
}

impl Hart {
    fn reset_cortex_m(&mut self, bus: &mut Bus) -> Result<(), String> {
        let thumb = self.thumb.as_mut().unwrap();
        let vectors = thumb.vector_base();
        let thumb2 = thumb.thumb2;
        let mut reset_thumb = Thumb::new(Some(vectors));
        reset_thumb.thumb2 = thumb2;
        let reset_context = reset_thumb.reset(bus)?;
        *thumb = reset_thumb;
        if let Some(nvic) = bus.nvic.as_mut() {
            nvic.set_vector_base(vectors);
            nvic.lockup = false;
            nvic.set_active_exceptions(0);
        }
        self.registers.fill(0);
        if let Some((sp, pc)) = reset_context {
            self.registers[13] = sp;
            self.pc = pc;
        } else {
            thumb.enter_reset_lockup();
            if let Some(nvic) = bus.nvic.as_mut() {
                nvic.record_reset_vector_fault();
            }
            self.pc = crate::thumb::LOCKUP_PC;
            self.registers[14] = u32::MAX;
        }
        Ok(())
    }

    pub fn configure_cortex_m(&mut self, vectors: Option<u32>, thumb2: bool) {
        let mut thumb = Thumb::new(vectors);
        thumb.thumb2 = thumb2;
        self.thumb = Some(thumb);
    }

    pub fn set_integer_extensions(&mut self, multiply: bool, atomic: bool, compressed: bool) {
        self.csr.m_extension = multiply;
        self.csr.a_extension = atomic;
        self.csr.c_extension = compressed;
    }

    pub fn enable_vex_irq_controller(&mut self) {
        self.csr.vex_irq_mask = Some(0);
    }

    pub fn set_vex_irq_mask(&mut self, value: u32) -> Result<(), String> {
        let mask = self
            .csr
            .vex_irq_mask
            .as_mut()
            .ok_or("--vex-irq-mask requires a VexRiscv platform")?;
        *mask = value;
        Ok(())
    }

    pub fn enable_user_mode(&mut self) {
        self.csr.enable_user_mode();
    }

    pub fn enable_supervisor_mode(&mut self) {
        self.csr.enable_supervisor_mode();
    }

    pub fn enable_pmp(&mut self) {
        self.csr.enable_pmp();
    }

    fn translate(
        &mut self,
        bus: &mut Bus,
        address: u32,
        width: usize,
        permissions: u8,
    ) -> Result<u32, BusError> {
        if address as usize & (width - 1) != 0 && permissions != crate::pmp::EXECUTE {
            return Err(BusError::Misaligned(address, width));
        }
        let physical = self
            .translations
            .translate(bus, &self.csr, address, permissions)?;
        self.csr
            .permits(
                physical,
                width,
                permissions,
                permissions == crate::pmp::EXECUTE,
            )
            .then_some(physical)
            .ok_or(BusError::Denied(address))
    }

    fn load<const N: usize>(&mut self, bus: &mut Bus, address: u32) -> Result<u32, Fault> {
        self.translate(bus, address, N, crate::pmp::READ)
            .and_then(|physical| {
                bus.read::<N>(physical)
                    .map_err(|error| error.at_virtual(address))
            })
            .map_err(|error| Fault::Bus(Access::Load, error))
    }

    pub fn set_mtvec(&mut self, value: u32) {
        self.csr.set_vector(value);
    }

    pub fn compressed_instructions(&self) -> bool {
        self.csr.c_extension
    }

    fn fetch(&mut self, bus: &mut Bus) -> Result<(u32, u32), BusError> {
        let pc = self.pc;
        let minimum = if self.csr.c_extension { 2 } else { 4 };
        if pc as usize & (minimum - 1) != 0 {
            return Err(BusError::Misaligned(pc, minimum));
        }
        let physical = self.translate(bus, pc, minimum, crate::pmp::EXECUTE)?;
        if minimum == 4 {
            return bus
                .read_ram::<4>(physical)
                .map(|word| (word, 4))
                .map_err(|error| error.at_virtual(pc));
        }
        let low = bus
            .read_ram::<2>(physical)
            .map_err(|error| error.at_virtual(pc))?;
        if low & 3 != 3 {
            return Ok((low, 2));
        }
        let upper = pc.wrapping_add(2);
        let high_physical = if pc & 0xfff == 0xffe {
            self.translate(bus, upper, 2, crate::pmp::EXECUTE)?
        } else {
            // The second parcel uses the same page; check PMP for the entire access.
            if !self.csr.permits(physical, 4, crate::pmp::EXECUTE, true) {
                return Err(BusError::Denied(pc));
            }
            physical + 2
        };
        let high = bus
            .read_ram::<2>(high_physical)
            .map_err(|error| error.at_virtual(upper))?;
        Ok((low | (high << 16), 4))
    }

    fn execute_step(
        &mut self,
        bus: &mut Bus,
        elapsed_steps: u64,
        halted: bool,
        output: &mut impl Write,
    ) -> Result<Step, String> {
        if self.thumb.is_some() && bus.reset_cortex_m_if_requested(false) {
            self.reset_cortex_m(bus)?;
        }
        if let Some(thumb) = self.thumb.as_mut() {
            let clock_ticks = bus.nrf_clock_ticks();
            for _ in 0..clock_ticks {
                bus.advance_nrf_audio()
                    .and_then(|()| bus.advance_nrf_ppi(output))
                    .and_then(|()| bus.advance_stm32_dma(output))
                    .map_err(|error| format!("pc={:#010x}: {error}", self.pc))?;
                thumb.clock_interrupts(bus);
                if let Some(dwt) = bus.dwt.as_mut() {
                    dwt.tick();
                }
            }
            if clock_ticks != 1 && bus.reset_cortex_m_if_requested(false) {
                self.reset_cortex_m(bus)?;
                return Ok(Step::Trap);
            }
            if halted {
                return Ok(Step::Trap);
            }
            if thumb
                .dispatch_interrupts(&mut self.pc, &mut self.registers, bus)
                .map_err(|error| format!("pc={:#010x}: {error}", self.pc))?
            {
                return Ok(Step::Trap);
            }
            if thumb.sleeping() {
                return Ok(Step::Trap);
            }
            if bus
                .apply_cpu_hooks(self.pc, output)
                .map_err(|error| error.to_string())?
            {
                return Ok(Step::Stop(Stop::Paused));
            }
            bus.thumb_read_watchpoint_hit = false;
            return thumb
                .step(&mut self.pc, &mut self.registers, bus, output)
                .map(|step| match step {
                    ThumbStep::Retired if bus.thumb_read_watchpoint_hit => Step::RetiredAndPaused,
                    ThumbStep::Retired => Step::Retired,
                    ThumbStep::Fault => Step::Trap,
                    ThumbStep::Semihosting(exit) => Step::Stop(Stop::Semihosting(exit)),
                    ThumbStep::Breakpoint => Step::Stop(Stop::Ebreak),
                })
                .map_err(|error| format!("pc={:#010x}: {error}", self.pc));
        }
        let clint = bus
            .clint
            .as_mut()
            .map_or(0, |clint| clint.tick(self.csr.hart_id));
        let count_retired = self.csr.tick_counters(
            bus.clint
                .as_ref()
                .map_or(elapsed_steps, |clint| clint.time()),
        );
        let uart = bus
            .uart
            .as_ref()
            .is_some_and(|uart| uart.interrupt_pending());
        let timer = bus.timer.as_mut().is_some_and(|(_, timer)| timer.tick());
        let plic = bus.tick_plic();
        let mut pending = clint;
        if self.csr.hart_id == 0 {
            let lines = (u32::from(uart) * EXTERNAL) | (u32::from(timer) * crate::clint::TIMER);
            let external = if let Some(mask) = self.csr.vex_irq_mask {
                self.csr.vex_irq_pending = lines;
                u32::from(lines & mask != 0) * EXTERNAL
            } else {
                lines
            };
            pending |= plic | external;
        }
        if (bus.clint.is_some()
            || bus.uart.is_some()
            || bus.timer.is_some()
            || bus.has_plic()
            || self.csr.has_injected_interrupts())
            && let Some(target) = self.csr.interrupt(self.pc, pending)
        {
            self.pc = target;
            return Ok(Step::Trap);
        }
        match self.step(bus, output) {
            Ok(Step::Retired) => {
                self.csr.retire(count_retired);
                Ok(Step::Retired)
            }
            Ok(step) => Ok(step),
            Err(fault) => {
                if let Some(target) = fault
                    .exception()
                    .and_then(|(cause, value)| self.csr.trap(self.pc, cause, value))
                {
                    self.pc = target;
                    Ok(Step::Trap)
                } else {
                    Err(format!(
                        "pc={:#010x}: {fault} (hart {})",
                        self.pc, self.csr.hart_id
                    ))
                }
            }
        }
    }

    fn step(&mut self, bus: &mut Bus, output: &mut impl Write) -> Result<Step, Fault> {
        let pc = self.pc;
        let (encoded, width) = self
            .fetch(bus)
            .map_err(|error| Fault::Bus(Access::Fetch, error))?;
        let illegal = || Fault::Illegal(encoded);
        let insn = if width == 2 {
            crate::compressed::expand(encoded).ok_or_else(illegal)?
        } else {
            encoded
        };
        let rd = ((insn >> 7) & 31) as usize;
        let a = self.registers[((insn >> 15) & 31) as usize];
        let b = self.registers[((insn >> 20) & 31) as usize];
        let funct3 = (insn >> 12) & 7;
        let funct7 = insn >> 25;
        let imm = sign(insn >> 20, 12);
        let fallthrough = pc.wrapping_add(width);
        let mut next = fallthrough;

        let result = match insn & 0x7f {
            0x37 => Some(insn & 0xffff_f000),
            0x17 => Some(pc.wrapping_add(insn & 0xffff_f000)),
            0x6f => {
                let offset = ((insn >> 31) << 20)
                    | (insn & 0xff000)
                    | ((insn >> 9) & 0x800)
                    | ((insn >> 20) & 0x7fe);
                next = pc.wrapping_add(sign(offset, 21));
                Some(fallthrough)
            }
            0x67 if funct3 == 0 => {
                next = a.wrapping_add(imm) & !1;
                Some(fallthrough)
            }
            0x63 => {
                let taken = match funct3 {
                    0 => a == b,
                    1 => a != b,
                    4 => (a as i32) < (b as i32),
                    5 => (a as i32) >= (b as i32),
                    6 => a < b,
                    7 => a >= b,
                    _ => return Err(illegal()),
                };
                if taken {
                    let offset = ((insn >> 31) << 12)
                        | ((insn << 4) & 0x800)
                        | ((insn >> 20) & 0x7e0)
                        | ((insn >> 7) & 0x1e);
                    next = pc.wrapping_add(sign(offset, 13));
                }
                None
            }
            0x03 => {
                let address = a.wrapping_add(imm);
                Some(match funct3 {
                    0 => sign(self.load::<1>(bus, address)?, 8),
                    1 => sign(self.load::<2>(bus, address)?, 16),
                    2 => self.load::<4>(bus, address)?,
                    4 => self.load::<1>(bus, address)?,
                    5 => self.load::<2>(bus, address)?,
                    _ => return Err(illegal()),
                })
            }
            0x23 => {
                let offset = ((insn >> 25) << 5) | ((insn >> 7) & 31);
                let address = a.wrapping_add(sign(offset, 12));
                let width = match funct3 {
                    0 => 1,
                    1 => 2,
                    2 => 4,
                    _ => return Err(illegal()),
                };
                self.translate(bus, address, width, crate::pmp::WRITE)
                    .and_then(|physical| {
                        bus.write(physical, width, b, output)
                            .map_err(|error| error.at_virtual(address))
                    })
                    .map_err(|error| Fault::Bus(Access::Store, error))?;
                None
            }
            0x13 => Some(match funct3 {
                0 => a.wrapping_add(imm),
                1 if funct7 == 0 => a << (imm & 31),
                2 => u32::from((a as i32) < (imm as i32)),
                3 => u32::from(a < imm),
                4 => a ^ imm,
                5 if funct7 == 0 => a >> (imm & 31),
                5 if funct7 == 0x20 => ((a as i32) >> (imm & 31)) as u32,
                6 => a | imm,
                7 => a & imm,
                _ => return Err(illegal()),
            }),
            0x33 => Some(match (funct3, funct7) {
                (_, 1) if self.csr.m_extension => multiply_divide(funct3, a, b),
                (0, 0) => a.wrapping_add(b),
                (0, 0x20) => a.wrapping_sub(b),
                (1, 0) => a << (b & 31),
                (2, 0) => u32::from((a as i32) < (b as i32)),
                (3, 0) => u32::from(a < b),
                (4, 0) => a ^ b,
                (5, 0) => a >> (b & 31),
                (5, 0x20) => ((a as i32) >> (b & 31)) as u32,
                (6, 0) => a | b,
                (7, 0) => a & b,
                _ => return Err(illegal()),
            }),
            0x2f if funct3 == 2 && self.csr.a_extension => {
                let operation = insn >> 27;
                let rs2 = (insn >> 20) & 31;
                let (access, permissions) = match operation {
                    2 if rs2 == 0 => (Access::Load, crate::pmp::READ),
                    3 => (Access::Store, crate::pmp::WRITE),
                    0 | 1 | 4 | 8 | 12 | 16 | 20 | 24 | 28 => {
                        (Access::Store, crate::pmp::READ | crate::pmp::WRITE)
                    }
                    _ => return Err(illegal()),
                };
                let physical = self
                    .translate(bus, a, 4, permissions)
                    .map_err(|error| Fault::Bus(access, error))?;
                let value = match operation {
                    2 if rs2 == 0 => bus
                        .load_reserved(self.csr.hart_id, physical)
                        .map_err(|error| Fault::Bus(Access::Load, error.at_virtual(a))),
                    3 => bus
                        .store_conditional(self.csr.hart_id, physical, b)
                        .map_err(|error| Fault::Bus(Access::Store, error.at_virtual(a))),
                    0 | 1 | 4 | 8 | 12 | 16 | 20 | 24 | 28 => bus
                        .atomic_update(physical, |previous| match operation {
                            0 => previous.wrapping_add(b),
                            1 => b,
                            4 => previous ^ b,
                            8 => previous | b,
                            12 => previous & b,
                            16 => (previous as i32).min(b as i32) as u32,
                            20 => (previous as i32).max(b as i32) as u32,
                            24 => previous.min(b),
                            28 => previous.max(b),
                            _ => unreachable!(),
                        })
                        .map_err(|error| Fault::Bus(Access::Store, error.at_virtual(a))),
                    _ => return Err(illegal()),
                };
                Some(value?)
            }
            // Every instruction fetch reads current RAM, so FENCE.I needs no invalidation.
            0x0f if funct3 <= 1 => None,
            0x73 if insn == 0x0000_0073 => {
                return Ok(self.environment_trap(self.csr.ecall_cause(), Stop::Ecall));
            }
            0x73 if insn == 0x0010_0073 => return Ok(self.environment_trap(3, Stop::Ebreak)),
            0x73 if insn & 0xfe00_7fff == 0x1200_0073 && self.csr.can_sfence() => {
                self.translations.clear();
                None
            }
            0x73 if insn == 0x1050_0073 && !self.csr.in_user_mode() => None, // WFI may resume immediately.
            0x73 if insn == 0x3020_0073 => {
                next = self.csr.return_from_trap().ok_or_else(illegal)?;
                None
            }
            0x73 if insn == 0x1020_0073 => {
                next = self.csr.return_from_supervisor_trap().ok_or_else(illegal)?;
                None
            }
            0x73 if matches!(funct3, 1..=3 | 5..=7) => {
                let value = self
                    .csr
                    .access(insn, a)
                    .map_err(|error| Fault::InvalidCsr(insn, error))?;
                if matches!(insn >> 20, 0x180 | 0x3a0..=0x3a3 | 0x3b0..=0x3bf) {
                    self.translations.clear();
                }
                Some(value)
            }
            _ => return Err(illegal()),
        };

        if next & if self.csr.c_extension { 1 } else { 3 } != 0 {
            return Err(Fault::MisalignedTarget(next));
        }
        if let Some(value) = result.filter(|_| rd != 0) {
            self.registers[rd] = value;
        }
        self.pc = next;
        Ok(Step::Retired)
    }

    fn environment_trap(&mut self, cause: u32, stop: Stop) -> Step {
        match self.csr.trap(self.pc, cause, 0) {
            Some(target) => {
                self.pc = target;
                Step::Trap
            }
            None => Step::Stop(stop),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn started_cortex_m() -> Machine {
        let mut machine = Machine::new(0x1000).unwrap();
        machine.bus.map_ram(0, 0x1000).unwrap();
        machine.bus.map_nvic(0xe0, None, true).unwrap();
        machine.bus.map_systick().unwrap();
        machine.primary.configure_cortex_m(Some(0), true);
        let mut output = Vec::new();
        for (address, value) in [(0, 0x800), (4, 0x101)] {
            machine.bus.write(address, 4, value, &mut output).unwrap();
        }
        // nop; nop; nop; jump: nop; b jump.
        for (address, value) in [
            (0x100, 0xbf00),
            (0x102, 0xbf00),
            (0x104, 0xbf00),
            (0x106, 0xbf00),
            (0x108, 0xe7fd),
        ] {
            machine.bus.write(address, 2, value, &mut output).unwrap();
        }
        machine
            .bus
            .cpu_hooks
            .insert(0x104, vec![crate::bus::CpuHook::Log("PC moved".into())]);
        machine.validate_entry().unwrap();
        assert_eq!(machine.run(3, &mut output).unwrap(), (Stop::Limit, 3));
        assert_eq!(output, b"PC moved\n");
        machine
    }

    fn clock_test_setup() -> (Machine, Vec<u8>) {
        let mut machine = started_cortex_m();
        let mut output = Vec::new();
        machine.bus.map_dwt(0xe0001000, 72_000_000).unwrap();
        machine
            .bus
            .systick
            .as_mut()
            .unwrap()
            .set_frequency(100_000_000);
        for (address, value) in [(0xe000e014, 0xff_ffff), (0xe000e010, 1), (0xe0001000, 1)] {
            machine.bus.write(address, 4, value, &mut output).unwrap();
        }
        (machine, output)
    }

    fn clock_test_values(machine: &mut Machine) -> (u32, u32) {
        (
            machine.bus.read::<4>(0xe000e018).unwrap(),
            machine.bus.read::<4>(0xe0001004).unwrap(),
        )
    }

    #[test]
    fn upstream_cortex_m_independent_systick_dwt_clocks() {
        let (mut machine, mut output) = clock_test_setup();
        machine.run(100_000, &mut output).unwrap();
        let before = clock_test_values(&mut machine);
        machine.run(100_000, &mut output).unwrap();
        let running = clock_test_values(&mut machine);
        assert_ne!(before.0, running.0);
        assert_eq!(running.1 - before.1, 72_000);

        machine.bus.systick.as_mut().unwrap().set_clocked(false);
        machine.bus.dwt.as_mut().unwrap().set_clocked(false);
        machine.run(100_000, &mut output).unwrap();
        assert_eq!(clock_test_values(&mut machine), running);

        machine.bus.systick.as_mut().unwrap().set_clocked(true);
        machine.bus.dwt.as_mut().unwrap().set_clocked(true);
        machine.run(100_000, &mut output).unwrap();
        let resumed = clock_test_values(&mut machine);
        assert_ne!(resumed.0, running.0);
        assert_eq!(resumed.1 - running.1, 72_000);

        machine.bus.systick.as_mut().unwrap().set_clocked(false);
        machine.run(100_000, &mut output).unwrap();
        let independent = clock_test_values(&mut machine);
        assert_eq!(independent.0, resumed.0);
        assert_eq!(independent.1 - resumed.1, 72_000);
    }

    #[test]
    fn upstream_cortex_m_clock_gate_persists_across_reset() {
        let (mut machine, mut output) = clock_test_setup();
        machine.run(100_000, &mut output).unwrap();
        machine.set_cpu_clocked(false);
        machine.bus.systick.as_mut().unwrap().set_clocked(false);
        machine.bus.dwt.as_mut().unwrap().set_clocked(false);
        machine
            .reset_cpu_control(CpuResetKind::Machine, &mut output)
            .unwrap();
        assert_eq!(clock_test_values(&mut machine), (0xff_ffff, 0));
        assert_eq!(machine.run(100_000, &mut output).unwrap(), (Stop::Limit, 0));
        assert_eq!(machine.primary.pc, 0x100);
        assert_eq!(clock_test_values(&mut machine), (0xff_ffff, 0));

        for (address, value) in [(0xe000e014, 0xff_ffff), (0xe000e010, 1), (0xe0001000, 1)] {
            machine.bus.write(address, 4, value, &mut output).unwrap();
        }
        machine.run(100_000, &mut output).unwrap();
        assert_eq!(clock_test_values(&mut machine), (0xff_ffff, 0));

        machine.set_cpu_clocked(true);
        machine.bus.systick.as_mut().unwrap().set_clocked(true);
        machine.bus.dwt.as_mut().unwrap().set_clocked(true);
        assert_eq!(
            machine.run(100_000, &mut output).unwrap(),
            (Stop::Limit, 100_000)
        );
        let resumed = clock_test_values(&mut machine);
        assert_ne!(resumed.0, 0xff_ffff);
        assert_eq!(resumed.1, 72_000);
    }

    fn check_cpu_flow(machine: &mut Machine, paused: bool, halted: bool) {
        assert_eq!(machine.cpu_paused, paused);
        assert_eq!(machine.cpu_halted, halted);
        let before = machine.elapsed_steps;
        let pc = machine.primary.pc;
        let (stop, retired) = machine.run(4, &mut std::io::sink()).unwrap();
        assert_eq!(machine.elapsed_steps, before + if paused { 0 } else { 4 });
        assert_eq!(stop, if paused { Stop::Paused } else { Stop::Limit });
        assert_eq!(retired, if paused || halted { 0 } else { 4 });
        if paused || halted {
            assert_eq!(machine.primary.pc, pc);
        }
    }

    #[test]
    fn upstream_cpu_control_starts() {
        check_cpu_flow(&mut started_cortex_m(), false, false);
    }

    #[test]
    fn upstream_cpu_control_pauses() {
        let mut machine = started_cortex_m();
        machine.pause_cpu();
        check_cpu_flow(&mut machine, true, false);
        machine.resume_cpu();
        check_cpu_flow(&mut machine, false, false);
    }

    #[test]
    fn upstream_cpu_control_halts() {
        let mut machine = started_cortex_m();
        machine.set_cpu_halted(true);
        check_cpu_flow(&mut machine, false, true);
        machine.resume_cpu();
        check_cpu_flow(&mut machine, false, true);
    }

    fn check_cpu_control_reset(paused: bool, halted: bool, kind: CpuResetKind) {
        let mut machine = started_cortex_m();
        if paused {
            machine.pause_cpu();
        }
        machine.set_cpu_halted(halted);
        machine.reset_log = Some("Machine resetting".into());
        let mut output = Vec::new();
        machine.reset_cpu_control(kind, &mut output).unwrap();
        assert_eq!(machine.primary.pc, 0x100);
        assert!(!machine.cpu_halted);
        match kind {
            CpuResetKind::Machine => {
                assert!(!machine.cpu_paused);
                assert_eq!(machine.run(3, &mut output).unwrap(), (Stop::Limit, 3));
                assert_eq!(output, b"Machine resetting\nPC moved\n");
                check_cpu_flow(&mut machine, false, false);
            }
            CpuResetKind::Cpu => {
                assert!(output.is_empty());
                check_cpu_flow(&mut machine, true, false);
            }
        }
    }

    #[test]
    fn upstream_cpu_reset_running_cpu() {
        check_cpu_control_reset(false, false, CpuResetKind::Cpu);
    }

    #[test]
    fn upstream_cpu_reset_running_machine() {
        check_cpu_control_reset(false, false, CpuResetKind::Machine);
    }

    #[test]
    fn upstream_cpu_reset_halted_cpu() {
        check_cpu_control_reset(false, true, CpuResetKind::Cpu);
    }

    #[test]
    fn upstream_cpu_reset_halted_machine() {
        check_cpu_control_reset(false, true, CpuResetKind::Machine);
    }

    #[test]
    fn upstream_cpu_reset_paused_cpu() {
        check_cpu_control_reset(true, false, CpuResetKind::Cpu);
    }

    #[test]
    fn upstream_cpu_reset_paused_machine() {
        check_cpu_control_reset(true, false, CpuResetKind::Machine);
    }

    #[test]
    fn upstream_cpu_reset_halted_paused_cpu() {
        check_cpu_control_reset(true, true, CpuResetKind::Cpu);
    }

    #[test]
    fn upstream_cpu_reset_halted_paused_machine() {
        check_cpu_control_reset(true, true, CpuResetKind::Machine);
    }

    fn check_pause_halt(pause_first: bool, resume_first: bool) {
        let mut machine = started_cortex_m();
        if pause_first {
            machine.pause_cpu();
            machine.set_cpu_halted(true);
        } else {
            machine.set_cpu_halted(true);
            machine.pause_cpu();
        }
        check_cpu_flow(&mut machine, true, true);
        if resume_first {
            machine.resume_cpu();
            check_cpu_flow(&mut machine, false, true);
            machine.set_cpu_halted(false);
        } else {
            machine.set_cpu_halted(false);
            check_cpu_flow(&mut machine, true, false);
            machine.resume_cpu();
        }
        check_cpu_flow(&mut machine, false, false);
    }

    #[test]
    fn upstream_cpu_control_pause_first_resume_first() {
        check_pause_halt(true, true);
    }

    #[test]
    fn upstream_cpu_control_pause_first_unhalt_first() {
        check_pause_halt(true, false);
    }

    #[test]
    fn upstream_cpu_control_halt_first_resume_first() {
        check_pause_halt(false, true);
    }

    #[test]
    fn upstream_cpu_control_halt_first_unhalt_first() {
        check_pause_halt(false, false);
    }

    #[test]
    fn upstream_invalid_reset_state_locks_on_second_step() {
        let mut machine = Machine::new(0x1000).unwrap();
        machine.bus.map_ram(0, 0x1000).unwrap();
        machine
            .bus
            .map_bus_faulting_peripheral(0x100000, None)
            .unwrap();
        machine.bus.map_nvic(0xe0, None, true).unwrap();
        machine.bus.map_systick().unwrap();
        machine.primary.configure_cortex_m(Some(0), true);
        machine.validate_entry().unwrap();
        let mut out = std::io::sink();

        assert_eq!(machine.run(1, &mut out).unwrap(), (Stop::Limit, 0));
        assert_eq!(machine.primary.pc, 0);
        assert_eq!(
            machine.primary.thumb.as_ref().unwrap().lockup_state(),
            (false, 0, 3)
        );
        assert!(!machine.bus.nvic.as_ref().unwrap().lockup);
        assert_eq!(machine.bus.read::<4>(0xe000_ed28).unwrap(), 1 << 17);
        assert_eq!(machine.bus.read::<4>(0xe000_ed2c).unwrap(), 1 << 30);

        assert_eq!(machine.run(1, &mut out).unwrap(), (Stop::Limit, 0));
        assert_eq!(machine.primary.pc, 0xefff_fffe);
        assert!(machine.primary.thumb.as_ref().unwrap().lockup_state().0);
        assert!(machine.bus.nvic.as_ref().unwrap().lockup);
        assert_eq!(machine.bus.read::<4>(0xe000_ed28).unwrap(), 1 << 17);
        assert_eq!(machine.bus.read::<4>(0xe000_ed2c).unwrap(), 1 << 30);
        let dhcsr = crate::nvic::DHCSR;
        assert_eq!(machine.bus.thumb_read::<4>(dhcsr).unwrap(), 0);
        assert_ne!(machine.bus.read::<4>(dhcsr).unwrap() & (1 << 19), 0);
        machine
            .bus
            .thumb_write(dhcsr, 4, u32::MAX, &mut out)
            .unwrap();
        assert_eq!(machine.bus.thumb_read::<4>(dhcsr).unwrap(), 0);
    }

    #[test]
    fn upstream_sets_and_clears_page_access_via_io() {
        const PC: u32 = 0x800000;
        const PAGES: [u32; 3] = [0x31000000, 0x32000000, 0x33000000];
        let mut machine = Machine::new(0x1000).unwrap();
        machine.bus.map_ram(PC, 0x40000).unwrap();
        machine.bus.map_nvic(0xe0, None, true).unwrap();
        machine.bus.map_systick().unwrap();
        machine.primary.configure_cortex_m(Some(PC), true);
        machine
            .bus
            .write(PC, 2, 0x6808, &mut std::io::sink())
            .unwrap();
        for (index, page) in PAGES.into_iter().enumerate() {
            let name = format!("test_mem{}", index + 1);
            machine.bus.map_named_ram(&name, page, 0x1000).unwrap();
            machine.bus.log_ram_access(&name).unwrap();
        }
        machine.primary.name = Some("cpu".into());
        let mut setup = Vec::new();
        machine
            .set_cortex_m_monitor_pc("cpu", PC, &mut setup)
            .unwrap();
        assert_eq!(
            String::from_utf8(setup).unwrap(),
            "cpu: Patching PC 0x800000 for Thumb mode.\n"
        );
        let phases: &[(&[(usize, bool)], u8)] = &[
            (&[], 0),
            (&[(0, true)], 1),
            (&[(1, true)], 3),
            (&[(2, true)], 7),
            (&[(0, false)], 6),
            (&[(1, false)], 4),
            (&[(2, false)], 0),
            (&[(1, true)], 2),
            (&[(0, true), (1, false), (2, true)], 5),
        ];
        for (changes, expected_pages) in phases {
            for &(index, enabled) in *changes {
                machine.bus.set_page_access_via_io(PAGES[index], enabled);
            }
            for (index, page) in PAGES.into_iter().enumerate() {
                machine.primary.registers[1] = page;
                machine
                    .set_cortex_m_monitor_pc("cpu", PC, &mut std::io::sink())
                    .unwrap();
                let mut output = Vec::new();
                assert_eq!(machine.run(1, &mut output).unwrap(), (Stop::Limit, 1));
                assert_eq!(machine.primary.registers[0], 0);
                if expected_pages & (1 << index) == 0 {
                    assert!(output.is_empty());
                } else {
                    assert_eq!(
                        String::from_utf8(output).unwrap(),
                        format!(
                            "test_mem{}: [cpu: 0x800000] ReadUInt32 from 0x0 (unknown), returned 0x0\n",
                            index + 1
                        )
                    );
                }
            }
        }
    }

    #[test]
    fn upstream_vtor_reads_privileged_vectors_without_trustzone() {
        let mut machine = Machine::new(0x1000).unwrap();
        machine.bus.map_privileged_ram(0x10000, 0x1000).unwrap();
        machine.bus.map_ram(0x20000, 0x1000).unwrap();
        machine.bus.map_nvic(0xe0, None, true).unwrap();
        machine.bus.map_systick().unwrap();
        machine.primary.configure_cortex_m(Some(0x10000), true);
        let mut output = std::io::sink();
        machine.bus.write(0x10000, 4, 0x20100, &mut output).unwrap();
        machine.bus.write(0x10004, 4, 0x20201, &mut output).unwrap();
        machine.bus.write(0x20200, 2, 0xe7fe, &mut output).unwrap();
        machine.validate_entry().unwrap();
        assert_eq!(machine.run(1, &mut output).unwrap(), (Stop::Limit, 1));
        assert_eq!(machine.primary.pc, 0x20200);
        assert_eq!(machine.primary.registers[13], 0x20100);

        machine.bus.thumb_unprivileged = true;
        assert!(
            machine
                .bus
                .thumb_read::<4>(0x10000)
                .unwrap_err()
                .contains("registration denied")
        );
        assert!(machine.bus.thumb_write(0x10004, 4, 0, &mut output).is_err());
        assert!(machine.bus.thumb_fetch(0x10000).is_err());
        assert_eq!(machine.bus.read::<4>(0x10004).unwrap(), 0x20201);
    }

    fn check_lazy_fp_fault(original_hard_fault: bool) {
        let mut machine = exception_vector_fault_machine(None);
        let mut out = std::io::sink();
        machine.bus.nvic.as_mut().unwrap().set_vector_base(0);
        for (address, value) in [(8, 0x381), (12, 0x341), (0xe000_ed88, 0xf00000)] {
            machine.bus.write(address, 4, value, &mut out).unwrap();
        }
        for (address, opcode) in [
            (0x340, 0xeeb0),
            (0x342, 0x0a40),
            (0x344, 0xe7fe),
            (0x380, 0xbf00),
            (0x382, 0xeeb0),
            (0x384, 0x0a40),
            (0x386, 0xe7fe),
        ] {
            machine.bus.write(address, 2, opcode, &mut out).unwrap();
        }
        if original_hard_fault {
            for (address, opcode) in [(0x200, 0x6802), (0x202, 0x2122), (0x204, 0xe7fe)] {
                machine.bus.write(address, 2, opcode, &mut out).unwrap();
            }
            machine.primary.registers[0] = 0x100000;
            machine.primary.registers[1] = 0x11;
            machine.primary.registers[2] = 0xa5a5_a5a5;
            assert_eq!(machine.run(1, &mut out).unwrap(), (Stop::Limit, 0));
            assert_eq!(machine.primary.pc, 0x340);
            machine
                .bus
                .write(0xe000_ed28, 4, u32::MAX, &mut out)
                .unwrap();
            machine
                .bus
                .write(0xe000_ed2c, 4, 1 << 30, &mut out)
                .unwrap();
        } else {
            machine.bus.write(0x200, 2, 0xeeb0, &mut out).unwrap();
            machine.bus.write(0x202, 2, 0x0a40, &mut out).unwrap();
            machine.bus.write(0x204, 2, 0xe7fe, &mut out).unwrap();
        }
        assert_eq!(machine.run(1, &mut out).unwrap(), (Stop::Limit, 1));
        machine.bus.set_mock_fault_offset(0x100000, 0x20);
        machine.primary.registers[13] = 0x100068;
        machine
            .bus
            .write(0xe000_ed04, 4, 1 << 31, &mut out)
            .unwrap();
        assert_eq!(machine.run(2, &mut out).unwrap(), (Stop::Limit, 1));
        assert_eq!(machine.primary.pc, 0x382);
        assert_eq!(machine.bus.read::<4>(0xe000_ef38).unwrap(), 0x100020);
        assert_eq!(
            machine.bus.read::<4>(0xe000_ef34).unwrap() & 0x11,
            if original_hard_fault { 1 } else { 0x11 }
        );
        machine.run(1, &mut out).unwrap();
        assert_eq!(machine.bus.read::<4>(0xe000_ed28).unwrap(), 1 << 13);
        let thumb = machine.primary.thumb.as_ref().unwrap();
        assert_eq!(thumb.lockup_state().0, original_hard_fault);
        assert_eq!(
            machine.bus.read::<4>(0xe000_ef34).unwrap() & 1,
            u32::from(original_hard_fault)
        );
        if original_hard_fault {
            assert_eq!(machine.primary.pc, 0xefff_fffe);
            assert_eq!(machine.primary.registers[13], 0x100000);
            assert_eq!(thumb.lockup_state().2, 2);
            assert!(machine.bus.nvic.as_ref().unwrap().lockup);
            let dhcsr = crate::nvic::DHCSR;
            assert_eq!(machine.bus.thumb_read::<4>(dhcsr).unwrap(), 0);
            assert_ne!(machine.bus.read::<4>(dhcsr).unwrap() & (1 << 19), 0);
            machine
                .bus
                .thumb_write(dhcsr, 4, u32::MAX, &mut out)
                .unwrap();
            assert_eq!(machine.bus.thumb_read::<4>(dhcsr).unwrap(), 0);
            assert_eq!(machine.bus.read::<4>(0xe000_ed2c).unwrap(), 0);
            assert_eq!(
                machine.bus.read::<4>(0xe000_ed24).unwrap(),
                (1 << 2) | (1 << 5)
            );
        } else {
            assert_eq!(machine.bus.read::<4>(0xe000_ed2c).unwrap(), 1 << 30);
            assert_ne!(machine.bus.read::<4>(0xe000_ed24).unwrap() & (1 << 21), 0);
        }
    }

    #[test]
    fn upstream_lazy_fp_fault_pends_hard_fault_when_ready() {
        check_lazy_fp_fault(false);
    }

    #[test]
    fn upstream_lazy_fp_fault_locks_over_hard_fault() {
        check_lazy_fp_fault(true);
    }

    fn unstack_fixture(nmi: &[u32]) -> Machine {
        let mut machine = exception_vector_fault_machine(None);
        let mut out = std::io::sink();
        machine.bus.nvic.as_mut().unwrap().set_vector_base(0);
        for (address, value) in [(8, 0x381), (12, 0x341), (20, 0x301)] {
            machine.bus.write(address, 4, value, &mut out).unwrap();
        }
        machine.bus.write(0x300, 2, 0xe7fe, &mut out).unwrap();
        for (index, &opcode) in nmi.iter().enumerate() {
            machine
                .bus
                .write(0x380 + index as u32 * 2, 2, opcode, &mut out)
                .unwrap();
        }
        machine
    }

    #[test]
    fn upstream_unstack_fault_locks_over_hard_fault() {
        let mut machine = unstack_fixture(&[0xf44f, 0x1080, 0xf380, 0x8808, 0x4770]);
        let mut out = std::io::sink();
        machine.bus.write(0x200, 2, 0x6802, &mut out).unwrap();
        machine.primary.registers[0] = 0x100000;
        machine.primary.registers[1] = 0x11;
        machine.primary.registers[2] = 0xa5a5_a5a5;
        assert_eq!(machine.run(1, &mut out).unwrap(), (Stop::Limit, 0));
        assert_eq!(machine.primary.pc, 0x340);
        machine
            .bus
            .write(0xe000_ed28, 4, u32::MAX, &mut out)
            .unwrap();
        machine
            .bus
            .write(0xe000_ed2c, 4, 1 << 30, &mut out)
            .unwrap();
        machine
            .bus
            .write(0xe000_ed04, 4, 1 << 31, &mut out)
            .unwrap();
        assert_eq!(machine.run(4, &mut out).unwrap(), (Stop::Limit, 2));
        assert_eq!(machine.primary.pc, 0xefff_fffe);
        assert_eq!(machine.primary.registers[13], 0x100020);
        assert_eq!(
            machine.primary.thumb.as_ref().unwrap().lockup_state(),
            (true, 0, 3)
        );
        assert!(machine.bus.nvic.as_ref().unwrap().lockup);
        let dhcsr = crate::nvic::DHCSR;
        assert_eq!(machine.bus.thumb_read::<4>(dhcsr).unwrap(), 0);
        assert_ne!(machine.bus.read::<4>(dhcsr).unwrap() & (1 << 19), 0);
        machine
            .bus
            .thumb_write(dhcsr, 4, u32::MAX, &mut out)
            .unwrap();
        assert_eq!(machine.bus.thumb_read::<4>(dhcsr).unwrap(), 0);
        assert_eq!(machine.bus.read::<4>(0xe000_ed28).unwrap(), 1 << 11);
        assert_eq!(machine.bus.read::<4>(0xe000_ed2c).unwrap(), 0);
        assert_eq!(machine.bus.read::<4>(0xe000_ed24).unwrap(), 4);
    }

    #[test]
    fn upstream_retpsr_integrity_failure_locks_over_hard_fault() {
        let mut machine = unstack_fixture(&[0x2000, 0x9007, 0x4770]);
        let mut out = std::io::sink();
        for (address, opcode) in [(0x200, 0x6802), (0x202, 0x2122), (0x204, 0xe7fe)] {
            machine.bus.write(address, 2, opcode, &mut out).unwrap();
        }
        machine.primary.registers[0] = 0x100000;
        machine.primary.registers[1] = 0x11;
        machine.primary.registers[2] = 0xa5a5_a5a5;
        assert_eq!(machine.run(1, &mut out).unwrap(), (Stop::Limit, 0));
        assert_eq!(machine.primary.pc, 0x340);
        machine
            .bus
            .write(0xe000_ed28, 4, u32::MAX, &mut out)
            .unwrap();
        machine
            .bus
            .write(0xe000_ed2c, 4, 1 << 30, &mut out)
            .unwrap();
        machine
            .bus
            .write(0xe000_ed04, 4, 1 << 31, &mut out)
            .unwrap();
        assert_eq!(machine.run(4, &mut out).unwrap(), (Stop::Limit, 2));
        assert_eq!(machine.primary.pc, 0xefff_fffe);
        assert_eq!(machine.primary.registers[13], 0xfe0);
        assert_eq!(
            machine.primary.thumb.as_ref().unwrap().lockup_state(),
            (true, 0, 3)
        );
        assert!(machine.bus.nvic.as_ref().unwrap().lockup);
        let dhcsr = crate::nvic::DHCSR;
        assert_eq!(machine.bus.thumb_read::<4>(dhcsr).unwrap(), 0);
        assert_ne!(machine.bus.read::<4>(dhcsr).unwrap() & (1 << 19), 0);
        machine
            .bus
            .thumb_write(dhcsr, 4, u32::MAX, &mut out)
            .unwrap();
        assert_eq!(machine.bus.thumb_read::<4>(dhcsr).unwrap(), 0);
        assert_eq!(machine.bus.read::<4>(0xe000_ed28).unwrap(), 1 << 18);
        assert_eq!(machine.bus.read::<4>(0xe000_ed2c).unwrap(), 0);
        assert_eq!(machine.bus.read::<4>(0xe000_ed24).unwrap(), 4);
    }

    fn check_unstack_tail_chain(fp: bool) {
        let mut machine = unstack_fixture(if fp {
            &[0xeeb0, 0x0a40, 0x4770]
        } else {
            &[0xbf00, 0x4770]
        });
        let mut out = std::io::sink();
        machine.bus.set_mock_fault_on_writes(0x100000, false);
        machine
            .bus
            .write(0xe000_ed24, 4, 1 << 17, &mut out)
            .unwrap();
        if fp {
            machine
                .bus
                .write(0xe000_ed88, 4, 0xf00000, &mut out)
                .unwrap();
        }
        machine.primary.registers[13] = 0x100020;
        machine
            .bus
            .write(0xe000_ed04, 4, 1 << 31, &mut out)
            .unwrap();
        assert_eq!(machine.run(2, &mut out).unwrap(), (Stop::Limit, 1));
        assert_eq!(machine.primary.pc, if fp { 0x384 } else { 0x382 });
        assert_eq!(
            machine.primary.thumb.as_ref().unwrap().fp_context_active(),
            fp
        );
        assert_eq!(machine.run(1, &mut out).unwrap(), (Stop::Limit, 0));
        assert_eq!(machine.primary.pc, 0x300);
        assert_eq!(machine.primary.registers[13], 0x100000);
        assert_eq!(machine.primary.registers[14], 0xffff_fff9);
        let thumb = machine.primary.thumb.as_ref().unwrap();
        assert_eq!(thumb.lockup_state(), (false, 0, 5));
        assert!(!thumb.fp_context_active());
        assert_eq!(machine.bus.read::<4>(0xe000_ed28).unwrap(), 1 << 11);
        assert_eq!(machine.bus.read::<4>(0xe000_ed2c).unwrap(), 0);
    }

    #[test]
    fn upstream_unstack_fault_tail_chains_to_bus_fault() {
        check_unstack_tail_chain(false);
    }

    #[test]
    fn upstream_unstack_fault_tail_chain_clears_fpca() {
        check_unstack_tail_chain(true);
    }

    fn check_nmi_entry_lockup(vector_fault: bool) {
        let mut machine = exception_vector_fault_machine(None);
        let mut out = std::io::sink();
        machine.bus.nvic.as_mut().unwrap().set_vector_base(0);
        for (address, value) in [(8, 0x381), (12, 0x341)] {
            machine.bus.write(address, 4, value, &mut out).unwrap();
        }
        for (address, opcode) in [
            (0x200, 0x6802),
            (0x202, 0x2122),
            (0x204, 0xe7fe),
            (0x340, 0x2d00),
            (0x342, 0xbf04),
            (0x344, 0x2101),
            (0x346, 0x2202),
            (0x348, 0xe7fe),
            (0x380, 0xe7fe),
        ] {
            machine.bus.write(address, 2, opcode, &mut out).unwrap();
        }
        machine.primary.registers[0] = 0x100000;
        machine.primary.registers[1] = 0x11;
        machine.primary.registers[2] = 0xa5a5_a5a5;
        assert_eq!(machine.run(1, &mut out).unwrap(), (Stop::Limit, 0));
        assert_eq!(machine.primary.pc, 0x340);
        machine
            .bus
            .write(0xe000_ed28, 4, u32::MAX, &mut out)
            .unwrap();
        machine
            .bus
            .write(0xe000_ed2c, 4, 1 << 30, &mut out)
            .unwrap();
        if vector_fault {
            machine.bus.nvic.as_mut().unwrap().set_vector_base(0x100000);
        } else {
            assert_eq!(machine.run(2, &mut out).unwrap(), (Stop::Limit, 2));
            assert_ne!(machine.primary.thumb.as_ref().unwrap().lockup_state().1, 0);
        }
        machine.primary.registers[13] = 0x100020;
        machine
            .bus
            .write(0xe000_ed04, 4, 1 << 31, &mut out)
            .unwrap();
        assert_eq!(machine.run(1, &mut out).unwrap(), (Stop::Limit, 0));
        assert_eq!(machine.primary.pc, 0xefff_fffe);
        assert_eq!(machine.primary.registers[13], 0x100000);
        assert_eq!(
            machine.primary.thumb.as_ref().unwrap().lockup_state(),
            (true, 0, 2)
        );
        assert!(machine.bus.nvic.as_ref().unwrap().lockup);
        assert_eq!(machine.bus.read::<4>(0xe000_ed28).unwrap(), 1 << 12);
        assert_eq!(
            machine.bus.read::<4>(0xe000_ed2c).unwrap(),
            if vector_fault { 2 } else { 0 }
        );
        assert_eq!(
            machine.bus.read::<4>(0xe000_ed24).unwrap(),
            (1 << 2) | (1 << 5)
        );
        let dhcsr = crate::nvic::DHCSR;
        assert_eq!(machine.bus.thumb_read::<4>(dhcsr).unwrap(), 0);
        assert_ne!(machine.bus.read::<4>(dhcsr).unwrap() & (1 << 19), 0);
        machine
            .bus
            .thumb_write(dhcsr, 4, u32::MAX, &mut out)
            .unwrap();
        assert_eq!(machine.bus.thumb_read::<4>(dhcsr).unwrap(), 0);
    }

    #[test]
    fn upstream_nmi_stacking_fault_locks_over_hard_fault() {
        check_nmi_entry_lockup(false);
    }

    #[test]
    fn upstream_nmi_stacking_lockup_records_vector_fault() {
        check_nmi_entry_lockup(true);
    }

    fn check_upstream_stacking_bus_fault(exception: u8, bus_fault_enabled: bool, handler: u8) {
        let mut machine = Machine::new(0x1000).unwrap();
        machine.bus.map_ram(0, 0x1000).unwrap();
        machine
            .bus
            .map_bus_faulting_peripheral(0x100000, None)
            .unwrap();
        machine.bus.map_nvic(0xe0, None, true).unwrap();
        machine.bus.map_systick().unwrap();
        machine.primary.configure_cortex_m(Some(0), true);
        let mut out = std::io::sink();
        for (address, value) in [
            (0, 0x100020),
            (4, 0x201),
            (8, 0x381),
            (12, 0x341),
            (20, 0x301),
            (64, 0x3c1),
        ] {
            machine.bus.write(address, 4, value, &mut out).unwrap();
        }
        for address in [0x200, 0x300, 0x340, 0x380, 0x3c0] {
            machine.bus.write(address, 2, 0xe7fe, &mut out).unwrap();
        }
        machine.validate_entry().unwrap();
        if bus_fault_enabled {
            machine
                .bus
                .write(0xe000_ed24, 4, 1 << 17, &mut out)
                .unwrap();
        }
        assert_eq!(machine.run(1, &mut out).unwrap(), (Stop::Limit, 1));
        assert_eq!(machine.primary.pc, 0x200);
        if exception == 2 {
            machine
                .bus
                .write(0xe000_ed04, 4, 1 << 31, &mut out)
                .unwrap();
        } else {
            machine.bus.write(0xe000_e100, 4, 1, &mut out).unwrap();
            machine.bus.write(0xe000_e200, 4, 1, &mut out).unwrap();
        }
        assert_eq!(machine.run(1, &mut out).unwrap(), (Stop::Limit, 0));
        assert_eq!(
            machine.primary.pc,
            match handler {
                2 => 0x380,
                3 => 0x340,
                5 => 0x300,
                _ => unreachable!(),
            }
        );
        assert_eq!(machine.primary.registers[13], 0x100000);
        assert_eq!(
            machine.primary.thumb.as_ref().unwrap().lockup_state(),
            (false, 0, handler)
        );
        assert_eq!(machine.bus.read::<4>(0xe000_ed28).unwrap(), 1 << 12);
        assert_eq!(
            machine.bus.read::<4>(0xe000_ed2c).unwrap(),
            if bus_fault_enabled { 0 } else { 1 << 30 }
        );
        if exception == 2 {
            assert_ne!(
                machine.bus.nvic.as_ref().unwrap().pending_exceptions() & (1 << 3),
                0
            );
        } else {
            assert_eq!(machine.bus.read::<4>(0xe000_e200).unwrap() & 1, 1);
            assert_eq!(machine.bus.read::<4>(0xe000_e300).unwrap() & 1, 0);
        }
    }

    #[test]
    fn upstream_nmi_stacking_fault_pends_hard_fault() {
        check_upstream_stacking_bus_fault(2, false, 2);
    }

    #[test]
    fn upstream_external_irq_stacking_fault_selects_hard_fault() {
        check_upstream_stacking_bus_fault(16, false, 3);
    }

    #[test]
    fn upstream_equal_priority_stacking_fault_selects_bus_fault() {
        check_upstream_stacking_bus_fault(16, true, 5);
    }

    fn exception_vector_fault_machine(offset: Option<usize>) -> Machine {
        let mut machine = Machine::new(0x1000).unwrap();
        machine.bus.map_ram(0, 0x1000).unwrap();
        machine
            .bus
            .map_bus_faulting_peripheral(0x100000, offset)
            .unwrap();
        machine.bus.map_nvic(0xe0, None, true).unwrap();
        machine.bus.map_systick().unwrap();
        machine.primary.configure_cortex_m(Some(0), true);
        let mut out = std::io::sink();
        for (address, value) in [(0, 0x1000), (4, 0x201)] {
            machine.bus.write(address, 4, value, &mut out).unwrap();
        }
        for address in [0x200, 0x340] {
            machine.bus.write(address, 2, 0xe7fe, &mut out).unwrap();
        }
        machine.validate_entry().unwrap();
        assert_eq!(machine.run(1, &mut out).unwrap(), (Stop::Limit, 1));
        machine.bus.nvic.as_mut().unwrap().set_vector_base(0x100000);
        machine
    }

    #[test]
    fn upstream_external_irq_vector_bus_fault_selects_hard_fault() {
        let mut machine = exception_vector_fault_machine(Some(0x40));
        let mut out = std::io::sink();
        machine.bus.write(0x10000c, 4, 0x341, &mut out).unwrap();
        machine.bus.write(0xe000_e100, 4, 1, &mut out).unwrap();
        machine.bus.write(0xe000_e200, 4, 1, &mut out).unwrap();
        assert_eq!(machine.run(1, &mut out).unwrap(), (Stop::Limit, 0));
        assert_eq!(machine.primary.pc, 0x340);
        assert_eq!(machine.primary.registers[13], 0xfe0);
        assert_eq!(
            machine.primary.thumb.as_ref().unwrap().lockup_state(),
            (false, 0, 3)
        );
        assert_eq!(machine.bus.read::<4>(0xe000_ed28).unwrap(), 0);
        assert_eq!(machine.bus.read::<4>(0xe000_ed2c).unwrap(), 2);
        assert_eq!(machine.bus.read::<4>(0xe000_e200).unwrap() & 1, 1);
        assert_eq!(machine.bus.read::<4>(0xe000_e300).unwrap() & 1, 0);
    }

    #[test]
    fn upstream_nmi_vector_bus_fault_locks_up() {
        let mut machine = exception_vector_fault_machine(None);
        let mut out = std::io::sink();
        machine
            .bus
            .write(0xe000_ed04, 4, 1 << 31, &mut out)
            .unwrap();
        assert_eq!(machine.run(1, &mut out).unwrap(), (Stop::Limit, 0));
        assert_eq!(machine.primary.pc, 0xefff_fffe);
        assert_eq!(machine.primary.registers[13], 0xfe0);
        assert_eq!(machine.primary.registers[14], 0xffff_fff9); // Armv7-M thread/MSP.
        assert_eq!(
            machine.primary.thumb.as_ref().unwrap().lockup_state(),
            (true, 0, 2)
        );
        assert!(machine.bus.nvic.as_ref().unwrap().lockup);
        assert_eq!(machine.bus.read::<4>(0xe000_ed28).unwrap(), 0);
        assert_eq!(machine.bus.read::<4>(0xe000_ed2c).unwrap(), 2);
        assert_eq!(machine.bus.read::<4>(0xe000_ed24).unwrap(), 1 << 5);
        let dhcsr = crate::nvic::DHCSR;
        assert_eq!(machine.bus.thumb_read::<4>(dhcsr).unwrap(), 0);
        assert_ne!(machine.bus.read::<4>(dhcsr).unwrap() & (1 << 19), 0);
        machine
            .bus
            .thumb_write(dhcsr, 4, u32::MAX, &mut out)
            .unwrap();
        assert_eq!(machine.bus.thumb_read::<4>(dhcsr).unwrap(), 0);
        machine.bus.write(0xe000_ed2c, 4, 2, &mut out).unwrap();
        assert_eq!(machine.bus.read::<4>(0xe000_ed2c).unwrap(), 0);
    }

    #[test]
    fn upstream_hard_fault_vector_bus_fault_locks_up() {
        let mut machine = exception_vector_fault_machine(None);
        let mut out = std::io::sink();
        machine
            .bus
            .nvic
            .as_mut()
            .unwrap()
            .set_hard_fault_pending(true);
        assert_eq!(machine.run(1, &mut out).unwrap(), (Stop::Limit, 0));
        assert_eq!(machine.primary.pc, 0xefff_fffe);
        assert_eq!(machine.primary.registers[13], 0xfe0);
        assert_eq!(machine.primary.registers[14], 0xffff_fff9); // Armv7-M thread/MSP.
        assert_eq!(
            machine.primary.thumb.as_ref().unwrap().lockup_state(),
            (true, 0, 3)
        );
        let nvic = machine.bus.nvic.as_ref().unwrap();
        assert!(nvic.lockup);
        assert_eq!(nvic.pending_exceptions() & (1 << 3), 0);
        assert_eq!(machine.bus.read::<4>(0xe000_ed28).unwrap(), 0);
        assert_eq!(machine.bus.read::<4>(0xe000_ed2c).unwrap(), 2);
        assert_eq!(machine.bus.read::<4>(0xe000_ed24).unwrap(), 1 << 2);
        let dhcsr = crate::nvic::DHCSR;
        assert_eq!(machine.bus.thumb_read::<4>(dhcsr).unwrap(), 0);
        assert_ne!(machine.bus.read::<4>(dhcsr).unwrap() & (1 << 19), 0);
        machine
            .bus
            .thumb_write(dhcsr, 4, u32::MAX, &mut out)
            .unwrap();
        assert_eq!(machine.bus.thumb_read::<4>(dhcsr).unwrap(), 0);
    }

    fn check_reset_bus_fault(offset: usize) {
        let mut machine = Machine::new(0x1000).unwrap();
        machine
            .bus
            .map_bus_faulting_peripheral(0x100000, Some(offset))
            .unwrap();
        machine.bus.map_nvic(0xe0, None, true).unwrap();
        machine.bus.map_systick().unwrap();
        machine.primary.configure_cortex_m(Some(0x100000), true);
        machine.validate_entry().unwrap();
        let mut out = std::io::sink();
        assert_eq!(machine.run(100, &mut out).unwrap(), (Stop::Limit, 0));
        assert_eq!(machine.primary.pc, 0xefff_fffe);
        assert_eq!(machine.primary.registers[13], 0);
        assert_eq!(machine.primary.registers[14], u32::MAX);
        assert_eq!(
            machine.primary.thumb.as_ref().unwrap().lockup_state(),
            (true, 0, 0)
        );
        assert!(machine.bus.nvic.as_ref().unwrap().lockup);
        assert_eq!(machine.bus.read::<4>(0xe000_ed28).unwrap(), 0);
        assert_eq!(machine.bus.read::<4>(0xe000_ed2c).unwrap(), 2);
        assert_eq!(machine.bus.read::<4>(0xe000_ed24).unwrap(), 4);
        let dhcsr = crate::nvic::DHCSR;
        assert_eq!(machine.bus.thumb_read::<4>(dhcsr).unwrap(), 0);
        assert_ne!(machine.bus.read::<4>(dhcsr).unwrap() & (1 << 19), 0);
        machine
            .bus
            .thumb_write(dhcsr, 4, u32::MAX, &mut out)
            .unwrap();
        assert_eq!(machine.bus.thumb_read::<4>(dhcsr).unwrap(), 0);
        assert_eq!(machine.bus.read::<4>(dhcsr).unwrap(), 1 << 19);
    }

    #[test]
    fn upstream_reset_vector_bus_fault_locks_up() {
        check_reset_bus_fault(4);
    }

    #[test]
    fn upstream_initial_msp_bus_fault_locks_up() {
        check_reset_bus_fault(0);
    }

    fn check_monitor_step_bus_fault(enable_bus_fault: bool) {
        let mut machine = started_cortex_m();
        let mut out = std::io::sink();
        machine
            .bus
            .map_bus_faulting_peripheral(0x100000, None)
            .unwrap();
        for (address, value) in [
            (0, 0x1000),
            (4, 0x201),
            (12, 0x341),
            (20, 0x301),
            (0xe000_ed24, u32::from(enable_bus_fault) << 17),
        ] {
            machine.bus.write(address, 4, value, &mut out).unwrap();
        }
        for base in [0x200, 0x300] {
            for (offset, word) in [(0, 0x6802), (2, 0x2122), (4, 0xe7fe)] {
                machine.bus.write(base + offset, 2, word, &mut out).unwrap();
            }
        }
        machine.bus.write(0x340, 2, 0xe7fe, &mut out).unwrap();
        machine.validate_entry().unwrap();
        machine.primary.registers[0] = 0x100000;
        machine.primary.registers[1] = 0x11;
        machine.primary.registers[2] = 0xa5a5_a5a5;
        machine.monitor_step_cortex_m(1, &mut out).unwrap();
        assert_eq!(machine.primary.pc, 0x340);
        assert_eq!(machine.primary.registers[1], 0x11);
        assert_eq!(machine.primary.registers[2], 0xa5a5_a5a5);
        let (stacked_pc_address, stacked_pc) = if enable_bus_fault {
            (0xfd8, 0x300)
        } else {
            (0xff8, 0x200)
        };
        assert_eq!(
            machine.bus.read::<4>(stacked_pc_address).unwrap(),
            stacked_pc
        );
        assert_eq!(machine.bus.read::<4>(0xe000_ed28).unwrap(), 0x8200);
        assert_eq!(machine.bus.read::<4>(0xe000_ed38).unwrap(), 0x100000);
        assert_eq!(machine.bus.read::<4>(0xe000_ed2c).unwrap(), 0x4000_0000);
        assert!(machine.cpu_paused);
    }

    #[test]
    fn upstream_monitor_step_escalates_nested_bus_fault() {
        check_monitor_step_bus_fault(true);
    }

    #[test]
    fn upstream_monitor_step_escalates_disabled_bus_fault() {
        check_monitor_step_bus_fault(false);
    }

    fn enter_instruction_time_lockup() -> Machine {
        let mut machine = Machine::new(0x1000).unwrap();
        machine.bus.map_ram(0, 0x1000).unwrap();
        machine
            .bus
            .map_bus_faulting_peripheral(0x100000, None)
            .unwrap();
        machine.bus.map_nvic(0xe0, None, true).unwrap();
        machine.bus.map_systick().unwrap();
        machine.primary.configure_cortex_m(Some(0), true);
        let mut out = std::io::sink();
        for (address, value) in [
            (0, 0x1000),
            (4, 0x201),
            (8, 0x381),
            (12, 0x341),
            (0x354, 0xe000_ed2c),
        ] {
            machine.bus.write(address, 4, value, &mut out).unwrap();
        }
        // The same READ_ASSEMBLY and LOCKUP_ASSEMBLY as the upstream Robot.
        for (base, words) in [
            (0x200, &[0x6802, 0x2122, 0xe7fe][..]),
            (
                0x340,
                &[
                    0x4b04, 0xf04f, 0x4480, 0x601c, 0x2d00, 0xbf04, 0x6802, 0x2122, 0xe7fe,
                ][..],
            ),
            (0x380, &[0xe7fe][..]),
        ] {
            for (index, &word) in words.iter().enumerate() {
                machine
                    .bus
                    .write(base + index as u32 * 2, 2, word, &mut out)
                    .unwrap();
            }
        }
        machine.primary.reset_cortex_m(&mut machine.bus).unwrap();
        machine.primary.registers[0] = 0x100000;
        machine.primary.registers[1] = 0x11;
        machine.primary.registers[2] = 0xa5a5_a5a5;
        assert_eq!(machine.run(20, &mut out).unwrap(), (Stop::Limit, 5));
        machine
    }

    #[test]
    fn upstream_double_bus_fault_locks_until_reset() {
        let mut machine = enter_instruction_time_lockup();
        let mut out = std::io::sink();
        let thumb = machine.primary.thumb.as_ref().unwrap();
        let state = thumb.lockup_state();
        assert!(state.0);
        assert_ne!(state.1, 0);
        assert!(!thumb.sleeping());
        assert!(machine.bus.nvic.as_ref().unwrap().lockup);
        assert_eq!(machine.primary.pc, 0xefff_fffe);
        assert_eq!(machine.primary.registers[1], 0x11);
        assert_eq!(machine.primary.registers[2], 0xa5a5_a5a5);
        for (address, expected) in [
            (0xe000_ed28, 0x8200),
            (0xe000_ed38, 0x100000),
            (0xe000_ed2c, 0),
            (0xe000_ed24, 4),
        ] {
            assert_eq!(machine.bus.read::<4>(address).unwrap(), expected);
        }
        assert_eq!(machine.run(10, &mut out).unwrap(), (Stop::Limit, 0));
        assert_eq!(machine.primary.pc, 0xefff_fffe);
        assert_eq!(
            machine.primary.thumb.as_ref().unwrap().lockup_state(),
            state
        );
        machine.primary.reset_cortex_m(&mut machine.bus).unwrap();
        assert!(!machine.primary.thumb.as_ref().unwrap().lockup_state().0);
        assert!(!machine.bus.nvic.as_ref().unwrap().lockup);
    }

    #[test]
    fn upstream_nmi_preempts_instruction_time_lockup() {
        let mut machine = enter_instruction_time_lockup();
        let mut out = std::io::sink();
        assert!(machine.bus.nvic.as_ref().unwrap().lockup);
        machine
            .bus
            .write(0xe000_ed04, 4, 1 << 31, &mut out)
            .unwrap();
        assert_eq!(machine.run(1, &mut out).unwrap(), (Stop::Limit, 0));
        assert_eq!(machine.primary.pc, 0x380);
        assert_eq!(machine.bus.read::<4>(0xfd8).unwrap(), 0xefff_fffe);
        assert!(!machine.primary.thumb.as_ref().unwrap().lockup_state().0);
        assert!(!machine.bus.nvic.as_ref().unwrap().lockup);
    }

    #[test]
    fn handles_ecall_and_returns_to_guest_code() {
        let mut machine = Machine::new(48).unwrap();
        let program: [u32; 12] = [
            0x0000_0297, // auipc t0, 0
            0x0182_8293, // addi t0, t0, 24
            0x3052_9073, // csrw mtvec, t0
            0x3004_6073, // csrsi mstatus, 8
            0x0000_0073, // ecall
            0x0070_0513, // addi a0, zero, 7
            0x3420_2373, // csrr t1, mcause
            0x3000_23f3, // csrr t2, mstatus
            0x3410_2e73, // csrr t3, mepc
            0x004e_0e13, // addi t3, t3, 4
            0x341e_1073, // csrw mepc, t3
            0x3020_0073, // mret
        ];
        for (destination, instruction) in machine
            .bus
            .memory(RAM_BASE, 48)
            .unwrap()
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(program)
        {
            *destination = instruction.to_le_bytes();
        }
        assert_eq!(
            machine.run(12, &mut std::io::sink()).unwrap(),
            (Stop::Limit, 11)
        );
        assert_eq!(machine.primary.pc, RAM_BASE + 24);
        assert_eq!(machine.primary.registers[6], 11);
        assert_eq!(machine.primary.registers[7], (3 << 11) | (1 << 7));
        assert_eq!(machine.primary.registers[28], RAM_BASE + 20);
        assert_eq!(machine.primary.registers[10], 7);
        // Verify that MRET restored MIE from MPIE.
        let read_status = 0x3000_2373u32;
        machine
            .bus
            .memory(machine.primary.pc, 4)
            .unwrap()
            .copy_from_slice(&read_status.to_le_bytes());
        machine.run(1, &mut std::io::sink()).unwrap();
        assert_eq!(
            machine.primary.registers[6],
            (3 << 11) | (1 << 7) | (1 << 3)
        );
    }

    #[test]
    fn delivers_synchronous_faults_with_precise_pc_and_value() {
        let mut machine = Machine::new(16).unwrap();
        let handler = RAM_BASE + 8;
        machine
            .primary
            .csr
            .access(0x3050_9073, handler | 1)
            .unwrap(); // vectored mtvec
        for (pc, opcode, cause, value) in [
            (RAM_BASE, 0x0020_02efu32, 0, RAM_BASE + 2), // jal t0, +2
            (0x2000, 0, 1, 0x2000),                      // unmapped fetch
            (RAM_BASE, u32::MAX, 2, u32::MAX),           // illegal instruction
            (RAM_BASE, 0x7770_22f3, 2, 0x7770_22f3),     // unknown CSR
            (RAM_BASE, 0x0010_2283, 4, 1),               // lw t0, 1(zero)
            (RAM_BASE, 0x0000_2283, 5, 0),               // lw t0, 0(zero)
            (RAM_BASE, 0x0050_20a3, 6, 1),               // sw t0, 1(zero)
            (RAM_BASE, 0x0050_2023, 7, 0),               // sw t0, 0(zero)
        ] {
            machine
                .bus
                .memory(RAM_BASE, 4)
                .unwrap()
                .copy_from_slice(&opcode.to_le_bytes());
            machine.primary.pc = pc;
            machine.primary.registers[5] = 42;
            assert_eq!(
                machine.run(1, &mut std::io::sink()).unwrap(),
                (Stop::Limit, 0)
            );
            assert_eq!(machine.primary.pc, handler);
            assert_eq!(machine.primary.registers[5], 42);
            assert_eq!(machine.primary.csr.access(0x3410_2073, 0).unwrap(), pc);
            assert_eq!(machine.primary.csr.access(0x3420_2073, 0).unwrap(), cause);
            assert_eq!(machine.primary.csr.access(0x3430_2073, 0).unwrap(), value);
        }
    }

    #[test]
    fn prioritizes_software_interrupt_then_delivers_timer_interrupt() {
        use crate::clint::{SOFTWARE, TIMER};

        const CLINT: u32 = 0x0200_0000;
        let mut machine = Machine::new(128).unwrap();
        machine.bus.map_clint(CLINT, 1).unwrap();
        assert!(machine.bus.map_ram(CLINT, 4).is_err());
        let vector = RAM_BASE + 32;
        machine.primary.csr.access(0x3050_9073, vector | 1).unwrap();
        machine
            .primary
            .csr
            .access(0x3040_9073, SOFTWARE | TIMER)
            .unwrap();
        machine.primary.csr.access(0x3000_9073, 1 << 3).unwrap();
        let mut output = std::io::sink();
        for (offset, value) in [(0, 1), (0x4000, 2), (0x4004, 0), (0xbff8, 1)] {
            machine
                .bus
                .write(CLINT + offset, 4, value, &mut output)
                .unwrap();
        }
        let software_entry = vector + 4 * 3;
        machine.primary.registers[5] = CLINT;
        for (address, opcode) in [
            (software_entry, 0x0002_a023u32),  // sw zero, 0(t0)
            (software_entry + 4, 0x3020_0073), // mret
        ] {
            machine
                .bus
                .memory(address, 4)
                .unwrap()
                .copy_from_slice(&opcode.to_le_bytes());
        }
        assert_eq!(machine.run(1, &mut output).unwrap(), (Stop::Limit, 0));
        assert_eq!(machine.primary.pc, software_entry);
        assert_eq!(machine.bus.read::<4>(CLINT + 0xbff8).unwrap(), 2);
        assert_eq!(
            machine.primary.csr.access(0x3440_2073, 0).unwrap(),
            SOFTWARE | TIMER
        );
        assert_eq!(
            machine.primary.csr.access(0x3420_2073, 0).unwrap(),
            0x8000_0003
        );

        assert_eq!(machine.run(2, &mut output).unwrap(), (Stop::Limit, 2));
        assert_eq!(machine.primary.pc, RAM_BASE);
        assert_eq!(machine.bus.read::<4>(CLINT).unwrap(), 0);
        assert_eq!(machine.run(1, &mut output).unwrap(), (Stop::Limit, 0));
        assert_eq!(machine.primary.pc, vector + 4 * 7);
        assert_eq!(
            machine.primary.csr.access(0x3420_2073, 0).unwrap(),
            0x8000_0007
        );
        assert_eq!(
            machine.primary.csr.access(0x3410_2073, 0).unwrap(),
            RAM_BASE
        );
    }

    #[test]
    fn murax_uart_asserts_receive_interrupt_until_data_is_read() {
        const UART: u32 = 0xf001_0000;
        let mut machine = Machine::new(64).unwrap();
        machine.bus.map_uart(UART, true).unwrap();
        machine.bus.uart.as_mut().unwrap().queue_input(vec![b'Q']);
        machine
            .primary
            .csr
            .access(0x3050_9073, RAM_BASE + 16)
            .unwrap();
        machine.primary.csr.access(0x3040_9073, EXTERNAL).unwrap();
        machine.primary.csr.access(0x3000_9073, 1 << 3).unwrap();
        let mut output = Vec::new();
        assert_eq!(machine.bus.read::<4>(UART + 4).unwrap(), 1 << 9);
        machine.bus.write(UART + 4, 4, 2, &mut output).unwrap();
        assert_eq!(machine.run(1, &mut output).unwrap(), (Stop::Limit, 0));
        assert_eq!(machine.primary.pc, RAM_BASE + 16);
        assert_eq!(
            machine.primary.csr.access(0x3420_2073, 0).unwrap(),
            0x8000_000b
        );
        assert_eq!(machine.bus.read::<4>(UART).unwrap(), u32::from(b'Q'));
        assert_eq!(machine.bus.read::<4>(UART + 4).unwrap(), 2);
        assert!(!machine.bus.uart.as_ref().unwrap().interrupt_pending());
    }

    #[test]
    fn executes_word_atomics_and_invalidates_overlapping_reservations() {
        let mut machine = Machine::new(64).unwrap();
        machine.primary.set_integer_extensions(true, true, false);
        machine.primary.registers[1] = RAM_BASE + 32;
        machine.primary.registers[2] = 7;
        let mut output = std::io::sink();
        for (offset, opcode) in [
            (0, 0x1000_a1afu32), // lr.w x3, (x1)
            (4, 0x1820_a22f),    // sc.w x4, x2, (x1)
            (8, 0x0620_a2af),    // amoadd.w.aqrl x5, x2, (x1)
        ] {
            machine
                .bus
                .memory(RAM_BASE + offset, 4)
                .unwrap()
                .copy_from_slice(&opcode.to_le_bytes());
        }
        assert_eq!(machine.run(3, &mut output).unwrap(), (Stop::Limit, 3));
        assert_eq!(&machine.primary.registers[3..6], &[0, 0, 7]);
        assert_eq!(machine.bus.read_ram::<4>(RAM_BASE + 32).unwrap(), 14);
        machine.primary.pc = RAM_BASE;
        machine.run(1, &mut output).unwrap();
        machine.bus.write(RAM_BASE + 33, 1, 1, &mut output).unwrap();
        machine.run(1, &mut output).unwrap();
        assert_eq!(machine.primary.registers[4], 1);
        assert_eq!(machine.bus.read_ram::<4>(RAM_BASE + 32).unwrap(), 0x10e);
    }

    #[test]
    fn executes_mixed_width_instructions_and_links_by_two_bytes() {
        let mut machine = Machine::new(14).unwrap();
        machine.primary.set_integer_extensions(true, true, true);
        let bytes = [
            0x11, 0x45, // c.li a0, 4
            0x13, 0x05, 0x15, 0x00, // addi a0, a0, 1 (halfword-aligned)
            0x11, 0x20, // c.jal +4
            0x02, 0x90, // c.ebreak
            0x7d, 0x15, // c.addi a0, -1
            0x82, 0x80, // c.jr ra at the final mapped halfword
        ];
        machine
            .bus
            .memory(RAM_BASE, bytes.len())
            .unwrap()
            .copy_from_slice(&bytes);
        assert_eq!(
            machine.run(8, &mut std::io::sink()).unwrap(),
            (Stop::Ebreak, 5)
        );
        assert_eq!(machine.primary.registers[1], RAM_BASE + 8);
        assert_eq!(machine.primary.registers[10], 4);
        assert_eq!(machine.primary.pc, RAM_BASE + 8);
    }

    #[test]
    fn user_mode_traps_ecall_and_rejects_machine_only_operations() {
        let mut machine = Machine::new(64).unwrap();
        machine.primary.enable_user_mode();
        let user = RAM_BASE + 4;
        let handler = RAM_BASE + 32;
        machine.primary.set_mtvec(handler);
        machine.primary.csr.access(0x3410_9073, user).unwrap();
        for opcode in [0x0000_0073u32, 0x3000_22f3, 0x3020_0073] {
            machine.primary.pc = RAM_BASE;
            machine.primary.csr.access(0x3000_9073, 0).unwrap(); // MPP=U
            machine
                .bus
                .memory(RAM_BASE, 8)
                .unwrap()
                .copy_from_slice(&[0x3020_0073u32.to_le_bytes(), opcode.to_le_bytes()].concat());
            let mut output = std::io::sink();
            assert_eq!(machine.run(1, &mut output).unwrap(), (Stop::Limit, 1));
            assert_eq!(machine.primary.pc, user);
            assert!(machine.primary.csr.in_user_mode());
            assert_eq!(machine.run(1, &mut output).unwrap(), (Stop::Limit, 0));
            assert_eq!(machine.primary.pc, handler);
            assert!(!machine.primary.csr.in_user_mode());
            assert_eq!(machine.primary.csr.access(0x3410_2073, 0).unwrap(), user);
            assert_eq!(
                machine.primary.csr.access(0x3420_2073, 0).unwrap(),
                if opcode == 0x0000_0073 { 8 } else { 2 }
            );
            assert_eq!(
                machine.primary.csr.access(0x3000_2073, 0).unwrap() & (3 << 11),
                0
            );
        }
    }

    #[test]
    fn pmp_blocks_user_load_from_machine_memory() {
        let mut machine = Machine::new(64).unwrap();
        machine.primary.enable_user_mode();
        machine.primary.enable_pmp();
        let user = RAM_BASE + 4;
        machine.primary.set_mtvec(RAM_BASE + 32);
        machine.primary.csr.access(0x3410_9073, user).unwrap();
        machine.primary.csr.access(0x3000_9073, 0).unwrap();
        machine.primary.csr.access(0x3b00_9073, user >> 2).unwrap();
        machine.primary.csr.access(0x3a00_9073, 0x14).unwrap(); // NA4, execute only
        machine.primary.registers[1] = RAM_BASE + 16;
        let code = [0x3020_0073u32, 0x0000_a283]; // mret; lw t0, 0(ra)
        machine
            .bus
            .memory(RAM_BASE, 8)
            .unwrap()
            .copy_from_slice(&code.map(u32::to_le_bytes).concat());
        assert_eq!(
            machine.run(2, &mut std::io::sink()).unwrap(),
            (Stop::Limit, 1)
        );
        assert_eq!(machine.primary.pc, RAM_BASE + 32);
        assert_eq!(machine.primary.csr.access(0x3420_2073, 0).unwrap(), 5);
        assert_eq!(
            machine.primary.csr.access(0x3430_2073, 0).unwrap(),
            RAM_BASE + 16
        );
        assert_eq!(machine.primary.registers[5], 0);
    }

    #[test]
    fn delegates_user_ecall_to_supervisor_and_returns_with_sret() {
        let mut machine = Machine::new(64).unwrap();
        machine.primary.enable_user_mode();
        machine.primary.enable_supervisor_mode();
        machine.primary.set_mtvec(RAM_BASE + 48);
        for (csr, value) in [
            (0x105, RAM_BASE + 16),
            (0x302, 1 << 8),
            (0x341, RAM_BASE + 4),
            (0x300, 0),
        ] {
            machine
                .primary
                .csr
                .access((csr << 20) | 0x9073, value)
                .unwrap();
        }
        for (offset, opcode) in [
            (0, 0x3020_0073u32), // mret to U
            (4, 0x0000_0073),    // ecall from U
            (8, 0x0070_0513),    // addi a0, zero, 7
            (16, 0x1410_2373),   // csrr t1, sepc
            (20, 0x0043_0313),   // addi t1, t1, 4
            (24, 0x1413_1073),   // csrw sepc, t1
            (28, 0x1020_0073),   // sret
        ] {
            machine
                .bus
                .memory(RAM_BASE + offset, 4)
                .unwrap()
                .copy_from_slice(&opcode.to_le_bytes());
        }
        let mut output = std::io::sink();
        assert_eq!(machine.run(2, &mut output).unwrap(), (Stop::Limit, 1));
        assert_eq!(machine.primary.pc, RAM_BASE + 16);
        assert_eq!(machine.primary.csr.access(0x1420_2073, 0).unwrap(), 8);
        assert_eq!(
            machine.primary.csr.access(0x1410_2073, 0).unwrap(),
            RAM_BASE + 4
        );
        assert!(machine.primary.csr.access(0x3420_2073, 0).is_err());
        assert_eq!(machine.run(5, &mut output).unwrap(), (Stop::Limit, 5));
        assert!(machine.primary.csr.in_user_mode());
        assert_eq!(machine.primary.registers[10], 7);
    }

    #[test]
    fn sv32_executes_translated_code_and_reports_faulting_virtual_address() {
        const VA: u32 = 0x4000_0000;
        let mut machine = Machine::new(0x4000).unwrap();
        machine.primary.enable_user_mode();
        machine.primary.enable_supervisor_mode();
        machine.primary.enable_pmp();
        machine.primary.set_mtvec(RAM_BASE + 0x40);
        let root = RAM_BASE + 0x1000;
        let leaf = RAM_BASE + 0x2000;
        let frame = RAM_BASE + 0x3000;
        let entry = ((frame >> 12) << 10) | 0xf;
        for (address, value) in [
            (RAM_BASE, 0x3020_0073u32), // mret
            (root + ((VA >> 22) * 4), ((leaf >> 12) << 10) | 1),
            (leaf, entry),
            (frame, 0x0020_a023),     // sw x2, 0(x1)
            (frame + 4, 0x0001_a203), // lw x4, 0(x3)
        ] {
            machine
                .bus
                .memory(address, 4)
                .unwrap()
                .copy_from_slice(&value.to_le_bytes());
        }
        for (csr, value) in [
            (0x3b0, u32::MAX),
            (0x3a0, 0x1f), // PMP permits the entire bus
            (0x180, 0x8000_0000 | (root >> 12)),
            (0x341, VA),
            (0x300, 1 << 11),
        ] {
            machine
                .primary
                .csr
                .access((csr << 20) | 0x9073, value)
                .unwrap();
        }
        machine.primary.registers[1] = VA + 0x80;
        machine.primary.registers[2] = 42;
        machine.primary.registers[3] = VA + 0x1000;
        assert_eq!(
            machine.run(3, &mut std::io::sink()).unwrap(),
            (Stop::Limit, 2)
        );
        assert_eq!(machine.bus.read_ram::<4>(frame + 0x80).unwrap(), 42);
        assert_eq!(machine.bus.read_ram::<4>(leaf).unwrap(), entry | 0xc0);
        assert_eq!(machine.primary.pc, RAM_BASE + 0x40);
        assert_eq!(machine.primary.csr.access(0x3420_2073, 0).unwrap(), 13);
        assert_eq!(
            machine.primary.csr.access(0x3430_2073, 0).unwrap(),
            VA + 0x1000
        );
        assert_eq!(machine.primary.csr.access(0x3410_2073, 0).unwrap(), VA + 4);
    }

    #[test]
    fn delivers_vectored_supervisor_software_interrupt_after_mret() {
        let mut machine = Machine::new(64).unwrap();
        machine.primary.enable_user_mode();
        machine.primary.enable_supervisor_mode();
        let vector = RAM_BASE + 16;
        for (csr, value) in [
            (0x303, 2),
            (0x104, 2),
            (0x344, 2),
            (0x105, vector | 1),
            (0x341, RAM_BASE + 4),
            (0x300, (1 << 11) | 2),
        ] {
            machine
                .primary
                .csr
                .access((csr << 20) | 0x9073, value)
                .unwrap();
        }
        for (address, opcode) in [
            (RAM_BASE, 0x3020_0073u32),  // mret
            (RAM_BASE + 4, 0x0070_0513), // addi a0, zero, 7
            (vector + 4, 0x1441_7073),   // csrci sip, 2
            (vector + 8, 0x1020_0073),   // sret
        ] {
            machine
                .bus
                .memory(address, 4)
                .unwrap()
                .copy_from_slice(&opcode.to_le_bytes());
        }
        let mut output = std::io::sink();
        assert_eq!(machine.run(2, &mut output).unwrap(), (Stop::Limit, 1));
        assert_eq!(machine.primary.pc, vector + 4);
        assert_eq!(
            machine.primary.csr.access(0x1420_2073, 0).unwrap(),
            0x8000_0001
        );
        assert_eq!(
            machine.primary.csr.access(0x1410_2073, 0).unwrap(),
            RAM_BASE + 4
        );
        assert_eq!(machine.primary.csr.access(0x1440_2073, 0).unwrap(), 2);
        assert_eq!(machine.run(3, &mut output).unwrap(), (Stop::Limit, 3));
        assert_eq!(machine.primary.csr.access(0x1440_2073, 0).unwrap(), 0);
        assert_eq!(machine.primary.registers[10], 7);
    }

    #[test]
    fn counters_track_retirement_and_clint_time_independently() {
        let mut machine = Machine::new(32).unwrap();
        let clint = 0x0200_0000;
        machine.bus.map_clint(clint, 1).unwrap();
        let mut output = std::io::sink();
        machine
            .bus
            .write(clint + 0xbffc, 4, 5, &mut output)
            .unwrap();
        for (index, opcode) in [
            0xb020_9073u32, // csrw minstret, x1
            0xc020_2173,    // csrr x2, instret
            0xc810_21f3,    // csrr x3, timeh
            0x3202_d073,    // csrwi mcountinhibit, 5
            0x0000_0013,    // nop
            0x0010_0073,    // ebreak
        ]
        .into_iter()
        .enumerate()
        {
            machine
                .bus
                .memory(RAM_BASE + index as u32 * 4, 4)
                .unwrap()
                .copy_from_slice(&opcode.to_le_bytes());
        }
        machine.primary.registers[1] = 40;
        assert_eq!(machine.run(6, &mut output).unwrap(), (Stop::Ebreak, 5));
        assert_eq!(machine.primary.registers[2], 40);
        assert_eq!(machine.primary.registers[3], 5);
        assert_eq!(machine.primary.csr.access(0xc020_2073, 0).unwrap(), 43);
        assert_eq!(machine.primary.csr.access(0xc000_2073, 0).unwrap(), 4);
        assert_eq!(machine.primary.csr.access(0xc010_2073, 0).unwrap(), 6);
    }

    #[test]
    fn schedules_harts_independently_against_shared_ram() {
        let mut machine = Machine::new(64).unwrap();
        let code = [
            0xf140_22f3u32, // csrr t0, mhartid
            0x0050_a023,    // sw t0, 0(ra)
        ];
        machine
            .bus
            .memory(RAM_BASE, 8)
            .unwrap()
            .copy_from_slice(&code.map(u32::to_le_bytes).concat());
        machine.primary.registers[1] = RAM_BASE + 32;
        machine.configure_harts(2);
        let mut output = std::io::sink();
        assert_eq!(machine.run(1, &mut output).unwrap(), (Stop::Limit, 1));
        assert_eq!(machine.primary.pc, RAM_BASE + 4);
        assert_eq!(machine.secondary[0].pc, RAM_BASE);
        assert_eq!(machine.run(3, &mut output).unwrap(), (Stop::Limit, 3));
        assert_eq!(machine.primary.registers[5], 0);
        assert_eq!(machine.secondary[0].registers[5], 1);
        assert_eq!(machine.bus.read_ram::<4>(RAM_BASE + 32).unwrap(), 1);

        let address = RAM_BASE + 32;
        assert_eq!(machine.bus.load_reserved(0, address).unwrap(), 1);
        assert_eq!(machine.bus.load_reserved(1, address).unwrap(), 1);
        assert_eq!(machine.bus.store_conditional(0, address, 2).unwrap(), 0);
        assert_eq!(machine.bus.store_conditional(1, address, 3).unwrap(), 1);
        assert_eq!(machine.bus.read_ram::<4>(address).unwrap(), 2);
    }

    #[test]
    fn clint_routes_software_and_timer_interrupts_to_the_target_hart() {
        use crate::clint::{SOFTWARE, TIMER};

        const CLINT: u32 = 0x0200_0000;
        let mut machine = Machine::new(64).unwrap();
        machine.bus.map_clint(CLINT, 2).unwrap();
        machine.primary.set_mtvec(RAM_BASE + 32);
        machine
            .primary
            .csr
            .access(0x3040_9073, SOFTWARE | TIMER)
            .unwrap();
        machine.primary.csr.access(0x3000_9073, 1 << 3).unwrap();
        machine.configure_harts(2);
        for offset in [0, 4] {
            machine
                .bus
                .memory(RAM_BASE + offset, 4)
                .unwrap()
                .copy_from_slice(&0x0000_0013u32.to_le_bytes());
        }
        let mut output = std::io::sink();
        machine.bus.write(CLINT + 4, 4, 1, &mut output).unwrap();
        assert_eq!(machine.run(2, &mut output).unwrap(), (Stop::Limit, 1));
        assert_eq!(machine.primary.pc, RAM_BASE + 4);
        let secondary = &mut machine.secondary[0];
        assert_eq!(secondary.pc, RAM_BASE + 32);
        assert_eq!(secondary.csr.access(0x3420_2073, 0).unwrap(), 0x8000_0003);

        secondary.csr.access(0x3000_9073, 1 << 3).unwrap();
        for (offset, value) in [(4, 0), (0x4008, 4), (0x400c, 0)] {
            machine
                .bus
                .write(CLINT + offset, 4, value, &mut output)
                .unwrap();
        }
        assert_eq!(machine.run(2, &mut output).unwrap(), (Stop::Limit, 1));
        assert_eq!(machine.primary.pc, RAM_BASE + 8);
        assert_eq!(
            machine.secondary[0].csr.access(0x3420_2073, 0).unwrap(),
            0x8000_0007
        );
        assert_eq!(machine.bus.read::<4>(CLINT + 0xbff8).unwrap(), 4);
    }

    #[test]
    fn uart_events_arrive_in_step_order_and_append_to_existing_input() {
        const UART: u32 = 0xf000_1800;
        let mut machine = Machine::new(8).unwrap();
        machine.bus.map_litex_uart(UART).unwrap();
        machine
            .bus
            .memory(RAM_BASE, 4)
            .unwrap()
            .copy_from_slice(&0x0000_006fu32.to_le_bytes());
        machine.schedule_uart_input(3, vec![b'B']).unwrap();
        machine.schedule_uart_input(1, vec![b'A']).unwrap();
        machine.schedule_uart_input(3, vec![b'C']).unwrap();
        let mut output = std::io::sink();
        assert_eq!(machine.run(2, &mut output).unwrap(), (Stop::Limit, 2));
        assert_eq!(machine.run(1, &mut output).unwrap(), (Stop::Limit, 1));
        for byte in b"ABC" {
            assert_eq!(machine.bus.read::<1>(UART).unwrap(), u32::from(*byte));
        }
        assert_eq!(machine.bus.read::<4>(UART + 8).unwrap(), 1);
    }

    #[test]
    fn sfence_vma_rewalks_cached_mapping_after_pte_change() {
        const VA: u32 = 0x4000_0000;
        let mut machine = Machine::new(0x5000).unwrap();
        machine.primary.enable_user_mode();
        machine.primary.enable_supervisor_mode();
        let root = RAM_BASE + 0x1000;
        let leaf = RAM_BASE + 0x2000;
        let old = RAM_BASE + 0x3000;
        let new = RAM_BASE + 0x4000;
        let pte = |address: u32| ((address >> 12) << 10) | 0x4b;
        for (address, word) in [
            (root + (VA >> 22) * 4, ((leaf >> 12) << 10) | 1),
            (leaf, pte(old)),
            (old + 4, 17),
            (new, 0x1200_0073), // sfence.vma
            (new + 4, 29),
        ] {
            machine
                .bus
                .memory(address, 4)
                .unwrap()
                .copy_from_slice(&word.to_le_bytes());
        }
        for (csr, value) in [
            (0x180, 0x8000_0000 | (root >> 12)),
            (0x300, 1 << 11),
            (0x341, VA),
        ] {
            machine
                .primary
                .csr
                .access((csr << 20) | 0x9073, value)
                .unwrap();
        }
        machine.primary.pc = machine.primary.csr.return_from_trap().unwrap();
        let load = |machine: &mut Machine| {
            machine
                .primary
                .load::<4>(&mut machine.bus, VA + 4)
                .unwrap_or_else(|fault| panic!("{fault}"))
        };
        assert_eq!(load(&mut machine), 17);
        machine
            .bus
            .memory(leaf, 4)
            .unwrap()
            .copy_from_slice(&pte(new).to_le_bytes());
        assert_eq!(load(&mut machine), 17);
        assert_eq!(
            machine.run(1, &mut std::io::sink()).unwrap(),
            (Stop::Limit, 1)
        );
        assert_eq!(load(&mut machine), 29);
    }

    #[test]
    fn executes_m_extension_edge_cases() {
        let mut machine = Machine::new(8).unwrap();
        for (operation, lhs, rhs, expected) in [
            (0, u32::MAX, 2, 0xffff_fffe),
            (1, u32::MAX, 2, u32::MAX),
            (2, 0x8000_0000, u32::MAX, 0x8000_0000),
            (3, u32::MAX, u32::MAX, 0xffff_fffe),
            (4, 0x8000_0000, u32::MAX, 0x8000_0000),
            (4, (-7i32) as u32, 3, (-2i32) as u32),
            (4, 42, 0, u32::MAX),
            (5, u32::MAX, 2, 0x7fff_ffff),
            (5, 42, 0, u32::MAX),
            (6, 0x8000_0000, u32::MAX, 0),
            (6, (-7i32) as u32, 3, u32::MAX),
            (6, 42, 0, 42),
            (7, u32::MAX, 2, 1),
            (7, 42, 0, 42),
        ] {
            let instruction = 0x0220_81b3u32 | (operation << 12);
            machine
                .bus
                .memory(RAM_BASE, 4)
                .unwrap()
                .copy_from_slice(&instruction.to_le_bytes());
            machine.primary.pc = RAM_BASE;
            machine.primary.registers[1] = lhs;
            machine.primary.registers[2] = rhs;
            assert_eq!(
                machine.run(1, &mut std::io::sink()).unwrap(),
                (Stop::Limit, 1)
            );
            assert_eq!(machine.primary.registers[3], expected, "funct3={operation}");
        }
    }
}
