mod fpu;
mod mve;
pub(crate) mod opcode_counter;
mod thumb2;
pub(crate) mod trace;

use std::io::Write;

use crate::{
    bus::{Bus, BusError},
    nvic::{ActiveExceptions, HARDFAULT, IRQ_LINES, Nvic},
    systick::{NMI, PENDSV, SYSTICK, SysTick},
};

#[derive(Clone, Copy, Default, PartialEq)]
enum Sleep {
    #[default]
    Running,
    Interrupt,
    Event,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InstructionBoundary {
    Begin,
    End,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ThumbBoundary {
    pub phase: InstructionBoundary,
    pub pc: u32,
    pub itstate: u8,
    pub active_exception: u8,
    pub exception_depth: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DebugStep {
    Retired,
    Sleeping,
    Breakpoint,
}

#[derive(Clone, Copy, Default)]
struct SecurityBank {
    msp: u32,
    psp: u32,
    thread_uses_psp: bool,
    interrupts_masked: bool,
    faultmask: bool,
    basepri: u8,
}

#[derive(Clone, Default)]
pub struct Thumb {
    vectors: Option<u32>,
    pub(crate) thumb2: bool,
    pub(crate) cortex_m33: bool,
    pub(crate) cortex_m55: bool,
    pub(crate) trustzone: bool,
    secure: bool,
    inactive_security_bank: SecurityBank,
    basepri: u8,
    faultmask: bool,
    itstate: u8,
    clock_gated: bool,
    retired_instructions: u64,
    negative: bool,
    zero: bool,
    carry: bool,
    overflow: bool,
    saturation: bool,
    greater_equal: u32,
    fp: [u32; 32],
    fpscr: u32,
    fp_active: bool,
    active_exception: u8,
    active_exceptions: ActiveExceptions,
    interrupts_masked: bool,
    sleep: Sleep,
    event: bool,
    pending: [u32; IRQ_LINES.div_ceil(32)],
    system_pending: u64,
    invalid_thumb_state: bool,
    locked_up: bool,
    instruction_lockup: bool,
    branch_exception: bool,
    thread_uses_psp: bool,
    msp: u32,
    psp: u32,
}

const LOCKUP_PC: u32 = 0xefff_fffe;
const SECURE_CALLEE_BYTES: u32 = 40;
const SECURE_INTEGRITY_SIGNATURE: u32 = 0xfefa_125b;

#[derive(Debug, PartialEq, Eq)]
pub enum ThumbStep {
    Retired,
    Breakpoint,
    Exception,
    Semihosting(crate::semihosting::Exit),
}

enum InstructionError {
    GuestFault,
    ExceptionTaken(u32),
    SemihostingExit(crate::semihosting::Exit, u32),
    Host(String),
}

impl From<String> for InstructionError {
    fn from(error: String) -> Self {
        Self::Host(error)
    }
}

impl From<BusError> for InstructionError {
    fn from(error: BusError) -> Self {
        match error {
            BusError::Command(..) | BusError::Unmapped(..) | BusError::Misaligned(..) | BusError::AddressError(_) => {
                Self::GuestFault
            }
            other => Self::Host(other.to_string()),
        }
    }
}

impl Thumb {
    pub fn new(vectors: Option<u32>) -> Self {
        Self {
            vectors,
            ..Self::default()
        }
    }

    pub(crate) fn save_snapshot(&self, output: &mut impl Write) -> std::io::Result<()> {
        use crate::snapshot::write_words;
        let sleep = match self.sleep { Sleep::Running => 0, Sleep::Interrupt => 1, Sleep::Event => 2 };
        write_words(output, &[
            self.vectors.map_or(1 << 32, u64::from), u64::from(self.thumb2), u64::from(self.basepri),
            u64::from(self.faultmask), u64::from(self.itstate), u64::from(self.clock_gated),
            self.retired_instructions, u64::from(self.apsr()), u64::from(self.fpscr), u64::from(self.fp_active),
            u64::from(self.active_exception), u64::from(self.interrupts_masked), sleep,
            u64::from(self.event), self.system_pending, u64::from(self.invalid_thumb_state),
            u64::from(self.locked_up), u64::from(self.instruction_lockup), u64::from(self.branch_exception),
            u64::from(self.thread_uses_psp), u64::from(self.msp), u64::from(self.psp),
        ])?;
        write_words(output, &self.fp.map(u64::from))?;
        write_words(output, &self.pending.map(u64::from))?;
        write_words(output, &self.active_exceptions.0.map(u64::from))
    }

    pub(crate) fn load_snapshot(&mut self, input: &mut impl std::io::Read) -> std::io::Result<()> {
        use crate::snapshot::{invalid, read_bool, read_words};
        let [vtor, thumb2, basepri, faultmask, itstate, clock_gated, retired, apsr, fpscr,
            fp_active, active, primask, sleep, event, system_pending, invalid_state, lockup,
            instruction_lockup, branch_exception, use_psp, msp, psp] = read_words(input)?;
        if read_bool(thumb2)? != self.thumb2 {
            return Err(invalid("snapshot Cortex-M instruction set differs from the platform"));
        }
        let word = |value| u32::try_from(value).map_err(std::io::Error::other);
        let byte = |value| u8::try_from(value).map_err(std::io::Error::other);
        self.vectors = if vtor == 1 << 32 { None } else { Some(word(vtor)?) };
        self.basepri = byte(basepri)?;
        self.faultmask = read_bool(faultmask)?;
        self.itstate = byte(itstate)?;
        self.clock_gated = read_bool(clock_gated)?;
        self.retired_instructions = retired;
        self.set_apsr(word(apsr)?);
        self.fpscr = word(fpscr)?;
        self.fp_active = read_bool(fp_active)?;
        self.active_exception = byte(active)?;
        self.interrupts_masked = read_bool(primask)?;
        self.sleep = match sleep {
            0 => Sleep::Running,
            1 => Sleep::Interrupt,
            2 => Sleep::Event,
            _ => return Err(invalid("invalid Cortex-M sleep state")),
        };
        self.event = read_bool(event)?;
        self.system_pending = system_pending;
        self.invalid_thumb_state = read_bool(invalid_state)?;
        self.locked_up = read_bool(lockup)?;
        self.instruction_lockup = read_bool(instruction_lockup)?;
        self.branch_exception = read_bool(branch_exception)?;
        self.thread_uses_psp = read_bool(use_psp)?;
        self.msp = word(msp)?;
        self.psp = word(psp)?;
        for state in &mut self.fp {
            let [value] = read_words(input)?;
            *state = word(value)?;
        }
        for registers in [&mut self.pending, &mut self.active_exceptions.0] {
            for register in registers {
                let [value] = read_words(input)?;
                *register = word(value)?;
            }
        }
        Ok(())
    }

    pub fn reset_state(&mut self) {
        *self = Self {
            vectors: self.vectors,
            thumb2: self.thumb2,
            cortex_m33: self.cortex_m33,
            cortex_m55: self.cortex_m55,
            trustzone: self.trustzone,
            secure: self.trustzone,
            clock_gated: self.clock_gated,
            ..Self::default()
        };
    }

    pub(crate) fn configure_trustzone(&mut self, enabled: bool) {
        self.trustzone = enabled;
        self.secure = enabled;
    }

    pub(crate) fn secure_state(&self) -> bool {
        self.trustzone && self.secure
    }

    pub(crate) fn switch_security(&mut self, secure: bool, r: &mut [u32; 32]) -> Result<(), String> {
        if !self.trustzone {
            return if secure { Err("TrustZone is disabled".into()) } else { Ok(()) };
        }
        if secure != self.secure {
            let bank = self.inactive_security_bank;
            let (msp, psp) = if self.uses_psp() {
                (self.msp, r[13])
            } else {
                (r[13], self.psp)
            };
            self.inactive_security_bank = SecurityBank {
                msp,
                psp,
                thread_uses_psp: self.thread_uses_psp,
                interrupts_masked: self.interrupts_masked,
                faultmask: self.faultmask,
                basepri: self.basepri,
            };
            self.secure = secure;
            self.msp = bank.msp;
            self.psp = bank.psp;
            self.thread_uses_psp = bank.thread_uses_psp;
            self.interrupts_masked = bank.interrupts_masked;
            self.faultmask = bank.faultmask;
            self.basepri = bank.basepri;
            r[13] = if self.uses_psp() { bank.psp } else { bank.msp };
        }
        Ok(())
    }

    pub fn set_clocked(&mut self, clocked: bool) {
        self.clock_gated = !clocked;
    }

    pub fn clocked(&self) -> bool {
        !self.clock_gated
    }

    pub(crate) fn it_state(&self) -> u8 {
        self.itstate
    }

    pub fn executed_instructions(&self) -> u64 {
        self.retired_instructions
    }

    pub fn vector_base(&self) -> u32 {
        self.vectors.unwrap_or(0)
    }

    pub fn patch_monitor_pc(
        &mut self,
        address: u32,
        cpu: &str,
        output: &mut impl Write,
    ) -> Result<u32, String> {
        if address & 1 == 0 {
            writeln!(output, "{cpu}: Patching PC {address:#x} for Thumb mode.")
                .map_err(|error| error.to_string())?;
        }
        self.invalid_thumb_state = false;
        Ok(address & !1)
    }

    pub fn set_vector_base(&mut self, base: u32) {
        self.vectors = Some(base);
    }

    pub fn infer_vector_base(&mut self, base: u32) {
        self.vectors.get_or_insert(base);
    }

    pub(crate) fn reset_m4(&mut self, bus: &mut Bus) -> Result<(u32, u32, u32), String> {
        self.reset_state();
        let base = self.vector_base();
        let vector = base.checked_add(4).ok_or("reset vector address overflow")?;
        let initial = bus.read_cortex_m_ram_or_mock_word(vector)
            .and_then(|target| bus.read_cortex_m_ram_or_mock_word(base).map(|sp| (target, sp)));
        match initial {
            Ok((target, sp)) => {
                self.invalid_thumb_state = target & 1 == 0;
                Ok((sp & !3, target & !1, 0))
            }
            Err(BusError::SlaveFault(_) | BusError::Unmapped(..)) if bus.nvic.is_some() => {
                let nvic = bus.nvic.as_mut().unwrap();
                nvic.record_vector_fault();
                nvic.set_active_exceptions(1 << HARDFAULT);
                nvic.enter_lockup();
                Ok((0, self.lockup(), u32::MAX))
            }
            Err(error) => Err(error.to_string()),
        }
    }

    fn nz(&mut self, value: u32) {
        self.negative = value >> 31 != 0;
        self.zero = value == 0;
    }

    fn arithmetic(&mut self, lhs: u32, rhs: u32, subtract: bool) -> u32 {
        let (result, carry, overflow) = if subtract {
            (
                lhs.wrapping_sub(rhs),
                lhs >= rhs,
                (lhs as i32).overflowing_sub(rhs as i32).1,
            )
        } else {
            let (result, carry) = lhs.overflowing_add(rhs);
            (result, carry, (lhs as i32).overflowing_add(rhs as i32).1)
        };
        self.nz(result);
        self.carry = carry;
        self.overflow = overflow;
        result
    }

    fn data_processing(&mut self, operation: u32, lhs: u32, rhs: u32) -> Option<u32> {
        let shift = rhs & 0xff;
        let value = match operation {
            0 | 8 => lhs & rhs,
            1 => lhs ^ rhs,
            2..=4 | 7 if shift == 0 => lhs,
            2 => {
                self.carry = shift <= 32 && lhs & (1 << (32 - shift)) != 0;
                lhs.checked_shl(shift).unwrap_or(0)
            }
            3 => {
                self.carry = shift <= 32 && lhs & (1 << (shift - 1)) != 0;
                lhs.checked_shr(shift).unwrap_or(0)
            }
            4 => {
                self.carry = lhs & (1 << (shift.min(32) - 1)) != 0;
                ((lhs as i32) >> shift.min(31)) as u32
            }
            5 | 6 => {
                let carry = u64::from(self.carry);
                let rhs = if operation == 6 { !rhs } else { rhs };
                let result = u64::from(lhs) + u64::from(rhs) + carry;
                let signed = i64::from(lhs as i32) + i64::from(rhs as i32) + carry as i64;
                self.carry = result >> 32 != 0;
                self.overflow = i32::try_from(signed).is_err();
                result as u32
            }
            7 => {
                let result = lhs.rotate_right(shift);
                self.carry = result >> 31 != 0;
                result
            }
            9 => return Some(self.arithmetic(0, rhs, true)),
            10 | 11 => {
                self.arithmetic(lhs, rhs, operation == 10);
                return None;
            }
            12 => lhs | rhs,
            13 => lhs.wrapping_mul(rhs),
            14 => lhs & !rhs,
            15 => !rhs,
            _ => unreachable!(),
        };
        self.nz(value);
        (operation != 8).then_some(value)
    }

    pub fn condition(&self, condition: u32) -> bool {
        if condition >= 14 {
            return true;
        }
        let positive = match condition >> 1 {
            0 => self.zero,
            1 => self.carry,
            2 => self.negative,
            3 => self.overflow,
            4 => self.carry && !self.zero,
            5 => self.negative == self.overflow,
            6 => !self.zero && self.negative == self.overflow,
            _ => unreachable!(),
        };
        positive ^ (condition & 1 != 0)
    }

    pub(crate) fn apsr(&self) -> u32 {
        (u32::from(self.negative) << 31)
            | (u32::from(self.zero) << 30)
            | (u32::from(self.carry) << 29)
            | ((u32::from(self.overflow) << 28) | (u32::from(self.saturation) << 27))
            | (self.greater_equal << 16)
    }

    pub(crate) fn set_apsr(&mut self, value: u32) {
        self.negative = value >> 31 != 0;
        self.zero = value & (1 << 30) != 0;
        self.carry = value & (1 << 29) != 0;
        self.overflow = value & (1 << 28) != 0;
        self.saturation = self.thumb2 && value & (1 << 27) != 0;
        self.greater_equal = if self.thumb2 { (value >> 16) & 15 } else { 0 };
    }

    pub fn xpsr(&self) -> u32 {
        self.apsr()
            | (u32::from(!self.invalid_thumb_state) << 24)
            | u32::from(self.active_exception)
    }

    pub fn lockup_state(&self) -> (bool, u8, u8) {
        (
            self.locked_up,
            (self.active_exceptions.depth() as u8).saturating_sub(u8::from(
                self.thumb2 && !self.instruction_lockup && self.active_exceptions.contains(self.active_exception),
            )),
            self.active_exception,
        )
    }
    pub fn fp_context_active(&self) -> bool {
        self.fp_active
    }

    pub fn locked_up(&self) -> bool {
        self.locked_up
    }

    pub(crate) fn reset_m0(&mut self, bus: &Bus) -> (u32, u32, u32) {
        self.reset_state();
        let vectors = self.vector_base();
        let target = vectors
            .checked_add(4)
            .and_then(|address| bus.read_ram::<4>(address).ok());
        if let (Some(target), Ok(sp)) = (target, bus.read_ram::<4>(vectors)) {
            self.invalid_thumb_state = target & 1 == 0;
            return (sp & !3, target & !1, 0);
        }
        self.active_exceptions.insert(HARDFAULT);
        let pc = self.lockup();
        (0, pc, u32::MAX)
    }

    fn uses_psp(&self) -> bool {
        self.active_exception == 0 && self.thread_uses_psp
    }

    fn select_thread_stack(&mut self, use_psp: bool, r: &mut [u32; 32]) {
        if self.active_exception != 0 || self.thread_uses_psp == use_psp {
            return;
        }
        if use_psp {
            self.msp = r[13];
            r[13] = self.psp;
        } else {
            self.psp = r[13];
            r[13] = self.msp;
        }
        self.thread_uses_psp = use_psp;
    }

    pub(crate) fn system_read(&self, register: u32, r: &[u32; 32]) -> Option<u32> {
        let ipsr = u32::from(self.active_exception);
        Some(match register {
            0 => self.apsr(),
            1 => self.apsr() | ipsr,
            2 => self.apsr(),
            3 => self.apsr() | ipsr,
            5 => ipsr,
            6 => 0,
            7 => ipsr,
            8 => {
                if self.uses_psp() {
                    self.msp
                } else {
                    r[13]
                }
            }
            9 => {
                if self.uses_psp() {
                    r[13]
                } else {
                    self.psp
                }
            }
            16 => u32::from(self.interrupts_masked),
            17 if self.thumb2 => u32::from(self.basepri),
            19 if self.thumb2 => self.faultmask.into(),
            20 => (u32::from(self.fp_active) << 2) | (u32::from(self.uses_psp()) << 1),
            _ => return None,
        })
    }

    pub(crate) fn system_write(&mut self, register: u32, value: u32, r: &mut [u32; 32]) -> bool {
        match register {
            0..=3 => self.set_apsr(value),
            5..=7 => {}
            8 if self.uses_psp() => self.msp = value & !3,
            8 => r[13] = value & !3,
            9 if self.uses_psp() => r[13] = value & !3,
            9 => self.psp = value & !3,
            16 => self.interrupts_masked = value & 1 != 0,
            17 if self.thumb2 => self.basepri = value as u8,
            18 if self.thumb2 => {
                let value = value as u8;
                if value != 0 && (self.basepri == 0 || value < self.basepri) {
                    self.basepri = value;
                }
            }
            19 if self.thumb2 => self.faultmask = value & 1 != 0,
            20 => {
                self.select_thread_stack(value & 2 != 0, r);
                if self.thumb2 {
                    self.fp_active = value & 4 != 0;
                }
            }
            _ => return false,
        }
        true
    }

    fn lockup(&mut self) -> u32 {
        self.locked_up = true;
        self.sleep = Sleep::Running;
        LOCKUP_PC
    }

    fn can_take_hardfault(&self) -> bool {
        !self.interrupts_masked
            && !self.active_exceptions.contains(NMI)
            && !self.active_exceptions.contains(HARDFAULT)
    }

    fn hardfault(&mut self, pc: u32, r: &mut [u32; 32], bus: &mut Bus) -> u32 {
        if self.can_take_hardfault() {
            self.exception_entry_m0(HARDFAULT, pc, r, bus)
        } else {
            self.lockup()
        }
    }

    fn exception_entry_m0(
        &mut self,
        mut exception: u8,
        return_pc: u32,
        r: &mut [u32; 32],
        bus: &mut Bus,
    ) -> u32 {
        let sp = r[13];
        let stack = (sp & !7).wrapping_sub(32);
        let xpsr = self.xpsr() | ((sp & 4) << 7);
        let frame = [r[0], r[1], r[2], r[3], r[12], r[14], return_pc, xpsr];
        let mut stacking_fault = false;
        // Match PushStack: allocate the whole frame and abandon writes after
        // the first fault, starting at the highest-addressed word.
        for (index, value) in frame.into_iter().enumerate().rev() {
            let address = stack.wrapping_add(index as u32 * 4);
            #[cfg(test)]
            if bus
                .faulting_stack_writes
                .as_ref()
                .is_some_and(|range| range.contains(&address))
            {
                stacking_fault = true;
                break;
            }
            let Ok(word) = bus.memory(address, 4) else {
                stacking_fault = true;
                break;
            };
            word.copy_from_slice(&value.to_le_bytes());
        }
        let mut lock_after_entry = false;
        if stacking_fault {
            match exception {
                NMI if self.can_take_hardfault() => {
                    if let Some(nvic) = bus.nvic.as_mut() {
                        nvic.set_hardfault_pending(true);
                    }
                }
                // Preserve the first exception boundary after an invalid reset.
                HARDFAULT if self.invalid_thumb_state && self.active_exception == 0 => {}
                NMI | HARDFAULT => lock_after_entry = true,
                _ if self.can_take_hardfault() => exception = HARDFAULT,
                _ => lock_after_entry = true,
            }
        }
        if self.uses_psp() {
            self.psp = stack;
            r[13] = self.msp;
            r[14] = 0xffff_fffd;
        } else {
            r[13] = stack;
            r[14] = if self.active_exception == 0 {
                0xffff_fff9
            } else {
                0xffff_fff1
            };
        }
        let handler = self.take_exception_vector(exception, bus);
        if lock_after_entry {
            self.lockup()
        } else {
            handler
        }
    }

    fn take_exception_vector(&mut self, mut exception: u8, bus: &Bus) -> u32 {
        let vector_base = bus
            .nvic
            .as_ref()
            .and_then(Nvic::vector_base)
            .unwrap_or_else(|| self.vector_base());
        let read_vector = |exception: u8| {
            vector_base
                .checked_add(u32::from(exception) * 4)
                .and_then(|vector| bus.read_ram::<4>(vector).ok())
        };
        let mut handler = read_vector(exception);
        if handler.is_none() && !matches!(exception, NMI | HARDFAULT) && self.can_take_hardfault() {
            exception = HARDFAULT;
            handler = read_vector(exception);
        }
        self.active_exceptions.insert(exception);
        self.active_exception = exception;
        self.sleep = Sleep::Running;
        match handler {
            Some(handler) => {
                self.locked_up = false;
                self.invalid_thumb_state = handler & 1 == 0;
                handler & !1
            }
            None => self.lockup(),
        }
    }

    fn exception_priority(bus: &Bus, exception: u8) -> i16 {
        match exception {
            0 => 256,
            NMI => -2,
            HARDFAULT => -1,
            _ => bus
                .nvic
                .as_ref()
                .map_or(0, |nvic| nvic.priority(exception))
                .into(),
        }
    }

    fn active_handler(bus: &Bus, active: ActiveExceptions) -> u8 {
        (NMI..=255)
            .filter(|number| active.contains(*number))
            .min_by_key(|&number| Self::exception_priority(bus, number))
            .unwrap_or(0)
    }

    fn pending_configurable(bus: &Bus, active: ActiveExceptions, current: u8) -> Option<u8> {
        let external = bus
            .nvic
            .as_ref()
            .and_then(|nvic| nvic.next_irq(&active))
            .map(|(line, _)| 16 + line);
        external
            .into_iter()
            .chain([PENDSV, SYSTICK].into_iter().filter(|&exception| {
                !active.contains(exception)
                    && bus
                        .systick
                        .as_ref()
                        .is_some_and(|timer| timer.pending(exception))
            }))
            .filter(|&exception| {
                Self::exception_priority(bus, exception) < Self::exception_priority(bus, current)
            })
            .min_by_key(|&exception| (Self::exception_priority(bus, exception), exception))
    }

    fn acknowledge(&mut self, exception: u8, bus: &mut Bus) {
        match exception {
            NMI | PENDSV | SYSTICK => bus.systick.as_mut().unwrap().acknowledge(exception),
            HARDFAULT => bus.nvic.as_mut().unwrap().set_hardfault_pending(false),
            _ => bus.nvic.as_mut().unwrap().acknowledge(exception - 16),
        }
    }

    fn tail_chain(
        &mut self,
        target: u32,
        remaining: ActiveExceptions,
        r: &mut [u32; 32],
        bus: &mut Bus,
    ) -> Option<u32> {
        if (target == 0xffff_fff1) != (!remaining.is_empty()) {
            return None;
        }
        let destination = Self::active_handler(bus, remaining);
        let nmi_pending = bus.systick.as_ref().is_some_and(|timer| timer.pending(NMI));
        let hardfault_pending = bus
            .nvic
            .as_ref()
            .is_some_and(|nvic| nvic.hardfault_pending());
        let exception = if nmi_pending && destination != NMI {
            NMI
        } else if self.interrupts_masked {
            return None;
        } else if hardfault_pending && !matches!(destination, NMI | HARDFAULT) {
            HARDFAULT
        } else {
            Self::pending_configurable(bus, remaining, destination)?
        };
        self.active_exceptions = remaining;
        self.active_exception = destination;
        r[14] = target;
        let handler = self.take_exception_vector(exception, bus);
        if self.active_exception == exception {
            self.acknowledge(exception, bus);
        }
        Some(handler)
    }

    fn branch_m0(
        &mut self,
        target: u32,
        r: &mut [u32; 32],
        bus: &mut Bus,
    ) -> Result<u32, InstructionError> {
        if self.active_exception != 0 && matches!(target, 0xffff_fff1 | 0xffff_fff9 | 0xffff_fffd) {
            let use_psp = target == 0xffff_fffd;
            let stack = if use_psp { self.psp } else { r[13] };
            let mut remaining = self.active_exceptions;
            remaining.remove(self.active_exception);
            if let Some(handler) = self.tail_chain(target, remaining, r, bus) {
                return Ok(handler);
            }
            let mut frame = [0; 8];
            for (index, word) in frame.iter_mut().enumerate() {
                match bus.read_ram::<4>(stack.wrapping_add(index as u32 * 4)) {
                    Ok(value) => *word = value,
                    Err(_) => {
                        let pc = self.unstack_fault(target, stack, &frame[..index], r, bus);
                        return Err(InstructionError::ExceptionTaken(pc));
                    }
                }
            }
            let [r0, r1, r2, r3, r12, lr, pc, xpsr] = frame;
            let previous = xpsr & 0x1ff;
            let valid_mode = if target != 0xffff_fff1 {
                previous == 0 && remaining.is_empty()
            } else {
                previous != 0 && previous <= u8::MAX.into() && remaining.contains(previous as u8)
            };
            if xpsr & (1 << 24) == 0 || !valid_mode || pc & 1 != 0 {
                if self.active_exception == NMI && remaining.contains(HARDFAULT) {
                    r[13] = stack.wrapping_add(32);
                    self.active_exception = HARDFAULT;
                    self.active_exceptions = remaining;
                }
                return Err(InstructionError::GuestFault);
            }
            r[..4].copy_from_slice(&[r0, r1, r2, r3]);
            r[12] = r12;
            if use_psp {
                self.msp = r[13];
            }
            if previous == 0 {
                self.thread_uses_psp = use_psp;
                if bus.nvic.as_ref().is_some_and(Nvic::sleep_on_exit) {
                    self.sleep = Sleep::Interrupt;
                }
            }
            r[13] = stack.wrapping_add(32 + ((xpsr >> 7) & 4));
            r[14] = lr;
            self.set_apsr(xpsr);
            self.active_exception = previous as u8;
            self.active_exceptions = remaining;
            self.invalid_thumb_state = false;
            return Ok(pc);
        }
        self.invalid_thumb_state = target & 1 == 0;
        Ok(target & !1)
    }

    fn unstack_fault(
        &mut self,
        target: u32,
        stack: u32,
        restored_words: &[u32],
        r: &mut [u32; 32],
        bus: &mut Bus,
    ) -> u32 {
        // The returning exception is no longer active. An eligible HardFault
        // uses the existing frame and the original EXC_RETURN.
        self.active_exceptions.remove(self.active_exception);
        self.active_exception = Self::active_handler(bus, self.active_exceptions);
        let use_psp = target == 0xffff_fffd;
        if self.can_take_hardfault() {
            r[14] = target;
            if use_psp {
                self.psp = stack;
            } else {
                r[13] = stack;
            }
            if let Some(nvic) = bus.nvic.as_mut() {
                nvic.set_hardfault_pending(false);
            }
            self.take_exception_vector(HARDFAULT, bus)
        } else {
            for (register, value) in [0, 1, 2, 3, 12, 14].into_iter().zip(restored_words) {
                r[register] = *value;
            }
            let consumed = stack.wrapping_add(32);
            if use_psp {
                self.msp = r[13];
                self.psp = consumed;
                self.thread_uses_psp = true;
            }
            r[13] = consumed;
            self.lockup()
        }
    }

    pub(crate) fn priority_boost(&self, ignore_primask: bool) -> i16 {
        if self.faultmask {
            -1
        } else if self.interrupts_masked && !ignore_primask {
            0
        } else if self.basepri != 0 {
            i16::from(self.basepri)
        } else {
            256
        }
    }

    fn can_take_sync_exception(&self, exception: u8, nvic: &Nvic) -> bool {
        let priority = |exception| nvic.preemption_priority(nvic.priority(exception));
        let candidate = priority(exception);
        !self.faultmask && !self.interrupts_masked
            && (self.basepri == 0 || candidate < nvic.preemption_priority(self.basepri))
            && (self.active_exception == 0
                || (self.active_exception > HARDFAULT && candidate < priority(self.active_exception)))
    }

    fn thumb2_exception_priority(&self, bus: &Bus, exception: u8) -> (i16, u8) {
        let priority = match exception {
            NMI => -2,
            HARDFAULT => -1,
            _ => i16::from(bus.nvic.as_ref().map_or(0, |nvic| nvic.priority(exception))),
        };
        (priority, exception)
    }

    fn vector_address(&self, bus: &Bus, exception: u8, nonsecure: bool) -> Result<u32, String> {
        let base = if nonsecure {
            bus.nvic.as_ref().and_then(Nvic::nonsecure_vector_base).unwrap_or(0)
        } else {
            bus.nvic.as_ref().and_then(Nvic::vector_base).unwrap_or_else(|| self.vector_base())
        };
        base.checked_add(u32::from(exception) * 4).ok_or("exception vector address overflow".into())
    }

    fn hardfault_masked(&self, nvic: &Nvic) -> bool {
        if self.trustzone && !self.secure && !nvic.bfhfnmins() {
            return false;
        }
        self.faultmask || matches!(self.active_exception, NMI | HARDFAULT)
    }

    fn exception_entry(
        &mut self,
        exception: u8,
        return_pc: u32,
        r: &mut [u32; 32],
        bus: &mut Bus,
    ) -> Result<u32, String> {
        if !self.thumb2 {
            return Ok(self.exception_entry_m0(exception, return_pc, r, bus));
        }
        let origin_secure = self.secure_state();
        let mut exception = exception;
        let mut nonsecure = bus.nvic.as_ref()
            .is_some_and(|nvic| nvic.exception_targets_nonsecure(exception, origin_secure));
        let initial_nonsecure = nonsecure;
        let mut locked = false;
        let mut handler = loop {
            let vector = self.vector_address(bus, exception, nonsecure)?;
            match bus.read_cortex_m_ram_or_mock_word(vector) {
                Ok(handler) => break handler,
                Err(BusError::SlaveFault(_) | BusError::Unmapped(..)) if bus.nvic.is_some() => {
                    let nvic = bus.nvic.as_mut().unwrap();
                    nvic.record_vector_fault();
                    let hardfault_nonsecure = nonsecure && nvic.bfhfnmins() && nvic.address_is_nonsecure(vector);
                    if (matches!(exception, NMI | HARDFAULT) && (!self.trustzone || nonsecure == hardfault_nonsecure))
                        || self.hardfault_masked(nvic)
                    {
                        locked = true;
                        break LOCKUP_PC;
                    }
                    exception = HARDFAULT;
                    nonsecure = hardfault_nonsecure;
                }
                Err(error) => return Err(error.to_string()),
            }
        };
        let sp = r[13];
        let full_fp_frame = self.fp_active && origin_secure && bus.nvic.as_ref().is_some_and(Nvic::secure_full_fp_frame);
        let frame_size = if full_fp_frame { 168 } else if self.fp_active { 104 } else { 32 };
        let mut stack = (sp & !7).wrapping_sub(frame_size as u32);
        bus.check_thumb_access(stack, frame_size, true, false)?;
        let it = u32::from(self.itstate);
        let xpsr = self.apsr()
            | (1 << 24)
            | ((sp & 4) << 7)
            | ((it & 3) << 25)
            | ((it & 0xfc) << 8)
            | u32::from(self.active_exception);
        let mut frame = [0; 42];
        frame[..8].copy_from_slice(&[r[0], r[1], r[2], r[3], r[12], r[14], return_pc, xpsr]);
        let lazy = self.fp_active && bus.nvic.as_ref().is_some_and(Nvic::lazy_fp_enabled);
        if self.fp_active && !lazy {
            frame[8..24].copy_from_slice(&self.fp[..16]);
            frame[24] = self.fpscr;
            if full_fp_frame {
                frame[26..].copy_from_slice(&self.fp[16..]);
            }
        }
        let stored_words = if lazy { 8 } else { frame_size / 4 };
        let stack_result = bus.write_cortex_m_exception_stack(stack, &frame[..stored_words]);
        let has_callee_frame = origin_secure && initial_nonsecure && stack_result.is_ok();
        if let Err(error) = stack_result {
            if !matches!(error, BusError::SlaveFault(_) | BusError::Unmapped(..)) || bus.nvic.is_none() {
                return Err(error.to_string());
            }
            let nvic = bus.nvic.as_mut().unwrap();
            if !(sp == 0 && self.invalid_thumb_state && self.active_exception == 0) {
                nvic.record_stacking_fault();
            }
            if !locked {
                const BUSFAULT: u8 = 5;
                let hardfault_blocked = self.hardfault_masked(nvic);
                let derived = if nvic.fault_enabled(BUSFAULT) && self.can_take_sync_exception(BUSFAULT, nvic) {
                    BUSFAULT
                } else if hardfault_blocked {
                    HARDFAULT
                } else {
                    nvic.force_hard_fault()
                };
                if derived == HARDFAULT && hardfault_blocked {
                    locked = true;
                } else if self.thumb2_exception_priority(bus, derived) < self.thumb2_exception_priority(bus, exception) {
                    exception = derived;
                    nonsecure = bus.nvic.as_ref().unwrap().exception_targets_nonsecure(exception, origin_secure);
                    let vector = self.vector_address(bus, exception, nonsecure)?;
                    match bus.read_cortex_m_ram_or_mock_word(vector) {
                        Ok(target) => handler = target,
                        Err(_) => {
                            bus.nvic.as_mut().unwrap().record_vector_fault();
                            locked = true;
                        }
                    }
                } else if derived == HARDFAULT && exception != HARDFAULT {
                    bus.nvic.as_mut().unwrap().set_hard_fault_pending(true);
                }
            }
        }
        let fp_address = stack.wrapping_add(32);
        if has_callee_frame {
            stack = stack.wrapping_sub(SECURE_CALLEE_BYTES);
            let mut callee = [0; 10];
            callee[0] = if self.fp_active { SECURE_INTEGRITY_SIGNATURE & !1 } else { SECURE_INTEGRITY_SIGNATURE };
            callee[2..].copy_from_slice(&r[4..12]);
            if let Err(error) = bus.write_cortex_m_exception_stack(stack, &callee) {
                if !matches!(error, BusError::SlaveFault(_) | BusError::Unmapped(..)) {
                    return Err(error.to_string());
                }
                let nvic = bus.nvic.as_mut().unwrap();
                nvic.record_stacking_fault();
                if self.hardfault_masked(nvic) || exception == NMI && !nonsecure {
                    locked = true;
                } else {
                    exception = nvic.force_hard_fault();
                    nonsecure = false;
                    let vector = self.vector_address(bus, exception, false)?;
                    match bus.read_cortex_m_ram_or_mock_word(vector) {
                        Ok(target) => handler = target,
                        Err(_) => {
                            bus.nvic.as_mut().unwrap().record_vector_fault();
                            locked = true;
                        }
                    }
                }
            }
        }
        if lazy && !locked {
            let nvic = bus.nvic.as_mut().unwrap();
            let hardfault_ready = !self.faultmask && !matches!(self.active_exception, NMI | HARDFAULT);
            nvic.reserve_lazy_fp(fp_address, hardfault_ready, full_fp_frame);
        }
        if self.uses_psp() {
            self.psp = stack;
            r[13] = self.msp;
            r[14] = 0xffff_fffd;
        } else {
            r[13] = stack;
            r[14] = if self.active_exception == 0 {
                0xffff_fff9
            } else {
                0xffff_fff1
            };
        }
        if self.fp_active {
            r[14] &= !0x10;
        }
        if self.cortex_m33 || self.cortex_m55 {
            r[14] = (r[14] & !0x41) | (u32::from(origin_secure) << 6) | u32::from(self.trustzone && !nonsecure);
            if has_callee_frame && !nonsecure {
                r[14] &= !0x20;
            }
        }
        self.fp_active = false;
        bus.clear_reservation(0);
        self.active_exceptions.insert(exception);
        if let Some(nvic) = bus.nvic.as_mut() { nvic.set_active(self.active_exceptions); }
        self.active_exception = exception;
        if self.trustzone {
            self.switch_security(!nonsecure, r)?;
        }
        self.itstate = 0;
        if locked {
            bus.nvic.as_mut().unwrap().enter_lockup();
            return Ok(self.lockup());
        }
        self.invalid_thumb_state = handler & 1 == 0;
        Ok(handler & !1)
    }

    fn exception_return_fault(
        &mut self,
        token: u32,
        remaining: ActiveExceptions,
        fault: u8,
        r: &mut [u32; 32],
        bus: &mut Bus,
    ) -> Result<u32, String> {
        self.active_exceptions = remaining;
        self.active_exception = (2..=u8::MAX).filter(|&number| remaining.contains(number))
            .min_by_key(|&number| self.thumb2_exception_priority(bus, number)).unwrap_or(0);
        let nvic = bus.nvic.as_mut().ok_or("exception return requires NVIC")?;
        nvic.set_active(remaining);
        match fault {
            5 => nvic.record_unstacking_fault(),
            6 => nvic.record_invalid_exception_return(),
            _ => {}
        }
        self.branch_exception = true;
        if (fault != 7 && self.hardfault_masked(nvic))
            || (fault == 7 && self.secure && self.hardfault_masked(nvic))
        {
            let frame_size = if token & 0x10 == 0 { 104 } else { 32 };
            if token & 4 == 0 {
                r[13] = r[13].wrapping_add(frame_size);
            } else {
                self.psp = self.psp.wrapping_add(frame_size);
            }
            nvic.enter_lockup();
            return Ok(self.lockup());
        }
        let mut exception = if nvic.fault_enabled(fault) && self.can_take_sync_exception(fault, nvic) {
            fault
        } else {
            nvic.force_hard_fault()
        };
        let mut nonsecure = fault != 7 && nvic.exception_targets_nonsecure(exception, self.secure_state());
        let handler = loop {
            let vector = self.vector_address(bus, exception, nonsecure)?;
            match bus.read_cortex_m_ram_or_mock_word(vector) {
                Ok(handler) => break handler,
                Err(BusError::SlaveFault(_) | BusError::Unmapped(..)) => {
                    let nvic = bus.nvic.as_mut().unwrap();
                    nvic.record_vector_fault();
                    if exception == HARDFAULT {
                        nvic.enter_lockup();
                        return Ok(self.lockup());
                    }
                    exception = HARDFAULT;
                    nonsecure = fault != 7 && nvic.exception_targets_nonsecure(exception, self.secure_state());
                }
                Err(error) => return Err(error.to_string()),
            }
        };
        r[14] = token;
        if self.trustzone {
            let source_secure = self.secure_state();
            r[14] = (token & !1) | u32::from(!nonsecure);
            if !source_secure && !nonsecure && token & 0x40 != 0 {
                r[14] &= !0x20;
            }
            self.switch_security(!nonsecure, r)?;
        }
        self.fp_active = false;
        self.itstate = 0;
        self.invalid_thumb_state = handler & 1 == 0;
        self.active_exception = exception;
        self.active_exceptions.insert(exception);
        bus.nvic.as_mut().unwrap().set_active(self.active_exceptions);
        Ok(handler & !1)
    }

    fn branch(&mut self, target: u32, r: &mut [u32; 32], bus: &mut Bus) -> Result<u32, String> {
        let basic_target = if self.trustzone && target & 0xffffff82 == 0xffffff80 {
            target | 0x71
        } else if (self.cortex_m33 || self.cortex_m55) && target & 0xffff_ffa1 == 0xffff_ffa0 {
            target | 0x51
        } else if self.thumb2 {
            target | 0x10
        } else {
            target
        };
        if self.active_exception != 0
            && matches!(basic_target, 0xffff_fff1 | 0xffff_fff9 | 0xffff_fffd)
        {
            let extended = target & 0x10 == 0;
            let use_psp = basic_target == 0xffff_fffd;
            let return_secure = self.trustzone && target & 0x40 != 0;
            let full_fp_frame = extended && return_secure && bus.nvic.as_ref().is_some_and(Nvic::secure_full_fp_frame);
            let frame_size = if full_fp_frame { 168 } else if extended { 104 } else { 32 };
            let mut remaining = self.active_exceptions;
            remaining.remove(self.active_exception);
            if self.trustzone && target & 0x21 == 0 {
                bus.nvic.as_mut().unwrap().record_secure_fault(1 << 2);
                return self.exception_return_fault(target, remaining, 7, r, bus);
            }
            let stack = if self.trustzone && return_secure != self.secure {
                let bank = self.inactive_security_bank;
                if use_psp { bank.psp } else { bank.msp }
            } else if use_psp {
                self.psp
            } else {
                r[13]
            };
            let restore_callee = return_secure && (!self.secure || target & 0x20 == 0);
            let mut callee = [0; 8];
            let stack = if restore_callee {
                let expected = if extended { SECURE_INTEGRITY_SIGNATURE & !1 } else { SECURE_INTEGRITY_SIGNATURE };
                match bus.read_cortex_m_ram_or_mock_word(stack) {
                    Ok(signature) if signature == expected => {}
                    Ok(_) => {
                        bus.nvic.as_mut().unwrap().record_secure_fault(1 << 1);
                        return self.exception_return_fault(target, remaining, 7, r, bus);
                    }
                    Err(BusError::SlaveFault(_) | BusError::Unmapped(..)) => {
                        return self.exception_return_fault(target, remaining, 5, r, bus);
                    }
                    Err(error) => return Err(error.to_string()),
                }
                for (index, value) in callee.iter_mut().enumerate() {
                    match bus.read_cortex_m_ram_or_mock_word(stack.wrapping_add((index as u32 + 2) * 4)) {
                        Ok(word) => *value = word,
                        Err(BusError::SlaveFault(_) | BusError::Unmapped(..)) => {
                            return self.exception_return_fault(target, remaining, 5, r, bus);
                        }
                        Err(error) => return Err(error.to_string()),
                    }
                }
                stack.wrapping_add(SECURE_CALLEE_BYTES)
            } else {
                stack
            };
            bus.check_thumb_access(stack, frame_size, false, false)?;
            let mut frame = [0; 8];
            for (index, word) in frame.iter_mut().enumerate() {
                match bus.read_cortex_m_ram_or_mock_word(stack.wrapping_add(index as u32 * 4)) {
                    Ok(value) => *word = value,
                    Err(BusError::SlaveFault(_) | BusError::Unmapped(..)) => {
                        return self.exception_return_fault(target, remaining, 5, r, bus);
                    }
                    Err(error) => return Err(error.to_string()),
                }
            }
            let [r0, r1, r2, r3, r12, lr, pc, xpsr] = frame;
            let previous = xpsr & 0x1ff;
            let valid_mode = if basic_target != 0xffff_fff1 {
                previous == 0 && remaining.is_empty()
            } else {
                previous != 0
                    && u8::try_from(previous).is_ok_and(|exception| remaining.contains(exception))
            };
            if !valid_mode && self.thumb2 {
                return self.exception_return_fault(target, remaining, 6, r, bus);
            }
            if xpsr & (1 << 24) == 0 || !valid_mode {
                return Err(format!(
                    "invalid Cortex-M exception return frame: pc={pc:#x} xpsr={xpsr:#x} target={target:#x}"
                ));
            }
            let lazy_address = bus.nvic.as_ref().and_then(Nvic::pending_lazy_fp);
            if extended && lazy_address.is_some_and(|(address, _, _)| address == stack.wrapping_add(32)) {
                bus.nvic.as_mut().unwrap().clear_lazy_fp();
            } else if extended {
                let saved = bus
                    .memory_slice(stack.wrapping_add(32), 68)
                    .map_err(|e| e.to_string())?;
                for (value, bytes) in self.fp[..16].iter_mut().zip(saved.as_chunks::<4>().0) {
                    *value = u32::from_le_bytes(*bytes);
                }
                self.fpscr = u32::from_le_bytes(saved[64..68].try_into().unwrap());
                if full_fp_frame {
                    let saved = bus.memory_slice(stack.wrapping_add(104), 64).map_err(|e| e.to_string())?;
                    for (value, bytes) in self.fp[16..].iter_mut().zip(saved.as_chunks::<4>().0) {
                        *value = u32::from_le_bytes(*bytes);
                    }
                }
            }
            if self.trustzone {
                self.switch_security(return_secure, r)?;
            }
            if restore_callee {
                r[4..12].copy_from_slice(&callee);
            }
            r[..4].copy_from_slice(&[r0, r1, r2, r3]);
            r[12] = r12;
            if use_psp {
                self.msp = r[13];
            }
            if previous == 0 {
                self.thread_uses_psp = use_psp;
                if bus.nvic.as_ref().is_some_and(Nvic::sleep_on_exit) {
                    self.sleep = Sleep::Interrupt;
                }
            }
            self.fp_active = extended;
            r[13] = stack.wrapping_add(frame_size as u32 + ((xpsr >> 7) & 4));
            r[14] = lr;
            self.faultmask = false;
            self.set_apsr(xpsr);
            self.itstate = (((xpsr >> 25) & 3) | ((xpsr >> 8) & 0xfc)) as u8;
            self.active_exception = previous as u8;
            self.active_exceptions = remaining;
            if let Some(nvic) = bus.nvic.as_mut() {
                nvic.set_active(remaining);
            }
            self.signal_event();
            return Ok(pc & !1);
        }
        if target & 1 == 0 {
            return Err(format!(
                "branch target {target:#010x} does not select Thumb state"
            ));
        }
        Ok(target & !1)
    }

    pub fn sleeping(&self) -> bool {
        self.sleep != Sleep::Running
    }

    fn signal_event(&mut self) {
        if self.sleep == Sleep::Event {
            self.sleep = Sleep::Running;
        } else {
            self.event = true;
        }
    }

    pub(crate) fn sample_interrupts(&mut self, bus: &mut Bus) {
        bus.sample_nvic(&self.active_exceptions, self.sleeping());
        bus.tick_systick();
        let pending = bus
            .nvic
            .as_ref()
            .map_or(Default::default(), Nvic::pending_exceptions);
        let system_pending = bus.systick.as_ref().map_or(0, SysTick::pending_exceptions);
        let nvic_event = bus.nvic.as_mut().is_some_and(Nvic::take_pending_event);
        let newly_pending = pending
            .iter()
            .zip(self.pending)
            .any(|(next, previous)| next & !previous != 0)
            || system_pending & !self.system_pending != 0;
        if nvic_event
            || (newly_pending && bus.nvic.as_ref().is_some_and(Nvic::send_event_on_pending))
        {
            self.signal_event();
        }
        self.pending = pending;
        self.system_pending = system_pending;
    }

    pub fn service_interrupts(
        &mut self,
        pc: &mut u32,
        r: &mut [u32; 32],
        bus: &mut Bus,
    ) -> Result<bool, String> {
        self.sample_interrupts(bus);
        if !self.thumb2 {
            if self.system_pending & (1 << NMI) != 0 && !self.active_exceptions.contains(NMI) {
                *pc = self.exception_entry_m0(NMI, *pc, r, bus);
                self.acknowledge(NMI, bus);
                return Ok(true);
            }
            if self.locked_up || self.active_exceptions.contains(NMI) {
                return Ok(false);
            }
            if bus.nvic.as_ref().is_some_and(Nvic::hardfault_pending) && self.can_take_hardfault() {
                *pc = self.exception_entry_m0(HARDFAULT, *pc, r, bus);
                self.acknowledge(HARDFAULT, bus);
                return Ok(true);
            }
        }
        if self.thumb2 {
            if self.locked_up && !self.instruction_lockup {
                return Ok(false);
            }
            if self.system_pending & (1 << NMI) != 0 && !self.active_exceptions.contains(NMI) {
                self.sleep = Sleep::Running;
                self.locked_up = false;
                self.instruction_lockup = false;
                if let Some(nvic) = bus.nvic.as_mut() {
                    nvic.leave_lockup();
                }
                *pc = self.exception_entry(NMI, *pc, r, bus)?;
                self.acknowledge(NMI, bus);
                return Ok(true);
            }
            if self.locked_up {
                return Ok(false);
            }
            if !self.faultmask && !matches!(self.active_exception, NMI | HARDFAULT)
                && bus.nvic.as_ref().is_some_and(Nvic::hardfault_pending)
            {
                self.sleep = Sleep::Running;
                *pc = self.exception_entry(HARDFAULT, *pc, r, bus)?;
                self.acknowledge(HARDFAULT, bus);
                return Ok(true);
            }
        }
        if self.faultmask || (self.interrupts_masked && self.sleep != Sleep::Interrupt) {
            return Ok(false);
        }
        let priority = |exception| bus.nvic.as_ref().map_or(0, |nvic| nvic.priority(exception));
        let current_priority = if self.active_exception == 0 {
            256
        } else {
            u16::from(bus.nvic.as_ref().map_or(0, |nvic| {
                nvic.preemption_priority(priority(self.active_exception))
            }))
        };
        let mut selected = bus
            .nvic
            .as_ref()
            .and_then(|nvic| nvic.next_irq(&self.active_exceptions))
            .map(|(line, priority)| (16 + line, priority));
        for exception in [PENDSV, SYSTICK] {
            if bus
                .systick
                .as_ref()
                .is_some_and(|timer| timer.pending(exception))
                && !self.active_exceptions.contains(exception)
            {
                let candidate = (exception, priority(exception));
                if selected.is_none_or(|(number, priority)| {
                    (candidate.1, candidate.0) < (priority, number)
                }) {
                    selected = Some(candidate);
                }
            }
        }
        if let Some((exception, _)) = selected.filter(|(_, priority)| {
            u16::from(
                bus.nvic
                    .as_ref()
                    .map_or(*priority, |nvic| nvic.preemption_priority(*priority)),
            ) < current_priority
                && (self.basepri == 0
                    || bus.nvic.as_ref().map_or(*priority < self.basepri, |nvic| {
                        nvic.preemption_priority(*priority) < nvic.preemption_priority(self.basepri)
                    }))
        }) {
            self.sleep = Sleep::Running;
            if self.interrupts_masked {
                return Ok(false);
            }
            *pc = self.exception_entry(exception, *pc, r, bus)?;
            if self.active_exception != exception {
                return Ok(true);
            }
            if matches!(exception, PENDSV | SYSTICK) {
                bus.systick.as_mut().unwrap().acknowledge(exception);
            } else {
                bus.nvic.as_mut().unwrap().acknowledge(exception - 16);
            }
            if exception < 16 {
                self.system_pending &= !(1 << exception);
            } else {
                let irq = usize::from(exception - 16);
                self.pending[irq / 32] &= !(1 << (irq % 32));
            }
            return Ok(true);
        }
        Ok(false)
    }

    fn stack_transfer(
        &mut self,
        mut mask: u32,
        pop: bool,
        next: &mut u32,
        r: &mut [u32; 32],
        bus: &mut Bus,
    ) -> Result<(), String> {
        let bytes = mask.count_ones() * 4;
        let sp = r[13];
        if sp & 3 != 0 {
            return Err(format!("unaligned stack pointer {sp:#010x}"));
        }
        if pop {
            bus.check_thumb_access(sp, bytes as usize, false, false)?;
            let mut restored = *r;
            let mut target = None;
            let mut address = sp;
            while mask != 0 {
                let register = mask.trailing_zeros() as usize;
                mask &= mask - 1;
                let value = bus
                    .read_ram::<4>(address)
                    .map_err(|error| error.to_string())?;
                if register == 15 {
                    target = Some(value);
                } else {
                    restored[register] = value;
                }
                address = address.wrapping_add(4);
            }
            restored[13] = sp.wrapping_add(bytes);
            if let Some(target) = target {
                *next = self.branch(target, &mut restored, bus)?;
            }
            *r = restored;
        } else {
            let new_sp = sp.wrapping_sub(bytes);
            bus.check_thumb_access(new_sp, bytes as usize, true, false)?;
            let stack = bus
                .memory(new_sp, bytes as usize)
                .map_err(|error| error.to_string())?;
            for word in stack.as_chunks_mut::<4>().0 {
                let register = mask.trailing_zeros() as usize;
                mask &= mask - 1;
                *word = r[register].to_le_bytes();
            }
            r[13] = new_sp;
        }
        Ok(())
    }

    pub(crate) fn debug_vfp_registers(&self) -> (&[u32; 32], u32) {
        (&self.fp, self.fpscr)
    }

    pub(crate) fn debug_step(
        &mut self,
        pc: &mut u32,
        registers: &mut [u32; 32],
        bus: &mut Bus,
        output: &mut impl Write,
        enter_debug_mode: bool,
    ) -> Result<DebugStep, String> {
        if !self.clocked() {
            return Ok(DebugStep::Sleeping);
        }
        if self.sleeping() {
            if !enter_debug_mode || self.sleep != Sleep::Interrupt {
                return Ok(DebugStep::Sleeping);
            }
            self.sleep = Sleep::Running;
        }
        if self.step(pc, registers, bus, output)? {
            Ok(DebugStep::Retired)
        } else {
            Ok(DebugStep::Breakpoint)
        }
    }

    pub(crate) fn execute_guest(
        &mut self,
        pc: &mut u32,
        r: &mut [u32; 32],
        bus: &mut Bus,
        output: &mut impl Write,
    ) -> Result<ThumbStep, String> {
        if self.locked_up {
            return Ok(ThumbStep::Exception);
        }
        if self.thumb2 {
            if self.invalid_thumb_state {
                const USAGEFAULT: u8 = 6;
                let nvic = bus.nvic.as_mut().ok_or("invalid Thumb state requires NVIC")?;
                nvic.record_instruction_fault(true);
                let exception = if nvic.fault_enabled(USAGEFAULT) && self.can_take_sync_exception(USAGEFAULT, nvic) {
                    USAGEFAULT
                } else {
                    nvic.force_hard_fault()
                };
                if exception == HARDFAULT && (self.faultmask || matches!(self.active_exception, NMI | HARDFAULT)) {
                    nvic.enter_lockup();
                    *pc = self.lockup();
                } else {
                    *pc = self.exception_entry(exception, *pc, r, bus)?;
                }
                return Ok(ThumbStep::Exception);
            }
            let fault_pc = *pc;
            let fault_itstate = self.itstate;
            bus.thumb_negative_priority = self.faultmask || matches!(self.active_exception, NMI | HARDFAULT);
            bus.execute_thumb_block_hook(false, *pc, self.itstate)?;
            let step = bus.thumb_fetch(*pc).map_err(InstructionError::Host)
                .and_then(|instruction| self.step_fetched(instruction, pc, r, bus, output));
            if let Some(address) = bus.take_thumb_data_fault() {
                const BUSFAULT: u8 = 5;
                self.itstate = fault_itstate;
                let nvic = bus.nvic.as_mut().ok_or("precise bus fault requires NVIC")?;
                nvic.record_precise_bus_fault(address, false);
                if bus.thumb_negative_priority {
                    self.instruction_lockup = true;
                    nvic.enter_lockup();
                    *pc = self.lockup();
                    return Ok(ThumbStep::Exception);
                }
                let exception = if nvic.fault_enabled(BUSFAULT) && self.can_take_sync_exception(BUSFAULT, nvic) {
                    BUSFAULT
                } else {
                    nvic.force_hard_fault()
                };
                *pc = self.exception_entry(exception, fault_pc, r, bus)?;
                return Ok(ThumbStep::Exception);
            }
            if std::mem::take(&mut self.branch_exception) {
                return Ok(ThumbStep::Exception);
            }
            return match step {
                Ok(true) => {
                    self.retired_instructions = self.retired_instructions.wrapping_add(1);
                    bus.execute_thumb_block_hook(true, *pc, self.itstate)?;
                    Ok(ThumbStep::Retired)
                }
                Ok(false) => Ok(ThumbStep::Breakpoint),
                Err(InstructionError::SemihostingExit(exit, next)) => {
                    *pc = next;
                    Ok(ThumbStep::Semihosting(exit))
                }
                Err(InstructionError::Host(error)) => Err(error),
                _ => Err("Thumb instruction fault".into()),
            };
        }
        if !self.invalid_thumb_state {
            bus.execute_thumb_block_hook(false, *pc, self.itstate)?;
        }
        let result = if self.invalid_thumb_state {
            Err(InstructionError::GuestFault)
        } else {
            bus.read_ram::<2>(*pc)
                .map_err(InstructionError::from)
                .and_then(|inst| self.step_fetched(inst, pc, r, bus, output))
        };
        match result {
            Ok(true) => {
                self.retired_instructions = self.retired_instructions.wrapping_add(1);
                bus.execute_thumb_block_hook(true, *pc, self.itstate)?;
                Ok(ThumbStep::Retired)
            }
            Ok(false) => Ok(ThumbStep::Breakpoint),
            Err(InstructionError::SemihostingExit(exit, next)) => {
                *pc = next;
                Ok(ThumbStep::Semihosting(exit))
            }
            Err(InstructionError::Host(error)) => Err(error),
            Err(InstructionError::ExceptionTaken(next)) => {
                *pc = next;
                Ok(ThumbStep::Exception)
            }
            Err(InstructionError::GuestFault) => {
                *pc = self.hardfault(*pc, r, bus);
                Ok(ThumbStep::Exception)
            }
        }
    }

    pub fn step(
        &mut self,
        pc: &mut u32,
        r: &mut [u32; 32],
        bus: &mut Bus,
        output: &mut impl Write,
    ) -> Result<bool, String> {
        self.step_with_boundaries(pc, r, bus, output, |_| {})
    }

    /// Observe instruction boundaries for MaximumBlockSize=1. PC at End
    /// is the next PC, including a taken branch or a skipped IT slot.
    pub(crate) fn step_with_boundaries(
        &mut self,
        pc: &mut u32,
        r: &mut [u32; 32],
        bus: &mut Bus,
        output: &mut impl Write,
        mut observe: impl FnMut(ThumbBoundary),
    ) -> Result<bool, String> {
        let inst = bus.thumb_fetch(*pc)?;
        observe(ThumbBoundary {
            phase: InstructionBoundary::Begin,
            pc: *pc,
            itstate: self.itstate,
            active_exception: self.active_exception,
            exception_depth: self.active_exceptions.depth(),
        });
        let retired = self
            .step_fetched(inst, pc, r, bus, output)
            .map_err(|error| match error {
                InstructionError::Host(error) => error,
                _ => "Thumb instruction fault".into(),
            })?;
        let retired = retired && !std::mem::take(&mut self.branch_exception);
        if retired {
            self.retired_instructions = self.retired_instructions.wrapping_add(1);
            observe(ThumbBoundary {
                phase: InstructionBoundary::End,
                pc: *pc,
                itstate: self.itstate,
                active_exception: self.active_exception,
                exception_depth: self.active_exceptions.depth(),
            });
        }
        Ok(retired)
    }

    fn step_fetched(
        &mut self,
        inst: u32,
        pc: &mut u32,
        r: &mut [u32; 32],
        bus: &mut Bus,
        output: &mut impl Write,
    ) -> Result<bool, InstructionError> {
        let unsupported = || format!("unsupported Thumb instruction {inst:#06x}");
        let mut next = pc.wrapping_add(2);
        let architectural_pc = pc.wrapping_add(4);
        let rd = (inst & 7) as usize;
        let rn = ((inst >> 3) & 7) as usize;
        let in_it = self.itstate != 0;
        if in_it {
            let condition = self.condition(u32::from(self.itstate >> 4));
            self.itstate = if self.itstate & 7 == 0 {
                0
            } else {
                (self.itstate & 0xe0) | ((self.itstate << 1) & 0x1f)
            };
            if !condition {
                *pc = pc.wrapping_add(if inst >= 0xe800 { 4 } else { 2 });
                return Ok(true);
            }
        }
        let saved_flags = self.apsr();
        match inst {
            0x1800..=0x1fff => {
                let field = ((inst >> 6) & 7) as usize;
                let operand = if inst & 0x400 != 0 {
                    field as u32
                } else {
                    r[field]
                };
                r[rd] = self.arithmetic(r[rn], operand, inst & 0x200 != 0);
            }
            0x0000..=0x17ff => {
                let shift = (inst >> 6) & 31;
                let value = r[rn];
                let result = match inst >> 11 {
                    0 if shift == 0 => value,
                    0 => {
                        self.carry = (value >> (32 - shift)) & 1 != 0;
                        value << shift
                    }
                    1 => {
                        let shift = if shift == 0 { 32 } else { shift };
                        self.carry = (value >> (shift - 1)) & 1 != 0;
                        value.checked_shr(shift).unwrap_or(0)
                    }
                    _ => {
                        let shift = if shift == 0 { 32 } else { shift };
                        self.carry = (value >> (shift - 1)) & 1 != 0;
                        ((value as i32) >> shift.min(31)) as u32
                    }
                };
                r[rd] = result;
                self.nz(result);
            }
            0x2000..=0x3fff => {
                let rd = ((inst >> 8) & 7) as usize;
                let value = inst & 0xff;
                match (inst >> 11) & 3 {
                    0 => {
                        r[rd] = value;
                        self.nz(value);
                    }
                    1 => {
                        self.arithmetic(r[rd], value, true);
                    }
                    2 => r[rd] = self.arithmetic(r[rd], value, false),
                    _ => r[rd] = self.arithmetic(r[rd], value, true),
                }
            }
            0x4000..=0x43ff => {
                if let Some(value) = self.data_processing((inst >> 6) & 15, r[rd], r[rn]) {
                    r[rd] = value;
                }
            }
            0x4400..=0x46ff => {
                let rd = rd | ((inst >> 4) & 8) as usize;
                let rm = ((inst >> 3) & 15) as usize;
                let lhs = if rd == 15 { architectural_pc } else { r[rd] };
                let rhs = if rm == 15 { architectural_pc } else { r[rm] };
                let value = match (inst >> 8) & 3 {
                    0 => Some(lhs.wrapping_add(rhs)),
                    1 => {
                        self.arithmetic(lhs, rhs, true);
                        None
                    }
                    _ => Some(rhs),
                };
                if let Some(value) = value {
                    if rd == 15 {
                        next = value & !1;
                    } else {
                        r[rd] = if rd == 13 { value & !3 } else { value };
                    }
                }
            }
            0x4700..=0x47ff if inst & 7 == 4 && self.trustzone && self.secure => {
                let rm = ((inst >> 3) & 15) as usize;
                if matches!(rm, 13 | 15) {
                    return Err(unsupported().into());
                }
                let target = r[rm];
                if target & 1 != 0 {
                    if inst & 0x80 != 0 {
                        r[14] = next | 1;
                    }
                } else {
                    if inst & 0x80 != 0 {
                        let stack = r[13].wrapping_sub(8);
                        let frame = [next | 1, u32::from(self.active_exception)];
                        match bus.write_cortex_m_exception_stack(stack, &frame) {
                            Ok(()) => {}
                            Err(BusError::SlaveFault(_) | BusError::Unmapped(..)) => {
                                let nvic = bus.nvic.as_mut().unwrap();
                                nvic.record_stacking_fault();
                                let exception = if nvic.fault_enabled(5) && self.can_take_sync_exception(5, nvic) {
                                    5
                                } else if self.hardfault_masked(nvic) {
                                    nvic.enter_lockup();
                                    *pc = self.lockup();
                                    self.branch_exception = true;
                                    return Ok(false);
                                } else {
                                    nvic.force_hard_fault()
                                };
                                *pc = self.exception_entry(exception, *pc, r, bus)?;
                                return Ok(true);
                            }
                            Err(error) => return Err(error.to_string().into()),
                        }
                        r[13] = stack;
                        r[14] = 0xfeff_ffff;
                    }
                    self.switch_security(false, r)?;
                }
                next = target & !1;
            }
            0x4700..=0x47ff if inst & 7 == 0 => {
                let rm = ((inst >> 3) & 15) as usize;
                let target = if rm == 15 { architectural_pc } else { r[rm] };
                if inst & 0x80 != 0 {
                    if target & 1 == 0 {
                        return Err(format!(
                            "branch target {target:#010x} does not select Thumb state"
                        )
                        .into());
                    }
                    r[14] = next | 1;
                    next = target & !1;
                } else {
                    next = self.instruction_branch(target, r, bus)?;
                }
            }
            0x4800..=0x4fff => {
                let address = (architectural_pc & !3).wrapping_add((inst & 0xff) << 2);
                r[((inst >> 8) & 7) as usize] = self.instruction_read::<4>(bus, address)?;
            }
            0x5000..=0x5fff => {
                let offset = ((inst >> 6) & 7) as usize;
                let address = r[rn].wrapping_add(r[offset]);
                let operation = (inst >> 9) & 7;
                if operation < 3 {
                    let width = [4, 2, 1][operation as usize];
                    self.instruction_write(bus, address, width, r[rd], output)?;
                } else {
                    let value = match operation {
                        3 | 6 => self.instruction_read::<1>(bus, address),
                        5 | 7 => self.instruction_read::<2>(bus, address),
                        _ => self.instruction_read::<4>(bus, address),
                    }?;
                    r[rd] = match operation {
                        3 => (value as i8 as i32) as u32,
                        7 => (value as i16 as i32) as u32,
                        _ => value,
                    };
                }
            }
            0x6000..=0x8fff => {
                let width = match inst >> 12 {
                    6 => 4,
                    7 => 1,
                    _ => 2,
                };
                let address = r[rn].wrapping_add(((inst >> 6) & 31) * width);
                if inst & 0x800 == 0 {
                    self.instruction_write(bus, address, width as usize, r[rd], output)?;
                } else {
                    r[rd] = match width {
                        1 => self.instruction_read::<1>(bus, address),
                        2 => self.instruction_read::<2>(bus, address),
                        _ => self.instruction_read::<4>(bus, address),
                    }?;
                }
            }
            0x9000..=0x9fff => {
                let register = ((inst >> 8) & 7) as usize;
                let address = r[13].wrapping_add((inst & 0xff) << 2);
                if inst & 0x800 == 0 {
                    self.instruction_write(bus, address, 4, r[register], output)?;
                } else {
                    r[register] = self.instruction_read::<4>(bus, address)?;
                }
            }
            0xa000..=0xafff => {
                let base = if inst & 0x800 == 0 {
                    architectural_pc & !3
                } else {
                    r[13]
                };
                r[((inst >> 8) & 7) as usize] = base.wrapping_add((inst & 0xff) << 2);
            }
            0xb000..=0xb0ff => {
                let bytes = (inst & 0x7f) << 2;
                r[13] = if inst & 0x80 == 0 {
                    r[13].wrapping_add(bytes)
                } else {
                    r[13].wrapping_sub(bytes)
                };
            }
            0xb100..=0xbbff if self.thumb2 && inst & 0xf500 == 0xb100 => {
                if (r[rd] != 0) == (inst & 0x800 != 0) {
                    let offset = ((inst & 0x200) >> 3) | ((inst & 0xf8) >> 2);
                    next = architectural_pc.wrapping_add(offset);
                }
            }
            0xb400..=0xb5ff | 0xbc00..=0xbdff if inst & 0x1ff != 0 => {
                let pop = inst & 0x800 != 0;
                let mut mask = inst & 0xff;
                if inst & 0x100 != 0 {
                    mask |= 1 << if pop { 15 } else { 14 };
                }
                self.stack_transfer(mask, pop, &mut next, r, bus)?;
            }
            0xb661 | 0xb663 | 0xb671 | 0xb673 if self.thumb2 => {
                self.faultmask = inst & 0x10 != 0;
                if inst & 2 != 0 {
                    self.interrupts_masked = self.faultmask;
                }
            }
            0xb662 => self.interrupts_masked = false, // cpsie i
            0xb672 => self.interrupts_masked = true,  // cpsid i
            0xb200..=0xb2ff => {
                r[rd] = match (inst >> 6) & 3 {
                    0 => (r[rn] as i16 as i32) as u32,
                    1 => (r[rn] as i8 as i32) as u32,
                    2 => r[rn] & 0xffff,
                    _ => r[rn] & 0xff,
                };
            }
            0xba00..=0xbaff => {
                let value = r[rn];
                r[rd] = match (inst >> 6) & 3 {
                    0 => value.swap_bytes(),
                    1 => ((value & 0xff00_ff00) >> 8) | ((value & 0x00ff_00ff) << 8),
                    3 => ((value as u16).swap_bytes() as i16 as i32) as u32,
                    _ => return Err(unsupported().into()),
                };
            }
            0xbeab => {
                if let Some(mut handler) = bus.semihosting.take() {
                    let result = handler.call(bus, r[0], r[1], output);
                    bus.semihosting = Some(handler);
                    let (value, exit) = result?;
                    r[0] = value;
                    if let Some(exit) = exit {
                        return Err(InstructionError::SemihostingExit(exit, next));
                    }
                } else {
                    crate::monitor_log::record(2, &format!(
                        "Semihosting handler is not registered; R0=0x{:X} R1=0x{:X}", r[0], r[1]
                    ));
                    r[0] = u32::MAX;
                }
            }
            0xbe00..=0xbeff => return Ok(false),
            0xbf00 | 0xbf10 => {} // NOP and scheduler yield.
            0xbf20 => {
                if !std::mem::take(&mut self.event) {
                    self.sleep = Sleep::Event;
                }
            }
            0xbf30 => self.sleep = Sleep::Interrupt,
            0xbf40 => self.signal_event(),
            0xbf01..=0xbfff if self.thumb2 && !in_it && inst & 15 != 0 && inst & 0xf0 < 0xe0 => {
                self.itstate = inst as u8;
            }
            0xc000..=0xcfff if inst & 0xff != 0 => {
                let registers = inst & 0xff;
                let base_register = ((inst >> 8) & 7) as usize;
                let address = r[base_register];
                let load = inst & 0x800 != 0;
                let count = registers.count_ones();
                if address & 3 != 0 {
                    return Err(
                        format!("unaligned multiple-register transfer at {address:#010x}").into(),
                    );
                }
                let selected = (0..8).filter(|register| registers & (1 << register) != 0);
                if load {
                    let mut values = *r;
                    for (index, register) in selected.enumerate() {
                        values[register] = self
                            .instruction_read::<4>(bus, address.wrapping_add(index as u32 * 4))?;
                    }
                    *r = values;
                } else {
                    if registers & (1 << base_register) != 0
                        && registers.trailing_zeros() as usize != base_register
                    {
                        return Err(unsupported().into());
                    }
                    for (index, register) in selected.enumerate() {
                        self.instruction_write(
                            bus,
                            address.wrapping_add(index as u32 * 4),
                            4,
                            r[register],
                            output,
                        )?;
                    }
                }
                if !load || registers & (1 << base_register) == 0 {
                    r[base_register] = address.wrapping_add(count * 4);
                }
            }
            0xd000..=0xddff => {
                if self.condition((inst >> 8) & 15) {
                    next = architectural_pc.wrapping_add(((inst as i8 as i32) * 2) as u32);
                }
            }
            0xde00..=0xdeff if self.thumb2 => {
                let nvic = bus.nvic.as_mut().ok_or_else(unsupported)?;
                let usage_fault_allowed = self.can_take_sync_exception(6, nvic);
                nvic.record_undefined_instruction();
                let exception = if usage_fault_allowed && nvic.fault_enabled(6) {
                    6
                } else if self.hardfault_masked(nvic) {
                    nvic.enter_lockup();
                    *pc = self.lockup();
                    self.branch_exception = true;
                    return Ok(false);
                } else {
                    nvic.force_hard_fault()
                };
                next = self.exception_entry(exception, *pc, r, bus)?;
                self.branch_exception = true;
            }
            0xdf00..=0xdfff
                if self.active_exception == 0 && !self.interrupts_masked && !self.faultmask =>
            {
                next = self.exception_entry(11, next, r, bus)?;
            }
            0xdf00..=0xdfff => return Err(InstructionError::GuestFault),
            0xe000..=0xe7ff => {
                let offset = (((inst << 21) as i32) >> 20) as u32;
                next = architectural_pc.wrapping_add(offset);
            }
            0xf000..=0xf7ff => {
                let low = bus.thumb_fetch(next)?;
                let link = low & 0xd000 == 0xd000;
                if link || (self.thumb2 && low & 0xd000 == 0x9000) {
                    let sign = (inst >> 10) & 1;
                    let i1 = 1 ^ (((low >> 13) & 1) ^ sign);
                    let i2 = 1 ^ (((low >> 11) & 1) ^ sign);
                    let offset = (sign << 24)
                        | (i1 << 23)
                        | (i2 << 22)
                        | ((inst & 0x3ff) << 12)
                        | ((low & 0x7ff) << 1);
                    let offset = (((offset << 7) as i32) >> 7) as u32;
                    if link {
                        r[14] = architectural_pc | 1;
                    }
                    next = architectural_pc.wrapping_add(offset);
                } else if self.thumb2 && low & 0xd000 == 0x8000 && (inst >> 6) & 15 < 14 {
                    let offset = ((inst & 0x400) << 10)
                        | ((low & 0x800) << 8)
                        | ((low & 0x2000) << 5)
                        | ((inst & 0x3f) << 12)
                        | ((low & 0x7ff) << 1);
                    let offset = ((offset << 11) as i32 >> 11) as u32;
                    if self.condition((inst >> 6) & 15) {
                        next = architectural_pc.wrapping_add(offset);
                    } else {
                        next = architectural_pc;
                    }
                } else if self.thumb2
                    && (self.add_sub_wide(inst, low, architectural_pc, r)
                        || self.move_wide(inst, low, r)
                        || self.extract_bits(inst, low, r)
                        || self.insert_bits(inst, low, r)
                        || self.modified_immediate(inst, low, r))
                {
                    next = architectural_pc;
                } else {
                    next = architectural_pc;
                    let sysm = low & 0xff;
                    match (inst, low) {
                        (0xf3bf, 0x8f2f) if self.thumb2 => bus.clear_reservation(0),
                        (0xf3bf, 0x8f4f | 0x8f5f | 0x8f6f) => {}
                        (0xf3bf, 0x8f40..=0x8f5f)
                            if self.thumb2 && matches!(low & 15, 2 | 3 | 6 | 7 | 10 | 11 | 14) =>
                        {
                            // Bus operations execute synchronously and instruction fetch reads RAM.
                        }
                        (0xf3ef, 0x8000..=0x8fff) => {
                            let register = ((low >> 8) & 15) as usize;
                            if matches!(register, 13 | 15) {
                                return Err(unsupported().into());
                            }
                            r[register] = self.system_read(sysm, r).ok_or_else(unsupported)?;
                        }
                        (0xf380..=0xf38f, 0x8400..=0x8403 | 0x8800..=0x8803 | 0x8c00..=0x8c03) => {
                            let register = (inst & 15) as usize;
                            if matches!(register, 13 | 15) || (!self.thumb2 && low & 0x400 != 0) {
                                return Err(unsupported().into());
                            }
                            let mut mask = 0;
                            if low & 0x800 != 0 {
                                mask |= 0xf800_0000;
                            }
                            if low & 0x400 != 0 {
                                mask |= 0x000f_0000;
                            }
                            self.set_apsr((self.apsr() & !mask) | (r[register] & mask));
                        }
                        (0xf380..=0xf38f, 0x8800..=0x88ff) => {
                            let register = (inst & 15) as usize;
                            if matches!(register, 13 | 15)
                                || !self.system_write(
                                    sysm,
                                    if matches!(sysm, 17 | 18) {
                                        r[register]
                                            & u32::from(
                                                bus.nvic.as_ref().map_or(0xff, Nvic::priority_mask),
                                            )
                                    } else {
                                        r[register]
                                    },
                                    r,
                                )
                            {
                                return Err(unsupported().into());
                            }
                        }
                        _ => {
                            return Err(format!(
                                "unsupported Thumb instruction {inst:#06x} {low:#06x}"
                            )
                            .into());
                        }
                    }
                }
            }
            0xe800..=0xefff | 0xf800..=0xffff if self.thumb2 => {
                let low = bus.thumb_fetch(next)?;
                next = architectural_pc;
                if matches!(inst, 0xe92d | 0xe8bd)
                    && low.count_ones() >= 2
                    && low & (1 << 13) == 0
                    && (low & (1 << 15) == 0 || (inst == 0xe8bd && low & (1 << 14) == 0))
                {
                    self.stack_transfer(low, inst == 0xe8bd, &mut next, r, bus)?;
                } else {
                    let vector = self.cortex_m55.then(|| mve::VectorTransfer::decode(inst, low)).flatten();
                    let secure_fp_transfer = self.trustzone && self.secure
                        && inst & 0xffe0 == 0xec20 && low == 0x0a00;
                    if (vector.is_some() || secure_fp_transfer || Self::is_fpu_instruction(inst, low))
                        && self.preserve_lazy_fp(bus)?
                    {
                        *pc = LOCKUP_PC;
                        self.branch_exception = true;
                        return Ok(false);
                    }
                    if let Some(transfer) = vector {
                        self.execute_vector_transfer(transfer, inst, low, r, bus, output)?;
                    } else if secure_fp_transfer {
                        self.transfer_secure_fp(inst, r, bus, output)?;
                    } else if !self.execute_fpu(inst, low, architectural_pc, r, bus, output)? {
                        self.execute_thumb2(inst, low, &mut next, r, bus, output)?;
                    }
                }
            }
            _ => return Err(unsupported().into()),
        }
        if in_it
            && (inst < 0x2800 || matches!(inst, 0x3000..=0x43ff))
            && !(inst & 0xfc00 == 0x4000 && matches!((inst >> 6) & 15, 8 | 10 | 11))
        {
            self.set_apsr(saved_flags);
        }
        *pc = next;
        Ok(true)
    }
}

impl Thumb {
    fn instruction_read<const N: usize>(
        &self,
        bus: &mut Bus,
        address: u32,
    ) -> Result<u32, InstructionError> {
        if self.thumb2 {
            Ok(bus.thumb_read::<N>(address)?)
        } else {
            Ok(bus.read::<N>(address)?)
        }
    }

    fn instruction_write(
        &self,
        bus: &mut Bus,
        address: u32,
        width: usize,
        value: u32,
        output: &mut impl Write,
    ) -> Result<(), InstructionError> {
        if self.thumb2 {
            Ok(bus.thumb_write(address, width, value, output)?)
        } else {
            Ok(bus.write(address, width, value, output)?)
        }
    }

    fn instruction_branch(
        &mut self,
        target: u32,
        r: &mut [u32; 32],
        bus: &mut Bus,
    ) -> Result<u32, InstructionError> {
        if self.thumb2 {
            Ok(self.branch(target, r, bus)?)
        } else {
            self.branch_m0(target, r, bus)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synchronized_instruction_boundaries_follow_taken_branch() {
        use InstructionBoundary::{Begin, End};
        let mut bus = Bus::default();
        let mut out = std::io::sink();
        bus.map_ram(0x1000, 0x1000).unwrap();
        for (index, opcode) in [0xbf00, 0xbf00, 0xbf00, 0xe7fb].into_iter().enumerate() {
            bus.write(0x1000 + index as u32 * 2, 2, opcode, &mut out)
                .unwrap();
        }
        let mut thumb = Thumb::default();
        let mut pc = 0x1000;
        let mut regs = [0; 32];
        let mut events = Vec::new();
        for _ in 0..20 {
            assert!(
                thumb
                    .step_with_boundaries(&mut pc, &mut regs, &mut bus, &mut out, |e| events
                        .push(e))
                    .unwrap()
            );
        }
        assert_eq!(events.len(), 40);
        for (step, pair) in events.chunks_exact(2).enumerate() {
            assert_eq!(
                pair[0],
                ThumbBoundary {
                    phase: Begin,
                    pc: 0x1000 + (step % 4) as u32 * 2,
                    itstate: 0,
                    active_exception: 0,
                    exception_depth: 0,
                }
            );
            assert_eq!(
                pair[1],
                ThumbBoundary {
                    phase: End,
                    pc: 0x1000 + ((step + 1) % 4) as u32 * 2,
                    itstate: 0,
                    active_exception: 0,
                    exception_depth: 0,
                }
            );
        }
        assert_eq!(
            events
                .iter()
                .filter(|e| e.phase == Begin && e.pc == 0x1004)
                .count(),
            5
        );
    }

    #[test]
    fn instruction_boundaries_capture_it_state_and_skipped_slot() {
        let mut bus = Bus::default();
        let mut out = std::io::sink();
        bus.map_ram(0x1000, 0x1000).unwrap();
        // ITTET EQ; MOV r1,1; MOV r2,2; MOV r3,3 (NE, skipped); MOV r4,4.
        for (index, opcode) in [0xbf05, 0x2101, 0x2202, 0x2303, 0x2404]
            .into_iter()
            .enumerate()
        {
            bus.write(0x1000 + index as u32 * 2, 2, opcode, &mut out)
                .unwrap();
        }
        let mut thumb = Thumb {
            thumb2: true,
            zero: true,
            ..Thumb::default()
        };
        let mut pc = 0x1000;
        let mut regs = [0; 32];
        let mut events = Vec::new();
        for _ in 0..5 {
            assert!(
                thumb
                    .step_with_boundaries(&mut pc, &mut regs, &mut bus, &mut out, |e| events
                        .push(e))
                    .unwrap()
            );
        }
        assert_eq!(&regs[1..5], &[1, 2, 0, 4]);
        for (step, pair) in events.chunks_exact(2).enumerate() {
            let states = [0, 5, 10, 20, 8, 0];
            assert_eq!(
                (pair[0].phase, pair[0].pc, pair[0].itstate),
                (
                    InstructionBoundary::Begin,
                    0x1000 + step as u32 * 2,
                    states[step]
                )
            );
            assert_eq!(
                (pair[1].phase, pair[1].pc, pair[1].itstate),
                (
                    InstructionBoundary::End,
                    0x1002 + step as u32 * 2,
                    states[step + 1]
                )
            );
        }
    }

    #[test]
    fn debug_step_wfi_respects_debug_mode() {
        for enabled in [false, true] {
            let mut bus = Bus::default();
            let mut out = std::io::sink();
            bus.map_ram(0, 0x1000).unwrap();
            bus.write(0x10, 2, 0xbf30, &mut out).unwrap();
            bus.write(0x12, 2, 0xbf00, &mut out).unwrap();
            let mut thumb = Thumb::default();
            let mut registers = [0; 32];
            let mut pc = 0x10;
            assert_eq!(
                thumb
                    .debug_step(&mut pc, &mut registers, &mut bus, &mut out, enabled)
                    .unwrap(),
                DebugStep::Retired
            );
            assert_eq!(pc, 0x12);
            let stop = thumb
                .debug_step(&mut pc, &mut registers, &mut bus, &mut out, enabled)
                .unwrap();
            if enabled {
                assert_eq!((stop, pc), (DebugStep::Retired, 0x14));
            } else {
                assert_eq!((stop, pc), (DebugStep::Sleeping, 0x12));
                assert!(thumb.sleeping());
            }
        }
    }

    #[test]
    fn debug_vfp_readback_preserves_scalar_register_number_and_bits() {
        let mut thumb = Thumb::default();
        for (i, register) in thumb.fp.iter_mut().enumerate() {
            *register = (100.0 + i as f32).to_bits();
        }
        thumb.fpscr = 0x6000_0000;
        let (registers, fpscr) = thumb.debug_vfp_registers();
        for (i, &register) in registers.iter().enumerate() {
            assert_eq!(f32::from_bits(register), 100.0 + i as f32);
        }
        assert_eq!(fpscr, 0x6000_0000);
    }

    // Instructions from the ELF in tlib/arm/thumb2-data-processing-instr.robot.
    #[test]
    fn renode_thumb2_fixture_branches_and_reads_apsr() {
        let mut bus = Bus::default();
        bus.map_ram(0x1000, 0x1000).unwrap();
        let mut registers = [0; 32];
        let mut thumb = Thumb {
            negative: true,
            ..Thumb::default()
        };
        let mut output = std::io::sink();
        for (address, halfwords, next, link) in [
            (0x1002, [0xf000u16, 0xf804u16], 0x100e, 0x1007),
            (0x1006, [0xf000, 0xfcbc], 0x1982, 0x100b),
        ] {
            for (offset, halfword) in halfwords.into_iter().enumerate() {
                bus.write(address + 2 * offset as u32, 2, halfword.into(), &mut output)
                    .unwrap();
            }
            let mut pc = address;
            assert!(
                thumb
                    .step(&mut pc, &mut registers, &mut bus, &mut output)
                    .unwrap()
            );
            assert_eq!((pc, registers[14]), (next, link));
        }

        // The upstream ORRS case expects N=1, Z=0, and C=0. Seed those flags
        // directly to test its MRS encoding without the ARMv8-M ORRS.W setup.
        let mut pc = 0x1a1e;
        bus.write(pc, 2, 0xf3ef, &mut output).unwrap();
        bus.write(pc + 2, 2, 0x8000, &mut output).unwrap();
        assert!(
            thumb
                .step(&mut pc, &mut registers, &mut bus, &mut output)
                .unwrap()
        );
        assert_eq!(pc, 0x1a22);
        assert_eq!(registers[0] & 0xe000_0000, 0x8000_0000);
    }

    // arm-cortex-m-exception-return.robot, the non-TrustZone BX LR case.
    #[test]
    fn renode_exception_bx_lr_restores_interrupted_thread() {
        let mut bus = Bus::default();
        bus.map_ram(0, 0x1000).unwrap();
        bus.map_nvic(0xc0, None).unwrap();
        let mut output = std::io::sink();
        bus.write(16 * 4, 4, 0x101, &mut output).unwrap();
        bus.write(0x20, 2, 0xe7fe, &mut output).unwrap(); // b .
        bus.write(0x100, 2, 0xbf00, &mut output).unwrap(); // nop
        bus.write(0x102, 2, 0x4770, &mut output).unwrap(); // bx lr
        let mut thumb = Thumb::new(Some(0));
        let mut pc = 0x20;
        let mut registers = [0; 32];
        registers[13] = 0x800;

        thumb
            .step(&mut pc, &mut registers, &mut bus, &mut output)
            .unwrap();
        assert_eq!(pc, 0x20);
        bus.write(0xe000_e100, 4, 1, &mut output).unwrap();
        bus.write(0xe000_e200, 4, 1, &mut output).unwrap();
        assert!(
            thumb
                .service_interrupts(&mut pc, &mut registers, &mut bus)
                .unwrap()
        );
        assert_eq!(
            (pc, registers[13], registers[14]),
            (0x100, 0x7e0, 0xffff_fff9)
        );
        thumb
            .step(&mut pc, &mut registers, &mut bus, &mut output)
            .unwrap();
        assert_eq!(pc, 0x102);
        thumb
            .step(&mut pc, &mut registers, &mut bus, &mut output)
            .unwrap();
        assert_eq!(
            (pc, registers[13], thumb.active_exception),
            (0x20, 0x800, 0)
        );
    }

    #[test]
    fn renode_thread_bx_and_handler_blx_do_not_unstack() {
        // arm-cortex-m-exception-return.robot: Thread BX and Handler BLX.
        const EXC_RETURN: u32 = 0xffff_fff9;
        for (opcode, handler_mode) in [(0x4700, false), (0x47f0, true)] {
            let mut bus = Bus::default();
            bus.map_ram(0, 0x1000).unwrap();
            let mut output = std::io::sink();
            bus.write(16 * 4, 4, 0x101, &mut output).unwrap();
            let mut thumb = Thumb::new(Some(0));
            let mut pc = 0x20;
            let mut registers = [0; 32];
            registers[0] = EXC_RETURN;
            registers[13] = 0x800;
            if handler_mode {
                pc = thumb
                    .exception_entry(16, pc, &mut registers, &mut bus)
                    .unwrap();
                assert_eq!(registers[14], EXC_RETURN);
            }
            let sp = registers[13];
            let link = pc + 3;
            bus.write(pc, 2, opcode, &mut output).unwrap();
            thumb
                .step(&mut pc, &mut registers, &mut bus, &mut output)
                .unwrap();
            assert_eq!(pc, EXC_RETURN & !1);
            assert_eq!(registers[13], sp);
            assert_eq!(thumb.active_exception, if handler_mode { 16 } else { 0 });
            assert_eq!(registers[14], if handler_mode { link } else { 0 });
        }
    }

    #[test]
    fn renode_exception_vector_load_does_not_unstack_an_exc_return_value() {
        // arm-cortex-m-exception-return.robot: use an IRQ vector in place of the MPU fault.
        let mut bus = Bus::default();
        bus.map_ram(0, 0x1000).unwrap();
        bus.map_nvic(0xc0, None).unwrap();
        let mut output = std::io::sink();
        bus.write(16 * 4, 4, 0xffff_fff9, &mut output).unwrap();
        bus.write(0xe000_e100, 4, 1, &mut output).unwrap();
        bus.write(0xe000_e200, 4, 1, &mut output).unwrap();
        let mut thumb = Thumb::new(Some(0));
        let mut pc = 0x20;
        let mut registers = [0; 32];
        registers[13] = 0x800;

        assert!(
            thumb
                .service_interrupts(&mut pc, &mut registers, &mut bus)
                .unwrap()
        );
        assert_eq!(
            (pc, registers[13], thumb.active_exception),
            (0xffff_fff8, 0x7e0, 16)
        );
        assert_eq!(bus.read_ram::<4>(0x7e0 + 24).unwrap(), 0x20);
    }

    #[test]
    fn profiler_boundaries_report_nested_context_depth() {
        let mut bus = Bus::default();
        let mut out = std::io::sink();
        bus.map_ram(0, 0x4000).unwrap();
        bus.write(0x38, 4, 0x1001, &mut out).unwrap(); // PendSV vector
        bus.write(0x14, 4, 0x1201, &mut out).unwrap(); // Nested BusFault vector
        bus.write(0x2000, 2, 0xbf00, &mut out).unwrap();
        bus.write(0x1000, 2, 0x4770, &mut out).unwrap();
        bus.write(0x1200, 2, 0x4770, &mut out).unwrap();

        let mut thumb = Thumb::new(Some(0));
        let mut regs = [0; 32];
        regs[13] = 0x4000;
        let mut pc = 0x2000;
        let mut events = Vec::new();
        let mut step = |thumb: &mut Thumb, pc: &mut u32, regs: &mut [u32; 32], bus: &mut Bus| {
            assert!(
                thumb
                    .step_with_boundaries(pc, regs, bus, &mut out, |event| events.push(event))
                    .unwrap()
            );
        };
        step(&mut thumb, &mut pc, &mut regs, &mut bus);
        pc = thumb
            .exception_entry(PENDSV, pc, &mut regs, &mut bus)
            .unwrap();
        pc = thumb.exception_entry(5, pc, &mut regs, &mut bus).unwrap();
        step(&mut thumb, &mut pc, &mut regs, &mut bus);
        step(&mut thumb, &mut pc, &mut regs, &mut bus);

        let contexts: Vec<_> = events
            .iter()
            .map(|e| (e.active_exception, e.exception_depth))
            .collect();
        assert_eq!(
            contexts,
            [(0, 0), (0, 0), (5, 2), (PENDSV, 1), (PENDSV, 1), (0, 0)]
        );
        assert_eq!(pc, 0x2002);
    }

    #[test]
    fn renode_profiler_fixture_pendsv_dispatches_from_icsr() {
        // profiler-trace.robot: the initial PendSV, before the MPU/profiler checks.
        const ICSR: u32 = 0xe000_ed04;
        const PENDSVSET: u32 = 1 << 28;
        let mut bus = Bus::default();
        bus.map_ram(0, 0x3000).unwrap();
        bus.map_nvic(0xc0, None).unwrap();
        bus.map_systick().unwrap();
        let mut output = std::io::sink();
        bus.write(0x38, 4, 0x1001, &mut output).unwrap();
        bus.write(0x2000, 2, 0x6001, &mut output).unwrap(); // str r1, [r0]
        bus.write(0x1000, 2, 0x4770, &mut output).unwrap(); // bx lr
        let mut thumb = Thumb::new(Some(0));
        let mut pc = 0x2000;
        let mut registers = [0; 32];
        registers[0] = ICSR;
        registers[1] = PENDSVSET;
        registers[13] = 0x100;

        thumb
            .step(&mut pc, &mut registers, &mut bus, &mut output)
            .unwrap();
        assert_eq!(bus.read::<4>(ICSR).unwrap() & PENDSVSET, PENDSVSET);
        bus.write(ICSR, 4, 1 << 27, &mut output).unwrap(); // PENDSVCLR
        assert!(
            !thumb
                .service_interrupts(&mut pc, &mut registers, &mut bus)
                .unwrap()
        );
        bus.write(ICSR, 4, PENDSVSET, &mut output).unwrap();
        assert!(
            thumb
                .service_interrupts(&mut pc, &mut registers, &mut bus)
                .unwrap()
        );
        assert_eq!((pc, thumb.active_exception), (0x1000, PENDSV));
        assert_eq!(bus.read::<4>(ICSR).unwrap() & PENDSVSET, 0);
        thumb
            .step(&mut pc, &mut registers, &mut bus, &mut output)
            .unwrap();
        assert_eq!(
            (pc, registers[13], thumb.active_exception),
            (0x2002, 0x100, 0)
        );
    }

    #[test]
    fn renode_nonsecure_return_token_branches_without_trustzone() {
        // arm-cortex-m-exception-return.robot: Should Not Return From Insecure Mode
        // When Not Using TrustZone. Seed r0 to avoid the wide immediate load.
        let mut bus = Bus::default();
        bus.map_ram(0, 0x1000).unwrap();
        let mut output = std::io::sink();
        bus.write(0x20, 2, 0x4700, &mut output).unwrap(); // bx r0
        let mut thumb = Thumb::new(Some(0));
        let mut pc = 0x20;
        let mut registers = [0; 32];
        registers[0] = 0xfeff_ffff;
        registers[13] = 0x800;

        thumb
            .step(&mut pc, &mut registers, &mut bus, &mut output)
            .unwrap();
        assert_eq!(
            (pc, registers[13], thumb.active_exception),
            (0xfeff_fffe, 0x800, 0)
        );
    }

    #[test]
    fn vtor_relocates_exception_vectors_without_resetting_the_cpu() {
        use crate::nvic::VTOR;

        let mut bus = Bus::default();
        bus.map_ram(0, 0x400).unwrap();
        bus.map_nvic(0xc0, None).unwrap();
        bus.memory(44, 4)
            .unwrap()
            .copy_from_slice(&0x81u32.to_le_bytes());
        bus.memory(0x100 + 44, 4)
            .unwrap()
            .copy_from_slice(&0x201u32.to_le_bytes());
        bus.memory(0x60, 2).unwrap().copy_from_slice(&[0x00, 0xdf]);
        bus.memory(0x200, 2).unwrap().copy_from_slice(&[0x70, 0x47]);
        let mut thumb = Thumb::new(Some(0));
        let mut r = [0; 32];
        r[13] = 0x400;
        let mut pc = 0x60;
        let mut output = std::io::sink();
        bus.write(VTOR, 4, 0x17f, &mut output).unwrap();
        assert_eq!(bus.read::<4>(VTOR).unwrap(), 0x100);
        bus.write(VTOR, 4, 0x8000, &mut output).unwrap();
        assert_eq!(bus.read::<4>(VTOR).unwrap(), 0x100);
        thumb.step(&mut pc, &mut r, &mut bus, &mut output).unwrap();
        assert_eq!(pc, 0x200);
        thumb.step(&mut pc, &mut r, &mut bus, &mut output).unwrap();
        assert_eq!((pc, r[13]), (0x62, 0x400));
    }

    #[test]
    fn pendsv_runs_after_systick_and_switches_thread_context() {
        const ICSR: u32 = 0xe000_ed04;
        let mut bus = Bus::default();
        bus.map_ram(0, 0x100).unwrap();
        bus.map_ram(0x2000_0000, 0x200).unwrap();
        bus.map_systick().unwrap();
        bus.map_nvic(0xc0, None).unwrap();
        for (offset, value) in [(56, 0x81u32), (60, 0x91)] {
            bus.memory(offset, 4)
                .unwrap()
                .copy_from_slice(&value.to_le_bytes());
        }
        bus.memory(0x80, 6).unwrap().copy_from_slice(&[
            0x80, 0xf3, 0x09, 0x88, // msr psp, r0
            0x70, 0x47, // bx lr
        ]);
        bus.memory(0x90, 2).unwrap().copy_from_slice(&[0x70, 0x47]);
        let mut thumb = Thumb::new(Some(0));
        let mut r = [0; 32];
        let other_task = 0x2000_0040;
        r[0] = other_task;
        r[13] = 0x2000_0200;
        thumb.system_write(9, 0x2000_0100, &mut r);
        thumb.system_write(20, 2, &mut r);
        let frame = [42u32, 0, 0, 0, 0, 0, 0xa0, 1 << 24];
        bus.memory(other_task, 32)
            .unwrap()
            .copy_from_slice(&frame.map(u32::to_le_bytes).concat());
        let mut pc = 0x60;
        let mut output = std::io::sink();
        bus.write(0xe000_ed20, 4, 0x00c0_0000, &mut output).unwrap();
        bus.write(ICSR, 4, (1 << 26) | (1 << 28), &mut output)
            .unwrap();
        assert!(thumb.service_interrupts(&mut pc, &mut r, &mut bus).unwrap());
        assert_eq!(pc, 0x90);
        assert_eq!(bus.read::<4>(ICSR).unwrap(), 1 << 28);
        thumb.step(&mut pc, &mut r, &mut bus, &mut output).unwrap();
        assert_eq!(pc, 0x80); // EXC_RETURN tail-chains directly to PendSV.
        assert_eq!(bus.read::<4>(ICSR).unwrap(), 0);
        thumb.step(&mut pc, &mut r, &mut bus, &mut output).unwrap();
        thumb.step(&mut pc, &mut r, &mut bus, &mut output).unwrap();
        assert_eq!((pc, r[0], r[13]), (0xa0, 42, other_task + 32));
    }

    #[test]
    fn svc_stacks_thread_on_psp_but_runs_handler_on_msp() {
        let mut bus = Bus::default();
        bus.map_ram(0, 0x100).unwrap();
        bus.map_ram(0x2000_0000, 0x200).unwrap();
        bus.memory(44, 4)
            .unwrap()
            .copy_from_slice(&0x81u32.to_le_bytes());
        bus.memory(0x60, 10).unwrap().copy_from_slice(&[
            0x80, 0xf3, 0x09, 0x88, // msr psp, r0
            0x81, 0xf3, 0x14, 0x88, // msr control, r1
            0x00, 0xdf, // svc
        ]);
        bus.memory(0x80, 2).unwrap().copy_from_slice(&[0x70, 0x47]); // bx lr
        let mut thumb = Thumb::new(Some(0));
        let mut r = [0; 32];
        r[0] = 0x2000_0084;
        r[1] = 2;
        r[13] = 0x2000_0200;
        let mut pc = 0x60;
        let mut output = std::io::sink();
        for _ in 0..3 {
            assert!(thumb.step(&mut pc, &mut r, &mut bus, &mut output).unwrap());
        }
        assert_eq!((pc, r[13], r[14]), (0x80, 0x2000_0200, 0xffff_fffd));
        assert_eq!(thumb.system_read(9, &r), Some(0x2000_0060));
        assert_eq!(bus.read_ram::<4>(0x2000_0078).unwrap(), 0x6a);
        assert_eq!(bus.read_ram::<4>(0x2000_01e0).unwrap(), 0);
        assert!(thumb.step(&mut pc, &mut r, &mut bus, &mut output).unwrap());
        assert_eq!((pc, r[13]), (0x6a, 0x2000_0084));
        assert_eq!(thumb.system_read(8, &r), Some(0x2000_0200));
        assert_eq!(thumb.system_read(20, &r), Some(2));
    }

    #[test]
    fn transfers_register_lists_and_executes_thumb_system_instructions() {
        let mut bus = Bus::default();
        bus.map_ram(0, 0x80).unwrap();
        bus.memory(0, 20).unwrap().copy_from_slice(&[
            0x06, 0xc0, // stmia r0!, {r1, r2}
            0x30, 0xcb, // ldmia r3!, {r4, r5}
            0x81, 0xf3, 0x10, 0x88, // msr primask, r1
            0xbf, 0xf3, 0x5f, 0x8f, // dmb sy
            0xef, 0xf3, 0x10, 0x86, // mrs r6, primask
            0xef, 0xf3, 0x08, 0x87, // mrs r7, msp
        ]);
        let mut thumb = Thumb::new(Some(0));
        let mut r = [0; 32];
        r[..4].copy_from_slice(&[0x40, 1, 42, 0x40]);
        r[13] = 0x80;
        let mut pc = 0;
        for _ in 0..6 {
            assert!(
                thumb
                    .step(&mut pc, &mut r, &mut bus, &mut std::io::sink())
                    .unwrap()
            );
        }
        assert_eq!(&r[..8], &[0x48, 1, 42, 0x48, 1, 42, 1, 0x80]);
        assert_eq!(pc, 20);
        assert!(thumb.interrupts_masked);
    }

    #[test]
    fn register_offset_load_and_alu_preserve_sign_extension_and_carry() {
        let mut bus = Bus::default();
        bus.map_ram(0, 0x80).unwrap();
        bus.memory(0, 10).unwrap().copy_from_slice(&[
            0x88, 0x56, // ldrsb r0, [r1, r2]
            0x80, 0x46, // mov r8, r0
            0x98, 0x40, // lsls r0, r3
            0x50, 0x41, // adcs r0, r2
            0x04, 0xba, // rev r4, r0
        ]);
        bus.memory(0x41, 1).unwrap()[0] = 0x81;
        let mut thumb = Thumb::new(Some(0));
        let mut registers = [0; 32];
        registers[1] = 0x40;
        registers[2] = 1;
        registers[3] = 32;
        let mut pc = 0;
        for _ in 0..5 {
            assert!(
                thumb
                    .step(&mut pc, &mut registers, &mut bus, &mut std::io::sink())
                    .unwrap()
            );
        }
        assert_eq!(registers[8], 0xffff_ff81);
        assert_eq!(registers[0], 2);
        assert_eq!(registers[4], 0x0200_0000);
        assert!(!thumb.carry);
    }

    #[test]
    fn high_priority_irq_preempts_and_returns_to_interrupted_handler() {
        let mut bus = Bus::default();
        bus.map_ram(0, 0x100).unwrap();
        bus.map_ram(0x2000_0000, 0x100).unwrap();
        bus.map_nvic(0xc0, None).unwrap();
        for (offset, value) in [(16 * 4, 0x81u32), (17 * 4, 0x91), (18 * 4, 0xa1)] {
            bus.memory(offset, 4)
                .unwrap()
                .copy_from_slice(&value.to_le_bytes());
        }
        for address in [0x80, 0x90, 0xa0] {
            bus.memory(address, 2)
                .unwrap()
                .copy_from_slice(&[0x70, 0x47]);
        }
        let mut thumb = Thumb::new(Some(0));
        let mut r = [0; 32];
        r[13] = 0x2000_0100;
        let mut pc = 0x60;
        let mut output = std::io::sink();
        bus.write(0xe000_e400, 4, 0x00c0_0080, &mut output).unwrap();
        bus.write(0xe000_e100, 4, 7, &mut output).unwrap();
        bus.write(0xe000_e200, 4, 1, &mut output).unwrap();
        assert!(thumb.service_interrupts(&mut pc, &mut r, &mut bus).unwrap());
        assert_eq!((pc, r[14]), (0x80, 0xffff_fff9));
        bus.write(0xe000_e200, 4, 4, &mut output).unwrap();
        assert!(!thumb.service_interrupts(&mut pc, &mut r, &mut bus).unwrap());
        bus.write(0xe000_e200, 4, 2, &mut output).unwrap();
        assert!(thumb.service_interrupts(&mut pc, &mut r, &mut bus).unwrap());
        assert_eq!((pc, r[14]), (0x90, 0xffff_fff1));
        assert_eq!(bus.read_ram::<4>(r[13] + 28).unwrap() & 0x1ff, 16);
        thumb.step(&mut pc, &mut r, &mut bus, &mut output).unwrap();
        assert_eq!((pc, thumb.active_exception), (0x80, 16));
        assert!(!thumb.service_interrupts(&mut pc, &mut r, &mut bus).unwrap());
        thumb.step(&mut pc, &mut r, &mut bus, &mut output).unwrap();
        assert_eq!((pc, r[13]), (0xa0, 0x2000_00e0));
        assert_eq!(thumb.active_exception, 18);
    }

    #[test]
    fn systick_defers_a_pending_interrupt_until_interrupts_are_enabled() {
        const CONTROL: u32 = 0xe000_e010;
        let mut bus = Bus::default();
        bus.map_ram(0, 0x100).unwrap();
        bus.map_ram(0x2000_0000, 0x100).unwrap();
        bus.map_systick().unwrap();
        bus.memory(60, 4)
            .unwrap()
            .copy_from_slice(&0x81u32.to_le_bytes());
        bus.memory(0x60, 4)
            .unwrap()
            .copy_from_slice(&[0x72, 0xb6, 0x62, 0xb6]);
        bus.memory(0x80, 2).unwrap().copy_from_slice(&[0x70, 0x47]);
        let mut thumb = Thumb::new(Some(0));
        let mut r = [0; 32];
        r[13] = 0x2000_0100;
        let mut pc = 0x60;
        let mut output = std::io::sink();
        thumb.step(&mut pc, &mut r, &mut bus, &mut output).unwrap(); // CPSID
        bus.write(CONTROL + 4, 4, 1, &mut output).unwrap();
        bus.write(CONTROL + 8, 4, 0, &mut output).unwrap();
        bus.write(CONTROL, 4, 3, &mut output).unwrap();
        assert!(!thumb.service_interrupts(&mut pc, &mut r, &mut bus).unwrap());
        assert!(!thumb.service_interrupts(&mut pc, &mut r, &mut bus).unwrap());
        assert_eq!(bus.read::<4>(CONTROL).unwrap(), 0x1_0007);
        assert_eq!(bus.read::<4>(CONTROL).unwrap(), 7);
        thumb.step(&mut pc, &mut r, &mut bus, &mut output).unwrap(); // CPSIE
        assert!(thumb.service_interrupts(&mut pc, &mut r, &mut bus).unwrap());
        assert_eq!(pc, 0x80);
        assert_eq!(thumb.active_exception, 15);
        thumb.step(&mut pc, &mut r, &mut bus, &mut output).unwrap(); // BX LR
        assert_eq!(pc, 0x64);
        assert_eq!(r[13], 0x2000_0100);
    }

    #[test]
    fn svc_stacks_and_restores_thread_state_with_alignment_padding() {
        let mut bus = Bus::default();
        bus.map_ram(0, 0x100).unwrap();
        bus.map_ram(0x2000_0000, 0x100).unwrap();
        let mut thumb = Thumb::new(Some(0));
        bus.memory(44, 4)
            .unwrap()
            .copy_from_slice(&0x81u32.to_le_bytes());
        bus.memory(0x60, 4)
            .unwrap()
            .copy_from_slice(&[0x00, 0xdf, 0x00, 0xbe]);
        bus.memory(0x80, 4)
            .unwrap()
            .copy_from_slice(&[0x00, 0x20, 0x70, 0x47]);
        let mut registers = [0; 32];
        registers[0] = 42;
        registers[13] = 0x2000_0104;
        registers[14] = 0x11;
        thumb.negative = true;
        let mut pc = 0x60;
        let mut output = std::io::sink();

        assert!(
            thumb
                .step(&mut pc, &mut registers, &mut bus, &mut output)
                .unwrap()
        );
        assert_eq!(pc, 0x80);
        assert_eq!(registers[13], 0x2000_00e0);
        assert_eq!(registers[14], 0xffff_fff9);
        assert_eq!(bus.read_ram::<4>(registers[13] + 24).unwrap(), 0x62);
        assert_eq!(bus.read_ram::<4>(registers[13] + 28).unwrap(), 0x8100_0200);
        assert!(
            thumb
                .step(&mut pc, &mut registers, &mut bus, &mut output)
                .unwrap()
        );
        assert!(thumb.zero);
        assert!(
            thumb
                .step(&mut pc, &mut registers, &mut bus, &mut output)
                .unwrap()
        );
        assert_eq!(pc, 0x62);
        assert_eq!(registers[0], 42);
        assert_eq!(registers[13], 0x2000_0104);
        assert_eq!(registers[14], 0x11);
        assert!(thumb.negative);
        assert!(!thumb.zero);
        assert_eq!(thumb.active_exception, 0);
    }
}

#[cfg(test)]
mod wait_tests {
    use super::*;

    // arm-cortex-m-interrupts.robot, Should Wake Up From WFE When SEVONPEND Is Set.
    #[test]
    fn renode_wfe_wakes_on_new_pending_irq_only_with_sevonpend() {
        const TARGET: u32 = 0x2000_1000;
        for enabled in [false, true] {
            let mut bus = Bus::default();
            bus.map_ram(0x2000_0000, 0x2000).unwrap();
            bus.map_nvic(0xc0, None).unwrap();
            let mut output = std::io::sink();
            bus.write(0xe000_ed10, 4, u32::from(enabled) << 4, &mut output)
                .unwrap();
            let mut pc = 0x2000_0000;
            bus.write(pc, 2, 0xbf20, &mut output).unwrap(); // wfe
            bus.write(pc + 2, 2, 0x6001, &mut output).unwrap(); // str r1, [r0]
            let mut registers = [0; 32];
            registers[0] = TARGET;
            registers[1] = 0xdead_beef;
            let mut thumb = Thumb::default();
            thumb
                .step(&mut pc, &mut registers, &mut bus, &mut output)
                .unwrap();
            assert!(thumb.sleeping());
            assert_eq!(bus.read_ram::<4>(TARGET).unwrap(), 0);

            bus.nvic
                .as_mut()
                .unwrap()
                .set_line(5, true, &ActiveExceptions::default());
            assert!(
                !thumb
                    .service_interrupts(&mut pc, &mut registers, &mut bus)
                    .unwrap()
            );
            assert_eq!(thumb.sleeping(), !enabled);
            if !thumb.sleeping() {
                thumb
                    .step(&mut pc, &mut registers, &mut bus, &mut output)
                    .unwrap();
            }
            assert_eq!(
                bus.read_ram::<4>(TARGET).unwrap(),
                if enabled { 0xdead_beef } else { 0 }
            );
        }
    }

    #[test]
    fn wfi_wakes_on_an_enabled_interrupt_even_while_primask_is_set() {
        let mut bus = Bus::default();
        bus.map_ram(0, 0x200).unwrap();
        bus.map_nvic(0xc0, None).unwrap();
        let mut output = std::io::sink();
        bus.write(0, 2, 0xbf30, &mut output).unwrap();
        let mut thumb = Thumb {
            interrupts_masked: true,
            ..Thumb::default()
        };
        let mut pc = 0;
        let mut registers = [0; 32];
        assert!(
            thumb
                .step(&mut pc, &mut registers, &mut bus, &mut output)
                .unwrap()
        );
        assert_eq!(pc, 2);
        assert!(thumb.sleeping());
        assert!(
            !thumb
                .service_interrupts(&mut pc, &mut registers, &mut bus)
                .unwrap()
        );
        assert!(thumb.sleeping());
        bus.write(0xe000_e200, 4, 1, &mut output).unwrap();
        assert!(
            !thumb
                .service_interrupts(&mut pc, &mut registers, &mut bus)
                .unwrap()
        );
        assert!(thumb.sleeping());
        bus.write(0xe000_e100, 4, 1, &mut output).unwrap();
        assert!(
            !thumb
                .service_interrupts(&mut pc, &mut registers, &mut bus)
                .unwrap()
        );
        assert!(!thumb.sleeping());
        assert_eq!(pc, 2);
    }

    fn fault_fixture() -> (Bus, Thumb, [u32; 32]) {
        let mut bus = Bus::default();
        bus.map_ram(0, 0x1000).unwrap();
        bus.map_nvic(0xc0, None).unwrap();
        bus.set_nvic_name("nvic".into());
        bus.map_systick().unwrap();
        bus.map_ram(0x100000, 0x1000).unwrap();
        bus.faulting_accesses = Some(0x100000..=0x100fff);
        let mut registers = [0; 32];
        registers[13] = 0x1000;
        (bus, Thumb::new(Some(0)), registers)
    }

    #[test]
    fn renode_thread_stacking_fault_pends_or_escalates_to_hardfault() {
        for interrupt in [NMI, 16] {
            let (mut bus, mut thumb, mut registers) = fault_fixture();
            let mut output = std::io::sink();
            let mut pc = 0x200;
            bus.write(pc, 2, 0xe7fe, &mut output).unwrap(); // b .
            bus.write(8, 4, 0x381, &mut output).unwrap();
            bus.write(12, 4, 0x341, &mut output).unwrap();
            bus.write(0xe000_e100, 4, 1, &mut output).unwrap();
            registers[13] = 0x100020;
            bus.faulting_stack_writes = Some(0x100000..=0x100fff);
            thumb
                .step(&mut pc, &mut registers, &mut bus, &mut output)
                .unwrap();
            bus.pend_nvic_exception("nvic", u32::from(interrupt))
                .unwrap();
            assert!(
                thumb
                    .service_interrupts(&mut pc, &mut registers, &mut bus)
                    .unwrap()
            );
            let (handler, active, pending) = if interrupt == NMI {
                (0x380, NMI, HARDFAULT)
            } else {
                (0x340, HARDFAULT, 16)
            };
            assert_eq!(
                (pc, registers[13], registers[14]),
                (handler, 0x100000, 0xfffffff9)
            );
            assert!(thumb.active_exceptions.contains(active));
            assert_eq!(thumb.active_exceptions.depth(), 1);
            let nvic = bus.nvic.as_ref().unwrap();
            if pending == HARDFAULT {
                assert!(nvic.hardfault_pending());
            } else {
                assert_ne!(nvic.pending_exceptions()[0] & 1, 0);
            }
            assert!(!thumb.locked_up());
        }
    }

    #[test]
    fn renode_nmi_stacking_fault_over_hardfault_allocates_frame_and_locks_up() {
        for (fault, vector_fault) in [
            (0x100000..=0x100fff, false),
            (0x100014..=0x100014, false),
            (0x100000..=0x100fff, true),
        ] {
            let partial = fault.start() == fault.end();
            let (mut bus, mut thumb, mut registers) = fault_fixture();
            let mut output = std::io::sink();
            let mut pc = 0x200;
            registers[0] = 0x100000;
            bus.write(pc, 2, 0x6802, &mut output).unwrap(); // ldr r2, [r0]
            bus.write(8, 4, 0x381, &mut output).unwrap();
            bus.write(12, 4, 0x341, &mut output).unwrap();
            assert_eq!(
                thumb.execute_guest(&mut pc, &mut registers, &mut bus, &mut output),
                Ok(ThumbStep::Exception)
            );
            assert_eq!(pc, 0x340);
            registers[13] = 0x100020;
            bus.memory(0x100000, 32).unwrap().fill(0xa5);
            bus.faulting_stack_writes = Some(fault);
            if vector_fault {
                bus.nvic.as_mut().unwrap().set_vector_base(0x100000);
            }
            bus.pend_nvic_exception("nvic", 2).unwrap();
            assert!(
                thumb
                    .service_interrupts(&mut pc, &mut registers, &mut bus)
                    .unwrap()
            );
            assert_eq!(
                (pc, registers[13], registers[14]),
                (LOCKUP_PC, 0x100000, 0xfffffff1)
            );
            assert_eq!(thumb.active_exception, NMI);
            assert!(thumb.active_exceptions.contains(NMI));
            assert!(thumb.active_exceptions.contains(HARDFAULT));
            assert_eq!(thumb.active_exceptions.depth(), 2);
            assert!(thumb.locked_up());

            bus.faulting_accesses = None;
            for offset in (0..32).step_by(4) {
                let expected = match (partial, offset) {
                    (true, 0x18) => 0x340,
                    (true, 0x1c) => 0x01000003,
                    _ => 0xa5a5a5a5,
                };
                assert_eq!(
                    bus.read_ram::<4>(0x100000 + offset).unwrap(),
                    expected,
                    "frame offset {offset:#x}"
                );
            }
        }
    }

    #[test]
    fn renode_unstack_fault_over_hardfault_consumes_frame_and_locks_up() {
        let (mut bus, mut thumb, mut registers) = fault_fixture();
        let mut output = std::io::sink();
        let mut pc = 0x200;
        registers[0] = 0x100000;
        for (address, opcode) in [
            (0x200, 0x6802), // ldr r2, [r0]
            (0x380, 0x4801), // ldr r0, =0x100000
            (0x382, 0xf380), // msr msp, r0
            (0x384, 0x8808),
            (0x386, 0x4770), // bx lr
        ] {
            bus.write(address, 2, opcode, &mut output).unwrap();
        }
        bus.write(0x388, 4, 0x100000, &mut output).unwrap();
        bus.write(8, 4, 0x381, &mut output).unwrap();
        bus.write(12, 4, 0x341, &mut output).unwrap();
        assert_eq!(
            thumb.execute_guest(&mut pc, &mut registers, &mut bus, &mut output),
            Ok(ThumbStep::Exception)
        );
        assert_eq!(pc, 0x340);
        bus.pend_nvic_exception("nvic", 2).unwrap();
        assert!(
            thumb
                .service_interrupts(&mut pc, &mut registers, &mut bus)
                .unwrap()
        );
        assert_eq!(bus.read_ram::<4>(0xfd8).unwrap(), 0x340);
        for _ in 0..2 {
            assert_eq!(
                thumb.execute_guest(&mut pc, &mut registers, &mut bus, &mut output),
                Ok(ThumbStep::Retired)
            );
        }
        assert_eq!(
            thumb.execute_guest(&mut pc, &mut registers, &mut bus, &mut output),
            Ok(ThumbStep::Exception)
        );
        assert_eq!(
            (pc, registers[13], registers[14]),
            (LOCKUP_PC, 0x100020, 0xfffffff1)
        );
        assert_eq!(thumb.active_exception, HARDFAULT);
        assert!(thumb.active_exceptions.contains(HARDFAULT));
        assert!(thumb.locked_up());
    }

    #[test]
    fn renode_unstack_fault_tail_chains_using_existing_frame() {
        let (mut bus, mut thumb, mut registers) = fault_fixture();
        let mut output = std::io::sink();
        let mut pc = 0x200;
        registers[13] = 0x100020;
        registers[0] = 0x11223344;
        bus.write(pc, 2, 0xe7fe, &mut output).unwrap(); // b .
        bus.write(8, 4, 0x381, &mut output).unwrap();
        bus.write(12, 4, 0x341, &mut output).unwrap();
        bus.write(0x380, 2, 0xbf00, &mut output).unwrap(); // nop
        bus.write(0x382, 2, 0x4770, &mut output).unwrap(); // bx lr
        thumb
            .step(&mut pc, &mut registers, &mut bus, &mut output)
            .unwrap();
        bus.pend_nvic_exception("nvic", 2).unwrap();
        assert!(
            thumb
                .service_interrupts(&mut pc, &mut registers, &mut bus)
                .unwrap()
        );
        assert_eq!(
            thumb.execute_guest(&mut pc, &mut registers, &mut bus, &mut output),
            Ok(ThumbStep::Retired)
        );
        assert_eq!(pc, 0x382);
        assert_eq!(
            thumb.execute_guest(&mut pc, &mut registers, &mut bus, &mut output),
            Ok(ThumbStep::Exception)
        );
        assert_eq!(
            (pc, registers[13], registers[14]),
            (0x340, 0x100000, 0xfffffff9)
        );
        assert!(thumb.active_exceptions.contains(HARDFAULT));
        assert!(!thumb.locked_up());

        bus.faulting_accesses = None;
        let expected = [0x11223344, 0, 0, 0, 0, 0, 0x200, 0x01000000];
        for (index, value) in expected.into_iter().enumerate() {
            assert_eq!(
                bus.read_ram::<4>(0x100000 + index as u32 * 4).unwrap(),
                value
            );
        }
        bus.write(0x340, 2, 0x4770, &mut output).unwrap(); // bx lr
        assert_eq!(
            thumb.execute_guest(&mut pc, &mut registers, &mut bus, &mut output),
            Ok(ThumbStep::Retired)
        );
        assert_eq!(
            (pc, registers[13], thumb.active_exceptions.is_empty()),
            (0x200, 0x100020, true)
        );
    }

    #[test]
    fn renode_precise_fault_on_peripheral_load_store_or_configured_unmapped_read() {
        for (address, opcode, mapped) in [
            (0x100000, 0x6802, true), // ldr r2, [r0]
            (0x100000, 0x6002, true), // str r2, [r0]
            (0x200000, 0x6802, false),
        ] {
            let (mut bus, mut thumb, mut registers) = fault_fixture();
            let mut output = std::io::sink();
            bus.faulting_accesses = None;
            if mapped {
                bus.faulting_device = Some(0x100000..=0x100fff);
            } else {
                bus.unhandled_access = Some(crate::bus::UnhandledAccess::ThrowException);
            }
            bus.write(12, 4, 0x341, &mut output).unwrap();
            bus.write(0x200, 2, opcode, &mut output).unwrap();
            bus.write(0x202, 2, 0x2122, &mut output).unwrap(); // movs r1, #0x22
            bus.write(0x204, 2, 0xe7fe, &mut output).unwrap(); // b .
            registers[..3].copy_from_slice(&[address, 0x11, 0xa5a5a5a5]);
            let mut pc = 0x200;
            assert_eq!(
                thumb.execute_guest(&mut pc, &mut registers, &mut bus, &mut output),
                Ok(ThumbStep::Exception)
            );
            assert_eq!(
                (pc, registers[13], thumb.active_exception),
                (0x340, 0xfe0, HARDFAULT)
            );
            assert_eq!(&registers[1..3], &[0x11, 0xa5a5a5a5]);
            for (addr, expected) in [(0xfe4, 0x11), (0xfe8, 0xa5a5a5a5), (0xff8, 0x200)] {
                assert_eq!(bus.read_ram::<4>(addr).unwrap(), expected);
            }
            if mapped {
                bus.faulting_device = None;
                assert_eq!(bus.read_ram::<4>(address).unwrap(), 0);
            }
        }
    }

    #[test]
    fn renode_invalid_reset_state_enters_lockup_after_two_steps() {
        let (mut bus, mut thumb, mut registers) = fault_fixture();
        let (sp, mut pc, lr) = thumb.reset_m0(&bus);
        registers[13] = sp;
        registers[14] = lr;
        let mut output = std::io::sink();
        assert_eq!(
            thumb.execute_guest(&mut pc, &mut registers, &mut bus, &mut output),
            Ok(ThumbStep::Exception)
        );
        assert_eq!(
            (pc, thumb.active_exception, thumb.locked_up()),
            (0, HARDFAULT, false)
        );
        assert_eq!(
            thumb.execute_guest(&mut pc, &mut registers, &mut bus, &mut output),
            Ok(ThumbStep::Exception)
        );
        assert_eq!((pc, thumb.locked_up()), (LOCKUP_PC, true));
    }

    #[test]
    fn renode_precise_load_hardfault_then_lockup_nmi_and_reset() {
        let (mut bus, mut thumb, mut registers) = fault_fixture();
        let mut output = std::io::sink();
        for (address, opcode) in [(0x200, 0x6802), (0x202, 0x2122), (0x340, 0x6802)] {
            bus.write(address, 2, opcode, &mut output).unwrap();
        }
        bus.write(4, 4, 0x201, &mut output).unwrap();
        bus.write(8, 4, 0x381, &mut output).unwrap();
        bus.write(12, 4, 0x341, &mut output).unwrap();
        bus.write(0, 4, 0x1000, &mut output).unwrap();
        registers[0] = 0x100000;
        registers[1] = 0x11;
        registers[2] = 0xa5a5a5a5;
        let mut pc = 0x200;

        assert_eq!(
            thumb.execute_guest(&mut pc, &mut registers, &mut bus, &mut output),
            Ok(ThumbStep::Exception)
        );
        assert_eq!((pc, thumb.active_exception), (0x340, HARDFAULT));
        assert_eq!((registers[1], registers[2]), (0x11, 0xa5a5a5a5));
        assert_eq!(registers[13], 0xfe0);
        assert_eq!(bus.read::<4>(0xff8).unwrap(), 0x200);

        assert_eq!(
            thumb.execute_guest(&mut pc, &mut registers, &mut bus, &mut output),
            Ok(ThumbStep::Exception)
        );
        assert!(thumb.locked_up());
        for _ in 0..10 {
            assert_eq!(
                thumb.execute_guest(&mut pc, &mut registers, &mut bus, &mut output),
                Ok(ThumbStep::Exception)
            );
        }
        assert_eq!(pc, LOCKUP_PC);
        bus.pend_nvic_exception("nvic", 2).unwrap();
        assert!(
            thumb
                .service_interrupts(&mut pc, &mut registers, &mut bus)
                .unwrap()
        );
        assert_eq!(
            (pc, thumb.active_exception, thumb.locked_up()),
            (0x380, NMI, false)
        );
        assert_eq!(bus.read::<4>(0xfd8).unwrap(), LOCKUP_PC);
        assert_eq!(thumb.reset_m0(&bus), (0x1000, 0x200, 0));
        assert!(!thumb.locked_up());
    }

    #[test]
    fn renode_nmi_and_hardfault_faulting_vectors_lock_up() {
        for exception in [NMI, HARDFAULT] {
            let (mut bus, mut thumb, mut registers) = fault_fixture();
            let mut output = std::io::sink();
            let mut pc = 0x200;
            bus.write(pc, 2, 0xe7fe, &mut output).unwrap();
            assert_eq!(
                thumb.execute_guest(&mut pc, &mut registers, &mut bus, &mut output),
                Ok(ThumbStep::Retired)
            );
            bus.nvic.as_mut().unwrap().set_vector_base(0x100000);
            bus.pend_nvic_exception("nvic", u32::from(exception))
                .unwrap();
            assert!(
                thumb
                    .service_interrupts(&mut pc, &mut registers, &mut bus)
                    .unwrap()
            );
            assert_eq!(
                (pc, thumb.active_exception, thumb.locked_up()),
                (LOCKUP_PC, exception, true)
            );
            assert_eq!((registers[13], registers[14]), (0xfe0, 0xfffffff9));
        }
    }

    #[test]
    fn renode_invalid_nmi_return_into_hardfault_locks_up() {
        let (mut bus, mut thumb, mut registers) = fault_fixture();
        let mut output = std::io::sink();
        bus.write(8, 4, 0x381, &mut output).unwrap();
        bus.write(12, 4, 0x341, &mut output).unwrap();
        bus.write(0x200, 2, 0x6802, &mut output).unwrap();
        for (address, opcode) in [(0x380, 0x2000), (0x382, 0x9007), (0x384, 0x4770)] {
            bus.write(address, 2, opcode, &mut output).unwrap();
        }
        registers[0] = 0x100000;
        let mut pc = 0x200;
        assert_eq!(
            thumb.execute_guest(&mut pc, &mut registers, &mut bus, &mut output),
            Ok(ThumbStep::Exception)
        );
        assert_eq!(pc, 0x340);
        bus.pend_nvic_exception("nvic", 2).unwrap();
        assert!(
            thumb
                .service_interrupts(&mut pc, &mut registers, &mut bus)
                .unwrap()
        );
        for expected in [ThumbStep::Retired, ThumbStep::Retired, ThumbStep::Exception] {
            assert_eq!(
                thumb.execute_guest(&mut pc, &mut registers, &mut bus, &mut output),
                Ok(expected)
            );
        }
        assert_eq!(
            (pc, registers[13], thumb.active_exception),
            (LOCKUP_PC, 0xfe0, HARDFAULT)
        );
        assert!(thumb.locked_up());
    }

    #[test]
    fn hardfault_can_return_to_the_repaired_stacked_pc() {
        let (mut bus, mut thumb, mut registers) = fault_fixture();
        let mut output = std::io::sink();
        for (address, opcode) in [
            (0x200, 0x6802), // ldr r2, [r0]
            (0x202, 0x2122), // movs r1, #0x22
            (0x340, 0x9106), // str r1, [sp, #24]
            (0x342, 0x4770), // bx lr
        ] {
            bus.write(address, 2, opcode, &mut output).unwrap();
        }
        bus.write(12, 4, 0x341, &mut output).unwrap();
        registers[0] = 0x100000;
        registers[1] = 0x202;
        let mut pc = 0x200;
        assert_eq!(
            thumb.execute_guest(&mut pc, &mut registers, &mut bus, &mut output),
            Ok(ThumbStep::Exception)
        );
        for _ in 0..3 {
            assert_eq!(
                thumb.execute_guest(&mut pc, &mut registers, &mut bus, &mut output),
                Ok(ThumbStep::Retired)
            );
        }
        assert_eq!(
            (pc, registers[1], registers[13], thumb.active_exception),
            (0x204, 0x22, 0x1000, 0)
        );
    }

    #[test]
    fn renode_fault_on_initial_msp_or_reset_pc_locks_up() {
        for fault_offset in [0, 4] {
            let (mut bus, _, _) = fault_fixture();
            let mut thumb = Thumb::new(Some(0x100000));
            let address = 0x100000 + fault_offset;
            bus.faulting_accesses = Some(address..=address);
            assert_eq!(thumb.reset_m0(&bus), (0, LOCKUP_PC, u32::MAX));
            assert!(thumb.locked_up());
            assert_eq!(thumb.active_exception, 0);
            assert!(thumb.active_exceptions.contains(HARDFAULT));
        }
    }

    // Instructions from the ELF in tlib/arm/thumb2-data-processing-instr.robot.
}

#[cfg(test)]
mod ge_upstream;

#[cfg(test)]
mod h7_upstream {
    use super::*;

    #[test]
    fn upstream_wfe_wakes_from_disabled_irq_with_sevonpend() {
        const START: u32 = 0x2000_0000;
        const TARGET: u32 = 0x2000_1000;
        let mut bus = Bus::default();
        bus.map_ram(START, 0x40000).unwrap();
        bus.map_nvic(0xf0, None).unwrap();
        let mut output = std::io::sink();
        bus.write(0xe000ed10, 4, 0x10, &mut output).unwrap();
        bus.memory(START, 6)
            .unwrap()
            .copy_from_slice(&[0x20, 0xbf, 0x01, 0x60, 0xfe, 0xe7]);
        let mut thumb = Thumb::new(None);
        thumb.thumb2 = true;
        let mut r = [0; 32];
        r[0] = TARGET;
        r[1] = 0xdeadbeef;
        r[13] = 0x2004_0000;
        let mut pc = START;
        let mut step = |thumb: &mut Thumb, bus: &mut Bus| {
            if !thumb.service_interrupts(&mut pc, &mut r, bus).unwrap() && !thumb.sleeping() {
                thumb.step(&mut pc, &mut r, bus, &mut output).unwrap();
            }
        };
        for _ in 0..crate::machine::VIRTUAL_STEPS_PER_SECOND / 1000 {
            step(&mut thumb, &mut bus);
        }
        assert_eq!(bus.read::<4>(TARGET).unwrap(), 0);
        bus.nvic
            .as_mut()
            .unwrap()
            .set_line(5, true, &ActiveExceptions::default());
        for _ in 0..crate::machine::VIRTUAL_STEPS_PER_SECOND {
            step(&mut thumb, &mut bus);
            if bus.read::<4>(TARGET).unwrap() == 0xdeadbeef {
                break;
            }
        }
        assert_eq!(bus.read::<4>(TARGET).unwrap(), 0xdeadbeef);
        assert_eq!(thumb.active_exception, 0);
    }

    #[test]
    fn upstream_basepri_priority_boost_clears_on_reset() {
        let mut thumb = Thumb::new(None);
        thumb.thumb2 = true;
        let mut registers = [0; 32];
        assert!(thumb.system_write(17, 0x50, &mut registers));
        let basepri = thumb.system_read(17, &registers).unwrap();
        let before_reset = thumb.priority_boost(false);
        assert_eq!(i16::try_from(basepri).unwrap(), before_reset);
        thumb.reset_state();
        assert_ne!(thumb.priority_boost(false), before_reset);
    }
}
