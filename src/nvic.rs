pub const BASE: u32 = 0xe000_e000;
pub const DHCSR: u32 = BASE + 0xdf0;
pub const VTOR: u32 = BASE + 0xd08;
const CPUID: u32 = BASE + 0xd00;
const AIRCR: u32 = BASE + 0xd0c;
pub const IRQ_LINES: usize = 240;
const BANKS: usize = IRQ_LINES.div_ceil(32);
const ENABLE: u32 = BASE + 0x100;
const DISABLE: u32 = BASE + 0x180;
const SET_PENDING: u32 = BASE + 0x200;
const ACTIVE: u32 = BASE + 0x300;
const CLEAR_PENDING: u32 = BASE + 0x280;
const PRIORITY: u32 = BASE + 0x400;
const FPCAR: u32 = BASE + 0xf38;
const FPCCR: u32 = BASE + 0xf34;
const CPACR: u32 = BASE + 0xd88;
const SYSTEM_PRIORITY: u32 = BASE + 0xd18;
const SYSTEM_PRIORITY_END: u32 = SYSTEM_PRIORITY + 11;
const PRIORITY_END: u32 = PRIORITY + IRQ_LINES as u32 - 1;

#[derive(Clone, Copy, Default)]
pub struct ActiveExceptions([u32; 8]);

impl ActiveExceptions {
    pub fn contains(&self, exception: u8) -> bool {
        self.0[usize::from(exception / 32)] & (1 << (exception % 32)) != 0
    }

    pub fn insert(&mut self, exception: u8) {
        self.0[usize::from(exception / 32)] |= 1 << (exception % 32);
    }

    pub fn remove(&mut self, exception: u8) {
        self.0[usize::from(exception / 32)] &= !(1 << (exception % 32));
    }

    pub fn is_empty(&self) -> bool {
        self.0.iter().all(|&bank| bank == 0)
    }
}

pub struct Nvic {
    vector_base: Option<u32>,
    enabled: [u32; BANKS],
    pending: [u32; BANKS],
    priority: [u8; IRQ_LINES],
    system_priority: [u8; 12],
    priority_mask: u8,
    priority_group: u8,
    scr: u32,
    pending_event: bool,
    ccr: u32,
    cpacr: u32,
    fpccr: Option<u32>,
    fpcar: u32,
    hardfault_pending: bool,
    fault_enables: u32,
    active: ActiveExceptions,
    locked_up: bool,
    cfsr: u32,
    hfsr: u32,
    bfar: u32,
    mpu: Option<Mpu>,
    has_cache: bool,
    cache_selector: u32,
    pub uart_irq: Option<u8>,
}

struct Mpu {
    count: u8,
    enabled: u32,
    selected: u8,
    regions: [[u32; 2]; 16],
}

impl Mpu {
    fn region(&self, offset: u32) -> Option<usize> {
        let alias = ((offset - 0xd9c) / 8) as u8;
        let region = if alias == 0 {
            self.selected
        } else {
            (self.selected & !3) + alias
        };
        (region < self.count).then_some(usize::from(region))
    }

    fn read(&self, offset: u32) -> u32 {
        match offset {
            0xd90 => u32::from(self.count) << 8,
            0xd94 => self.enabled,
            0xd98 => u32::from(self.selected),
            _ => self
                .region(offset)
                .map_or(0, |r| self.regions[r][usize::from(offset & 4 == 0)]),
        }
    }

    fn write(&mut self, offset: u32, value: u32) {
        match offset {
            0xd90 => {}
            0xd94 => self.enabled = value & 7,
            0xd98 => self.selected = (value & 0xff) as u8,
            _ => {
                if offset & 4 != 0 && value & 0x10 != 0 {
                    self.selected = (value & 0xf) as u8;
                }
                if let Some(r) = self.region(offset) {
                    let index = usize::from(offset & 4 == 0);
                    self.regions[r][index] = if index == 0 { value & !31 } else { value };
                }
            }
        }
    }
}

impl Nvic {
    pub fn new(priority_mask: u8, uart_irq: Option<u8>) -> Self {
        Self {
            vector_base: None,
            enabled: [0; BANKS],
            pending: [0; BANKS],
            priority: [0; IRQ_LINES],
            system_priority: [0; 12],
            priority_mask,
            priority_group: 0,
            scr: 0,
            pending_event: false,
            ccr: 1 << 9,
            cpacr: 0,
            fpccr: None,
            fpcar: 0,
            hardfault_pending: false,
            fault_enables: 0,
            active: ActiveExceptions::default(),
            locked_up: false,
            cfsr: 0,
            hfsr: 0,
            bfar: 0,
            mpu: None,
            has_cache: false,
            cache_selector: 0,
            uart_irq,
        }
    }

    pub fn enable_fpu(&mut self) {
        self.fpccr = Some(3 << 30);
    }

    pub fn has_fpu(&self) -> bool {
        self.fpccr.is_some()
    }

    pub fn fpu_access(&self) -> bool {
        self.has_fpu() && self.cpacr & (0xf << 20) == 0xf << 20
    }

    pub fn fpu_auto_preserve(&self) -> bool {
        self.fpccr.is_some_and(|value| value & (1 << 31) != 0)
    }

    pub const RANGES: [(u32, u32); 12] = [
        (ENABLE, PRIORITY_END + 1),
        (CPUID, CPUID + 4),
        (VTOR, AIRCR + 4),
        (CPACR, CPACR + 4),
        (FPCCR, FPCAR + 4),
        (BASE + 0xd24, BASE + 0xd30),
        (BASE + 0xd38, BASE + 0xd3c),
        (BASE + 0xdf0, BASE + 0xdf4),
        (BASE + 0xd90, BASE + 0xdbc),
        (BASE + 0xd78, BASE + 0xd88),
        (BASE + 0xf50, BASE + 0xf78),
        (BASE + 0xd10, SYSTEM_PRIORITY_END + 1),
    ];

    pub fn overlaps(start: u64, end: u64) -> bool {
        Self::RANGES
            .into_iter()
            .any(|(low, high)| start < u64::from(high) && end > u64::from(low))
    }

    pub fn unaligned_access_traps(&self) -> bool {
        self.ccr & (1 << 3) != 0
    }

    pub fn divide_by_zero_traps(&self) -> bool {
        self.ccr & (1 << 4) != 0
    }

    pub fn reset(&mut self) {
        let mpu = self.mpu.as_ref().map(|mpu| (mpu.count, self.has_cache));
        let fpu = self.has_fpu();
        *self = Self::new(self.priority_mask, self.uart_irq);
        if fpu {
            self.enable_fpu();
        }
        if let Some((count, has_cache)) = mpu {
            self.configure_mpu(count, has_cache);
        }
    }

    pub fn configure_mpu(&mut self, count: u8, has_cache: bool) {
        self.has_cache = has_cache;
        self.mpu = Some(Mpu {
            count,
            enabled: 0,
            selected: 0,
            regions: [[0; 2]; 16],
        });
    }

    pub fn vector_base(&self) -> Option<u32> {
        self.vector_base
    }

    pub fn set_vector_base(&mut self, address: u32) {
        self.vector_base = Some(address);
    }

    pub fn set_uart_level(&mut self, high: bool, active: &ActiveExceptions) {
        if let Some(line) = self.uart_irq {
            self.set_line(line, high, active);
        }
    }

    pub fn set_line(&mut self, line: u8, high: bool, active: &ActiveExceptions) {
        if high && usize::from(line) < IRQ_LINES && !active.contains(16 + line) {
            self.pend(usize::from(line / 32), 1 << (line % 32));
        }
    }

    fn pend(&mut self, bank: usize, bits: u32) {
        if bits & !self.pending[bank] != 0 {
            self.signal_pending_event();
        }
        self.pending[bank] |= bits;
    }

    pub fn signal_pending_event(&mut self) {
        self.pending_event |= self.scr & (1 << 4) != 0; // SEVONPEND
    }

    pub fn take_pending_event(&mut self) -> bool {
        std::mem::take(&mut self.pending_event)
    }

    pub fn priority(&self, exception: u8) -> u8 {
        match exception {
            4..=15 => self.system_priority[usize::from(exception - 4)],
            16..=255 => self.priority[usize::from(exception - 16)],
            _ => 0,
        }
    }

    pub fn preemption_priority(&self, priority: u8) -> u8 {
        (u16::from(priority & self.priority_mask) & (0xff_u16 << (self.priority_group + 1))) as u8
    }

    pub fn next_irq(&self, active: &ActiveExceptions) -> Option<(u8, u8)> {
        let mut selected = None;
        for bank in 0..BANKS {
            let mut candidates = self.enabled[bank] & self.pending[bank];
            while candidates != 0 {
                let line = ((bank * 32) as u32 + candidates.trailing_zeros()) as u8;
                candidates &= candidates - 1;
                let priority = self.priority[usize::from(line)];
                if !active.contains(16 + line) && selected.is_none_or(|(_, best)| priority < best) {
                    selected = Some((line, priority));
                }
            }
        }
        selected
    }

    pub fn acknowledge(&mut self, line: u8) {
        self.pending[usize::from(line / 32)] &= !(1 << (line % 32));
    }

    fn bank(address: u32) -> Option<(u32, usize)> {
        let base = address & !0x7f;
        let register = address.wrapping_sub(base);
        let bank = (register / 4) as usize;
        (matches!(base, ENABLE | DISABLE | SET_PENDING | CLEAR_PENDING)
            && register & 3 == 0
            && bank < BANKS)
            .then_some((base, bank))
    }

    fn priority_registers(address: u32) -> Option<(bool, usize)> {
        match address {
            PRIORITY..=PRIORITY_END => Some((false, (address - PRIORITY) as usize)),
            SYSTEM_PRIORITY..=SYSTEM_PRIORITY_END => {
                Some((true, (address - SYSTEM_PRIORITY) as usize))
            }
            _ => None,
        }
    }

    pub fn set_active(&mut self, active: ActiveExceptions) {
        self.active = active;
    }

    pub fn enter_lockup(&mut self) {
        self.locked_up = true;
    }

    pub fn leave_lockup(&mut self) {
        self.locked_up = false;
    }

    pub fn ignores_negative_priority_data_fault(&self) -> bool {
        self.ccr & (1 << 8) != 0
    }

    pub fn record_stacking_fault(&mut self) {
        self.cfsr |= 1 << 12; // STKERR
    }

    pub fn record_unstacking_fault(&mut self) {
        self.cfsr |= 1 << 11;
    }

    pub fn record_lazy_fault(&mut self) {
        self.cfsr |= 1 << 13;
    }

    pub fn pend_hardfault(&mut self) {
        self.hardfault_pending = true;
    }

    pub fn take_pending_hardfault(&mut self) -> bool {
        std::mem::take(&mut self.hardfault_pending)
    }

    pub fn lazy_fp_enabled(&self) -> bool {
        self.fpccr.is_some_and(|value| value & (1 << 30) != 0)
    }

    pub fn reserve_lazy_fp(&mut self, address: u32, hardfault_ready: bool) {
        self.fpcar = address;
        if let Some(fpccr) = &mut self.fpccr {
            *fpccr = (*fpccr & (3 << 30)) | (u32::from(hardfault_ready) << 4) | 1;
        }
    }

    pub fn clear_lazy_fp(&mut self) {
        if let Some(fpccr) = &mut self.fpccr {
            *fpccr &= !1;
        }
    }

    pub fn record_vector_fault(&mut self) {
        self.hfsr |= 1 << 1; // VECTTBL
    }

    pub fn record_instruction_fault(&mut self, invalid_state: bool) {
        self.cfsr |= if invalid_state { 1 << 17 } else { 1 << 8 }; // INVSTATE or IBUSERR
    }

    pub fn record_fault(&mut self, address: Option<u32>) {
        if let Some(address) = address {
            self.bfar = address;
            self.cfsr |= (1 << 9) | (1 << 15); // PRECISERR | BFARVALID
        } else {
            self.cfsr |= 1 << 19; // NOCP
        }
    }

    pub fn fault_enabled(&self, exception: u8) -> bool {
        self.fault_enables & (1 << (exception + 12)) != 0
    }

    pub fn force_hard_fault(&mut self) {
        self.hfsr |= 1 << 30;
    }

    pub fn read(&self, address: u32, width: usize) -> Option<u32> {
        let cfsr_offset = address.wrapping_sub(BASE + 0xd28);
        if matches!(width, 1 | 2 | 4) && cfsr_offset <= 4 - width as u32 {
            return Some((self.cfsr >> (cfsr_offset * 8)) & (u32::MAX >> (32 - width * 8)));
        }
        if width == 4 {
            let offset = address.wrapping_sub(BASE);
            if self.has_cache {
                match offset {
                    0xd78 => return Some(0x0300_0003), // L1 instruction and data caches
                    0xd7c => return Some(0x8333_8003),
                    0xd80 => {
                        return Some(if self.cache_selector == 0 {
                            (127 << 13) | (3 << 3) | 1 // 128 sets, 4 ways, 32-byte line
                        } else {
                            (255 << 13) | (1 << 3) | 1 // 256 sets, 2 ways, 32-byte line
                        });
                    }
                    0xd84 => return Some(self.cache_selector),
                    _ => {}
                }
            }
            if (0xd90..=0xdb8).contains(&offset) {
                return self.mpu.as_ref().map(|mpu| mpu.read(offset));
            }
            if address == BASE + 0xd10 {
                return Some(self.scr);
            }
            if address == BASE + 0xd14 {
                return Some(self.ccr);
            }
            if address == BASE + 0xd24 {
                return Some(
                    self.fault_enables
                        | (u32::from(self.hardfault_pending) << 21)
                        | (u32::from(self.active.contains(4)))
                        | (u32::from(self.active.contains(5)) << 1)
                        | (u32::from(self.active.contains(6)) << 3),
                );
            }
            if address == DHCSR {
                return Some(u32::from(self.locked_up) << 19);
            }
            if let Some(bank) = address
                .checked_sub(ACTIVE)
                .filter(|offset| offset & 3 == 0 && *offset < BANKS as u32 * 4)
            {
                let bank = (bank / 4) as usize;
                let lower = self.active.0[bank] >> 16;
                let upper = self.active.0.get(bank + 1).copied().unwrap_or(0) << 16;
                return Some(lower | upper);
            }
            if address == BASE + 0xd2c {
                return Some(self.hfsr);
            }
            if address == BASE + 0xd38 {
                return Some(self.bfar);
            }
            if address == CPUID && self.has_fpu() {
                return Some(0x410f_c241);
            }
            if address == FPCAR && self.has_fpu() {
                return Some(self.fpcar);
            }
            if address == FPCCR {
                return self.fpccr;
            }
            if address == CPACR {
                return Some(self.cpacr);
            }
            if address == AIRCR {
                return Some(0xfa05_0000 | (u32::from(self.priority_group) << 8));
            }
            if address == VTOR {
                return Some(self.vector_base.unwrap_or(0));
            }
            if let Some((base, bank)) = Self::bank(address) {
                return Some(if matches!(base, ENABLE | DISABLE) {
                    self.enabled[bank]
                } else {
                    self.pending[bank]
                });
            }
        }
        if !matches!(width, 1 | 4) {
            return None;
        }
        let (system, offset) = Self::priority_registers(address)?;
        let priority = if system {
            &self.system_priority[..]
        } else {
            &self.priority[..]
        };
        let bytes = priority.get(offset..offset + width)?;
        let mut word = [0; 4];
        word[..width].copy_from_slice(bytes);
        Some(u32::from_le_bytes(word))
    }

    pub fn write(&mut self, address: u32, width: usize, value: u32) -> bool {
        if width == 4 && address == AIRCR {
            if value >> 16 == 0x05fa {
                self.priority_group = ((value >> 8) & 7) as u8;
            }
            return true;
        }
        let offset = address.wrapping_sub(BASE);
        if width == 4 && self.has_cache {
            if offset == 0xd84 {
                self.cache_selector = value & 1;
                return true;
            }
            // Maintenance operations are synchronous on the coherent bus.
            if matches!(
                offset,
                0xf50 | 0xf58 | 0xf5c | 0xf60 | 0xf64 | 0xf68 | 0xf6c | 0xf70 | 0xf74
            ) {
                return true;
            }
        }
        if width == 4
            && (0xd90..=0xdb8).contains(&offset)
            && let Some(mpu) = &mut self.mpu
        {
            mpu.write(offset, value);
            return true;
        }
        if width == 4 && address == DHCSR {
            // Halting debug controls are not implemented.
            return true;
        }
        if width == 4 && address == BASE + 0xd10 {
            self.scr = value & 0x16;
            return true;
        }
        if width == 4 && address == BASE + 0xd14 {
            self.ccr = value & 0x0007_031b;
            return true;
        }
        if width == 4 && address == BASE + 0xd24 {
            self.fault_enables = value & (7 << 16);
            return true;
        }
        let cfsr_offset = address.wrapping_sub(BASE + 0xd28);
        if matches!(width, 1 | 2 | 4) && cfsr_offset <= 4 - width as u32 {
            self.cfsr &= !((value & (u32::MAX >> (32 - width * 8))) << (cfsr_offset * 8));
            return true;
        }
        if width == 4 && address == BASE + 0xd2c {
            self.hfsr &= !value;
            return true;
        }
        if width == 4 && address == BASE + 0xd38 {
            self.bfar = value;
            return true;
        }
        if width == 4
            && address == FPCCR
            && let Some(fpccr) = &mut self.fpccr
        {
            *fpccr = (*fpccr & !(3 << 30)) | (value & (3 << 30));
            return true;
        }
        if width == 4 && address == CPACR {
            self.cpacr = value & (0xf << 20);
            return true;
        }
        if width == 4
            && let Some((base, bank)) = Self::bank(address)
        {
            let value = if bank + 1 == BANKS {
                value & 0xffff
            } else {
                value
            };
            match base {
                ENABLE => self.enabled[bank] |= value,
                DISABLE => self.enabled[bank] &= !value,
                SET_PENDING => self.pend(bank, value),
                CLEAR_PENDING => self.pending[bank] &= !value,
                _ => unreachable!(),
            }
            return true;
        }
        if !matches!(width, 1 | 4) {
            return false;
        }
        let Some((system, offset)) = Self::priority_registers(address) else {
            return false;
        };
        let registers = if system {
            &mut self.system_priority[..]
        } else {
            &mut self.priority[..]
        };
        let Some(bytes) = registers.get_mut(offset..offset + width) else {
            return false;
        };
        for (index, (priority, value)) in bytes.iter_mut().zip(value.to_le_bytes()).enumerate() {
            if !system || matches!(offset + index, 0..=2 | 7 | 8 | 10 | 11) {
                *priority = value & self.priority_mask;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aircr_requires_key_and_splits_preemption_from_subpriority() {
        let mut nvic = Nvic::new(0xf0, None);
        assert!(nvic.write(AIRCR, 4, 0x500));
        assert_eq!(nvic.read(AIRCR, 4), Some(0xfa05_0000));
        assert!(nvic.write(AIRCR, 4, 0x05fa_0500));
        assert_eq!(nvic.read(AIRCR, 4), Some(0xfa05_0500));
        assert_eq!(nvic.preemption_priority(0x70), 0x40);
        assert_eq!(nvic.preemption_priority(0x50), 0x40);
        nvic.reset();
        assert_eq!(nvic.preemption_priority(0x70), 0x70);
    }
}
