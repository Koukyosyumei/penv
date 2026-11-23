mod fpu;
mod thumb2;

use std::io::Write;

use crate::{
    bus::{Bus, BusError},
    nvic::Nvic,
    systick::{NMI, PENDSV, SYSTICK},
};

const HARD_FAULT: u8 = 3;
const BUS_FAULT: u8 = 5;
pub(crate) const LOCKUP_PC: u32 = 0xefff_fffe;

#[derive(Clone, Copy, Default, PartialEq)]
enum Sleep {
    #[default]
    Running,
    Interrupt,
    Event,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ThumbStep {
    Retired,
    Fault,
    Semihosting(crate::semihosting::Exit),
    Breakpoint,
}

#[derive(Clone, Default)]
pub struct Thumb {
    vectors: Option<u32>,
    pub(crate) thumb2: bool,
    basepri: u8,
    faultmask: bool,
    itstate: u8,
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
    locked_up: bool,
    unstack_fault_taken: bool,
    active_exceptions: u128,
    interrupts_masked: bool,
    sleep: Sleep,
    event: bool,
    pending: u128,
    thread_uses_psp: bool,
    thread_unprivileged: bool,
    msp: u32,
    psp: u32,
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

    /// None indicates a BusFault while loading the reset context.
    pub fn reset(&self, bus: &Bus) -> Result<Option<(u32, u32)>, String> {
        let base = self.vector_base();
        let vector = base.checked_add(4).ok_or("reset vector address overflow")?;
        let words = bus
            .read_cortex_m_ram_or_mock_word(vector)
            .and_then(|target| {
                bus.read_cortex_m_ram_or_mock_word(base)
                    .map(|sp| (sp, target))
            });
        let (sp, target) = match words {
            Ok(words) => words,
            Err(BusError::SlaveFault(_)) if self.thumb2 => return Ok(None),
            Err(error) => return Err(error.to_string()),
        };
        if target & 1 == 0 {
            return Err(format!(
                "Cortex-M reset vector {target:#010x} must select Thumb state"
            ));
        }
        bus.read_ram::<2>(target & !1)
            .map_err(|error| error.to_string())?;
        Ok(Some((sp & !3, target & !1)))
    }

    pub fn enter_reset_lockup(&mut self) {
        self.locked_up = true;
        self.active_exceptions = 1 << HARD_FAULT;
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

    fn condition(&self, condition: u32) -> bool {
        let positive = match condition >> 1 {
            0 => self.zero,
            1 => self.carry,
            2 => self.negative,
            3 => self.overflow,
            4 => self.carry && !self.zero,
            5 => self.negative == self.overflow,
            6 => !self.zero && self.negative == self.overflow,
            7 => true,
            _ => unreachable!(),
        };
        positive ^ (condition & 1 != 0)
    }

    fn apsr(&self) -> u32 {
        (u32::from(self.negative) << 31)
            | (u32::from(self.zero) << 30)
            | (u32::from(self.carry) << 29)
            | ((u32::from(self.overflow) << 28) | (u32::from(self.saturation) << 27))
            | (self.greater_equal << 16)
    }

    fn set_apsr(&mut self, value: u32) {
        self.negative = value >> 31 != 0;
        self.zero = value & (1 << 30) != 0;
        self.carry = value & (1 << 29) != 0;
        self.overflow = value & (1 << 28) != 0;
        self.saturation = self.thumb2 && value & (1 << 27) != 0;
        self.greater_equal = if self.thumb2 { (value >> 16) & 15 } else { 0 };
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
            17 if self.thumb2 => u32::from(self.basepri),
            19 if self.thumb2 => self.faultmask.into(),
            20 => {
                (u32::from(self.fp_active) << 2)
                    | (u32::from(self.uses_psp()) << 1)
                    | u32::from(self.thread_unprivileged)
            }
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
            17 if self.thumb2 => self.basepri = value as u8 & 0xe0,
            18 if self.thumb2 => {
                let value = value as u8 & 0xe0;
                if value != 0 && (self.basepri == 0 || value < self.basepri) {
                    self.basepri = value;
                }
            }
            19 if self.thumb2 => self.faultmask = value & 1 != 0,
            20 => {
                if self.thumb2 {
                    if self.active_exception == 0 && self.thread_unprivileged {
                        return true;
                    }
                    self.thread_unprivileged = value & 1 != 0;
                    self.fp_active = value & 4 != 0;
                }
                self.select_thread_stack(value & 2 != 0, r);
            }
            _ => return false,
        }
        true
    }

    fn no_coprocessor(&mut self, pc: u32, r: &mut [u32; 32], bus: &mut Bus) -> Result<u32, String> {
        const USAGE_FAULT: u8 = 6;
        self.synchronous_fault(USAGE_FAULT, None, pc, r, bus)
    }

    fn synchronous_fault(
        &mut self,
        exception: u8,
        bus_address: Option<u32>,
        pc: u32,
        r: &mut [u32; 32],
        bus: &mut Bus,
    ) -> Result<u32, String> {
        let nvic = bus.nvic.as_mut().ok_or("Cortex-M fault requires an NVIC")?;
        let priority = nvic.priority(exception);
        let enabled = if bus_address.is_some() {
            nvic.bus_fault_enabled()
        } else {
            nvic.usage_fault_enabled()
        };
        let can_preempt = self.active_exception == 0
            || (self.active_exception > HARD_FAULT
                && priority < nvic.priority(self.active_exception));
        let escalate = !enabled
            || !can_preempt
            || self.interrupts_masked
            || self.faultmask
            || (self.basepri != 0 && priority >= self.basepri);
        let cannot_escalate =
            escalate && (matches!(self.active_exception, NMI | HARD_FAULT) || self.faultmask);
        if let Some(address) = bus_address {
            nvic.record_precise_bus_fault(address, escalate && !cannot_escalate);
            if cannot_escalate {
                self.locked_up = true;
                self.sleep = Sleep::Running;
                nvic.lockup = true;
                return Ok(LOCKUP_PC);
            }
        } else {
            nvic.record_no_coprocessor_fault(escalate);
            if cannot_escalate {
                return Err(format!("Cortex-M lockup on NOCP at {pc:#010x}"));
            }
        }
        self.exception_entry(if escalate { HARD_FAULT } else { exception }, pc, r, bus)
    }

    fn read_exception_handler(&self, bus: &Bus, exception: u8) -> Result<Option<u32>, String> {
        let vector_base = bus
            .nvic
            .as_ref()
            .and_then(|nvic| nvic.vector_base())
            .unwrap_or_else(|| self.vector_base());
        let vector = vector_base
            .checked_add(u32::from(exception) * 4)
            .ok_or("exception vector address overflow")?;
        match bus.read_cortex_m_ram_or_mock_word(vector) {
            Ok(handler) if handler & 1 != 0 => Ok(Some(handler & !1)),
            Ok(handler) => Err(format!(
                "exception {exception} vector {handler:#010x} does not select Thumb state"
            )),
            Err(BusError::SlaveFault(_)) if self.thumb2 => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }

    fn exception_entry(
        &mut self,
        exception: u8,
        return_pc: u32,
        r: &mut [u32; 32],
        bus: &mut Bus,
    ) -> Result<u32, String> {
        let mut handler = self.read_exception_handler(bus, exception)?;
        let vector_fault = handler.is_none();
        let mut exception = exception;
        if handler.is_none() {
            if let Some(nvic) = bus.nvic.as_mut() {
                nvic.record_exception_vector_fault();
            }
            if !matches!(exception, NMI | HARD_FAULT)
                && !matches!(self.active_exception, NMI | HARD_FAULT)
                && !self.faultmask
            {
                exception = HARD_FAULT;
                handler = self.read_exception_handler(bus, exception)?;
            }
        }
        let sp = r[13];
        let frame_size = if self.fp_active { 104 } else { 32 };
        let stack = (sp & !7).wrapping_sub(frame_size as u32);
        bus.check_thumb_access(stack, frame_size, true, false)?;
        let it = u32::from(self.itstate);
        let xpsr = self.apsr()
            | (1 << 24)
            | ((sp & 4) << 7)
            | ((it & 3) << 25)
            | ((it & 0xfc) << 8)
            | u32::from(self.active_exception);
        let mut frame = [0; 26];
        frame[..8].copy_from_slice(&[r[0], r[1], r[2], r[3], r[12], r[14], return_pc, xpsr]);
        if self.fp_active {
            frame[8..24].copy_from_slice(&self.fp[..16]);
            frame[24] = self.fpscr;
        }
        match bus.write_cortex_m_exception_stack(stack, &frame[..frame_size / 4]) {
            Ok(()) => {}
            Err(BusError::SlaveFault(_)) if self.thumb2 => {
                let nvic = bus
                    .nvic
                    .as_mut()
                    .ok_or("stacking BusFault requires an NVIC")?;
                let priority = nvic.priority(BUS_FAULT);
                let configurable = nvic.bus_fault_enabled()
                    && !self.interrupts_masked
                    && !self.faultmask
                    && (self.basepri == 0 || priority < self.basepri)
                    && (self.active_exception == 0
                        || (self.active_exception > HARD_FAULT
                            && priority < nvic.priority(self.active_exception)));
                let derived = if configurable { BUS_FAULT } else { HARD_FAULT };
                let cannot_escalate = !configurable
                    && (matches!(self.active_exception, NMI | HARD_FAULT) || self.faultmask);
                // If NMI preempts an active HardFault, its terminal entry still
                // records a failed vector read before asserting lockup.
                if vector_fault && !(cannot_escalate && exception == NMI) {
                    return Err(
                        "simultaneous Cortex-M stacking and vector BusFaults are unsupported"
                            .into(),
                    );
                }
                nvic.record_stacking_bus_fault(!configurable && !cannot_escalate);

                if cannot_escalate {
                    handler = None;
                } else if derived == HARD_FAULT && exception == NMI {
                    nvic.set_hard_fault_pending(true);
                } else if !matches!(exception, NMI | HARD_FAULT)
                    && (derived == HARD_FAULT
                        || (priority, derived) < (nvic.priority(exception), exception))
                {
                    exception = derived;
                    handler = self.read_exception_handler(bus, exception)?;
                    if handler.is_none()
                        && let Some(nvic) = bus.nvic.as_mut()
                    {
                        nvic.record_exception_vector_fault();
                    }
                } else if derived != exception {
                    return Err(
                        "deferring a configurable Cortex-M stacking BusFault is unsupported".into(),
                    );
                }
            }
            Err(error) => return Err(error.to_string()),
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
        self.fp_active = false;
        bus.clear_reservation(0);
        self.active_exceptions |= 1 << exception;
        self.active_exception = exception;
        self.itstate = 0;
        if let Some(nvic) = bus.nvic.as_mut() {
            nvic.set_active_exceptions(self.active_exceptions);
        }
        if handler.is_none() || exception == NMI {
            self.locked_up = handler.is_none();
            self.sleep = Sleep::Running;
            if let Some(nvic) = bus.nvic.as_mut() {
                nvic.lockup = self.locked_up;
            }
        }
        Ok(handler.unwrap_or(LOCKUP_PC))
    }

    fn unstack_bus_fault(
        &mut self,
        target: u32,
        stack: u32,
        frame_size: usize,
        remaining: u128,
        r: &mut [u32; 32],
        bus: &mut Bus,
    ) -> Result<u32, String> {
        let nvic = bus
            .nvic
            .as_mut()
            .ok_or("unstacking BusFault requires an NVIC")?;
        if self.active_exception != NMI {
            self.faultmask = false;
        }
        let previous = (1..80)
            .filter(|&exception| remaining & (1 << exception) != 0)
            .min_by_key(|&exception| match exception {
                NMI => (0, exception),
                HARD_FAULT => (1, exception),
                _ => (u16::from(nvic.priority(exception)) + 2, exception),
            })
            .unwrap_or(0);
        let priority = nvic.priority(BUS_FAULT);
        let blocked = matches!(previous, NMI | HARD_FAULT) || self.faultmask;
        let configurable = nvic.bus_fault_enabled()
            && !self.interrupts_masked
            && !blocked
            && (self.basepri == 0 || priority < self.basepri)
            && (previous == 0 || priority < nvic.priority(previous));
        nvic.record_unstacking_bus_fault(!configurable && !blocked);
        self.active_exceptions = remaining;
        self.itstate = 0;
        self.sleep = Sleep::Running;
        self.unstack_fault_taken = true;
        if blocked {
            self.active_exception = previous;
            self.locked_up = true;
            nvic.lockup = true;
            nvic.set_active_exceptions(remaining);
            let consumed = stack.wrapping_add(frame_size as u32);
            if target & 4 != 0 {
                self.psp = consumed;
            } else {
                r[13] = consumed;
            }
            return Ok(LOCKUP_PC);
        }

        let exception = if configurable { BUS_FAULT } else { HARD_FAULT };
        self.active_exception = exception;
        self.active_exceptions |= 1 << exception;
        self.fp_active = false;
        r[14] = target;
        nvic.set_active_exceptions(self.active_exceptions);
        self.read_exception_handler(bus, exception)?
            .ok_or_else(|| "vector BusFault during exception return is unsupported".into())
    }

    fn branch(&mut self, target: u32, r: &mut [u32; 32], bus: &mut Bus) -> Result<u32, String> {
        let basic_target = if self.thumb2 { target | 0x10 } else { target };
        if self.active_exception != 0
            && matches!(basic_target, 0xffff_fff1 | 0xffff_fff9 | 0xffff_fffd)
        {
            let extended = target & 0x10 == 0;
            let use_psp = basic_target == 0xffff_fffd;
            let frame_size = if extended { 104 } else { 32 };
            let stack = if use_psp { self.psp } else { r[13] };
            bus.check_thumb_access(stack, frame_size, false, false)?;
            let remaining = self.active_exceptions & !(1 << self.active_exception);
            let mut frame = [0; 25];
            let words = if extended { 25 } else { 8 };
            for (index, word) in frame[..words].iter_mut().enumerate() {
                match bus.read_cortex_m_ram_or_mock_word(stack.wrapping_add(index as u32 * 4)) {
                    Ok(value) => *word = value,
                    Err(BusError::SlaveFault(_)) if self.thumb2 => {
                        return self
                            .unstack_bus_fault(target, stack, frame_size, remaining, r, bus);
                    }
                    Err(error) => return Err(error.to_string()),
                }
            }
            let [r0, r1, r2, r3, r12, lr, pc, xpsr] = frame[..8].try_into().unwrap();
            let previous = xpsr & 0x1ff;
            let valid_mode = if basic_target != 0xffff_fff1 {
                previous == 0 && remaining == 0
            } else {
                previous != 0 && previous < 80 && remaining & (1 << previous) != 0
            };
            if xpsr & (1 << 24) == 0 || !valid_mode {
                return Err(format!(
                    "invalid Cortex-M exception return frame: pc={pc:#x} xpsr={xpsr:#x} target={target:#x}"
                ));
            }
            if extended {
                self.fp[..16].copy_from_slice(&frame[8..24]);
                self.fpscr = frame[24];
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
            if self.active_exception != NMI {
                self.faultmask = false;
            }
            self.set_apsr(xpsr);
            self.itstate = (((xpsr >> 25) & 3) | ((xpsr >> 8) & 0xfc)) as u8;
            self.active_exception = previous as u8;
            self.active_exceptions = remaining;
            if let Some(nvic) = bus.nvic.as_mut() {
                nvic.set_active_exceptions(remaining);
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

    #[cfg(test)]
    pub(crate) fn fp_context_active(&self) -> bool {
        self.fp_active
    }

    #[cfg(test)]
    pub(crate) fn lockup_state(&self) -> (bool, u8, u8) {
        (self.locked_up, self.itstate, self.active_exception)
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

    pub fn clock_interrupts(&mut self, bus: &mut Bus) {
        bus.sample_nvic(self.active_exceptions, self.sleeping());
        if let Some(timer) = bus.systick.as_mut() {
            timer.tick();
        }
    }

    #[cfg(test)]
    pub fn service_interrupts(
        &mut self,
        pc: &mut u32,
        r: &mut [u32; 32],
        bus: &mut Bus,
    ) -> Result<bool, String> {
        self.clock_interrupts(bus);
        self.dispatch_interrupts(pc, r, bus)
    }

    pub fn dispatch_interrupts(
        &mut self,
        pc: &mut u32,
        r: &mut [u32; 32],
        bus: &mut Bus,
    ) -> Result<bool, String> {
        let pending = bus.nvic.as_ref().map_or(0, Nvic::pending_exceptions)
            | bus
                .systick
                .as_ref()
                .map_or(0, |timer| u128::from(timer.pending_exceptions()));
        if pending & !self.pending != 0
            && bus.nvic.as_ref().is_some_and(Nvic::send_event_on_pending)
        {
            self.signal_event();
        }
        self.pending = pending;
        if pending & (1 << NMI) != 0 && self.active_exception != NMI {
            *pc = self.exception_entry(NMI, *pc, r, bus)?;
            self.sleep = Sleep::Running;
            bus.systick.as_mut().unwrap().acknowledge(NMI);
            self.pending &= !(1 << NMI);
            return Ok(true);
        }
        if pending & (1 << HARD_FAULT) != 0
            && !self.locked_up
            && !self.faultmask
            && self.active_exceptions & ((1 << HARD_FAULT) | (1 << NMI)) == 0
        {
            *pc = self.exception_entry(HARD_FAULT, *pc, r, bus)?;
            self.sleep = Sleep::Running;
            bus.nvic.as_mut().unwrap().set_hard_fault_pending(false);
            self.pending &= !(1 << HARD_FAULT);
            return Ok(true);
        }
        if self.locked_up
            || self.faultmask
            || (self.interrupts_masked && self.sleep != Sleep::Interrupt)
        {
            return Ok(false);
        }
        let priority = |exception| bus.nvic.as_ref().map_or(0, |nvic| nvic.priority(exception));
        let current_priority = if self.active_exception == 0 {
            256
        } else {
            u16::from(priority(self.active_exception))
        };
        let mut selected = bus
            .nvic
            .as_ref()
            .and_then(|nvic| nvic.next_irq(self.active_exceptions))
            .map(|(line, priority)| (16 + line, priority));
        for exception in [PENDSV, SYSTICK] {
            if bus
                .systick
                .as_ref()
                .is_some_and(|timer| timer.pending(exception))
                && self.active_exceptions & (1 << exception) == 0
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
            u16::from(*priority) < current_priority
                && (self.basepri == 0 || *priority < self.basepri)
        }) {
            self.sleep = Sleep::Running;
            if self.interrupts_masked {
                return Ok(false);
            }
            *pc = self.exception_entry(exception, *pc, r, bus)?;
            if self.active_exception == exception {
                if matches!(exception, PENDSV | SYSTICK) {
                    bus.systick.as_mut().unwrap().acknowledge(exception);
                } else {
                    bus.nvic.as_mut().unwrap().acknowledge(exception - 16);
                }
                self.pending &= !(1 << exception);
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

    pub fn step(
        &mut self,
        pc: &mut u32,
        r: &mut [u32; 32],
        bus: &mut Bus,
        output: &mut impl Write,
    ) -> Result<ThumbStep, String> {
        if self.locked_up {
            return Ok(ThumbStep::Fault);
        }
        let original_itstate = self.itstate;
        bus.thumb_negative_priority = matches!(self.active_exception, NMI | HARD_FAULT);
        let result = self.step_instruction(pc, r, bus, output);
        bus.thumb_negative_priority = false;
        if let Some(address) = bus.take_thumb_data_fault() {
            self.itstate = original_itstate;
            *pc = self.synchronous_fault(BUS_FAULT, Some(address), *pc, r, bus)?;
            return Ok(ThumbStep::Fault);
        }
        if std::mem::take(&mut self.unstack_fault_taken) && result.is_ok() {
            return Ok(ThumbStep::Fault);
        }
        result
    }

    fn step_instruction(
        &mut self,
        pc: &mut u32,
        r: &mut [u32; 32],
        bus: &mut Bus,
        output: &mut impl Write,
    ) -> Result<ThumbStep, String> {
        let inst = bus.thumb_fetch(*pc)?;
        let unsupported = || format!("unsupported Thumb instruction {inst:#06x}");
        let mut next = pc.wrapping_add(2);
        let architectural_pc = pc.wrapping_add(4);
        let rd = (inst & 7) as usize;
        let rn = ((inst >> 3) & 7) as usize;
        let original_itstate = self.itstate;
        let in_it = original_itstate != 0;
        if in_it {
            let condition = self.condition(u32::from(self.itstate >> 4));
            self.itstate = if self.itstate & 7 == 0 {
                0
            } else {
                (self.itstate & 0xe0) | ((self.itstate << 1) & 0x1f)
            };
            if !condition {
                *pc = pc.wrapping_add(if inst >= 0xe800 { 4 } else { 2 });
                return Ok(ThumbStep::Retired);
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
            0x4700..=0x47ff if inst & 7 == 0 => {
                let rm = ((inst >> 3) & 15) as usize;
                let target = if rm == 15 { architectural_pc } else { r[rm] };
                if inst & 0x80 != 0 {
                    if target & 1 == 0 {
                        return Err(format!(
                            "branch target {target:#010x} does not select Thumb state"
                        ));
                    }
                    r[14] = next | 1;
                    next = target & !1;
                } else {
                    next = self.branch(target, r, bus)?;
                }
            }
            0x4800..=0x4fff => {
                let address = (architectural_pc & !3).wrapping_add((inst & 0xff) << 2);
                r[((inst >> 8) & 7) as usize] = bus.thumb_read::<4>(address)?;
            }
            0x5000..=0x5fff => {
                let offset = ((inst >> 6) & 7) as usize;
                let address = r[rn].wrapping_add(r[offset]);
                let operation = (inst >> 9) & 7;
                if operation < 3 {
                    let width = [4, 2, 1][operation as usize];
                    bus.thumb_write(address, width, r[rd], output)?;
                } else {
                    let value = match operation {
                        3 | 6 => bus.thumb_read::<1>(address),
                        5 | 7 => bus.thumb_read::<2>(address),
                        _ => bus.thumb_read::<4>(address),
                    }
                    .map_err(|error| error.to_string())?;
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
                    bus.thumb_write(address, width as usize, r[rd], output)?;
                } else {
                    r[rd] = match width {
                        1 => bus.thumb_read::<1>(address),
                        2 => bus.thumb_read::<2>(address),
                        _ => bus.thumb_read::<4>(address),
                    }
                    .map_err(|error| error.to_string())?;
                }
            }
            0x9000..=0x9fff => {
                let register = ((inst >> 8) & 7) as usize;
                let address = r[13].wrapping_add((inst & 0xff) << 2);
                if inst & 0x800 == 0 {
                    bus.thumb_write(address, 4, r[register], output)?;
                } else {
                    r[register] = bus.thumb_read::<4>(address)?;
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
                    _ => return Err(unsupported()),
                };
            }
            0xbeab => {
                if let Some(mut handler) = bus.semihosting.take() {
                    let result = handler.call(bus, r[0], r[1], output);
                    bus.semihosting = Some(handler);
                    let (value, exit) = result?;
                    r[0] = value;
                    if let Some(exit) = exit {
                        *pc = next;
                        return Ok(ThumbStep::Semihosting(exit));
                    }
                } else {
                    r[0] = u32::MAX;
                }
            }
            0xbe00..=0xbeff => return Ok(ThumbStep::Breakpoint),
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
                    return Err(format!(
                        "unaligned multiple-register transfer at {address:#010x}"
                    ));
                }
                bus.check_thumb_access(address, count as usize * 4, !load, false)?;
                let selected = (0..8).filter(|register| registers & (1 << register) != 0);
                if load {
                    let mut values = *r;
                    for (index, register) in selected.enumerate() {
                        values[register] = bus
                            .read_ram::<4>(address.wrapping_add(index as u32 * 4))
                            .map_err(|error| error.to_string())?;
                    }
                    *r = values;
                } else {
                    if registers & (1 << base_register) != 0
                        && registers.trailing_zeros() as usize != base_register
                    {
                        return Err(unsupported());
                    }
                    let memory = bus
                        .memory(address, count as usize * 4)
                        .map_err(|error| error.to_string())?;
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
            0xdf00..=0xdfff
                if self.active_exception == 0 && !self.interrupts_masked && !self.faultmask =>
            {
                next = self.exception_entry(11, next, r, bus)?;
            }
            0xdf00..=0xdfff => return Err("Cortex-M SVC escalation is unsupported".into()),
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
                                return Err(unsupported());
                            }
                            r[register] = self.system_read(sysm, r).ok_or_else(unsupported)?;
                        }
                        (0xf380..=0xf38f, 0x8800..=0x88ff) => {
                            let register = (inst & 15) as usize;
                            if matches!(register, 13 | 15)
                                || !self.system_write(sysm, r[register], r)
                            {
                                return Err(unsupported());
                            }
                        }
                        _ => {
                            return Err(format!(
                                "unsupported Thumb instruction {inst:#06x} {low:#06x}"
                            ));
                        }
                    }
                    next = architectural_pc;
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
                    let privileged = self.active_exception != 0 || !self.thread_unprivileged;
                    if Self::is_fpu_instruction(inst, low)
                        && !bus
                            .nvic
                            .as_ref()
                            .is_some_and(|nvic| nvic.fpu_access(privileged))
                    {
                        self.itstate = original_itstate;
                        *pc = self.no_coprocessor(*pc, r, bus)?;
                        return Ok(ThumbStep::Fault);
                    }
                    if !self.execute_fpu(inst, low, architectural_pc, r, bus, output)? {
                        self.execute_thumb2(inst, low, &mut next, r, bus, output)?;
                    }
                }
            }
            _ => return Err(unsupported()),
        }
        if in_it
            && (inst < 0x2800 || matches!(inst, 0x3000..=0x43ff))
            && !(inst & 0xfc00 == 0x4000 && matches!((inst >> 6) & 15, 8 | 10 | 11))
        {
            self.set_apsr(saved_flags);
        }
        *pc = next;
        Ok(ThumbStep::Retired)
    }
}

#[cfg(test)]
mod exception_return_upstream;
#[cfg(test)]
mod fpu_upstream;
#[cfg(test)]
mod ge_upstream;
#[cfg(test)]
mod it_upstream;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vtor_relocates_exception_vectors_without_resetting_the_cpu() {
        use crate::nvic::VTOR;

        let mut bus = Bus::default();
        bus.map_ram(0, 0x400).unwrap();
        bus.map_nvic(0xc0, None, false).unwrap();
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
        bus.map_nvic(0xc0, None, false).unwrap();
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
        assert!(thumb.service_interrupts(&mut pc, &mut r, &mut bus).unwrap());
        assert_eq!(pc, 0x80);
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
            assert_eq!(
                thumb.step(&mut pc, &mut r, &mut bus, &mut output).unwrap(),
                ThumbStep::Retired
            );
        }
        assert_eq!((pc, r[13], r[14]), (0x80, 0x2000_0200, 0xffff_fffd));
        assert_eq!(thumb.system_read(9, &r), Some(0x2000_0060));
        assert_eq!(bus.read_ram::<4>(0x2000_0078).unwrap(), 0x6a);
        assert_eq!(bus.read_ram::<4>(0x2000_01e0).unwrap(), 0);
        assert_eq!(
            thumb.step(&mut pc, &mut r, &mut bus, &mut output).unwrap(),
            ThumbStep::Retired
        );
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
            assert_eq!(
                thumb
                    .step(&mut pc, &mut r, &mut bus, &mut std::io::sink())
                    .unwrap(),
                ThumbStep::Retired
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
            assert_eq!(
                thumb
                    .step(&mut pc, &mut registers, &mut bus, &mut std::io::sink())
                    .unwrap(),
                ThumbStep::Retired
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
        bus.map_nvic(0xc0, None, false).unwrap();
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
        assert_eq!((pc, r[13]), (0x60, 0x2000_0100));
        assert!(thumb.service_interrupts(&mut pc, &mut r, &mut bus).unwrap());
        assert_eq!(pc, 0xa0);
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

        assert_eq!(
            thumb
                .step(&mut pc, &mut registers, &mut bus, &mut output)
                .unwrap(),
            ThumbStep::Retired
        );
        assert_eq!(pc, 0x80);
        assert_eq!(registers[13], 0x2000_00e0);
        assert_eq!(registers[14], 0xffff_fff9);
        assert_eq!(bus.read_ram::<4>(registers[13] + 24).unwrap(), 0x62);
        assert_eq!(bus.read_ram::<4>(registers[13] + 28).unwrap(), 0x8100_0200);
        assert_eq!(
            thumb
                .step(&mut pc, &mut registers, &mut bus, &mut output)
                .unwrap(),
            ThumbStep::Retired
        );
        assert!(thumb.zero);
        assert_eq!(
            thumb
                .step(&mut pc, &mut registers, &mut bus, &mut output)
                .unwrap(),
            ThumbStep::Retired
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

    #[test]
    fn wfi_wakes_on_an_enabled_interrupt_even_while_primask_is_set() {
        let mut bus = Bus::default();
        bus.map_ram(0, 0x200).unwrap();
        bus.map_nvic(0xc0, None, false).unwrap();
        let mut output = std::io::sink();
        bus.write(0, 2, 0xbf30, &mut output).unwrap();
        let mut thumb = Thumb {
            interrupts_masked: true,
            ..Thumb::default()
        };
        let mut pc = 0;
        let mut registers = [0; 32];
        assert_eq!(
            thumb
                .step(&mut pc, &mut registers, &mut bus, &mut output)
                .unwrap(),
            ThumbStep::Retired
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

#[cfg(test)]
mod bus_fault_tests {
    use super::*;

    fn check_precise_fault(instructions: &[u32], enabled: bool, expected_handler: u32, fp: bool) {
        // arm-cortex-m-bus-fault.robot: Prepare Faulting Instruction /
        // Execute Faulting Instruction / Fault Should Be Precise.
        const CODE: u32 = 0x200;
        const ADDRESS: u32 = 0x10_0000;
        const SHCSR: u32 = 0xe000_ed24;
        const CFSR: u32 = 0xe000_ed28;
        const HFSR: u32 = 0xe000_ed2c;
        const BFAR: u32 = 0xe000_ed38;
        const FORCED: u32 = 1 << 30;
        let mut bus = Bus::default();
        bus.map_ram(0, 0x1000).unwrap();
        bus.map_bus_faulting_peripheral(ADDRESS, fp.then_some(0))
            .unwrap();
        bus.map_nvic(0xe0, None, true).unwrap();
        let mut output = std::io::sink();
        bus.write(0x14, 4, 0x301, &mut output).unwrap();
        bus.write(0xc, 4, 0x341, &mut output).unwrap();
        for (index, instruction) in instructions.iter().enumerate() {
            bus.write(CODE + index as u32 * 2, 2, *instruction, &mut output)
                .unwrap();
        }
        for address in [0x300, 0x340] {
            bus.write(address, 2, 0xe7fe, &mut output).unwrap();
        }
        if fp {
            bus.write(0xe000_ed88, 4, 0x00f0_0000, &mut output).unwrap();
            // FaultOffset = 0 faults only on the first byte of the peripheral.
            bus.write(ADDRESS + 4, 4, 0x1234_5678, &mut output).unwrap();
            assert_eq!(bus.read::<4>(ADDRESS + 4).unwrap(), 0x1234_5678);
        }
        if enabled {
            bus.write(SHCSR, 4, 1 << 17, &mut output).unwrap();
        }
        let mut thumb = Thumb::new(Some(0));
        thumb.thumb2 = true;
        let mut registers = [0; 32];
        registers[0] = ADDRESS;
        registers[1] = 0x11;
        registers[2] = 0xa5a5_a5a5;
        registers[13] = 0x1000;
        let mut pc = CODE;
        assert_eq!(
            thumb
                .step(&mut pc, &mut registers, &mut bus, &mut output)
                .unwrap(),
            ThumbStep::Fault
        );
        assert_eq!(pc, expected_handler);
        assert_eq!(registers[1], 0x11);
        assert_eq!(registers[2], 0xa5a5_a5a5);
        let sp = if fp { 0xf98 } else { 0xfe0 };
        assert_eq!(registers[13], sp);
        assert_eq!(bus.read::<4>(sp + 4).unwrap(), 0x11);
        assert_eq!(bus.read::<4>(sp + 8).unwrap(), 0xa5a5_a5a5);
        assert_eq!(bus.read::<4>(sp + 24).unwrap(), CODE);
        assert_eq!(bus.read::<4>(CFSR).unwrap(), 0x8200);
        assert_eq!(bus.read::<4>(BFAR).unwrap(), ADDRESS);
        assert_eq!(
            bus.read::<4>(HFSR).unwrap(),
            if enabled { 0 } else { FORCED }
        );
        if !enabled {
            bus.write(HFSR, 4, FORCED, &mut output).unwrap();
            assert_eq!(bus.read::<4>(HFSR).unwrap(), 0);
        }
    }

    #[test]
    fn upstream_precise_bus_fault_on_peripheral_read() {
        check_precise_fault(&[0x6802, 0x2122, 0xe7fe], true, 0x300, false); // ldr r2, [r0]
    }

    #[test]
    fn upstream_precise_bus_fault_on_peripheral_write() {
        check_precise_fault(&[0x6002, 0x2122, 0xe7fe], true, 0x300, false); // str r2, [r0]
    }

    #[test]
    fn upstream_disabled_bus_fault_escalates_to_hard_fault() {
        check_precise_fault(&[0x6802, 0x2122, 0xe7fe], false, 0x340, false);
    }
    #[test]
    fn upstream_precise_bus_fault_on_vstr_s0() {
        check_precise_fault(&[0xed80, 0x0a00, 0xe7fe], true, 0x300, true);
    }

    #[test]
    fn upstream_precise_bus_fault_on_vstr_d0() {
        check_precise_fault(&[0xed80, 0x0b00, 0xe7fe], true, 0x300, true);
    }

    #[test]
    fn upstream_precise_bus_fault_on_vldr_s0() {
        check_precise_fault(&[0xed90, 0x0a00, 0xe7fe], true, 0x300, true);
    }

    #[test]
    fn upstream_precise_bus_fault_on_vldr_d0() {
        check_precise_fault(&[0xed90, 0x0b00, 0xe7fe], true, 0x300, true);
    }
}
