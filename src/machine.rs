use std::{collections::VecDeque, io::Write};

use crate::{
    bus::{Bus, BusError, RAM_BASE},
    csr::{Csr, EXTERNAL},
    sv32::TranslationCache,
    thumb::Thumb,
    zephyr_boot::BusyWait,
};

pub const MAX_HARTS: usize = 8;
pub const VIRTUAL_STEPS_PER_SECOND: u32 = 1_000_000;

pub struct Machine {
    pub bus: Bus,
    pub primary: Hart,
    secondary: Vec<Hart>,
    next_hart: usize,
    elapsed_steps: u64,
    uart_input: VecDeque<(u64, Vec<u8>)>,
}

#[derive(Clone)]
pub struct Hart {
    pub name: Option<String>,
    csr: Csr,
    translations: TranslationCache,
    thumb: Option<Thumb>,
    busy_wait: Option<BusyWait>,
    pub pc: u32,
    pub registers: [u32; 32],
}

#[derive(Debug, PartialEq, Eq)]
pub enum Stop {
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
                BusError::Output(_) => return None,
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
                busy_wait: None,
                pc: RAM_BASE,
                registers: [0; 32],
            },
            secondary: Vec::new(),
            next_hart: 0,
            elapsed_steps: 0,
            uart_input: VecDeque::new(),
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
        self.named_hart(name)?.pc = entry;
        Ok(())
    }

    pub fn set_boot_vectors(&mut self, name: &str, address: u32) -> Result<(), String> {
        let hart = self.named_hart(name)?;
        let thumb = hart
            .thumb
            .as_mut()
            .ok_or("VectorTableOffset requires Cortex-M")?;
        thumb.set_vector_base(address);
        Ok(())
    }

    pub fn set_zephyr_wait_hooks(
        &mut self,
        name: &str,
        entries: impl Iterator<Item = u32>,
    ) -> Result<(), String> {
        let hart = self.named_hart(name)?;
        if hart.thumb.is_none() {
            return Err("Zephyr busy-wait hooks currently require Cortex-M".into());
        }
        hart.busy_wait = BusyWait::new(entries);
        Ok(())
    }

    fn named_hart(&mut self, name: &str) -> Result<&mut Hart, String> {
        std::iter::once(&mut self.primary)
            .chain(&mut self.secondary)
            .find(|hart| hart.name.as_deref() == Some(name))
            .ok_or_else(|| format!("unknown CPU {name:?}"))
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

    fn reset_cortex_m(&mut self) -> Result<(), String> {
        let thumb = self
            .primary
            .thumb
            .as_mut()
            .ok_or("IWDG requires Cortex-M")?;
        thumb.reset_state();
        self.primary.registers.fill(0);
        if let Some(wait) = self.primary.busy_wait.as_mut() {
            wait.reset();
        }
        self.bus.reset_cortex_m_devices();
        self.validate_entry()
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

    pub fn run(&mut self, limit: u64, output: &mut impl Write) -> Result<(Stop, u64), String> {
        let mut retired = 0;
        for _ in 0..limit {
            self.elapsed_steps = self.elapsed_steps.wrapping_add(1);
            if self.bus.tick_watchdogs() {
                self.reset_cortex_m()?;
            }
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
            self.bus.tick_h7_peripherals();
            let rate = self.bus.step_hz();
            if let Some(dwt) = &mut self.bus.dwt {
                dwt.tick(rate);
            }
            while self
                .uart_input
                .front()
                .is_some_and(|(step, _)| *step <= self.elapsed_steps)
            {
                let (_, bytes) = self.uart_input.pop_front().unwrap();
                self.bus.queue_uart_input(bytes);
            }
            match hart.execute_step(&mut self.bus, self.elapsed_steps, output)? {
                Step::Retired => retired += 1,
                Step::Trap => {}
                Step::Stop(stop) => return Ok((stop, retired)),
            }
        }
        Ok((Stop::Limit, retired))
    }
}

impl Hart {
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
        output: &mut impl Write,
    ) -> Result<Step, String> {
        if let Some(thumb) = self.thumb.as_mut() {
            if self.busy_wait.as_mut().is_some_and(BusyWait::waiting) {
                bus.tick_systick();
                return Ok(Step::Trap);
            }
            if thumb
                .service_interrupts(&mut self.pc, &mut self.registers, bus)
                .map_err(|error| format!("pc={:#010x}: {error}", self.pc))?
            {
                return Ok(Step::Trap);
            }
            if thumb.sleeping() {
                return Ok(Step::Trap);
            }
            if let Some(wait) = &mut self.busy_wait
                && wait.contains(self.pc)
            {
                let return_address = self.registers[14];
                if return_address & 1 == 0 {
                    return Err(format!(
                        "invalid Zephyr busy-wait return address {return_address:#010x}"
                    ));
                }
                self.pc = return_address & !1;
                let delay = self.registers[0];
                if delay != 0 {
                    wait.skip(delay, bus.step_hz());
                    return Ok(Step::Trap);
                }
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
            let ethernet = bus
                .ethernet
                .iter()
                .fold(0, |lines, device| lines | device.vex_irq());
            let lines =
                (u32::from(uart) * EXTERNAL) | (u32::from(timer) * crate::clint::TIMER) | ethernet;
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
            || !bus.ethernet.is_empty()
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
