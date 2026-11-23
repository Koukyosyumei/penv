use std::io::Write;

use crate::{
    bus::{Bus, BusError},
    nvic::{HARDFAULT, Nvic},
    systick::{NMI, PENDSV, SYSTICK, SysTick},
};

#[derive(Clone, Copy, Default, PartialEq)]
enum Sleep {
    #[default]
    Running,
    Interrupt,
    Event,
}

#[derive(Clone, Default)]
pub struct Thumb {
    vectors: Option<u32>,
    negative: bool,
    zero: bool,
    carry: bool,
    overflow: bool,
    active_exception: u8,
    active_exceptions: u64,
    interrupts_masked: bool,
    sleep: Sleep,
    event: bool,
    invalid_thumb_state: bool,
    locked_up: bool,
    pending: u64,
    thread_uses_psp: bool,
    msp: u32,
    psp: u32,
}

const LOCKUP_PC: u32 = 0xefff_fffe;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ThumbStep {
    Retired,
    Breakpoint,
    Exception,
}

enum InstructionError {
    GuestFault,
    ExceptionTaken(u32),
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
            BusError::Unmapped(..) | BusError::Misaligned(..) | BusError::AddressError(_) => {
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

    pub fn vector_base(&self) -> u32 {
        self.vectors.unwrap_or(0)
    }

    pub fn infer_vector_base(&mut self, base: u32) {
        self.vectors.get_or_insert(base);
    }

    pub fn reset(&mut self, bus: &Bus) -> (u32, u32, u32) {
        *self = Self::new(Some(self.vector_base()));
        let vectors = self.vector_base();
        let target = vectors
            .checked_add(4)
            .and_then(|address| bus.read_ram::<4>(address).ok());
        if let (Some(target), Ok(sp)) = (target, bus.read_ram::<4>(vectors)) {
            self.invalid_thumb_state = target & 1 == 0;
            return (sp & !3, target & !1, 0);
        }
        self.locked_up = true;
        self.active_exceptions = 1 << HARDFAULT;
        (0, LOCKUP_PC, u32::MAX)
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
        let positive = match condition >> 1 {
            0 => self.zero,
            1 => self.carry,
            2 => self.negative,
            3 => self.overflow,
            4 => self.carry && !self.zero,
            5 => self.negative == self.overflow,
            6 => !self.zero && self.negative == self.overflow,
            _ => true,
        };
        positive ^ (condition & 1 != 0)
    }

    fn apsr(&self) -> u32 {
        (u32::from(self.negative) << 31)
            | (u32::from(self.zero) << 30)
            | (u32::from(self.carry) << 29)
            | (u32::from(self.overflow) << 28)
    }

    pub fn xpsr(&self) -> u32 {
        self.apsr()
            | (u32::from(!self.invalid_thumb_state) << 24)
            | u32::from(self.active_exception)
    }

    fn set_apsr(&mut self, value: u32) {
        self.negative = value >> 31 != 0;
        self.zero = value & (1 << 30) != 0;
        self.carry = value & (1 << 29) != 0;
        self.overflow = value & (1 << 28) != 0;
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

    fn system_read(&self, register: u32, r: &[u32; 32]) -> Option<u32> {
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
            20 => u32::from(self.uses_psp()) << 1,
            _ => return None,
        })
    }

    fn system_write(&mut self, register: u32, value: u32, r: &mut [u32; 32]) -> bool {
        match register {
            0..=3 => self.set_apsr(value),
            5..=7 => {}
            8 if self.uses_psp() => self.msp = value & !3,
            8 => r[13] = value & !3,
            9 if self.uses_psp() => r[13] = value & !3,
            9 => self.psp = value & !3,
            16 => self.interrupts_masked = value & 1 != 0,
            20 => self.select_thread_stack(value & 2 != 0, r),
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
        !self.interrupts_masked && self.active_exceptions & ((1 << NMI) | (1 << HARDFAULT)) == 0
    }

    fn hardfault(&mut self, pc: u32, r: &mut [u32; 32], bus: &mut Bus) -> u32 {
        if self.can_take_hardfault() {
            self.exception_entry(HARDFAULT, pc, r, bus)
        } else {
            self.lockup()
        }
    }

    fn exception_entry(
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
        self.active_exceptions |= 1 << exception;
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

    fn active_handler(bus: &Bus, active: u64) -> u8 {
        (NMI..64)
            .filter(|number| active & (1 << number) != 0)
            .min_by_key(|&number| Self::exception_priority(bus, number))
            .unwrap_or(0)
    }

    fn pending_configurable(bus: &Bus, active: u64, current: u8) -> Option<u8> {
        let external = bus
            .nvic
            .as_ref()
            .and_then(|nvic| nvic.next_irq(active))
            .map(|(line, _)| 16 + line);
        external
            .into_iter()
            .chain([PENDSV, SYSTICK].into_iter().filter(|&exception| {
                active & (1 << exception) == 0
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
        self.pending &= !(1 << exception);
    }

    fn tail_chain(
        &mut self,
        target: u32,
        remaining: u64,
        r: &mut [u32; 32],
        bus: &mut Bus,
    ) -> Option<u32> {
        if (target == 0xffff_fff1) != (remaining != 0) {
            return None;
        }
        let destination = Self::active_handler(bus, remaining);
        let nmi_pending = bus.systick.as_ref().is_some_and(|timer| timer.pending(NMI));
        let hardfault_pending = bus
            .nvic
            .as_ref()
            .is_some_and(|nvic| nvic.pending_exceptions() & (1 << HARDFAULT) != 0);
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

    fn branch(
        &mut self,
        target: u32,
        r: &mut [u32; 32],
        bus: &mut Bus,
    ) -> Result<u32, InstructionError> {
        if self.active_exception != 0 && matches!(target, 0xffff_fff1 | 0xffff_fff9 | 0xffff_fffd) {
            let use_psp = target == 0xffff_fffd;
            let stack = if use_psp { self.psp } else { r[13] };
            let remaining = self.active_exceptions & !(1 << self.active_exception);
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
                previous == 0 && remaining == 0
            } else {
                previous != 0 && previous < 64 && remaining & (1 << previous) != 0
            };
            if xpsr & (1 << 24) == 0 || !valid_mode || pc & 1 != 0 {
                if self.active_exception == NMI && remaining & (1 << HARDFAULT) != 0 {
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
        self.active_exceptions &= !(1 << self.active_exception);
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

    pub fn service_interrupts(
        &mut self,
        pc: &mut u32,
        r: &mut [u32; 32],
        bus: &mut Bus,
    ) -> Result<bool, String> {
        bus.sample_nvic(self.active_exceptions);
        if let Some(timer) = bus.systick.as_mut() {
            timer.tick();
        }
        let pending = bus.nvic.as_ref().map_or(0, Nvic::pending_exceptions)
            | bus.systick.as_ref().map_or(0, SysTick::pending_exceptions);
        if pending & !self.pending != 0
            && bus.nvic.as_ref().is_some_and(Nvic::send_event_on_pending)
        {
            self.signal_event();
        }
        self.pending = pending;
        if pending & (1 << NMI) != 0 && self.active_exceptions & (1 << NMI) == 0 {
            *pc = self.exception_entry(NMI, *pc, r, bus);
            self.acknowledge(NMI, bus);
            self.sleep = Sleep::Running;
            return Ok(true);
        }
        if self.locked_up || self.active_exceptions & (1 << NMI) != 0 {
            return Ok(false);
        }
        if pending & (1 << HARDFAULT) != 0 && self.can_take_hardfault() {
            *pc = self.exception_entry(HARDFAULT, *pc, r, bus);
            self.acknowledge(HARDFAULT, bus);
            return Ok(true);
        }
        if self.interrupts_masked && self.sleep != Sleep::Interrupt {
            return Ok(false);
        }
        if let Some(exception) =
            Self::pending_configurable(bus, self.active_exceptions, self.active_exception)
        {
            self.sleep = Sleep::Running;
            if self.interrupts_masked {
                return Ok(false);
            }
            *pc = self.exception_entry(exception, *pc, r, bus);
            if self.active_exception == exception {
                self.acknowledge(exception, bus);
            }
            return Ok(true);
        }
        Ok(false)
    }

    pub fn locked_up(&self) -> bool {
        self.locked_up
    }

    pub(crate) fn step(
        &mut self,
        pc: &mut u32,
        r: &mut [u32; 32],
        bus: &mut Bus,
        output: &mut impl Write,
    ) -> Result<ThumbStep, String> {
        if self.locked_up {
            return Ok(ThumbStep::Exception);
        }
        let result = if self.invalid_thumb_state {
            Err(InstructionError::GuestFault)
        } else {
            self.instruction(pc, r, bus, output)
        };
        match result {
            Ok(true) => Ok(ThumbStep::Retired),
            Ok(false) => Ok(ThumbStep::Breakpoint),
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

    fn instruction(
        &mut self,
        pc: &mut u32,
        r: &mut [u32; 32],
        bus: &mut Bus,
        output: &mut impl Write,
    ) -> Result<bool, InstructionError> {
        let inst = bus.read_ram::<2>(*pc)?;
        let unsupported = || format!("unsupported Thumb instruction {inst:#06x}");
        let mut next = pc.wrapping_add(2);
        let architectural_pc = pc.wrapping_add(4);
        let rd = (inst & 7) as usize;
        let rn = ((inst >> 3) & 7) as usize;
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
            0x4700..=0x47ff if inst & 7 == 0 => {
                let rm = ((inst >> 3) & 15) as usize;
                let target = if rm == 15 { architectural_pc } else { r[rm] };
                if inst & 0x80 != 0 {
                    self.invalid_thumb_state = target & 1 == 0;
                    r[14] = next | 1;
                    next = target & !1;
                } else {
                    next = self.branch(target, r, bus)?;
                }
            }
            0x4800..=0x4fff => {
                let address = (architectural_pc & !3).wrapping_add((inst & 0xff) << 2);
                r[((inst >> 8) & 7) as usize] = bus.read::<4>(address)?;
            }
            0x5000..=0x5fff => {
                let offset = ((inst >> 6) & 7) as usize;
                let address = r[rn].wrapping_add(r[offset]);
                let operation = (inst >> 9) & 7;
                if operation < 3 {
                    let width = [4, 2, 1][operation as usize];
                    bus.write(address, width, r[rd], output)?;
                } else {
                    let value = match operation {
                        3 | 6 => bus.read::<1>(address),
                        5 | 7 => bus.read::<2>(address),
                        _ => bus.read::<4>(address),
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
                    bus.write(address, width as usize, r[rd], output)?;
                } else {
                    r[rd] = match width {
                        1 => bus.read::<1>(address),
                        2 => bus.read::<2>(address),
                        _ => bus.read::<4>(address),
                    }?;
                }
            }
            0x9000..=0x9fff => {
                let register = ((inst >> 8) & 7) as usize;
                let address = r[13].wrapping_add((inst & 0xff) << 2);
                if inst & 0x800 == 0 {
                    bus.write(address, 4, r[register], output)?;
                } else {
                    r[register] = bus.read::<4>(address)?;
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
            0xb400..=0xb5ff | 0xbc00..=0xbdff if inst & 0x1ff != 0 => {
                let pop = inst & 0x800 != 0;
                let mut mask = inst & 0xff;
                if inst & 0x100 != 0 {
                    mask |= 1 << if pop { 15 } else { 14 };
                }
                let bytes = mask.count_ones() * 4;
                let sp = r[13];
                if sp & 3 != 0 {
                    return Err(InstructionError::GuestFault);
                }
                if pop {
                    let mut restored = *r;
                    let mut target = None;
                    let mut address = sp;
                    while mask != 0 {
                        let register = mask.trailing_zeros() as usize;
                        mask &= mask - 1;
                        let value = bus.read_ram::<4>(address)?;
                        if register == 15 {
                            target = Some(value);
                        } else {
                            restored[register] = value;
                        }
                        address = address.wrapping_add(4);
                    }
                    restored[13] = sp.wrapping_add(bytes);
                    if let Some(target) = target {
                        let branch = self.branch(target, &mut restored, bus);
                        if matches!(branch, Err(InstructionError::ExceptionTaken(_))) {
                            *r = restored;
                        }
                        next = branch?;
                    }
                    *r = restored;
                } else {
                    let new_sp = sp.wrapping_sub(bytes);
                    let stack = bus.memory(new_sp, bytes as usize)?;
                    for word in stack.as_chunks_mut::<4>().0 {
                        let register = mask.trailing_zeros() as usize;
                        mask &= mask - 1;
                        *word = r[register].to_le_bytes();
                    }
                    r[13] = new_sp;
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
            0xbe00..=0xbeff => return Ok(false),
            0xbf00 | 0xbf10 => {} // NOP and scheduler yield.
            0xbf20 => {
                if !std::mem::take(&mut self.event) {
                    self.sleep = Sleep::Event;
                }
            }
            0xbf30 => self.sleep = Sleep::Interrupt,
            0xbf40 => self.signal_event(),
            0xc000..=0xcfff if inst & 0xff != 0 => {
                let registers = inst & 0xff;
                let base_register = ((inst >> 8) & 7) as usize;
                let address = r[base_register];
                let load = inst & 0x800 != 0;
                let count = registers.count_ones();
                if address & 3 != 0 {
                    return Err(InstructionError::GuestFault);
                }
                let selected = (0..8).filter(|register| registers & (1 << register) != 0);
                if load {
                    let mut values = *r;
                    for (index, register) in selected.enumerate() {
                        values[register] =
                            bus.read_ram::<4>(address.wrapping_add(index as u32 * 4))?;
                    }
                    *r = values;
                } else {
                    if registers & (1 << base_register) != 0
                        && registers.trailing_zeros() as usize != base_register
                    {
                        return Err(unsupported().into());
                    }
                    let memory = bus.memory(address, count as usize * 4)?;
                    for (word, register) in memory.as_chunks_mut::<4>().0.iter_mut().zip(selected) {
                        *word = r[register].to_le_bytes();
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
            0xdf00..=0xdfff if self.active_exception == 0 && !self.interrupts_masked => {
                next = self.exception_entry(11, next, r, bus);
            }
            0xde00..=0xdfff => return Err(InstructionError::GuestFault),
            0xe000..=0xe7ff => {
                let offset = (((inst << 21) as i32) >> 20) as u32;
                next = architectural_pc.wrapping_add(offset);
            }
            0xf000..=0xf7ff => {
                let low = bus.read_ram::<2>(next)?;
                if low & 0xd000 == 0xd000 {
                    let sign = (inst >> 10) & 1;
                    let i1 = 1 ^ (((low >> 13) & 1) ^ sign);
                    let i2 = 1 ^ (((low >> 11) & 1) ^ sign);
                    let offset = (sign << 24)
                        | (i1 << 23)
                        | (i2 << 22)
                        | ((inst & 0x3ff) << 12)
                        | ((low & 0x7ff) << 1);
                    let offset = (((offset << 7) as i32) >> 7) as u32;
                    r[14] = architectural_pc | 1;
                    next = architectural_pc.wrapping_add(offset);
                } else {
                    let sysm = low & 0xff;
                    match (inst, low) {
                        (0xf3bf, 0x8f4f | 0x8f5f | 0x8f6f) => {
                            // Bus operations execute synchronously and instruction fetch reads RAM.
                        }
                        (0xf3ef, 0x8000..=0x8fff) => {
                            let register = ((low >> 8) & 15) as usize;
                            if matches!(register, 13 | 15) {
                                return Err(unsupported().into());
                            }
                            r[register] = self.system_read(sysm, r).ok_or_else(unsupported)?;
                        }
                        (0xf380..=0xf38f, 0x8800..=0x88ff) => {
                            let register = (inst & 15) as usize;
                            if matches!(register, 13 | 15)
                                || !self.system_write(sysm, r[register], r)
                            {
                                return Err(unsupported().into());
                            }
                        }
                        _ => return Err(unsupported().into()),
                    }
                    next = architectural_pc;
                }
            }
            _ => return Err(unsupported().into()),
        }
        *pc = next;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            assert_eq!(thumb.active_exceptions, 1 << active);
            assert_eq!(
                bus.nvic.as_ref().unwrap().pending_exceptions(),
                1 << pending
            );
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
                thumb.step(&mut pc, &mut registers, &mut bus, &mut output),
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
            assert_eq!(thumb.active_exceptions, (1 << NMI) | (1 << HARDFAULT));
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
            thumb.step(&mut pc, &mut registers, &mut bus, &mut output),
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
                thumb.step(&mut pc, &mut registers, &mut bus, &mut output),
                Ok(ThumbStep::Retired)
            );
        }
        assert_eq!(
            thumb.step(&mut pc, &mut registers, &mut bus, &mut output),
            Ok(ThumbStep::Exception)
        );
        assert_eq!(
            (pc, registers[13], registers[14]),
            (LOCKUP_PC, 0x100020, 0xfffffff1)
        );
        assert_eq!(thumb.active_exception, HARDFAULT);
        assert_eq!(thumb.active_exceptions, 1 << HARDFAULT);
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
            thumb.step(&mut pc, &mut registers, &mut bus, &mut output),
            Ok(ThumbStep::Retired)
        );
        assert_eq!(pc, 0x382);
        assert_eq!(
            thumb.step(&mut pc, &mut registers, &mut bus, &mut output),
            Ok(ThumbStep::Exception)
        );
        assert_eq!(
            (pc, registers[13], registers[14]),
            (0x340, 0x100000, 0xfffffff9)
        );
        assert_eq!(thumb.active_exceptions, 1 << HARDFAULT);
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
            thumb.step(&mut pc, &mut registers, &mut bus, &mut output),
            Ok(ThumbStep::Retired)
        );
        assert_eq!(
            (pc, registers[13], thumb.active_exceptions),
            (0x200, 0x100020, 0)
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
                thumb.step(&mut pc, &mut registers, &mut bus, &mut output),
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
        let (sp, mut pc, lr) = thumb.reset(&bus);
        registers[13] = sp;
        registers[14] = lr;
        let mut output = std::io::sink();
        assert_eq!(
            thumb.step(&mut pc, &mut registers, &mut bus, &mut output),
            Ok(ThumbStep::Exception)
        );
        assert_eq!(
            (pc, thumb.active_exception, thumb.locked_up()),
            (0, HARDFAULT, false)
        );
        assert_eq!(
            thumb.step(&mut pc, &mut registers, &mut bus, &mut output),
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
            thumb.step(&mut pc, &mut registers, &mut bus, &mut output),
            Ok(ThumbStep::Exception)
        );
        assert_eq!((pc, thumb.active_exception), (0x340, HARDFAULT));
        assert_eq!((registers[1], registers[2]), (0x11, 0xa5a5a5a5));
        assert_eq!(registers[13], 0xfe0);
        assert_eq!(bus.read::<4>(0xff8).unwrap(), 0x200);

        assert_eq!(
            thumb.step(&mut pc, &mut registers, &mut bus, &mut output),
            Ok(ThumbStep::Exception)
        );
        assert!(thumb.locked_up());
        for _ in 0..10 {
            assert_eq!(
                thumb.step(&mut pc, &mut registers, &mut bus, &mut output),
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
        assert_eq!(thumb.reset(&bus), (0x1000, 0x200, 0));
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
                thumb.step(&mut pc, &mut registers, &mut bus, &mut output),
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
            thumb.step(&mut pc, &mut registers, &mut bus, &mut output),
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
                thumb.step(&mut pc, &mut registers, &mut bus, &mut output),
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
            thumb.step(&mut pc, &mut registers, &mut bus, &mut output),
            Ok(ThumbStep::Exception)
        );
        for _ in 0..3 {
            assert_eq!(
                thumb.step(&mut pc, &mut registers, &mut bus, &mut output),
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
            assert_eq!(thumb.reset(&bus), (0, LOCKUP_PC, u32::MAX));
            assert!(thumb.locked_up());
            assert_eq!(thumb.active_exception, 0);
            assert_eq!(thumb.active_exceptions, 1 << HARDFAULT);
        }
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
                    == ThumbStep::Retired
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
                == ThumbStep::Retired
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
    fn renode_nmi_bx_lr_restores_interrupted_thread() {
        let mut bus = Bus::default();
        bus.map_ram(0, 0x1000).unwrap();
        bus.map_nvic(0xc0, None).unwrap();
        let mut output = std::io::sink();
        bus.map_systick().unwrap();
        bus.set_nvic_name("nvic".into());
        bus.write(2 * 4, 4, 0x101, &mut output).unwrap();
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
        bus.pend_nvic_exception("nvic", 2).unwrap();
        assert_eq!(bus.read::<4>(0xe000_ed04).unwrap(), 1 << 31);
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
        assert_eq!(bus.read::<4>(0xe000_ed04).unwrap(), 0);
        thumb
            .step(&mut pc, &mut registers, &mut bus, &mut output)
            .unwrap();
        assert_eq!(
            (pc, registers[13], thumb.active_exception),
            (0x20, 0x800, 0)
        );
    }

    #[test]
    fn renode_thread_bx_and_nmi_handler_blx_do_not_unstack() {
        let (mut bus, mut thumb, mut registers) = fault_fixture();
        let mut output = std::io::sink();
        for (address, opcode) in [
            (0x20, 0x4800),
            (0x22, 0x4700),
            (0x100, 0xbf00),
            (0x102, 0x47f0),
        ] {
            bus.write(address, 2, opcode, &mut output).unwrap();
        }
        bus.write(0x24, 4, 0xffff_ffb8, &mut output).unwrap();
        bus.write(8, 4, 0x101, &mut output).unwrap();
        registers[13] = 0x800;
        let mut pc = 0x20;
        for _ in 0..2 {
            assert_eq!(
                thumb.step(&mut pc, &mut registers, &mut bus, &mut output),
                Ok(ThumbStep::Retired)
            );
        }
        assert_eq!(
            (pc, registers[13], thumb.active_exception),
            (0xffff_ffb8, 0x800, 0)
        );

        let mut thumb = Thumb::new(Some(0));
        bus.write(0x20, 2, 0xe7fe, &mut output).unwrap();
        let mut pc = 0x20;
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
            (pc, registers[13], registers[14], thumb.active_exception),
            (0xffff_fff8, 0x7e0, 0x105, NMI)
        );
    }

    #[test]
    fn renode_exception_vector_load_does_not_unstack_an_exc_return_value() {
        // Armv6-M uses an unmapped stack in place of the Cortex-M33 MPU.
        let (mut bus, mut thumb, mut registers) = fault_fixture();
        let mut output = std::io::sink();
        bus.write(8, 4, 0x101, &mut output).unwrap();
        bus.write(12, 4, 0xffff_ffb0, &mut output).unwrap();
        for (address, opcode) in [
            (0x100, 0x4801), // ldr r0, =0x2000
            (0x102, 0x4685), // mov sp, r0
            (0x104, 0x4770), // bx lr
        ] {
            bus.write(address, 2, opcode, &mut output).unwrap();
        }
        bus.write(0x108, 4, 0x2000, &mut output).unwrap();
        let mut pc = 0x20;
        registers[13] = 0x800;
        bus.pend_nvic_exception("nvic", 2).unwrap();
        assert!(
            thumb
                .service_interrupts(&mut pc, &mut registers, &mut bus)
                .unwrap()
        );
        for expected in [ThumbStep::Retired, ThumbStep::Retired, ThumbStep::Exception] {
            assert_eq!(
                thumb.step(&mut pc, &mut registers, &mut bus, &mut output),
                Ok(expected)
            );
        }
        assert_eq!(
            (pc, registers[13], thumb.active_exception),
            (0xffff_ffb0, 0x2000, HARDFAULT)
        );
        assert_eq!(bus.read_ram::<4>(0x7f8).unwrap(), 0x20);
    }

    #[test]
    fn exception_return_tail_chains_without_reading_the_saved_frame() {
        let (mut bus, mut thumb, mut registers) = fault_fixture();
        let mut output = std::io::sink();
        for (address, value) in [
            (8, 0x101),
            (16 * 4, 0x201),
            (17 * 4, 0x301),
            (0xe000_e100, 3),
        ] {
            bus.write(address, 4, value, &mut output).unwrap();
        }
        for address in [0x100, 0x200, 0x300] {
            bus.write(address, 2, 0x4770, &mut output).unwrap(); // bx lr
        }
        let mut pc = 0x20;
        registers[13] = 0x800;
        bus.pend_nvic_exception("nvic", 2).unwrap();
        assert!(
            thumb
                .service_interrupts(&mut pc, &mut registers, &mut bus)
                .unwrap()
        );
        bus.pend_nvic_exception("nvic", 17).unwrap();
        bus.pend_nvic_exception("nvic", 16).unwrap();
        bus.faulting_accesses = Some(0x7e0..=0x7ff);
        for (handler, irq) in [(0x200, 16), (0x300, 17)] {
            assert!(
                !thumb
                    .service_interrupts(&mut pc, &mut registers, &mut bus)
                    .unwrap()
            );
            assert_eq!(
                thumb.step(&mut pc, &mut registers, &mut bus, &mut output),
                Ok(ThumbStep::Retired)
            );
            assert_eq!(
                (pc, registers[13], registers[14], thumb.active_exceptions),
                (handler, 0x7e0, 0xffff_fff9, 1 << irq)
            );
        }
        bus.faulting_accesses = None;
        assert_eq!(
            thumb.step(&mut pc, &mut registers, &mut bus, &mut output),
            Ok(ThumbStep::Retired)
        );
        assert_eq!(
            (pc, registers[13], thumb.active_exceptions),
            (0x20, 0x800, 0)
        );
    }

    #[test]
    fn renode_irq_vector_fault_preserves_pending_and_reuses_frame() {
        let (mut bus, mut thumb, mut registers) = fault_fixture();
        let mut output = std::io::sink();
        bus.faulting_accesses = Some(0x100040..=0x100040);
        bus.write(0x10000c, 4, 0x341, &mut output).unwrap();
        bus.write(0x200, 2, 0xe7fe, &mut output).unwrap();
        let mut pc = 0x200;
        thumb
            .step(&mut pc, &mut registers, &mut bus, &mut output)
            .unwrap();
        bus.nvic.as_mut().unwrap().set_vector_base(0x100000);
        bus.write(0xe000_e100, 4, 1, &mut output).unwrap();
        bus.pend_nvic_exception("nvic", 16).unwrap();
        assert!(
            thumb
                .service_interrupts(&mut pc, &mut registers, &mut bus)
                .unwrap()
        );
        assert_eq!(
            (pc, registers[13], thumb.active_exceptions),
            (0x340, 0xfe0, 1 << HARDFAULT)
        );
        assert_eq!(bus.read::<4>(0xe000_e200).unwrap() & 1, 1);
        assert_eq!(bus.read_ram::<4>(0xff8).unwrap(), 0x200);
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
        // When Not Using TrustZone. Load the same immediate via an Armv6-M literal.
        let mut bus = Bus::default();
        bus.map_ram(0, 0x1000).unwrap();
        let mut output = std::io::sink();
        bus.write(0x20, 2, 0x4800, &mut output).unwrap(); // ldr r0, =0xfeffffff
        bus.write(0x22, 2, 0x4700, &mut output).unwrap(); // bx r0
        bus.write(0x24, 4, 0xfeff_ffff, &mut output).unwrap();
        let mut thumb = Thumb::new(Some(0));
        let mut pc = 0x20;
        let mut registers = [0; 32];
        registers[13] = 0x800;

        for _ in 0..2 {
            assert_eq!(
                thumb.step(&mut pc, &mut registers, &mut bus, &mut output),
                Ok(ThumbStep::Retired)
            );
        }
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
        assert_eq!((pc, r[13], r[14]), (0x80, 0x2000_0200, 0xffff_fffd));
        assert_eq!(thumb.psp, 0x2000_00e0);
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
            assert!(
                thumb.step(&mut pc, &mut r, &mut bus, &mut output).unwrap() == ThumbStep::Retired
            );
        }
        assert_eq!((pc, r[13], r[14]), (0x80, 0x2000_0200, 0xffff_fffd));
        assert_eq!(thumb.system_read(9, &r), Some(0x2000_0060));
        assert_eq!(bus.read_ram::<4>(0x2000_0078).unwrap(), 0x6a);
        assert_eq!(bus.read_ram::<4>(0x2000_01e0).unwrap(), 0);
        assert!(thumb.step(&mut pc, &mut r, &mut bus, &mut output).unwrap() == ThumbStep::Retired);
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
                    == ThumbStep::Retired
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
                    == ThumbStep::Retired
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
        assert_eq!((pc, r[13], r[14]), (0xa0, 0x2000_00e0, 0xffff_fff9));
        assert_eq!(bus.read_ram::<4>(r[13] + 24).unwrap(), 0x60);
        thumb.step(&mut pc, &mut r, &mut bus, &mut output).unwrap();
        assert_eq!((pc, r[13], thumb.active_exception), (0x60, 0x2000_0100, 0));
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
                == ThumbStep::Retired
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
                == ThumbStep::Retired
        );
        assert!(thumb.zero);
        assert!(
            thumb
                .step(&mut pc, &mut registers, &mut bus, &mut output)
                .unwrap()
                == ThumbStep::Retired
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

            bus.nvic.as_mut().unwrap().set_line(5, true, 0);
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
                == ThumbStep::Retired
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
}
