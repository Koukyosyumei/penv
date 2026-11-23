pub const BASE: u32 = 0xe000_e000;
pub const HARDFAULT: u8 = 3;
pub const VTOR: u32 = BASE + 0xd08;
const AIRCR: u32 = BASE + 0xd0c;
const SCR: u32 = BASE + 0xd10;
const ENABLE: u32 = BASE + 0x100;
const DISABLE: u32 = BASE + 0x180;
const SET_PENDING: u32 = BASE + 0x200;
const CLEAR_PENDING: u32 = BASE + 0x280;
const PRIORITY: u32 = BASE + 0x400;
const SYSTEM_PRIORITY: u32 = BASE + 0xd1c;
const SYSTEM_PRIORITY_END: u32 = SYSTEM_PRIORITY + 7;
const PRIORITY_END: u32 = PRIORITY + 31;

pub struct Nvic {
    vector_base: Option<u32>,
    enabled: u32,
    sleep_control: u32,
    reset_requested: bool,
    hardfault_pending: bool,
    pending: u32,
    priority: [u8; 32],
    system_priority: [u8; 8],
    priority_mask: u8,
    pub uart_irq: Option<u8>,
}

impl Nvic {
    pub fn new(priority_mask: u8, uart_irq: Option<u8>) -> Self {
        Self {
            vector_base: None,
            enabled: 0,
            sleep_control: 0,
            reset_requested: false,
            hardfault_pending: false,
            pending: 0,
            priority: [0; 32],
            system_priority: [0; 8],
            priority_mask,
            uart_irq,
        }
    }

    pub const RANGES: [(u32, u32); 5] = [
        (ENABLE, PRIORITY_END + 1),
        (VTOR, VTOR + 4),
        (AIRCR, AIRCR + 4),
        (SCR, SCR + 4),
        (SYSTEM_PRIORITY, SYSTEM_PRIORITY_END + 1),
    ];

    pub fn overlaps(start: u64, end: u64) -> bool {
        Self::RANGES
            .into_iter()
            .any(|(low, high)| start < u64::from(high) && end > u64::from(low))
    }

    pub fn vector_base(&self) -> Option<u32> {
        self.vector_base
    }

    pub fn set_vector_base(&mut self, address: u32) {
        self.vector_base = Some(address);
    }

    pub fn set_uart_level(&mut self, high: bool, active: u64) {
        if let Some(line) = self.uart_irq {
            self.set_line(line, high, active);
        }
    }

    pub fn set_line(&mut self, line: u8, high: bool, active: u64) {
        if high && active & (1 << (16 + line)) == 0 {
            self.pending |= 1 << line;
        }
    }

    pub fn take_reset_request(&mut self) -> bool {
        std::mem::take(&mut self.reset_requested)
    }

    pub fn reset(&mut self) {
        *self = Self::new(self.priority_mask, self.uart_irq);
    }

    pub fn sleep_on_exit(&self) -> bool {
        self.sleep_control & (1 << 1) != 0
    }

    pub fn send_event_on_pending(&self) -> bool {
        self.sleep_control & (1 << 4) != 0
    }

    pub fn set_hardfault_pending(&mut self, pending: bool) {
        self.hardfault_pending = pending;
    }

    pub fn pending_exceptions(&self) -> u64 {
        (u64::from(self.pending) << 16) | (u64::from(self.hardfault_pending) << HARDFAULT)
    }

    pub fn priority(&self, exception: u8) -> u8 {
        match exception {
            11 => self.system_priority[3],
            14 => self.system_priority[6],
            15 => self.system_priority[7],
            16..=47 => self.priority[usize::from(exception - 16)],
            _ => 0,
        }
    }

    pub fn next_irq(&self, active: u64) -> Option<(u8, u8)> {
        let mut candidates = self.enabled & self.pending & !((active >> 16) as u32);
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

    fn priority_registers(address: u32) -> Option<(bool, usize)> {
        match address {
            PRIORITY..=PRIORITY_END => Some((false, (address - PRIORITY) as usize)),
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
            (SCR, 4) => self.sleep_control,
            (VTOR, 4) => self.vector_base.unwrap_or(0),
            (ENABLE | DISABLE, 4) => self.enabled,
            (SET_PENDING | CLEAR_PENDING, 4) => self.pending,
            (_, 1 | 4) => {
                let (system, offset) = Self::priority_registers(address)?;
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
        match (address, width) {
            (AIRCR, 4) => {
                self.reset_requested |= value >> 16 == 0x05fa && value & (1 << 2) != 0;
            }
            (SCR, 4) => self.sleep_control = value & 0x16,
            (ENABLE, 4) => self.enabled |= value,
            (DISABLE, 4) => self.enabled &= !value,
            (SET_PENDING, 4) => self.pending |= value,
            (CLEAR_PENDING, 4) => self.pending &= !value,
            (_, 1 | 4) => {
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
