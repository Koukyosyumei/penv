pub const BASE: u32 = 0xe000_e000;
pub const VTOR: u32 = BASE + 0xd08;
const CPACR: u32 = BASE + 0xd88;
const AIRCR: u32 = BASE + 0xd0c;
const SCR: u32 = BASE + 0xd10;
const ENABLE: u32 = BASE + 0x100;
const DISABLE: u32 = BASE + 0x180;
const SET_PENDING: u32 = BASE + 0x200;
const CLEAR_PENDING: u32 = BASE + 0x280;
const PRIORITY: u32 = BASE + 0x400;
const SYSTEM_PRIORITY: u32 = BASE + 0xd1c;
const SYSTEM_PRIORITY_END: u32 = SYSTEM_PRIORITY + 7;
const PRIORITY_END: u32 = PRIORITY + 63;

const ENABLE_HIGH: u32 = ENABLE + 4;
const DISABLE_HIGH: u32 = DISABLE + 4;
const SET_PENDING_HIGH: u32 = SET_PENDING + 4;
const CLEAR_PENDING_HIGH: u32 = CLEAR_PENDING + 4;

pub struct Nvic {
    vector_base: Option<u32>,
    cortex_m4: bool,
    enabled: u64,
    sleep_control: u32,
    reset_requested: bool,
    pending: u64,
    priority: [u8; 64],
    system_priority: [u8; 8],
    priority_mask: u8,
    pub uart_irq: Option<u8>,
}

impl Nvic {
    pub fn new(priority_mask: u8, uart_irq: Option<u8>, cortex_m4: bool) -> Self {
        Self {
            vector_base: None,
            cortex_m4,
            enabled: 0,
            sleep_control: 0,
            reset_requested: false,
            pending: 0,
            priority: [0; 64],
            system_priority: [0; 8],
            priority_mask,
            uart_irq,
        }
    }

    pub fn ranges(cortex_m4: bool) -> impl Iterator<Item = (u32, u32)> {
        [
            Some((ENABLE, PRIORITY + if cortex_m4 { 64 } else { 32 })),
            Some((VTOR, VTOR + 4)),
            Some((AIRCR, AIRCR + 4)),
            Some((SCR, SCR + 4)),
            Some((SYSTEM_PRIORITY, SYSTEM_PRIORITY_END + 1)),
            cortex_m4.then_some((CPACR, CPACR + 4)),
        ]
        .into_iter()
        .flatten()
    }

    pub fn overlaps(&self, start: u64, end: u64) -> bool {
        Self::ranges(self.cortex_m4)
            .any(|(low, high)| start < u64::from(high) && end > u64::from(low))
    }

    pub fn vector_base(&self) -> Option<u32> {
        self.vector_base
    }

    pub fn set_vector_base(&mut self, address: u32) {
        self.vector_base = Some(address);
    }

    pub fn set_uart_level(&mut self, high: bool, active: u128) {
        if let Some(line) = self.uart_irq {
            self.set_line(line, high, active);
        }
    }

    pub fn set_line(&mut self, line: u8, high: bool, active: u128) {
        if (self.cortex_m4 || line < 32) && high && active & (1 << (16 + line)) == 0 {
            self.pending |= 1 << line;
        }
    }

    pub fn take_reset_request(&mut self) -> bool {
        std::mem::take(&mut self.reset_requested)
    }

    pub fn reset(&mut self) {
        *self = Self::new(self.priority_mask, self.uart_irq, self.cortex_m4);
    }

    pub fn sleep_on_exit(&self) -> bool {
        self.sleep_control & (1 << 1) != 0
    }

    pub fn send_event_on_pending(&self) -> bool {
        self.sleep_control & (1 << 4) != 0
    }

    pub fn pending_exceptions(&self) -> u128 {
        u128::from(self.pending) << 16
    }

    pub fn priority(&self, exception: u8) -> u8 {
        match exception {
            11 => self.system_priority[3],
            14 => self.system_priority[6],
            15 => self.system_priority[7],
            16..=79 => self.priority[usize::from(exception - 16)],
            _ => 0,
        }
    }

    pub fn next_irq(&self, active: u128) -> Option<(u8, u8)> {
        let mut candidates = self.enabled & self.pending & !((active >> 16) as u64);
        let mut selected = None;
        while candidates != 0 {
            let line = candidates.trailing_zeros() as u8;
            candidates &= candidates - 1;
            let priority = self.priority[usize::from(line)];
            if selected.is_none_or(|(_, best)| priority < best) {
                selected = Some((line, priority));
            }
        }
        selected
    }

    fn priority_registers(&self, address: u32) -> Option<(bool, usize)> {
        match address {
            PRIORITY..=PRIORITY_END if self.cortex_m4 || address < PRIORITY + 32 => {
                Some((false, (address - PRIORITY) as usize))
            }
            SYSTEM_PRIORITY..=SYSTEM_PRIORITY_END => {
                Some((true, (address - SYSTEM_PRIORITY) as usize))
            }
            _ => None,
        }
    }

    pub fn acknowledge(&mut self, line: u8) {
        self.pending &= !(1 << line);
    }

    pub fn read(&self, address: u32, width: usize) -> Option<u32> {
        Some(match (address, width) {
            (AIRCR, 4) => 0xfa05_0000,
            (CPACR, 4) if self.cortex_m4 => 0,
            (SCR, 4) => self.sleep_control,
            (VTOR, 4) => self.vector_base.unwrap_or(0),
            (ENABLE | DISABLE, 4) => self.enabled as u32,
            (SET_PENDING | CLEAR_PENDING, 4) => self.pending as u32,
            (ENABLE_HIGH | DISABLE_HIGH, 4) if self.cortex_m4 => (self.enabled >> 32) as u32,
            (SET_PENDING_HIGH | CLEAR_PENDING_HIGH, 4) if self.cortex_m4 => {
                (self.pending >> 32) as u32
            }
            (_, 1 | 4) => {
                let (system, offset) = self.priority_registers(address)?;
                let priority = if system {
                    &self.system_priority[..]
                } else {
                    &self.priority[..]
                };
                let bytes = priority.get(offset..offset + width)?;
                let mut word = [0; 4];
                word[..width].copy_from_slice(bytes);
                u32::from_le_bytes(word)
            }
            _ => return None,
        })
    }

    pub fn write(&mut self, address: u32, width: usize, value: u32) -> bool {
        if !self.cortex_m4
            && matches!(
                address,
                ENABLE_HIGH | DISABLE_HIGH | SET_PENDING_HIGH | CLEAR_PENDING_HIGH
            )
        {
            return false;
        }
        match (address, width) {
            (CPACR, 4) if self.cortex_m4 && value & 0x00f0_0000 == 0 => {}
            (AIRCR, 4) => {
                self.reset_requested |= value >> 16 == 0x05fa && value & (1 << 2) != 0;
            }
            (SCR, 4) => self.sleep_control = value & 0x16,
            (ENABLE | ENABLE_HIGH, 4) => {
                self.enabled |= u64::from(value) << ((address - ENABLE) * 8)
            }
            (DISABLE | DISABLE_HIGH, 4) => {
                self.enabled &= !(u64::from(value) << ((address - DISABLE) * 8))
            }
            (SET_PENDING | SET_PENDING_HIGH, 4) => {
                self.pending |= u64::from(value) << ((address - SET_PENDING) * 8)
            }
            (CLEAR_PENDING | CLEAR_PENDING_HIGH, 4) => {
                self.pending &= !(u64::from(value) << ((address - CLEAR_PENDING) * 8))
            }
            (_, 1 | 4) => {
                let Some((system, offset)) = self.priority_registers(address) else {
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
                for (index, (priority, value)) in
                    bytes.iter_mut().zip(value.to_le_bytes()).enumerate()
                {
                    if !system || matches!(offset + index, 3 | 6 | 7) {
                        *priority = value & self.priority_mask;
                    }
                }
            }
            _ => return false,
        }
        true
    }
}
