mod fpu;
mod thumb2;

use std::io::Write;

use crate::{
    bus::Bus,
    nvic::{ActiveExceptions, IRQ_LINES, Nvic},
    systick::{PENDSV, SYSTICK, SysTick},
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
    active_exceptions: ActiveExceptions,
    interrupts_masked: bool,
    sleep: Sleep,
    event: bool,
    pending: [u32; IRQ_LINES.div_ceil(32)],
    system_pending: u64,
    thread_uses_psp: bool,
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

    pub fn reset_state(&mut self) {
        *self = Self {
            vectors: self.vectors,
            thumb2: self.thumb2,
            ..Self::default()
        };
    }

    pub fn vector_base(&self) -> u32 {
        self.vectors.unwrap_or(0)
    }

    pub fn set_vector_base(&mut self, base: u32) {
        self.vectors = Some(base);
    }

    pub fn infer_vector_base(&mut self, base: u32) {
        self.vectors.get_or_insert(base);
    }

    pub fn reset(&self, bus: &Bus) -> Result<(u32, u32), String> {
        let sp = bus
            .read_ram::<4>(self.vector_base())
            .map_err(|error| error.to_string())?;
        let vector = self
            .vector_base()
            .checked_add(4)
            .ok_or("reset vector address overflow")?;
        let target = bus
            .read_ram::<4>(vector)
            .map_err(|error| error.to_string())?;
        if target & 1 == 0 {
            return Err(format!(
                "Cortex-M reset vector {target:#010x} must select Thumb state"
            ));
        }
        bus.read_ram::<2>(target & !1)
            .map_err(|error| error.to_string())?;
        Ok((sp & !3, target & !1))
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
            20 => (u32::from(self.fp_active) << 2) | (u32::from(self.uses_psp()) << 1),
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
                self.select_thread_stack(value & 2 != 0, r);
                if self.thumb2 {
                    self.fp_active = value & 4 != 0;
                }
            }
            _ => return false,
        }
        true
    }

    fn exception_entry(
        &mut self,
        exception: u8,
        return_pc: u32,
        r: &mut [u32; 32],
        bus: &mut Bus,
    ) -> Result<u32, String> {
        let vector = bus
            .nvic
            .as_ref()
            .and_then(|nvic| nvic.vector_base())
            .unwrap_or_else(|| self.vector_base())
            .checked_add(u32::from(exception) * 4)
            .ok_or("exception vector address overflow")?;
        let handler = bus
            .read_ram::<4>(vector)
            .map_err(|error| error.to_string())?;
        if handler & 1 == 0 {
            return Err(format!(
                "exception {exception} vector {handler:#010x} does not select Thumb state"
            ));
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
        let frame = [r[0], r[1], r[2], r[3], r[12], r[14], return_pc, xpsr];
        let memory = bus
            .memory(stack, frame_size)
            .map_err(|error| error.to_string())?;
        if self.fp_active {
            for (word, value) in memory[32..]
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .zip(self.fp[..16].iter().copied().chain([self.fpscr, 0]))
            {
                *word = value.to_le_bytes();
            }
        }
        for (word, value) in memory.as_chunks_mut::<4>().0.iter_mut().zip(frame) {
            *word = value.to_le_bytes();
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
        self.active_exceptions.insert(exception);
        self.active_exception = exception;
        self.itstate = 0;
        Ok(handler & !1)
    }

    fn branch(&mut self, target: u32, r: &mut [u32; 32], bus: &Bus) -> Result<u32, String> {
        let basic_target = if self.thumb2 { target | 0x10 } else { target };
        if self.active_exception != 0
            && matches!(basic_target, 0xffff_fff1 | 0xffff_fff9 | 0xffff_fffd)
        {
            let extended = target & 0x10 == 0;
            let use_psp = basic_target == 0xffff_fffd;
            let frame_size = if extended { 104 } else { 32 };
            let stack = if use_psp { self.psp } else { r[13] };
            bus.check_thumb_access(stack, frame_size, false, false)?;
            let mut frame = [0; 8];
            for (index, word) in frame.iter_mut().enumerate() {
                *word = bus
                    .read_ram::<4>(stack.wrapping_add(index as u32 * 4))
                    .map_err(|error| error.to_string())?;
            }
            let [r0, r1, r2, r3, r12, lr, pc, xpsr] = frame;
            let previous = xpsr & 0x1ff;
            let mut remaining = self.active_exceptions;
            remaining.remove(self.active_exception);
            let valid_mode = if basic_target != 0xffff_fff1 {
                previous == 0 && remaining.is_empty()
            } else {
                previous != 0
                    && u8::try_from(previous).is_ok_and(|exception| remaining.contains(exception))
            };
            if xpsr & (1 << 24) == 0 || !valid_mode {
                return Err(format!(
                    "invalid Cortex-M exception return frame: pc={pc:#x} xpsr={xpsr:#x} target={target:#x}"
                ));
            }
            if extended {
                let saved = bus
                    .memory_slice(stack.wrapping_add(32), 68)
                    .map_err(|e| e.to_string())?;
                for (value, bytes) in self.fp[..16].iter_mut().zip(saved.as_chunks::<4>().0) {
                    *value = u32::from_le_bytes(*bytes);
                }
                self.fpscr = u32::from_le_bytes(saved[64..68].try_into().unwrap());
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

    pub fn service_interrupts(
        &mut self,
        pc: &mut u32,
        r: &mut [u32; 32],
        bus: &mut Bus,
    ) -> Result<bool, String> {
        bus.sample_nvic(&self.active_exceptions, self.sleeping());
        if let Some(timer) = bus.systick.as_mut() {
            timer.tick();
        }
        let pending = bus
            .nvic
            .as_ref()
            .map_or(Default::default(), Nvic::pending_exceptions);
        let system_pending = bus.systick.as_ref().map_or(0, SysTick::pending_exceptions);
        let newly_pending = pending
            .iter()
            .zip(self.pending)
            .any(|(next, previous)| next & !previous != 0)
            || system_pending & !self.system_pending != 0;
        if newly_pending && bus.nvic.as_ref().is_some_and(Nvic::send_event_on_pending) {
            self.signal_event();
        }
        self.pending = pending;
        self.system_pending = system_pending;
        if self.faultmask || (self.interrupts_masked && self.sleep != Sleep::Interrupt) {
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
            u16::from(*priority) < current_priority
                && (self.basepri == 0 || *priority < self.basepri)
        }) {
            self.sleep = Sleep::Running;
            if self.interrupts_masked {
                return Ok(false);
            }
            *pc = self.exception_entry(exception, *pc, r, bus)?;
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

    pub fn step(
        &mut self,
        pc: &mut u32,
        r: &mut [u32; 32],
        bus: &mut Bus,
        output: &mut impl Write,
    ) -> Result<bool, String> {
        let inst = bus.thumb_fetch(*pc)?;
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
                        .guest_memory(address, count as usize * 4)
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
                }
            }
            0xe800..=0xefff | 0xf800..=0xffff if self.thumb2 => {
                let low = bus.read_ram::<2>(next).map_err(|error| error.to_string())?;
                next = architectural_pc;
                if matches!(inst, 0xe92d | 0xe8bd)
                    && low.count_ones() >= 2
                    && low & (1 << 13) == 0
                    && (low & (1 << 15) == 0 || (inst == 0xe8bd && low & (1 << 14) == 0))
                {
                    self.stack_transfer(low, inst == 0xe8bd, &mut next, r, bus)?;
                } else {
                    self.execute_thumb2(inst, low, &mut next, r, bus, output)?;
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
                } else if !self.execute_fpu(inst, low, architectural_pc, r, bus, output)? {
                    self.execute_thumb2(inst, low, &mut next, r, bus, output)?;
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
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
