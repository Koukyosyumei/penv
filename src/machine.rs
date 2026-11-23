use std::{
    collections::{BTreeMap, VecDeque},
    io::Write,
    path::PathBuf,
};

use crate::{
    bitmanip::BitManip,
    bus::{Bus, BusError, RAM_BASE},
    csr::{Csr, EXTERNAL},
    litex_panel::ButtonAction,
    litex_video::ReferenceFrame,
    sv32::TranslationCache,
    thumb::Thumb,
};

mod floating;

pub const MAX_HARTS: usize = 8;

pub struct Machine {
    pub bus: Bus,
    pub primary: Hart,
    pub function_symbols: Option<BTreeMap<u32, String>>,
    secondary: Vec<Hart>,
    next_hart: usize,
    elapsed_steps: u64,
    inputs: VecDeque<(u64, Input)>,
    queued_frame_waits: VecDeque<(u64, Box<ReferenceFrame>)>,
    intermediate_frame_matches: Vec<u64>,
}

enum Input {
    Uart(Vec<u8>),
    Button(String, ButtonAction),
    PhyVendorSpecific1 { name: String, value: u16 },
    SensorTemperature { name: String, value: i32 },
    LedAssertion { name: String, expected: bool },
    FramebufferCapture(PathBuf),
    FrameWait(Box<ReferenceFrame>),
}

#[derive(Clone)]
pub struct Hart {
    pub name: Option<String>,
    slot: usize,
    csr: Csr,
    translations: TranslationCache,
    thumb: Option<Thumb>,
    zfh: bool,
    floating: Option<Box<[u64; 32]>>,
    unaligned_accesses: bool,
    zicond: bool,
    zacas: bool,
    bitmanip: BitManip,
    pub pc: u32,
    reset_vector: u32,
    pc_written: bool,
    pub registers: [u32; 32],
}

#[derive(Debug, PartialEq, Eq)]
pub enum Stop {
    FrameMatched(u64),
    Tohost(u64),
    Ecall,
    Ebreak,
    Limit,
}

enum Step {
    Retired,
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
    Breakpoint(u32),
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
                BusError::Unmapped(address, _) | BusError::Denied(address) => (
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
                BusError::Output(_) | BusError::AudioOutput(_) => return None,
            },
            Self::Illegal(insn) | Self::InvalidCsr(insn, _) => (2, *insn),
            Self::MisalignedTarget(address) => (0, *address),
            Self::Breakpoint(address) => (3, *address),
        })
    }
}

impl std::fmt::Display for Fault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bus(_, error) => error.fmt(f),
            Self::Illegal(insn) => write!(f, "unsupported instruction {insn:#010x}"),
            Self::InvalidCsr(_, error) => f.write_str(error),
            Self::Breakpoint(address) => write!(f, "address breakpoint at {address:#010x}"),
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
            function_symbols: None,
            primary: Hart {
                name: None,
                slot: 0,
                csr: Csr::default(),
                translations: TranslationCache::default(),
                thumb: None,
                zfh: false,
                floating: None,
                unaligned_accesses: false,
                zicond: false,
                zacas: false,
                bitmanip: BitManip::default(),
                pc: RAM_BASE,
                reset_vector: RAM_BASE,
                pc_written: false,
                registers: [0; 32],
            },
            secondary: Vec::new(),
            next_hart: 0,
            elapsed_steps: 0,
            inputs: VecDeque::new(),
            queued_frame_waits: VecDeque::new(),
            intermediate_frame_matches: Vec::new(),
        }
    }

    pub fn enable_function_logging(&mut self) -> Result<(), String> {
        if self.is_cortex_m() {
            return Err("LogFunctionNames requires an RV32 machine".into());
        }
        self.function_symbols = Some(BTreeMap::new());
        Ok(())
    }

    pub fn configure_harts(&mut self, count: usize) {
        self.secondary = (1..count)
            .map(|id| {
                let mut hart = self.primary.clone();
                hart.slot = id;
                hart.csr.hart_id =
                    (self.primary.csr.hart_id as u32).wrapping_add(id as u32) as usize;
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

    pub fn monitor_external_pmp(
        &mut self,
        path: &str,
        config: bool,
        index: u32,
        value: Option<u32>,
    ) -> Result<Option<u32>, String> {
        for hart in std::iter::once(&mut self.primary).chain(&mut self.secondary) {
            if let Some(pmp) = hart.csr.external_pmp(path) {
                let result = pmp.external_csr(config, index, value);
                if value.is_some() {
                    hart.translations.clear();
                }
                return Ok(result);
            }
        }
        Err(format!("unknown external PMP {path:?}"))
    }

    pub fn monitor_pc(&mut self, name: &str, value: Option<u32>) -> Result<Option<u32>, String> {
        let hart = std::iter::once(&mut self.primary)
            .chain(&mut self.secondary)
            .find(|hart| hart.name.as_deref() == Some(name))
            .ok_or_else(|| format!("unknown CPU {name:?}"))?;
        if let Some(value) = value {
            hart.pc = value;
            hart.pc_written = true;
            Ok(None)
        } else {
            Ok(Some(hart.pc))
        }
    }

    pub fn set_entry(&mut self, entry: u32) {
        for hart in std::iter::once(&mut self.primary).chain(&mut self.secondary) {
            hart.pc = entry;
            hart.pc_written = true;
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

    pub fn validate_entry(&mut self) -> Result<(), String> {
        for hart in std::iter::once(&mut self.primary).chain(&mut self.secondary) {
            if let Some(thumb) = &hart.thumb {
                let (sp, pc) = thumb.reset(&self.bus)?;
                if let Some(nvic) = self.bus.nvic.as_mut() {
                    nvic.set_vector_base(thumb.vector_base());
                }
                hart.registers[13] = sp;
                hart.pc = pc;
                continue;
            }
            hart.fetch(&mut self.bus)
                .map_err(|error| format!("hart {}: {error}", hart.csr.hart_id))?;
        }
        Ok(())
    }

    pub fn schedule_uart_input(&mut self, step: u64, bytes: Vec<u8>) -> Result<(), String> {
        if !self.bus.has_uart_input() {
            return Err("UART input requires a platform UART".into());
        }
        self.schedule_input(step, Input::Uart(bytes));
        Ok(())
    }

    pub fn schedule_button(
        &mut self,
        step: u64,
        name: String,
        action: ButtonAction,
    ) -> Result<(), String> {
        if !self.bus.has_litex_button(&name) {
            return Err(format!("unknown LiteX button {name:?}"));
        }
        self.schedule_input(step, Input::Button(name, action));
        Ok(())
    }

    pub fn schedule_phy_vendor_specific1(
        &mut self,
        step: u64,
        name: String,
        value: u16,
    ) -> Result<(), String> {
        self.bus.litex_phy_mut(&name)?;
        self.schedule_input(step, Input::PhyVendorSpecific1 { name, value });
        Ok(())
    }

    pub fn schedule_sensor_temperature(
        &mut self,
        step: u64,
        name: String,
        value: i32,
    ) -> Result<(), String> {
        self.bus.litex_sensor_mut(&name)?;
        self.schedule_input(step, Input::SensorTemperature { name, value });
        Ok(())
    }

    pub fn schedule_led_assertion(
        &mut self,
        step: u64,
        name: String,
        expected: bool,
    ) -> Result<(), String> {
        self.bus.litex_led_state(&name)?;
        self.schedule_input(step, Input::LedAssertion { name, expected });
        Ok(())
    }

    pub fn schedule_framebuffer_capture(&mut self, step: u64, path: PathBuf) -> Result<(), String> {
        if self.bus.litex_video.is_none() {
            return Err("--framebuffer-event requires a LiteX framebuffer".into());
        }
        self.schedule_input(step, Input::FramebufferCapture(path));
        Ok(())
    }

    pub fn schedule_frame_wait(
        &mut self,
        step: u64,
        reference: ReferenceFrame,
    ) -> Result<(), String> {
        if self.bus.litex_video.is_none() {
            return Err("framebuffer wait requires a LiteX framebuffer".into());
        }
        let reference = Box::new(reference);
        if self
            .inputs
            .iter()
            .any(|(_, input)| matches!(input, Input::FrameWait(_)))
        {
            self.queued_frame_waits.push_back((step, reference));
        } else {
            self.schedule_input(step, Input::FrameWait(reference));
        }
        Ok(())
    }

    pub fn intermediate_frame_matches(&self) -> &[u64] {
        &self.intermediate_frame_matches
    }

    pub fn verify_required_events_completed(&self) -> Result<(), String> {
        for (step, input) in &self.inputs {
            let event = match input {
                Input::LedAssertion { name, .. } => format!("LED assertion for {name:?}"),
                Input::FramebufferCapture(path) => {
                    format!("framebuffer capture to {}", path.display())
                }
                Input::FrameWait(_) => {
                    return Err("execution ended before the reference frame matched".into());
                }
                _ => continue,
            };
            return Err(format!("execution ended before {event} at step {step}"));
        }
        Ok(())
    }

    fn schedule_input(&mut self, step: u64, input: Input) {
        let index = self
            .inputs
            .partition_point(|(scheduled, _)| *scheduled <= step);
        self.inputs.insert(index, (step, input));
    }

    pub fn run(&mut self, limit: u64, output: &mut impl Write) -> Result<(Stop, u64), String> {
        let mut retired = 0;
        for _ in 0..limit {
            self.elapsed_steps = self.elapsed_steps.wrapping_add(1);
            while self
                .inputs
                .front()
                .is_some_and(|(step, _)| *step <= self.elapsed_steps)
            {
                match self.inputs.pop_front().unwrap().1 {
                    Input::Uart(bytes) => self.bus.queue_uart_input(bytes),
                    Input::Button(name, action) => self.bus.set_litex_button(&name, action)?,
                    Input::PhyVendorSpecific1 { name, value } => {
                        self.bus.litex_phy_mut(&name)?.set_vendor_specific1(value);
                    }
                    Input::SensorTemperature { name, value } => {
                        self.bus.litex_sensor_mut(&name)?.temperature = value;
                    }
                    Input::FrameWait(reference) => {
                        let video = self
                            .bus
                            .litex_video
                            .as_ref()
                            .ok_or("no LiteX framebuffer")?;
                        if video.matches(&self.bus, &reference)? {
                            let Some((step, reference)) = self.queued_frame_waits.pop_front()
                            else {
                                return Ok((Stop::FrameMatched(self.elapsed_steps), retired));
                            };
                            self.intermediate_frame_matches.push(self.elapsed_steps);
                            self.schedule_input(
                                step.max(self.elapsed_steps),
                                Input::FrameWait(reference),
                            );
                        } else {
                            const FRAME_POLL_STEPS: u64 = 1_000_000;
                            let next = self.elapsed_steps.checked_add(FRAME_POLL_STEPS).ok_or(
                                "step counter exhausted before the reference frame matched",
                            )?;
                            self.schedule_input(next, Input::FrameWait(reference));
                        }
                    }
                    Input::FramebufferCapture(path) => {
                        self.bus.capture_litex_framebuffer(&path).map_err(|error| {
                            format!(
                                "framebuffer capture at step {}: {error}",
                                self.elapsed_steps
                            )
                        })?;
                    }
                    Input::LedAssertion { name, expected } => {
                        let actual = self.bus.litex_led_state(&name)?;
                        if actual != expected {
                            let state = |on| if on { "on" } else { "off" };
                            return Err(format!(
                                "LED {name:?} at step {}: expected {}, got {}",
                                self.elapsed_steps,
                                state(expected),
                                state(actual)
                            ));
                        }
                    }
                }
            }
            let index = self.next_hart;
            if index == self.secondary.len() {
                self.next_hart = 0;
            } else {
                self.next_hart += 1;
            }
            match self.execute_hart(index, output)? {
                Step::Retired => retired += 1,
                Step::Trap => {}
                Step::Stop(stop) => return Ok((stop, retired)),
            }
            if let Some(code) = self.bus.tohost_exit {
                return Ok((Stop::Tohost(code), retired));
            }
        }
        Ok((Stop::Limit, retired))
    }

    fn execute_hart(&mut self, index: usize, output: &mut impl Write) -> Result<Step, String> {
        let hart = if index == 0 {
            &mut self.primary
        } else {
            &mut self.secondary[index - 1]
        };
        self.bus
            .tick_litex_uart(self.elapsed_steps, output)
            .map_err(|error| format!("UART: {error}"))?;
        hart.execute_step(
            &mut self.bus,
            self.elapsed_steps,
            output,
            self.function_symbols.as_ref(),
        )
    }
}

impl Hart {
    pub fn set_hart_id(&mut self, id: usize) {
        self.csr.hart_id = id;
    }

    pub fn configure_cortex_m(&mut self, vectors: Option<u32>) {
        self.thumb = Some(Thumb::new(vectors));
    }

    pub fn set_embedded_base(&mut self, enabled: bool) {
        self.csr.embedded_base = enabled;
    }

    pub fn embedded_base(&self) -> bool {
        self.csr.embedded_base
    }

    pub fn set_integer_extensions(&mut self, multiply: bool, atomic: bool, compressed: bool) {
        self.csr.m_extension = multiply;
        self.csr.a_extension = atomic;
        self.csr.c_extension = compressed;
    }

    pub fn enable_vector_bootstrap(&mut self) {
        self.csr.enable_vector_bootstrap();
    }

    pub fn enable_floating(&mut self, double: bool, half: bool) {
        self.csr.f_extension = true;
        self.csr.d_extension = double;
        self.zfh = half;
        self.floating = Some(Box::new([0; 32]));
    }

    pub fn set_bitmanip(&mut self, extensions: BitManip) {
        self.bitmanip = extensions;
    }

    pub fn set_sscofpmf(&mut self, enabled: bool) {
        self.csr.sscofpmf = enabled;
    }

    pub fn set_zacas(&mut self, enabled: bool) {
        self.zacas = enabled;
    }

    pub fn set_zicond(&mut self, enabled: bool) {
        self.zicond = enabled;
    }

    pub fn set_unaligned_accesses(&mut self, enabled: bool) {
        self.unaligned_accesses = enabled;
    }

    pub fn enable_ibex(&mut self) {
        self.csr.ibex = true;
    }

    pub fn enable_vex_core(&mut self, built_in_irq: bool) {
        self.csr.vex = true;
        self.csr.local_irq_mask = built_in_irq.then_some(0);
    }

    pub fn enable_minerva(&mut self) {
        self.csr.minerva = true;
        self.csr.local_irq_mask = Some(0);
    }

    pub fn set_vex_supervisor_irq_mask(&mut self, value: u32) -> Result<(), String> {
        if !self.csr.vex || self.csr.local_irq_mask.is_none() {
            return Err("SetSupervisorIrqMask requires a VexRiscv platform".into());
        }
        self.csr.vex_supervisor_irq_mask = value;
        Ok(())
    }

    pub fn set_vex_irq_mask(&mut self, value: u32) -> Result<(), String> {
        if !self.csr.vex {
            return Err("--vex-irq-mask requires a VexRiscv platform".into());
        }
        let mask = self
            .csr
            .local_irq_mask
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

    pub fn enable_address_triggers(&mut self) {
        self.csr.enable_address_triggers();
    }

    pub fn enable_smepmp(&mut self) {
        self.csr.enable_smepmp();
    }

    pub fn attach_external_pmp(&mut self, path: String, entries: usize) {
        self.csr.attach_external_pmp(path, entries);
    }

    pub fn enable_pmp(&mut self, entries: usize) {
        self.csr.enable_pmp(entries);
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
        if self.csr.address_trigger_matches(address, crate::pmp::READ) {
            return Err(Fault::Breakpoint(address));
        }
        if N > 1 && address as usize & (N - 1) != 0 && self.unaligned_accesses {
            let mut value = 0;
            for index in 0..N {
                let byte = self.load::<1>(bus, address.wrapping_add(index as u32))?;
                value |= byte << (index * 8);
            }
            return Ok(value);
        }
        self.translate(bus, address, N, crate::pmp::READ)
            .and_then(|physical| {
                bus.read::<N>(physical)
                    .map_err(|error| error.at_virtual(address))
            })
            .map_err(|error| Fault::Bus(Access::Load, error))
    }

    fn store(
        &mut self,
        bus: &mut Bus,
        address: u32,
        width: usize,
        value: u32,
        output: &mut impl Write,
    ) -> Result<(), Fault> {
        if self.csr.address_trigger_matches(address, crate::pmp::WRITE) {
            return Err(Fault::Breakpoint(address));
        }
        if address as usize & (width - 1) != 0 && self.unaligned_accesses {
            for (index, byte) in value.to_le_bytes()[..width].iter().enumerate() {
                self.store(
                    bus,
                    address.wrapping_add(index as u32),
                    1,
                    u32::from(*byte),
                    output,
                )?;
            }
            return Ok(());
        }
        self.translate(bus, address, width, crate::pmp::WRITE)
            .and_then(|physical| {
                bus.write(physical, width, value, output)
                    .map_err(|error| error.at_virtual(address))
            })
            .map_err(|error| Fault::Bus(Access::Store, error))
    }

    pub fn set_mstatus(&mut self, value: u32) {
        self.csr.set_mstatus(value);
        self.translations.clear();
    }

    pub fn configure_interrupt_mode(&mut self, mode: crate::csr::InterruptMode) {
        self.csr.configure_interrupt_mode(mode);
    }

    pub fn configure_reset_vector(&mut self, address: u32) {
        self.reset_vector = address;
    }

    pub fn initialize_reset_pc(&mut self) {
        self.pc = self.reset_vector;
    }

    fn set_reset_vector(&mut self, address: u32) {
        self.reset_vector = address;
        if !self.pc_written {
            self.pc = address;
        }
    }

    pub fn monitor_reset_vector(&mut self, address: u32) -> Result<(), String> {
        if self.thumb.is_some() {
            return Err("ResetVector requires a RISC-V CPU".into());
        }
        self.set_reset_vector(address);
        Ok(())
    }

    pub fn monitor_reset(&mut self, initial: &Self) -> Result<(), String> {
        if self.thumb.is_some() {
            return Err("CPU Reset requires a RISC-V CPU".into());
        }
        let reset_vector = self.reset_vector;
        *self = initial.clone();
        self.pc_written = false;
        self.set_reset_vector(reset_vector);
        Ok(())
    }

    pub fn monitor_integer_register(
        &mut self,
        index: u32,
        value: Option<u32>,
    ) -> Result<Option<u32>, String> {
        if self.thumb.is_some() {
            return Err("RISC-V register command requires a RISC-V CPU".into());
        }
        let count = if self.csr.embedded_base { 16 } else { 32 };
        let register = self.registers[..count]
            .get_mut(index as usize)
            .ok_or_else(|| format!("Wrong register index: {index}"))?;
        match value {
            Some(_) if index == 0 => Err("The 'X0' register is read-only.".into()),
            Some(value) => {
                *register = value;
                Ok(None)
            }
            None => Ok(Some(*register)),
        }
    }

    pub fn monitor_trap_vector(
        &mut self,
        register: crate::csr::TrapVector,
        value: Option<u32>,
    ) -> Result<Option<u32>, String> {
        if self.thumb.is_some() {
            return Err("RISC-V trap vector command requires a RISC-V CPU".into());
        }
        self.csr.monitor_trap_vector(register, value)
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
        output: &mut impl Write,
        symbols: Option<&BTreeMap<u32, String>>,
    ) -> Result<Step, String> {
        if let Some(thumb) = self.thumb.as_mut() {
            if thumb
                .service_interrupts(&mut self.pc, &mut self.registers, bus)
                .map_err(|error| format!("pc={:#010x}: {error}", self.pc))?
            {
                return Ok(Step::Trap);
            }
            return thumb
                .step(&mut self.pc, &mut self.registers, bus, output)
                .map(|retired| {
                    if retired {
                        Step::Retired
                    } else {
                        Step::Stop(Stop::Ebreak)
                    }
                })
                .map_err(|error| format!("pc={:#010x}: {error}", self.pc));
        }
        let clint = bus.clint.as_mut().map_or(0, |clint| clint.tick(self.slot));
        let mut pending = clint;
        let mut time = bus
            .clint
            .as_ref()
            .map_or(elapsed_steps, |clint| clint.time());
        if let Some(timer) = bus.litex_cpu_timer.as_mut() {
            if timer.tick() {
                pending |= crate::clint::TIMER;
            }
            time = timer.time;
        }
        let count_retired = self.csr.tick_counters(time);
        let uart = bus
            .uart
            .as_ref()
            .is_some_and(|uart| uart.interrupt_pending());
        let timer = bus.timer.as_mut().is_some_and(|(_, timer)| timer.tick());
        let plic = bus.tick_plic();
        let mut lines = bus.tick_litex(self.slot, elapsed_steps);
        if self.slot == 0 {
            lines |=
                u64::from((u32::from(uart) * EXTERNAL) | (u32::from(timer) * crate::clint::TIMER));
            if self.csr.vex && self.csr.local_irq_mask.is_some() {
                lines |= u64::from(plic);
            } else {
                pending |= plic;
            }
        }
        pending |= self.csr.sample_local_irq(lines);
        if (pending != 0
            || self.csr.has_local_irq_to_clear()
            || bus.clint.is_some()
            || bus.litex_cpu_timer.is_some()
            || bus.uart.is_some()
            || bus.timer.is_some()
            || bus.has_litex_irq()
            || bus.has_plic()
            || self.csr.has_injected_interrupts())
            && let Some(target) = self.csr.interrupt(self.pc, pending)
        {
            self.pc = target;
            return Ok(Step::Trap);
        }
        match self.step(bus, output, symbols) {
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

    fn uses_rv32e_registers(insn: u32) -> bool {
        const RD_HIGH: u32 = 1 << 11;
        const RS1_HIGH: u32 = 1 << 19;
        const RS2_HIGH: u32 = 1 << 24;

        let mask = match insn & 0x7f {
            0x17 | 0x37 | 0x6f => RD_HIGH,
            0x03 | 0x13 | 0x67 => RD_HIGH | RS1_HIGH,
            0x23 | 0x63 => RS1_HIGH | RS2_HIGH,
            0x2f | 0x33 => RD_HIGH | RS1_HIGH | RS2_HIGH,
            0x5b => match (insn >> 12) & 7 {
                0 => RD_HIGH | RS1_HIGH | RS2_HIGH,
                2 | 3 => RD_HIGH | RS1_HIGH,
                5..=7 => RS1_HIGH,
                _ => 0,
            },
            0x73 => match (insn >> 12) & 7 {
                1..=3 => RD_HIGH | RS1_HIGH,
                5..=7 => RD_HIGH,
                _ => 0,
            },
            _ => 0,
        };
        insn & mask == 0
    }

    fn step(
        &mut self,
        bus: &mut Bus,
        output: &mut impl Write,
        symbols: Option<&BTreeMap<u32, String>>,
    ) -> Result<Step, Fault> {
        let pc = self.pc;
        if self.csr.address_trigger_matches(pc, crate::pmp::EXECUTE) {
            return Err(Fault::Breakpoint(pc));
        }
        let (encoded, width) = self
            .fetch(bus)
            .map_err(|error| Fault::Bus(Access::Fetch, error))?;
        if let Some(name) = symbols.and_then(|symbols| symbols.get(&pc)) {
            eprintln!("Entering function {name} at 0x{pc:X}");
        }
        let illegal = || Fault::Illegal(encoded);
        let insn = if width == 2 {
            crate::compressed::expand(encoded, self.bitmanip.zcb).ok_or_else(illegal)?
        } else {
            encoded
        };
        if self.csr.embedded_base && !Self::uses_rv32e_registers(insn) {
            return Err(illegal());
        }
        let rd = ((insn >> 7) & 31) as usize;
        let a = self.registers[((insn >> 15) & 31) as usize];
        let b = self.registers[((insn >> 20) & 31) as usize];
        let funct3 = (insn >> 12) & 7;
        let funct7 = insn >> 25;
        let imm = sign(insn >> 20, 12);
        let fallthrough = pc.wrapping_add(width);
        let mut next = fallthrough;

        // Fixed Renode exposes the compressed extensions even without scalar Zbb.
        let bitmanip = BitManip {
            zbb: self.bitmanip.zbb || (width == 2 && self.bitmanip.zcb),
            ..self.bitmanip
        };
        let result = match insn & 0x7f {
            0x07 | 0x27 | 0x43 | 0x47 | 0x4b | 0x4f | 0x53 => self
                .floating_instruction(insn, bus, output)
                .map_err(|fault| {
                    if matches!(fault, Fault::Illegal(_)) {
                        illegal()
                    } else {
                        fault
                    }
                })?,
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
                self.store(bus, address, width, b, output)?;
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
                _ => bitmanip
                    .immediate(funct3, funct7, a, imm)
                    .ok_or_else(illegal)?,
            }),
            0x33 => Some(match (funct3, funct7) {
                (_, 1) if self.csr.m_extension => multiply_divide(funct3, a, b),
                (5, 7) if self.zicond => {
                    if b == 0 {
                        0
                    } else {
                        a
                    }
                }
                (7, 7) if self.zicond => {
                    if b != 0 {
                        0
                    } else {
                        a
                    }
                }
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
                _ => bitmanip
                    .register(funct3, funct7, a, b, (insn >> 20) & 31)
                    .ok_or_else(illegal)?,
            }),
            0x2f if insn >> 27 == 5 && self.zacas => {
                let rs2 = ((insn >> 20) & 31) as usize;
                let width = match funct3 {
                    2 => 4,
                    3 if (rd | rs2) & 1 == 0 => 8,
                    _ => return Err(illegal()),
                };
                let operand = |reg| {
                    if width == 8 && reg != 0 {
                        u64::from(self.registers[reg]) | (u64::from(self.registers[reg + 1]) << 32)
                    } else {
                        u64::from(self.registers[reg])
                    }
                };
                let (expected, replacement) = (operand(rd), operand(rs2));
                let permissions = crate::pmp::READ | crate::pmp::WRITE;
                if self.csr.address_trigger_matches(a, permissions) {
                    return Err(Fault::Breakpoint(a));
                }
                let physical = self
                    .translate(bus, a, width, permissions)
                    .map_err(|error| Fault::Bus(Access::Store, error))?;
                let previous = bus
                    .atomic_compare_swap(physical, width, expected, replacement)
                    .map_err(|error| Fault::Bus(Access::Store, error.at_virtual(a)))?;
                if width == 8 && rd != 0 {
                    self.registers[rd + 1] = (previous >> 32) as u32;
                }
                Some(previous as u32)
            }
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
                if self.csr.address_trigger_matches(a, permissions) {
                    return Err(Fault::Breakpoint(a));
                }
                let physical = self
                    .translate(bus, a, 4, permissions)
                    .map_err(|error| Fault::Bus(access, error))?;
                let value = match operation {
                    2 if rs2 == 0 => bus
                        .load_reserved(self.slot, physical)
                        .map_err(|error| Fault::Bus(Access::Load, error.at_virtual(a))),
                    3 => bus
                        .store_conditional(self.slot, physical, b)
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
            // Renode VexRiscv accepts the custom whole-data-cache flush.
            0x0f if insn == 0x0000_500f && self.csr.vex => None,
            0x5b if funct3 == 0 => Some(bitmanip.andes_register(funct7, a, b).ok_or_else(illegal)?),
            0x5b if bitmanip.xandesperf && matches!(funct3, 2 | 3) && insn & 0x8200_0000 == 0 => {
                Some(BitManip::andes_bit_field(insn, a, funct3 == 3))
            }
            0x5b if bitmanip.xandesperf && matches!(funct3, 5..=7) => {
                let immediate = (insn >> 20) & 31;
                let bit30 = (insn >> 30) & 1;
                let bit7 = (insn >> 7) & 1;
                let taken = if funct3 == 7 {
                    if bit7 != 0 {
                        return Err(illegal());
                    }
                    ((a >> immediate) & 1) == bit30
                } else {
                    let constant = (bit30 << 6) | (bit7 << 5) | immediate;
                    (a == constant) == (funct3 == 5)
                };
                if taken {
                    let offset =
                        ((insn >> 31) << 10) | ((insn >> 20) & 0x3e0) | ((insn >> 7) & 0x1e);
                    next = pc.wrapping_add(sign(offset, 11));
                }
                None
            }
            0x57 if !self.csr.vector_configured() => {
                eprintln!(
                    "PC: 0x{pc:x}, opcode: 0x{encoded:08x}, RISC-V 'V' instruction set is not enabled for this CPU!"
                );
                return Err(illegal());
            }
            0x57 if funct3 == 7 => Some(self.csr.configure_vector(insn, a, b).ok_or_else(illegal)?),
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
                if matches!(insn >> 20, 0x180 | 0x3a0..=0x3ef | 0x747) {
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

    #[test]
    fn upstream_rv32_writes_invalidate_other_harts_reservations() {
        // riscv-lr-sc-strong-atomicity.robot runs only the two selected RV32 harts.
        let directory =
            std::env::temp_dir().join(format!("penv-rv32-lr-sc-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let repl = directory.join("cpu.repl");
        std::fs::write(
            &repl,
            r#"dram: Memory.MappedMemory @ sysbus 0x80000000 { size: 0x80000000 }
mmio: Memory.ArrayMemory @ sysbus 0x100000 { size: 0x10000 }
cpu32_0: CPU.RiscV32 @ sysbus
    cpuType: "rv32gc_zicsr_zifencei"
    hartId: 2
    privilegedArchitecture: PrivilegedArchitecture.Priv1_10
    timeProvider: empty
    allowUnalignedAccesses: true
cpu32_1: CPU.RiscV32 @ sysbus
    cpuType: "rv32gc_zicsr_zifencei"
    hartId: 3
    privilegedArchitecture: PrivilegedArchitecture.Priv1_10
    timeProvider: empty
    allowUnalignedAccesses: true
"#,
        )
        .unwrap();
        let mut machine = crate::platform::load(&repl, None).unwrap();
        const CORE0: u32 = 0x8000_0000;
        const CORE1: u32 = 0x8000_0100;
        const NEW: u32 = 0xbeee_eeef;
        let mut output = std::io::sink();
        for address in [0x8100_0000, 0x0010_0000] {
            for (name, writer) in [
                ("lr.w/sc.w", &[0x1005_282fu32, 0x18e5_292f][..]),
                ("amoswap.w", &[0x08e5_292f]),
                ("amoadd.w", &[0x0005_272f]),
                ("sb", &[0x00e5_0023]),
                ("sh", &[0x00e5_1023]),
                ("sw", &[0x00e5_2023]),
            ] {
                machine.monitor_pc("cpu32_0", Some(CORE0)).unwrap();
                machine.monitor_pc("cpu32_1", Some(CORE1)).unwrap();
                machine
                    .bus
                    .memory(address, 8)
                    .unwrap()
                    .copy_from_slice(&5u64.to_le_bytes());
                for slot in 0..2 {
                    let hart = machine.hart_mut(slot);
                    hart.registers[10] = address;
                    hart.registers[12] = NEW;
                    hart.registers[14] = 5;
                }
                for (pc, code) in [(CORE0, &[0x1005_25afu32, 0x18c5_26af][..]), (CORE1, writer)] {
                    for (index, instruction) in code.iter().enumerate() {
                        machine
                            .bus
                            .memory(pc + index as u32 * 4, 4)
                            .unwrap()
                            .copy_from_slice(&instruction.to_le_bytes());
                    }
                }
                let mut step = |slot| {
                    machine.elapsed_steps += 1;
                    assert!(matches!(
                        machine.execute_hart(slot, &mut output).unwrap(),
                        Step::Retired
                    ));
                };
                step(0);
                for _ in writer {
                    step(1);
                }
                step(0);

                let lower = machine.bus.read_ram::<4>(address).unwrap();
                let upper = machine.bus.read_ram::<4>(address + 4).unwrap();
                assert_ne!(
                    u64::from(lower) | (u64::from(upper) << 32),
                    u64::from(NEW),
                    "{name} {address:#x}"
                );
                assert_eq!(machine.primary.registers[13], 1, "{name} {address:#x}");
            }
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn upstream_rv32_amocas_single_page() {
        // Renode riscv-amocas.robot selects only cpu32 for these single-step cases.
        let directory =
            std::env::temp_dir().join(format!("penv-rv32-amocas-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let repl = directory.join("cpu.repl");
        std::fs::write(
            &repl,
            r#"cpu32: CPU.RiscV32 @ sysbus
    cpuType: "rv32gc_zicsr_zifencei_zacas"
    hartId: 2
    privilegedArchitecture: PrivilegedArchitecture.Priv1_10
    timeProvider: empty
    allowUnalignedAccesses: true
dram: Memory.MappedMemory @ sysbus 0x80000000
    size: 0x80000000
mtvec: Memory.MappedMemory @ sysbus 0x1000
    size: 0x40000
"#,
        )
        .unwrap();
        const DATA: u32 = RAM_BASE + 0x1000;
        const OLD: u64 = 0x1afe_dead_beef_d00d;
        const NEW: u64 = 0xbeef_f00d_babe_dead;
        const ADJACENT: u64 = 0x2bad_c000_10ff_b0ba;
        for (width, rd, rs2, old, expected, new, result, illegal) in [
            (
                4,
                10,
                18,
                OLD,
                OLD & 0xffff_ffff,
                0xbeef_babe,
                (OLD & !0xffff_ffff) | 0xbeef_babe,
                false,
            ),
            (4, 10, 18, OLD, 0x1234, 0xbeef_babe, OLD, false),
            (8, 10, 18, OLD, OLD, NEW, NEW, false),
            (8, 10, 0, OLD, OLD, 0, 0, false),
            (8, 0, 18, 0, 0, NEW, NEW, false),
            (8, 10, 18, OLD, 0x1_2345_6789_1011, NEW, OLD, false),
            (8, 11, 19, OLD, OLD, NEW, OLD, true),
        ] {
            let mut machine = crate::platform::load(&repl, None).unwrap();
            assert_eq!(machine.primary.csr.access(0xf140_2073, 0).unwrap(), 2);
            machine.set_entry(RAM_BASE);
            let opcode = 0x2800_002fu32
                | (rs2 as u32) << 20
                | 13 << 15
                | (if width == 4 { 2 } else { 3 }) << 12
                | (rd as u32) << 7;
            machine
                .bus
                .memory(RAM_BASE, 4)
                .unwrap()
                .copy_from_slice(&opcode.to_le_bytes());
            machine
                .bus
                .memory(DATA, 8)
                .unwrap()
                .copy_from_slice(&old.to_le_bytes());
            machine
                .bus
                .memory(DATA + 8, 8)
                .unwrap()
                .copy_from_slice(&ADJACENT.to_le_bytes());
            let registers = &mut machine.primary.registers;
            if rd == 0 || rs2 == 0 {
                registers[1] = 0x1234;
            }
            for (register, value) in [(rd, expected), (rs2, new)] {
                if register != 0 {
                    registers[register] = value as u32;
                    if width == 8 {
                        registers[register + 1] = (value >> 32) as u32;
                    }
                }
            }
            registers[13] = DATA;
            let before = *registers;
            machine.run(1, &mut std::io::sink()).unwrap();
            let read = |address| machine.bus.read_ram::<4>(address).unwrap();
            assert_eq!(
                u64::from(read(DATA)) | u64::from(read(DATA + 4)) << 32,
                result
            );
            assert_eq!(
                u64::from(read(DATA + 8)) | u64::from(read(DATA + 12)) << 32,
                ADJACENT
            );
            let mut after = before;
            if illegal {
                let csr = &mut machine.primary.csr;
                assert_eq!(machine.primary.pc, 0x1010);
                assert_eq!(csr.access(0x3420_2073, 0).unwrap(), 2);
                assert_eq!(csr.access(0x3430_2073, 0).unwrap(), opcode);
                assert_eq!(csr.access(0x3410_2073, 0).unwrap(), RAM_BASE);
            } else {
                assert_eq!(machine.primary.pc, RAM_BASE + 4);
                if rd != 0 {
                    after[rd] = old as u32;
                    if width == 8 {
                        after[rd + 1] = (old >> 32) as u32;
                    }
                }
            }
            assert_eq!(machine.primary.registers, after, "{opcode:#010x}");
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rv32_amocas_double_handles_register_pairs_and_faults() {
        let data = RAM_BASE + 16;
        for (rd, rs2, enabled, address, initial, expected, cause) in [
            (4, 6, true, data, 3, 5, None),
            (4, 6, true, data, 7, 7, None),
            (0, 6, true, data, 0, 5, None),
            (4, 0, true, data, 3, 0, None),
            (5, 6, true, data, 3, 3, Some(2)),
            (4, 7, true, data, 3, 3, Some(2)),
            (4, 6, false, data, 3, 3, Some(2)),
            (4, 6, true, data + 4, 3, 3, Some(6)),
        ] {
            let mut machine = Machine::new(32).unwrap();
            machine.primary.set_zacas(enabled);
            machine.primary.set_mtvec(RAM_BASE + 4);
            // aqrl, rs1=x2, funct3=3 (doubleword).
            let instruction = 0x2e01_302fu32 | ((rs2 as u32) << 20) | ((rd as u32) << 7);
            machine
                .bus
                .memory(RAM_BASE, 4)
                .unwrap()
                .copy_from_slice(&instruction.to_le_bytes());
            let pair = |value: u32| u64::from(value) * 0x1_0000_0001;
            machine
                .bus
                .memory(data, 8)
                .unwrap()
                .copy_from_slice(&pair(initial).to_le_bytes());
            machine.primary.registers[1] = 42;
            machine.primary.registers[2] = address;
            machine.primary.registers[4..8].copy_from_slice(&[3, 3, 5, 5]);
            let (_, retired) = machine.run(1, &mut std::io::sink()).unwrap();
            assert_eq!(machine.bus.read_ram::<4>(data).unwrap(), expected);
            assert_eq!(machine.bus.read_ram::<4>(data + 4).unwrap(), expected);
            assert_eq!(machine.primary.registers[1], 42);
            if let Some(cause) = cause {
                assert_eq!(retired, 0);
                assert_eq!(machine.primary.csr.access(0x3420_2073, 0).unwrap(), cause);
                assert_eq!(&machine.primary.registers[4..6], &[3, 3]);
            } else {
                assert_eq!(retired, 1);
                if rd != 0 {
                    assert_eq!(&machine.primary.registers[rd..rd + 2], &[initial, initial]);
                }
            }
        }
    }

    #[test]
    fn zicond_rv32_matches_zero_nonzero_and_disabled_cases() {
        // Renode riscv-zicond-extension.robot: RV32 data and trap cases.
        for (funct3, selector, expected) in
            [(5, 0, 0), (5, 1, u32::MAX), (7, 0, u32::MAX), (7, 1, 0)]
        {
            for enabled in [false, true] {
                let mut machine = Machine::new(8).unwrap();
                machine.primary.set_zicond(enabled);
                machine.primary.set_mtvec(RAM_BASE + 4);
                machine.primary.registers[10] = u32::MAX;
                machine.primary.registers[11] = selector;
                // Use rd=rs1 to also check read-before-write behavior.
                let instruction = 0x0eb5_0533u32 | (funct3 << 12);
                machine
                    .bus
                    .memory(RAM_BASE, 4)
                    .unwrap()
                    .copy_from_slice(&instruction.to_le_bytes());
                let (_, retired) = machine.run(1, &mut std::io::sink()).unwrap();
                if enabled {
                    assert_eq!(retired, 1);
                    assert_eq!(machine.primary.registers[10], expected);
                } else {
                    assert_eq!(retired, 0);
                    assert_eq!(machine.primary.pc, RAM_BASE + 4);
                    assert_eq!(machine.primary.csr.access(0x3420_2073, 0).unwrap(), 2);
                    assert_eq!(
                        machine.primary.csr.access(0x3410_2073, 0).unwrap(),
                        RAM_BASE
                    );
                }
            }
        }
    }

    #[test]
    fn vexriscv_accepts_its_data_cache_operations() {
        let mut machine = Machine::new(16).unwrap();
        let instructions = [
            0x0000_500fu32, // VexRiscv data cache flush
            0xcc0a_d073,    // csrwi 0xcc0, 21
            0xcc00_2573,    // csrr a0, 0xcc0
            0x0010_0073,    // ebreak
        ];
        machine
            .bus
            .memory(RAM_BASE, 16)
            .unwrap()
            .copy_from_slice(instructions.map(u32::to_le_bytes).as_flattened());
        assert!(
            machine
                .run(1, &mut std::io::sink())
                .unwrap_err()
                .contains("unsupported instruction")
        );
        machine.primary.enable_vex_core(true);
        assert_eq!(
            machine.run(5, &mut std::io::sink()).unwrap(),
            (Stop::Ebreak, 3)
        );
        assert_eq!(machine.primary.registers[10], 21);
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
    fn upstream_rv32_andes_self_branches_preserve_a0() {
        for (opcode, a0) in [(0x0005_705bu32, 0), (0x4005_705b, 1)] {
            let mut machine = Machine::new(4).unwrap();
            machine.primary.set_bitmanip(BitManip {
                xandesperf: true,
                ..BitManip::default()
            });
            machine.primary.registers[10] = a0;
            machine
                .bus
                .memory(RAM_BASE, 4)
                .unwrap()
                .copy_from_slice(&opcode.to_le_bytes());
            assert_eq!(
                machine.run(1, &mut std::io::sink()).unwrap(),
                (Stop::Limit, 1)
            );
            assert_eq!(machine.primary.pc, RAM_BASE);
            assert_eq!(machine.primary.registers[10], a0);
        }
    }

    #[test]
    fn upstream_rv32_rori() {
        // Renode riscv-zbb-extension.robot: Should Handle RORI Properly on 32 Bit CPU.
        let directory = std::env::temp_dir().join(format!("penv-rv32-rori-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let repl = directory.join("cpu.repl");
        std::fs::write(
            &repl,
            r#"cpu: CPU.RiscV32 @ sysbus { cpuType: "rv32imac_zicsr_zba_zbb"; timeProvider: empty }
mem: Memory.MappedMemory @ sysbus 0x0 { size: 0x1000 }
"#,
        )
        .unwrap();
        let mut machine = crate::platform::load(&repl, None).unwrap();
        machine.monitor_pc("cpu", Some(0)).unwrap();
        for (address, opcode) in [(0, 0x4505u16), (2, 0x0505)] {
            machine
                .bus
                .memory(address, 2)
                .unwrap()
                .copy_from_slice(&opcode.to_le_bytes());
        }
        for address in [4, 8] {
            machine
                .bus
                .memory(address, 4)
                .unwrap()
                .copy_from_slice(&0x6015_5513u32.to_le_bytes());
        }
        let mut output = std::io::sink();
        for (pc, expected) in [(2, 1), (4, 2), (8, 1), (12, 0x8000_0000)] {
            assert_eq!(machine.run(1, &mut output).unwrap(), (Stop::Limit, 1));
            assert_eq!(machine.primary.pc, pc);
            assert_eq!(
                machine.primary.monitor_integer_register(10, None).unwrap(),
                Some(expected)
            );
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn upstream_rv32_zbkb() {
        // Renode riscv-bkb.robot: exact RV32 input, result and non-trap checks.
        let directory = std::env::temp_dir().join(format!("penv-rv32-zbkb-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let repl = directory.join("cpu.repl");
        std::fs::write(
            &repl,
            r#"cpu: CPU.RiscV32 @ sysbus { cpuType: "rv32gc_zbkb"; timeProvider: empty }
dram: Memory.MappedMemory @ sysbus 0x80000000 { size: 0x80000000 }
mtvec: Memory.MappedMemory @ sysbus 0x1000 { size: 0x40000 }
"#,
        )
        .unwrap();
        let mut machine = crate::platform::load(&repl, None).unwrap();
        for (opcode, rd, inputs) in [
            (
                0x08b5_4633u32,
                12,
                &[(0xffff_beef, 0xffff_dead, 0xdead_beef)][..],
            ),
            (0x08b5_7633, 12, &[(0xffff_ffcd, 0xffff_ffab, 0xabcd)]),
            (
                0x08f5_1593,
                11,
                &[
                    (u32::MAX, 0, u32::MAX),
                    (0xffff, 0, 0x5555_5555),
                    (0xffff_0000, 0, 0xaaaa_aaaa),
                    (0xdead_beef, 0, 0xe7fc_dcf7),
                ],
            ),
            (
                0x08f5_5593,
                11,
                &[
                    (u32::MAX, 0, u32::MAX),
                    (0x5555_5555, 0, 0xffff),
                    (0xaaaa_aaaa, 0, 0xffff_0000),
                ],
            ),
            (
                0x6875_5593,
                11,
                &[(u32::MAX, 0, u32::MAX), (0xdead_beaf, 0, 0x7bb5_7df5)],
            ),
        ] {
            machine.set_entry(RAM_BASE);
            for &(a, b, expected) in inputs {
                let pc = machine.primary.pc;
                machine.primary.registers[10] = a;
                machine.primary.registers[11] = b;
                machine
                    .bus
                    .memory(pc, 4)
                    .unwrap()
                    .copy_from_slice(&opcode.to_le_bytes());
                assert_eq!(
                    machine.run(1, &mut std::io::sink()).unwrap(),
                    (Stop::Limit, 1)
                );
                assert_ne!(machine.primary.pc, 0x1010);
                assert_eq!(
                    machine.primary.registers[rd], expected,
                    "{opcode:#010x}, a0={a:#010x}"
                );
            }
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn upstream_rv32_monitor_rejects_writes_to_x0() {
        let directory = std::env::temp_dir().join(format!("penv-rv32-x0-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let repl = directory.join("cpu.repl");
        std::fs::write(
            &repl,
            r#"clint: IRQControllers.CoreLevelInterruptor @ sysbus 0x44000000 { frequency: 66000000 }
cpu: CPU.RiscV32 @ sysbus { timeProvider: clint; cpuType: "rv32gc_Zfh" }
mem: Memory.MappedMemory @ sysbus 0x1000 { size: 0x40000 }
"#,
        )
        .unwrap();
        let mut machine = crate::platform::load(&repl, None).unwrap();
        machine.set_entry(0x2000);
        let hart = &mut machine.primary;
        let error = hart.monitor_integer_register(0, Some(1)).unwrap_err();
        assert!(error.contains("register is read-only"), "{error}");
        assert_eq!(hart.monitor_integer_register(0, None).unwrap(), Some(0));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn upstream_rv32_unit_data_accesses() {
        // Renode riscv-unit-tests.robot: RV32 LB/LH/LW/SB/SH/SW cases.
        let directory = std::env::temp_dir().join(format!("penv-rv32-data-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let repl = directory.join("cpu.repl");
        std::fs::write(
            &repl,
            r#"clint: IRQControllers.CoreLevelInterruptor @ sysbus 0x44000000 { frequency: 66000000 }
cpu: CPU.RiscV32 @ sysbus { timeProvider: clint; cpuType: "rv32gc_Zfh" }
mem: Memory.MappedMemory @ sysbus 0x1000 { size: 0x40000 }
"#,
        )
        .unwrap();
        for (name, opcode, a1, cause) in [
            ("Should Handle LB", 0x0055_8503u32, 0, None),
            ("Should Handle LH", 0x0055_9503, 1, None),
            ("Should Fail LH", 0x0055_9503, 0, Some(4)),
            ("Should Handle LW", 0x0055_a503, 3, None),
            ("Should Fail LW", 0x0055_a503, 0, Some(4)),
            ("Should Handle SB", 0x00a5_82a3, 0, None),
            ("Should Handle SH", 0x00a5_92a3, 1, None),
            ("Should Fail SH", 0x00a5_92a3, 0, Some(6)),
            ("Should Handle SW", 0x00a5_a2a3, 3, None),
            ("Should Fail SW", 0x00a5_a2a3, 0, Some(6)),
        ] {
            let mut machine = crate::platform::load(&repl, None).unwrap();
            machine.set_entry(0x2000);
            machine.primary.registers[11] = a1;
            machine
                .bus
                .memory(0x2000, 4)
                .unwrap()
                .copy_from_slice(&opcode.to_le_bytes());
            machine.run(1, &mut std::io::sink()).unwrap();
            let expected_pc = if cause.is_some() { 0x1010 } else { 0x2004 };
            assert_eq!(machine.primary.pc, expected_pc, "{name}");
            if let Some(cause) = cause {
                assert_eq!(
                    machine.primary.csr.access(0x3420_2073, 0).unwrap(),
                    cause,
                    "{name}"
                );
            }
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn upstream_rv32_unit_traps_and_sret() {
        // Renode riscv-unit-tests.robot: three MEPC cases, EBREAK/ECALL,
        // and SRET executed in machine mode.
        let directory =
            std::env::temp_dir().join(format!("penv-rv32-traps-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let repl = directory.join("cpu.repl");
        std::fs::write(
            &repl,
            r#"clint: IRQControllers.CoreLevelInterruptor @ sysbus 0x44000000 { frequency: 66000000 }
cpu: CPU.RiscV32 @ sysbus { timeProvider: clint; cpuType: "rv32gc_Zfh" }
mem: Memory.MappedMemory @ sysbus 0x1000 { size: 0x40000 }
"#,
        )
        .unwrap();
        let write_word = |machine: &mut Machine, address, word: u32| {
            machine
                .bus
                .memory(address, 4)
                .unwrap()
                .copy_from_slice(&word.to_le_bytes());
        };
        let mut output = std::io::sink();
        for (name, word) in [
            ("Should Set MEPC on Illegal Instruction", 0x0000_8000),
            ("Should Set MEPC on Illegal CSR access", 0xf120_d073),
            ("Should Set MEPC on Non-Existing CSR access", 0xfff0_d073),
            ("Should Allow SRET In Machine Mode", 0x1020_0073),
        ] {
            let mut machine = crate::platform::load(&repl, None).unwrap();
            machine.set_entry(0x2000);
            write_word(&mut machine, 0x1010, 0x0000_006f); // j .
            write_word(&mut machine, 0x2000, 0x0000_206f); // j 0x4000
            write_word(&mut machine, 0x4000, 0x0000_0013); // nop
            write_word(&mut machine, 0x4004, word);
            if word == 0x1020_0073 {
                machine.primary.csr.access(0x1410_9073, 0x1234).unwrap();
                machine.run(3, &mut output).unwrap();
                assert_eq!(machine.primary.pc, 0x1234, "{name}");
            } else {
                // penv always validates CSR accesses, as in the upstream Full mode.
                machine.run(3, &mut output).unwrap();
                assert_eq!(machine.primary.pc, 0x1010, "{name}");
                let csr = &mut machine.primary.csr;
                assert_eq!(csr.access(0x3420_2073, 0).unwrap(), 2, "{name}");
                assert_eq!(csr.access(0x3430_2073, 0).unwrap(), word, "{name}");
                assert_eq!(csr.access(0x3410_2073, 0).unwrap(), 0x4004, "{name}");
            }
        }

        let mut machine = crate::platform::load(&repl, None).unwrap();
        machine.primary.set_mtvec(0x1010);
        write_word(&mut machine, 0x1010, 0x0000_006f); // j .
        write_word(&mut machine, 0x2000, 0x0010_0073); // ebreak
        write_word(&mut machine, 0x2004, 0x0000_0073); // ecall
        for (address, cause) in [(0x2000, 3), (0x2004, 11)] {
            machine.set_entry(address);
            machine.primary.csr.access(0x3430_9073, 0x1234).unwrap();
            machine.run(1, &mut output).unwrap();
            assert_eq!(machine.primary.pc, 0x1010);
            assert_eq!(machine.primary.csr.access(0x3420_2073, 0).unwrap(), cause);
            assert_eq!(machine.primary.csr.access(0x3430_2073, 0).unwrap(), 0);
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn upstream_rv32_mret_restores_pc_and_sets_mpp_to_lowest_privilege() {
        // Renode riscv-mret.robot: User Level [Does Not] Exist[s] 32Bits.
        const START: u32 = 0x2000;
        const TRAP: u32 = 0x4000;
        const CLINT: u32 = 0x4400_0000;
        let directory = std::env::temp_dir().join(format!("penv-rv32-mret-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let repl = directory.join("cpu.repl");
        for (levels, lowest) in [("MachineUser", 0), ("Machine", 3)] {
            std::fs::write(
                &repl,
                format!(
                    r#"clint: IRQControllers.CoreLevelInterruptor @ sysbus 0x44000000 {{ frequency: 66000000 }}
cpu: CPU.RiscV32 @ sysbus {{ timeProvider: clint; cpuType: "rv32gc"; privilegeLevels: PrivilegeLevels.{levels}; interruptMode: 1 }}
mem: Memory.MappedMemory @ sysbus 0x1000 {{ size: 0x40000 }}
"#
                ),
            )
            .unwrap();
            let mut machine = crate::platform::load(&repl, None).unwrap();
            machine.set_entry(START);
            machine.primary.set_mtvec(TRAP);
            for (address, word) in [(START, 0xc000_1073u32), (TRAP, 0x3020_0073)] {
                machine
                    .bus
                    .memory(address, 4)
                    .unwrap()
                    .copy_from_slice(&word.to_le_bytes());
            }

            let mut output = std::io::sink();
            machine.run(1, &mut output).unwrap();
            assert_eq!(machine.primary.pc, TRAP, "{levels}");
            assert_eq!(
                (machine.primary.csr.access(0x3000_2073, 0).unwrap() >> 11) & 3,
                3,
                "{levels}"
            );

            machine.run(1, &mut output).unwrap();
            assert_eq!(machine.primary.pc, START, "{levels}");
            assert_eq!(
                (machine.primary.csr.access(0x3000_2073, 0).unwrap() >> 11) & 3,
                lowest,
                "{levels}"
            );

            // The fixture supplies time and MMIO without wiring interrupt outputs.
            let clint = machine.bus.clint.as_mut().unwrap();
            assert_eq!(clint.time(), 1);
            clint.write(CLINT, 1);
            clint.write(CLINT + 0x4000, 0);
            clint.write(CLINT + 0x4004, 0);
            assert_eq!(clint.tick(0), 0);
            assert_eq!(clint.read(CLINT), 1);
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn privilege_controls_gate_sret_and_address_translation_operations() {
        // Renode riscv-priv.robot: RV32 TSR and U-mode SFENCE.VMA cases,
        // plus the equivalent RV32 instructions for its supervisor TVM cases.
        const HANDLER: u32 = RAM_BASE + 12;
        const SRET_TARGET: u32 = RAM_BASE + 16;
        const SUPERVISOR: u32 = (1 << 11) | (1 << 8); // MPP=S, SPP=S
        const TSR: u32 = 1 << 22;
        const TVM: u32 = 1 << 20;
        for (instruction, status, expected_pc) in [
            (0x1020_0073u32, SUPERVISOR | TSR, HANDLER), // sret
            (0x1020_0073, SUPERVISOR, SRET_TARGET),
            (0x1200_0073, 0, HANDLER), // sfence.vma in U
            (0x1200_0073, SUPERVISOR | TVM, HANDLER),
            (0x1200_0073, SUPERVISOR, RAM_BASE + 8),
            (0x1800_2573, SUPERVISOR | TVM, HANDLER), // csrr a0, satp
            (0x1800_2573, SUPERVISOR, RAM_BASE + 8),
            (0x1800_1073, SUPERVISOR | TVM, HANDLER), // csrw satp, zero
            (0x1800_1073, SUPERVISOR, RAM_BASE + 8),
        ] {
            let mut machine = Machine::new(20).unwrap();
            machine.primary.enable_user_mode();
            machine.primary.enable_supervisor_mode();
            machine.primary.set_mtvec(HANDLER);
            machine.primary.csr.access(0x3000_9073, status).unwrap();
            assert_eq!(
                machine.primary.csr.access(0x3000_2073, 0).unwrap() & (TSR | TVM),
                status & (TSR | TVM)
            );
            machine
                .primary
                .csr
                .access(0x3410_9073, RAM_BASE + 4)
                .unwrap();
            machine
                .primary
                .csr
                .access(0x1410_9073, SRET_TARGET)
                .unwrap();
            machine.bus.memory(RAM_BASE, 8).unwrap().copy_from_slice(
                [0x3020_0073u32, instruction]
                    .map(u32::to_le_bytes)
                    .as_flattened(),
            );
            let mut output = std::io::sink();
            assert_eq!(machine.run(1, &mut output).unwrap(), (Stop::Limit, 1));
            let trapped = status == 0 || status & (TSR | TVM) != 0;
            assert_eq!(
                machine.run(1, &mut output).unwrap(),
                (Stop::Limit, u64::from(!trapped))
            );
            assert_eq!(machine.primary.pc, expected_pc);
            if trapped {
                assert_eq!(machine.primary.csr.access(0x3420_2073, 0).unwrap(), 2);
                assert_eq!(
                    machine.primary.csr.access(0x3410_2073, 0).unwrap(),
                    RAM_BASE + 4
                );
                assert_eq!(
                    machine.primary.csr.access(0x3430_2073, 0).unwrap(),
                    instruction
                );
            }
        }
    }

    #[test]
    fn mret_restores_pc_and_resets_mpp_to_lowest_supported_privilege() {
        // Renode riscv-mret.robot: both 32-bit User Level cases.
        const HANDLER: u32 = RAM_BASE + 4;
        for (user_mode, expected_mpp) in [(false, 3), (true, 0)] {
            let mut machine = Machine::new(8).unwrap();
            if user_mode {
                machine.primary.enable_user_mode();
            }
            machine.primary.set_mtvec(HANDLER);
            for (address, instruction) in [
                (RAM_BASE, 0xc000_1073u32), // csrrw zero, cycle, zero
                (HANDLER, 0x3020_0073),     // mret
            ] {
                machine
                    .bus
                    .memory(address, 4)
                    .unwrap()
                    .copy_from_slice(&instruction.to_le_bytes());
            }
            let mut output = std::io::sink();
            assert_eq!(machine.run(1, &mut output).unwrap(), (Stop::Limit, 0));
            assert_eq!(machine.primary.pc, HANDLER);
            assert_eq!(machine.primary.csr.access(0x3420_2073, 0).unwrap(), 2);
            assert_eq!(
                (machine.primary.csr.access(0x3000_2073, 0).unwrap() >> 11) & 3,
                3
            );
            assert_eq!(machine.run(1, &mut output).unwrap(), (Stop::Limit, 1));
            assert_eq!(machine.primary.pc, RAM_BASE);
            assert_eq!(
                (machine.primary.csr.access(0x3000_2073, 0).unwrap() >> 11) & 3,
                expected_mpp
            );
        }
    }

    #[test]
    fn clint_traps_at_the_exact_reprogrammed_compare_tick() {
        // Renode timer-reconfiguration.robot: Should Tick At Exact Time.
        const CLINT: u32 = 0x0200_0000;
        const HANDLER: u32 = RAM_BASE + 16;
        let mut machine = Machine::new(20).unwrap();
        machine
            .bus
            .map_clint_with_frequency(CLINT, 1, 10_000_000)
            .unwrap();
        machine.primary.set_mtvec(HANDLER);
        machine
            .primary
            .csr
            .access(0x3040_9073, crate::clint::TIMER)
            .unwrap();
        machine.primary.csr.access(0x3000_9073, 1 << 3).unwrap();
        machine.primary.registers[1] = CLINT + 0x4000;
        machine.primary.registers[2] = 6;
        machine.primary.registers[3] = CLINT + 0x4004;
        let program = [0x0000_0013u32, 0x0020_a023, 0x0001_a023, 0x0000_006f];
        machine
            .bus
            .memory(RAM_BASE, 16)
            .unwrap()
            .copy_from_slice(program.map(u32::to_le_bytes).as_flattened());

        let mut output = std::io::sink();
        assert_eq!(machine.run(59, &mut output).unwrap(), (Stop::Limit, 59));
        assert_eq!(machine.primary.pc, RAM_BASE + 12);
        assert_eq!(machine.bus.read::<4>(CLINT + 0xbff8).unwrap(), 5);
        assert_eq!(machine.run(1, &mut output).unwrap(), (Stop::Limit, 0));
        assert_eq!(machine.primary.pc, HANDLER);
        assert_eq!(
            machine.primary.csr.access(0x3420_2073, 0).unwrap(),
            0x8000_0007
        );
        assert_eq!(
            machine.primary.csr.access(0x3410_2073, 0).unwrap(),
            RAM_BASE + 12
        );
    }

    #[test]
    fn virt_ns16550_receive_interrupt_routes_through_plic() {
        // Device addresses and source ID from Renode platforms/cpus/riscv_virt.repl.
        const UART: u32 = 0x1000_0000;
        const PLIC: u32 = 0x0c00_0000;
        const CLAIM: u32 = PLIC + 0x20_0004;
        const HANDLER: u32 = RAM_BASE + 4;
        let mut machine = Machine::without_default_ram();
        machine.bus.map_ram(RAM_BASE, 8).unwrap();
        machine.bus.map_plic(PLIC, 52, &[EXTERNAL], true).unwrap();
        machine.bus.map_ns16550(UART, Some(3), false).unwrap();
        machine.primary.set_mtvec(HANDLER);
        machine.primary.csr.access(0x3040_9073, EXTERNAL).unwrap();
        machine.primary.csr.access(0x3000_9073, 1 << 3).unwrap();
        let mut output = Vec::new();
        for (address, width, value) in [(PLIC + 12, 4, 1), (PLIC + 0x2000, 4, 8), (UART + 1, 1, 1)]
        {
            machine
                .bus
                .write(address, width, value, &mut output)
                .unwrap();
        }
        machine.schedule_uart_input(1, b"Q".to_vec()).unwrap();
        assert_eq!(machine.run(1, &mut output).unwrap(), (Stop::Limit, 0));
        assert_eq!(machine.primary.pc, HANDLER);
        assert_eq!(
            machine.primary.csr.access(0x3420_2073, 0).unwrap(),
            0x8000_000b
        );
        assert_eq!(machine.bus.read::<4>(CLAIM).unwrap(), 3);
        assert_eq!(machine.bus.read::<1>(UART + 2).unwrap(), 4);
        let received = machine.bus.read::<1>(UART).unwrap();
        machine.bus.write(UART, 1, received, &mut output).unwrap();
        assert_eq!(output, b"Q");
        assert_eq!(machine.bus.read::<1>(UART + 2).unwrap(), 1);
        assert_eq!(machine.bus.tick_plic(), 0);
        machine.bus.write(CLAIM, 4, 3, &mut output).unwrap();
        assert_eq!(machine.bus.tick_plic(), 0);
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
    fn renode_pmp_napot_and_unmatched_user_fetch() {
        // Renode tests/unit-tests/pmp.robot: fixed PC, MEPC, MTVEC and NAPOT inputs.
        const USER: u32 = 0x8000_1000;
        const DATA: u32 = 0x8000_2000;
        const TRAP: u32 = 0x8008_0000;
        for (pmp, napot) in [(true, true), (true, false), (false, false)] {
            let mut machine = Machine::new(0x81000).unwrap();
            machine.primary.enable_user_mode();
            if pmp {
                machine.primary.enable_pmp(16);
            }
            machine.primary.set_mtvec(TRAP);
            let csr = &mut machine.primary.csr;
            csr.access(0x3410_9073, USER).unwrap();
            if napot {
                csr.access(0x3b00_9073, 0x7fff_ffff).unwrap();
                csr.access(0x3a00_9073, 31).unwrap();
            }
            csr.access(0x3000_9073, 0).unwrap();
            machine
                .bus
                .memory(RAM_BASE, 4)
                .unwrap()
                .copy_from_slice(&0x3020_0073u32.to_le_bytes()); // mret
            machine.bus.memory(USER, 16).unwrap().copy_from_slice(
                &[0x0000_0013u32, 0x8000_2437, 0x0004_2483, 0x0094_2223]
                    .map(u32::to_le_bytes)
                    .concat(),
            ); // nop; lui s0, 0x80002; lw s1, 0(s0); sw s1, 4(s0)
            machine
                .bus
                .memory(DATA, 4)
                .unwrap()
                .copy_from_slice(&0x65657au32.to_le_bytes());
            let (_, retired) = machine
                .run(if napot { 5 } else { 2 }, &mut std::io::sink())
                .unwrap();
            if napot {
                assert_eq!(retired, 5);
                assert_eq!(machine.bus.read_ram::<4>(DATA + 4).unwrap(), 0x65657a);
            } else if pmp {
                assert_eq!(retired, 1);
                assert_eq!(machine.primary.pc, TRAP);
                assert_eq!(machine.primary.csr.access(0x3420_2073, 0).unwrap(), 1);
                assert_eq!(machine.primary.csr.access(0x3410_2073, 0).unwrap(), USER);
            } else {
                assert_eq!(retired, 2);
                assert_eq!(machine.primary.pc, USER + 4);
            }
        }
    }

    #[test]
    fn pmp_blocks_user_load_from_machine_memory() {
        let mut machine = Machine::new(64).unwrap();
        machine.primary.enable_user_mode();
        machine.primary.enable_pmp(16);
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
        machine.primary.enable_pmp(16);
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
        machine.bus.map_litex_uart(vec![UART], None).unwrap();
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
    fn led_assertions_require_the_named_state_at_the_scheduled_step() {
        use crate::litex_panel::Panel;

        const PANEL: u32 = 0xf000_6800;
        const SHADOW: u32 = 0x6000_6800;
        let mut machine = Machine::new(8).unwrap();
        machine
            .bus
            .map_litex_panel(Panel::new(
                vec![PANEL, SHADOW],
                None,
                Vec::new(),
                vec![("cas.led0".into(), 2)],
            ))
            .unwrap();
        machine
            .bus
            .memory(RAM_BASE, 4)
            .unwrap()
            .copy_from_slice(&0x0000_006fu32.to_le_bytes());
        assert!(
            machine
                .schedule_led_assertion(1, "other.led0".into(), false)
                .is_err()
        );
        machine
            .schedule_led_assertion(1, "cas.led0".into(), false)
            .unwrap();
        machine
            .schedule_led_assertion(2, "sysbus.cas.led0".into(), true)
            .unwrap();
        machine
            .schedule_led_assertion(3, "cas.led0".into(), false)
            .unwrap();

        let mut output = std::io::sink();
        machine.run(1, &mut output).unwrap();
        let missing = machine.verify_required_events_completed().unwrap_err();
        assert!(missing.contains("at step 2"));
        machine.bus.write(SHADOW, 4, 1 << 2, &mut output).unwrap();
        machine.run(1, &mut output).unwrap();
        let mismatch = machine.run(1, &mut output).unwrap_err();
        assert!(mismatch.contains("at step 3: expected off, got on"));
    }

    #[test]
    fn button_events_apply_at_the_scheduled_step_in_submission_order() {
        use crate::litex_panel::{Button, Panel};

        const PANEL: u32 = 0xf000_6800;
        let mut machine = Machine::new(8).unwrap();
        machine
            .bus
            .map_litex_panel(Panel::new(
                vec![PANEL],
                None,
                vec![Button::new("button0".into(), 64, false)],
                Vec::new(),
            ))
            .unwrap();
        machine
            .bus
            .memory(RAM_BASE, 4)
            .unwrap()
            .copy_from_slice(&0x0000_006fu32.to_le_bytes());

        assert!(
            machine
                .schedule_button(9, "missing".into(), ButtonAction::Press)
                .is_err()
        );
        for action in [ButtonAction::Press, ButtonAction::Release] {
            machine
                .schedule_button(3, "cas.button0".into(), action)
                .unwrap();
        }

        let mut output = std::io::sink();
        machine.run(2, &mut output).unwrap();
        assert_eq!(machine.bus.read::<4>(PANEL + 0xc).unwrap(), 0);
        machine.run(1, &mut output).unwrap();
        assert_eq!(machine.bus.read::<4>(PANEL + 8).unwrap(), 0);
        assert_eq!(machine.bus.read::<4>(PANEL + 0xc).unwrap(), 1);
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
