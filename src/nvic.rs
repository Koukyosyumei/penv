mod mpu;

pub const BASE: u32 = 0xe000_e000;
pub const HARDFAULT: u8 = 3;
pub const VTOR: u32 = BASE + 0xd08;
pub const IRQ_LINES: usize = 240;
const BANKS: usize = IRQ_LINES.div_ceil(32);
const AIRCR: u32 = BASE + 0xd0c;
const NS_SCS_OFFSET: u32 = 0x20000;
const ITNS: u32 = BASE + 0x380;
const SAU_CTRL: u32 = BASE + 0xdd0;
const SAU_TYPE: u32 = BASE + 0xdd4;
const SAU_RNR: u32 = BASE + 0xdd8;
const SAU_RBAR: u32 = BASE + 0xddc;
const SAU_RLAR: u32 = BASE + 0xde0;
const SFSR: u32 = BASE + 0xde4;
const BFHFNMINS: u32 = 1 << 13;
const ENABLE: u32 = BASE + 0x100;
const DISABLE: u32 = BASE + 0x180;
const SET_PENDING: u32 = BASE + 0x200;
const CLEAR_PENDING: u32 = BASE + 0x280;
const PRIORITY: u32 = BASE + 0x400;
pub const DHCSR: u32 = 0xe000_edf0;
const CPACR: u32 = BASE + 0xd88;
const SYSTEM_PRIORITY: u32 = BASE + 0xd18;
const SYSTEM_PRIORITY_END: u32 = SYSTEM_PRIORITY + 11;
const PRIORITY_END: u32 = PRIORITY + IRQ_LINES as u32 - 1;

#[derive(Clone, Copy, Default)]
pub struct ActiveExceptions(pub(crate) [u32; 8]);

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

    pub(crate) fn depth(&self) -> u32 {
        self.0.iter().map(|bank| bank.count_ones()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.0.iter().all(|&bank| bank == 0)
    }
}

#[derive(Clone, Default)]
struct Security {
    bfhfnmins: bool,
    vector_base_ns: u32,
    nonsecure_irqs: [u32; BANKS],
    sau_control: u32,
    sau_region: usize,
    sau_regions: [(u32, u32); 8],
    sfsr: u32,
}

impl Security {
    fn read_sau(&self, address: u32) -> Option<u32> {
        let region = self.sau_regions.get(self.sau_region).copied().unwrap_or_default();
        Some(match address {
            SAU_CTRL => self.sau_control,
            SAU_TYPE => self.sau_regions.len() as u32,
            SAU_RNR => self.sau_region as u32,
            SAU_RBAR => region.0,
            SAU_RLAR => region.1,
            SFSR => self.sfsr,
            _ => return None,
        })
    }

    fn write_sau(&mut self, address: u32, value: u32) -> bool {
        match address {
            SAU_CTRL => self.sau_control = value & 3,
            SAU_RNR => self.sau_region = (value & 0xff) as usize,
            SAU_RBAR | SAU_RLAR => {
                if let Some(region) = self.sau_regions.get_mut(self.sau_region) {
                    if address == SAU_RBAR {
                        region.0 = value & !31;
                    } else {
                        region.1 = value & !28;
                    }
                }
            }
            SFSR => self.sfsr &= !value,
            _ => return false,
        }
        true
    }
}

#[derive(Clone)]
pub struct Nvic {
    vector_base: Option<u32>,
    security: Option<Box<Security>>,
    external_lines: usize,
    enabled: [u32; BANKS],
    pending: [u32; BANKS],
    priority: [u8; IRQ_LINES],
    system_priority: [u8; 12],
    priority_mask: u8,
    priority_group: u8,
    pending_event: bool,
    hardfault_pending: bool,
    fpccr: Option<u32>,
    scr: u32,
    reset_requested: bool,
    pub lockup: bool,
    active_system: u16,
    debug_status: bool,
    active: ActiveExceptions,
    fpcar: u32,
    lazy_fp_full_frame: bool,
    bus_fault_status: u32,
    bus_fault_address: u32,
    ccr: u32,
    cpacr: u32,
    fault_enables: u32,
    cfsr: u32,
    memory_fault_address: u32,
    hfsr: u32,
    mpu: Option<Mpu>,
    cache_selector: u32,
    nrf_mpu: Option<mpu::Mpu>,
    demcr: u32,
    pub uart_irq: Option<u8>,
}

#[derive(Clone)]
struct Mpu {
    count: u8,
    enabled: u32,
    selected: u8,
    regions: [[u32; 2]; 16],
}

impl Mpu {
    fn permits(&self, address: u32, width: usize, write: bool, execute: bool, unprivileged: bool, negative_priority: bool) -> bool {
        if self.enabled & 1 == 0 || negative_priority && self.enabled & 2 == 0 {
            return true;
        }
        (0..width).all(|offset| {
            let Some(address) = address.checked_add(offset as u32) else { return false };
            if (0xe000_0000..0xe010_0000).contains(&address) {
                return !unprivileged && !execute;
            }
            for &[base, attrs] in self.regions[..usize::from(self.count)].iter().rev() {
                let size_field = (attrs >> 1) & 31;
                if attrs & 1 == 0 || size_field < 4 {
                    continue;
                }
                let size = 1u64 << (size_field + 1);
                let aligned = u64::from(base) & !(size - 1);
                let offset = u64::from(address).wrapping_sub(aligned);
                if offset >= size || size >= 256 && attrs & (1 << (8 + offset / (size / 8))) != 0 {
                    continue;
                }
                return (!execute || attrs & (1 << 28) == 0) && match (attrs >> 24) & 7 {
                    1 => !unprivileged,
                    2 => !unprivileged || !write,
                    3 => true,
                    5 => !unprivileged && !write,
                    6 | 7 => !write,
                    _ => false,
                };
            }
            !unprivileged && self.enabled & 4 != 0
                && (!execute || address < 0x4000_0000 || (0x6000_0000..0xa000_0000).contains(&address))
        })
    }

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
            security: None,
            external_lines: IRQ_LINES,
            enabled: [0; BANKS],
            pending: [0; BANKS],
            priority: [0; IRQ_LINES],
            system_priority: [0; 12],
            priority_mask,
            priority_group: 0,
            pending_event: false,
            hardfault_pending: false,
            fpccr: None,
            scr: 0,
            reset_requested: false,
            lockup: false,
            active_system: 0,
            debug_status: false,
            active: ActiveExceptions::default(),
            fpcar: 0,
            lazy_fp_full_frame: false,
            bus_fault_status: 0,
            bus_fault_address: 0,
            ccr: 1 << 9,
            cpacr: 0,
            fault_enables: 0,
            cfsr: 0,
            memory_fault_address: 0,
            hfsr: 0,
            mpu: None,
            cache_selector: 0,
            nrf_mpu: None,
            demcr: 0,
            uart_irq,
        }
    }

    pub const RANGES: [(u32, u32); 13] = [
        (ENABLE, PRIORITY_END + 1),
        (VTOR, VTOR + 4),
        (AIRCR, AIRCR + 4),
        (BASE + 0xd00, BASE + 0xd04),
        (BASE + 0xf00, BASE + 0xf04),
        (BASE + 0xf34, BASE + 0xf38),
        (CPACR, CPACR + 4),
        (BASE + 0xd24, BASE + 0xd30),
        (BASE + 0xd34, BASE + 0xd38),
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

    pub(crate) fn supports_snapshot(&self) -> bool {
        self.security.is_none()
    }

    pub(crate) fn save_snapshot(&self, output: &mut impl std::io::Write) -> std::io::Result<()> {
        use crate::snapshot::write_words;
        write_words(output, &[
            self.vector_base.map_or(1 << 32, u64::from), self.external_lines as u64,
            u64::from(self.priority_mask), u64::from(self.priority_group), u64::from(self.pending_event),
            u64::from(self.hardfault_pending), self.fpccr.map_or(1 << 32, u64::from),
            u64::from(self.scr), u64::from(self.reset_requested), u64::from(self.lockup),
            u64::from(self.active_system), u64::from(self.debug_status), u64::from(self.fpcar),
            u64::from(self.lazy_fp_full_frame), u64::from(self.bus_fault_status),
            u64::from(self.bus_fault_address), u64::from(self.ccr), u64::from(self.cpacr),
            u64::from(self.fault_enables), u64::from(self.cfsr) | (u64::from(self.memory_fault_address) << 32), u64::from(self.hfsr),
            u64::from(self.cache_selector), u64::from(self.demcr), u64::from(self.mpu.is_some()), u64::from(self.nrf_mpu.is_some()),
        ])?;
        write_words(output, &self.enabled.map(u64::from))?;
        write_words(output, &self.pending.map(u64::from))?;
        write_words(output, &self.active.0.map(u64::from))?;
        output.write_all(&self.priority)?;
        output.write_all(&self.system_priority)?;
        if let Some(mpu) = &self.mpu {
            write_words(output, &[u64::from(mpu.count), u64::from(mpu.enabled), u64::from(mpu.selected)])?;
            for region in mpu.regions {
                write_words(output, &region.map(u64::from))?;
            }
        }
        if let Some(mpu) = &self.nrf_mpu {
            mpu.save_snapshot(output)?;
        }
        Ok(())
    }

    pub(crate) fn load_snapshot(&mut self, input: &mut impl std::io::Read) -> std::io::Result<()> {
        use crate::snapshot::{invalid, read_bool, read_words};
        let [vtor, lines, mask, group, event, hard_pending, fpccr, scr, reset,
            lockup, active_system, debug, fpcar, full_fp, fault_status, fault_address,
            ccr, cpacr, fault_enable, cfsr, hfsr, cache, demcr, mpu, nrf_mpu] = read_words(input)?;
        let word = |value| u32::try_from(value).map_err(std::io::Error::other);
        let optional_word = |value| if value == 1 << 32 { Ok(None) } else { word(value).map(Some) };
        if lines != self.external_lines as u64 || mask != u64::from(self.priority_mask)
            || group > 7 || read_bool(mpu)? != self.mpu.is_some() || read_bool(nrf_mpu)? != self.nrf_mpu.is_some()
        {
            return Err(invalid("NVIC configuration differs from the snapshot"));
        }
        self.vector_base = optional_word(vtor)?;
        self.priority_group = group as u8;
        self.pending_event = read_bool(event)?;
        self.hardfault_pending = read_bool(hard_pending)?;
        self.fpccr = optional_word(fpccr)?;
        self.scr = word(scr)?;
        self.reset_requested = read_bool(reset)?;
        self.lockup = read_bool(lockup)?;
        self.active_system = u16::try_from(active_system).map_err(std::io::Error::other)?;
        self.debug_status = read_bool(debug)?;
        self.fpcar = word(fpcar)?;
        self.lazy_fp_full_frame = read_bool(full_fp)?;
        self.bus_fault_status = word(fault_status)?;
        self.bus_fault_address = word(fault_address)?;
        self.ccr = word(ccr)?;
        self.cpacr = word(cpacr)?;
        self.fault_enables = word(fault_enable)?;
        self.cfsr = cfsr as u32;
        self.memory_fault_address = (cfsr >> 32) as u32;
        self.hfsr = word(hfsr)?;
        self.cache_selector = word(cache)?;
        self.demcr = word(demcr)?;
        for bank in [&mut self.enabled, &mut self.pending, &mut self.active.0] {
            for entry in bank {
                let [value] = read_words(input)?;
                *entry = word(value)?;
            }
        }
        input.read_exact(&mut self.priority)?;
        input.read_exact(&mut self.system_priority)?;
        if let Some(mpu) = &mut self.mpu {
            let [count, enabled, selected] = read_words(input)?;
            if count != u64::from(mpu.count) || selected > u64::from(u8::MAX) {
                return Err(invalid("invalid MPU selector in NVIC snapshot"));
            }
            mpu.enabled = word(enabled)?;
            mpu.selected = selected as u8;
            for region in &mut mpu.regions {
                let [base, limit] = read_words(input)?;
                *region = [word(base)?, word(limit)?];
            }
        }
        if let Some(mpu) = &mut self.nrf_mpu {
            mpu.load_snapshot(input)?;
        }
        Ok(())
    }

    pub fn reset(&mut self) {
        let external_lines = self.external_lines;
        let has_fpu = self.fpccr.is_some();
        let has_nrf_mpu = self.nrf_mpu.is_some();
        let mpu_count = self.mpu.as_ref().map(|mpu| mpu.count);
        let trustzone = self.security.is_some();
        *self = Self::new(self.priority_mask, self.uart_irq);
        self.external_lines = external_lines;
        if has_fpu {
            self.enable_fpu();
        }
        if trustzone {
            self.enable_trustzone();
        }
        if has_nrf_mpu {
            self.enable_nrf_mpu();
        }
        if let Some(count) = mpu_count {
            self.configure_mpu(count);
        }
    }

    pub fn enable_trustzone(&mut self) {
        self.security = Some(Box::default());
    }

    pub fn bfhfnmins(&self) -> bool {
        self.security.as_ref().is_some_and(|security| security.bfhfnmins)
    }

    pub fn exception_targets_nonsecure(&self, exception: u8, source_secure: bool) -> bool {
        let Some(security) = &self.security else { return false };
        match exception {
            2 | 5 => security.bfhfnmins,
            HARDFAULT => security.bfhfnmins && !source_secure,
            16.. => {
                let irq = usize::from(exception - 16);
                security.nonsecure_irqs[irq / 32] & (1 << (irq % 32)) != 0
            }
            _ => !source_secure,
        }
    }

    pub fn nonsecure_vector_base(&self) -> Option<u32> {
        self.security.as_ref().map(|security| security.vector_base_ns)
    }

    pub fn set_nonsecure_vector_base(&mut self, address: u32) -> Result<(), String> {
        let security = self.security.as_mut().ok_or("TrustZone is disabled")?;
        if address & 0x7f != 0 {
            return Err("Non-secure vector table requires a 128-byte-aligned address".into());
        }
        security.vector_base_ns = address;
        Ok(())
    }

    pub fn configure_sau(&mut self, register: &str, value: Option<u32>) -> Result<Option<u32>, String> {
        let security = self.security.as_mut().ok_or("TrustZone is disabled")?;
        let address = match register {
            "SAUControl" => SAU_CTRL,
            "SAURegionNumber" => SAU_RNR,
            "SAURegionBaseAddress" => SAU_RBAR,
            "SAURegionLimitAddress" => SAU_RLAR,
            _ => return Err(format!("unknown SAU register {register:?}")),
        };
        if let Some(value) = value {
            security.write_sau(address, value);
            Ok(None)
        } else {
            Ok(security.read_sau(address))
        }
    }

    pub fn address_is_nonsecure(&self, address: u32) -> bool {
        self.security.as_ref().is_some_and(|security| {
            if security.sau_control & 1 == 0 {
                security.sau_control & 2 != 0
            } else {
                security.sau_regions.iter().any(|&(base, limit)| {
                    limit & 3 == 1 && (base..=(limit | 31)).contains(&address)
                })
            }
        })
    }

    pub fn take_reset_request(&mut self) -> bool {
        std::mem::take(&mut self.reset_requested)
    }

    pub(crate) fn deep_sleep_enabled(&self) -> bool {
        self.scr & (1 << 2) != 0
    }

    pub fn sleep_on_exit(&self) -> bool {
        self.scr & (1 << 1) != 0
    }

    pub fn send_event_on_pending(&self) -> bool {
        self.scr & (1 << 4) != 0
    }

    pub fn set_active_exceptions(&mut self, active: u128) {
        self.active_system = active as u16;
    }
    pub fn has_debug_status(&self) -> bool {
        self.debug_status || self.has_fpu()
    }

    pub(crate) fn set_debug_status_supported(&mut self, supported: bool) {
        self.debug_status = supported;
    }
    pub fn ignores_negative_priority_bus_fault(&self) -> bool {
        self.ccr & (1 << 8) != 0
    }
    pub fn record_precise_bus_fault(&mut self, address: u32, escalate: bool) {
        self.bus_fault_status |= (1 << 9) | (1 << 15);
        self.bus_fault_address = address;
        if escalate {
            self.hardfault_pending = true;
        }
    }

    pub(crate) fn record_memory_fault(&mut self, address: u32, execute: bool) {
        if execute {
            self.cfsr |= 1;
        } else {
            self.cfsr |= (1 << 1) | (1 << 7);
            self.memory_fault_address = address;
        }
    }

    pub(crate) fn user_can_pend_interrupt(&self) -> bool {
        self.ccr & 2 != 0
    }

    pub fn set_hard_fault_pending(&mut self, pending: bool) {
        self.set_hardfault_pending(pending);
    }

    pub fn set_hardfault_pending(&mut self, pending: bool) {
        self.hardfault_pending = pending;
    }

    pub fn hardfault_pending(&self) -> bool {
        self.hardfault_pending
    }

    pub fn pending_exceptions(&self) -> [u32; BANKS] {
        self.pending
    }

    pub fn enable_nrf_mpu(&mut self) {
        self.nrf_mpu = Some(mpu::Mpu::default());
    }

    pub fn enable_fpu(&mut self) {
        self.fpccr = Some(3 << 30);
    }

    pub fn has_fpu(&self) -> bool {
        self.fpccr.is_some() || self.nrf_mpu.is_some()
    }

    pub fn fpu_auto_preserve(&self) -> bool {
        self.fpccr.is_none_or(|value| value & (1 << 31) != 0)
    }

    pub fn priority_mask(&self) -> u8 {
        self.priority_mask
    }

    pub fn preemption_priority(&self, priority: u8) -> u8 {
        (u16::from(priority & self.priority_mask) & (0xff_u16 << (self.priority_group + 1))) as u8
    }

    pub(crate) fn has_wake_event(&self) -> bool {
        self.pending_event || self.hardfault_pending || self.reset_requested
            || self.enabled.iter().zip(self.pending).any(|(&enabled, pending)| enabled & pending != 0)
    }

    pub fn signal_pending_event(&mut self) {
        self.pending_event |= self.send_event_on_pending();
    }

    pub fn take_pending_event(&mut self) -> bool {
        std::mem::take(&mut self.pending_event)
    }

    fn pend(&mut self, bank: usize, bits: u32) {
        if bits & !self.pending[bank] != 0 {
            self.signal_pending_event();
        }
        self.pending[bank] |= bits;
    }

    pub fn fpu_access(&self) -> bool {
        self.cpacr & (0xf << 20) == 0xf << 20
    }

    pub(crate) fn set_fpu_access(&mut self, enabled: bool) {
        self.cpacr = if enabled { 0xf << 20 } else { 0 };
    }

    pub fn allows_unaligned_data(&self) -> bool {
        (self.has_fpu() || self.mpu.is_some()) && !self.unaligned_access_traps()
    }

    pub fn div_zero_trap(&self) -> bool {
        self.divide_by_zero_traps()
    }

    pub fn thumb_mpu_permits(
        &self,
        address: u32,
        width: usize,
        write: bool,
        execute: bool,
        unprivileged: bool,
        negative_priority: bool,
    ) -> bool {
        self.nrf_mpu
            .as_ref()
            .is_none_or(|mpu| mpu.permits(address, width, write, execute))
            && self.mpu.as_ref().is_none_or(|mpu| {
                mpu.permits(address, width, write, execute, unprivileged, negative_priority)
            })
    }

    pub(crate) fn mpu_privileged_background(&mut self, enabled: Option<bool>) -> Result<bool, String> {
        let mpu = self.mpu.as_mut().ok_or("CPU has no configurable MPU")?;
        if let Some(enabled) = enabled {
            mpu.enabled = (mpu.enabled & !4) | (u32::from(enabled) << 2);
        }
        Ok(mpu.enabled & 4 != 0)
    }

    pub fn configure_mpu(&mut self, count: u8) {
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

    pub fn set_external_lines(&mut self, lines: usize) {
        self.external_lines = lines.min(IRQ_LINES);
    }

    pub fn enable_extended_lines(&mut self) {
        self.set_external_lines(48);
    }

    pub fn set_line(&mut self, line: u8, high: bool, active: &ActiveExceptions) {
        if high && usize::from(line) < self.external_lines && !active.contains(16 + line) {
            self.pend(usize::from(line / 32), 1 << (line % 32));
        }
    }

    pub fn take_event(&mut self) -> bool {
        self.take_pending_event()
    }

    pub fn record_undefined_instruction(&mut self) {
        self.bus_fault_status |= 1 << 16; // UNDEFINSTR
    }

    pub fn record_secure_fault(&mut self, status: u32) {
        if let Some(security) = &mut self.security {
            security.sfsr |= status;
        }
    }

    pub fn force_hard_fault(&mut self) -> u8 {
        self.hfsr |= 1 << 30;
        3
    }

    pub fn priority(&self, exception: u8) -> u8 {
        match exception {
            4..=15 => self.system_priority[usize::from(exception - 4)],
            16..=255 => self.priority[usize::from(exception - 16)],
            _ => 0,
        }
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
        self.lockup = true;
    }

    pub fn leave_lockup(&mut self) {
        self.lockup = false;
    }

    pub fn ignores_negative_priority_data_fault(&self) -> bool {
        self.ccr & (1 << 8) != 0
    }

    pub fn record_stacking_fault(&mut self) {
        self.bus_fault_status |= 1 << 12; // STKERR
    }

    pub fn record_unstacking_fault(&mut self) {
        self.bus_fault_status |= 1 << 11;
    }

    pub fn record_invalid_exception_return(&mut self) {
        self.bus_fault_status |= 1 << 18; // INVPC
    }

    pub fn record_lazy_fault(&mut self) {
        self.bus_fault_status |= 1 << 13;
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

    pub fn secure_full_fp_frame(&self) -> bool {
        self.security.is_some() && self.fpccr.is_some_and(|fpccr| fpccr & (1 << 26) != 0)
    }

    pub fn reserve_lazy_fp(&mut self, address: u32, hardfault_ready: bool, full_frame: bool) {
        self.fpcar = address;
        self.lazy_fp_full_frame = full_frame;
        if let Some(fpccr) = &mut self.fpccr {
            *fpccr = (*fpccr & ((3 << 30) | (1 << 26))) | (u32::from(hardfault_ready) << 4) | 1;
        }
    }

    pub fn pending_lazy_fp(&self) -> Option<(u32, bool, bool)> {
        self.fpccr
            .filter(|value| value & 1 != 0)
            .map(|value| (self.fpcar, value & (1 << 4) != 0, self.lazy_fp_full_frame))
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
        self.bus_fault_status |= if invalid_state { 1 << 17 } else { 1 << 8 }; // INVSTATE or IBUSERR
    }

    pub fn record_fault(&mut self, address: Option<u32>) {
        if let Some(address) = address {
            self.bus_fault_address = address;
            self.bus_fault_status |= (1 << 9) | (1 << 15); // PRECISERR | BFARVALID
        } else {
            self.bus_fault_status |= 1 << 19; // NOCP
        }
    }

    pub fn fault_enabled(&self, exception: u8) -> bool {
        self.fault_enables & (1 << (exception + 12)) != 0
    }

    pub(crate) fn read_fault_status_part(&self, address: u32, width: usize) -> Option<u32> {
        let offset = address.checked_sub(BASE + 0xd28)?;
        if !matches!(width, 1 | 2 | 4) || offset == 0 && width == 4 || offset >= 4 {
            return None;
        }
        let status = u64::from(self.bus_fault_status | self.cfsr) | (u64::from(self.hfsr) << 32);
        let mask = u32::MAX >> (32 - width * 8);
        Some((status >> (offset * 8)) as u32 & mask)
    }

    pub fn read(&self, address: u32, width: usize) -> Option<u32> {
        if width == 4 {
            if let Some(security) = &self.security {
                if let Some(value) = security.read_sau(address) {
                    return Some(value);
                }
                if let Some(offset) = address.checked_sub(ITNS)
                    .filter(|offset| *offset < (BANKS * 4) as u32)
                {
                    return Some(security.nonsecure_irqs[offset as usize / 4]);
                }
                if (BASE + NS_SCS_OFFSET..BASE + NS_SCS_OFFSET + 0x1000).contains(&address) {
                    let secure_address = address - NS_SCS_OFFSET;
                    return match secure_address {
                        DHCSR | SFSR => Some(0),
                        VTOR => Some(security.vector_base_ns),
                        address if address == BASE + 0xd28 => {
                            Some(if security.bfhfnmins { self.bus_fault_status & 0xff00 } else { 0 })
                        }
                        address if address == BASE + 0xd38 => {
                            Some(if security.bfhfnmins { self.bus_fault_address } else { 0 })
                        }
                        _ => None,
                    };
                }
            }
            let offset = address.wrapping_sub(BASE);
            if offset == 4 {
                return Some((self.external_lines.div_ceil(32).saturating_sub(1)) as u32);
            }
            if self.mpu.is_some() {
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
            if let Some(mpu) = &self.nrf_mpu {
                if (0xd90..=0xda0).contains(&offset) {
                    return mpu.read(offset - 0xd90);
                }
                if offset == 0xdfc {
                    return Some(self.demcr);
                }
            }
            if (0xd90..=0xdb8).contains(&offset) {
                return self.mpu.as_ref().map(|mpu| mpu.read(offset));
            }
            if offset == 0xd00 && self.has_fpu() {
                return Some(0x410f_c241);
            }
            if offset == 0xf34 {
                return self.fpccr;
            }
            if address == DHCSR && self.has_debug_status() {
                return Some(u32::from(self.lockup) << 19);
            }
            if address == BASE + 0xd2c {
                return Some(self.hfsr);
            }
            if address == BASE + 0xf38 {
                return Some(self.fpcar);
            }
            if address == BASE + 0xd38 {
                return Some(self.bus_fault_address);
            }
            if address == BASE + 0xd34 {
                return Some(self.memory_fault_address);
            }
            if address == BASE + 0xd28 {
                return Some(self.bus_fault_status | self.cfsr);
            }
            if address == AIRCR {
                return Some(0xfa05_0000 | (u32::from(self.priority_group) << 8)
                    | (u32::from(self.bfhfnmins()) * BFHFNMINS));
            }
            if address == BASE + 0xd10 {
                return Some(self.scr);
            }
            if address == BASE + 0xd14 {
                return Some(self.ccr);
            }
            if address == BASE + 0xd24 {
                let active = [(4, 0), (5, 1), (3, 2), (6, 3), (2, 5),
                    (11, 7), (12, 8), (14, 10), (15, 11)];
                return Some(active.into_iter().fold(self.fault_enables | (u32::from(self.hardfault_pending) << 21), |value, (exception, bit)| {
                    let set = self.active.contains(exception) || self.active_system & (1 << exception) != 0;
                    value | (u32::from(set) << bit)
                }));
            }
            if address == CPACR {
                return Some(self.cpacr);
            }
            if address == VTOR {
                return Some(self.vector_base.unwrap_or(0));
            }
            let active_offset = address.wrapping_sub(BASE + 0x300);
            if active_offset < (self.external_lines.div_ceil(32) * 4) as u32
                && active_offset.is_multiple_of(4)
            {
                let first_line = (active_offset / 4) as usize * 32;
                return Some((first_line..(first_line + 32).min(self.external_lines))
                    .fold(0, |value, line| value | (u32::from(self.active.contains(16 + line as u8)) << (line % 32))));
            }
            if let Some((base, bank)) = Self::bank(address) {
                if bank * 32 >= self.external_lines {
                    return None;
                }
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
            &self.priority[..self.external_lines]
        };
        let bytes = priority.get(offset..offset + width)?;
        let mut word = [0; 4];
        word[..width].copy_from_slice(bytes);
        Some(u32::from_le_bytes(word))
    }

    pub fn write(&mut self, address: u32, width: usize, value: u32) -> bool {
        if width == 4 && let Some(security) = &mut self.security {
            if security.write_sau(address, value) {
                return true;
            }
            if let Some(offset) = address.checked_sub(ITNS)
                .filter(|offset| *offset < (BANKS * 4) as u32)
            {
                security.nonsecure_irqs[offset as usize / 4] = value;
                return true;
            }
            if address == VTOR + NS_SCS_OFFSET {
                security.vector_base_ns = value & !0x7f;
                return true;
            }
            if address == BASE + NS_SCS_OFFSET + 0xd28 {
                if security.bfhfnmins {
                    self.bus_fault_status &= !(value & 0xff00);
                }
                return true;
            }
        }
        let offset = address.wrapping_sub(BASE);
        if width == 4 && offset == 0xf00 && self.has_fpu() {
            let irq = (value & 0x1ff) as usize;
            if irq < self.external_lines {
                self.pend(irq / 32, 1 << (irq % 32));
            }
            return true;
        }

        if width == 4
            && let Some(mpu) = &mut self.nrf_mpu
        {
            if (0xd90..=0xda0).contains(&offset) {
                return mpu.write(offset - 0xd90, value);
            }
            if offset == 0xdfc {
                self.demcr = value;
                return true;
            }
        }
        if width == 4 && self.mpu.is_some() {
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
        if width == 4
            && offset == 0xf34
            && let Some(fpccr) = &mut self.fpccr
        {
            let writable = (3 << 30) | (u32::from(self.security.is_some()) << 26);
            *fpccr = (*fpccr & !writable) | (value & writable);
            return true;
        }
        if width == 4 && address == AIRCR {
            if value >> 16 == 0x05fa {
                self.priority_group = ((value >> 8) & 7) as u8;
                if let Some(security) = &mut self.security {
                    security.bfhfnmins = value & BFHFNMINS != 0;
                }
                self.reset_requested |= value & (1 << 2) != 0;
            }
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
        if width == 4 && address == DHCSR && self.has_debug_status() {
            return true;
        }
        if width == 4 && address == BASE + 0xd24 {
            self.fault_enables = value & (7 << 16);
            return true;
        }
        if width == 4 && address == BASE + 0xd28 {
            self.cfsr &= !value;
            self.bus_fault_status &= !value;
            return true;
        }
        if width == 4 && address == BASE + 0xd34 {
            self.memory_fault_address = value;
            return true;
        }
        if width == 4 && address == BASE + 0xd2c {
            self.hfsr &= !value;
            return true;
        }
        if width == 4 && address == CPACR {
            self.cpacr = value & (0xf << 20);
            return true;
        }
        if width == 4
            && let Some((base, bank)) = Self::bank(address)
        {
            if bank * 32 >= self.external_lines {
                return false;
            }
            let value = value & (u32::MAX >> (32 - (self.external_lines - bank * 32).min(32)));

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
            &mut self.priority[..self.external_lines]
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

    #[test]
    fn renode_neonkey_nvic_routes_second_bank_irqs() {
        let mut nvic = Nvic::new(0xf0, None);
        nvic.set_external_lines(32);
        assert!(!nvic.write(ENABLE + 4, 4, 1 << 5));
        nvic.enable_extended_lines();
        assert!(nvic.write(ENABLE + 4, 4, (1 << 5) | (1 << 9)));
        assert!(nvic.write(PRIORITY + 37, 1, 0x71));
        assert!(nvic.write(PRIORITY + 41, 1, 0x21));
        nvic.set_line(37, true, &ActiveExceptions::default());
        nvic.set_line(41, true, &ActiveExceptions::default());
        assert_eq!(nvic.read(SET_PENDING + 4, 4), Some((1 << 5) | (1 << 9)));
        assert_eq!(
            nvic.next_irq(&ActiveExceptions::default()),
            Some((41, 0x20))
        );
        nvic.acknowledge(41);
        assert_eq!(
            nvic.next_irq(&ActiveExceptions::default()),
            Some((37, 0x70))
        );
        nvic.reset();
        assert_eq!(nvic.read(ENABLE + 4, 4), Some(0));
    }
}
