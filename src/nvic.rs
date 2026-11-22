pub const BASE: u32 = 0xe000_e000;
pub const VTOR: u32 = BASE + 0xd08;
pub const IRQ_LINES: usize = 240;
const BANKS: usize = IRQ_LINES.div_ceil(32);
const ENABLE: u32 = BASE + 0x100;
const DISABLE: u32 = BASE + 0x180;
const SET_PENDING: u32 = BASE + 0x200;
const CLEAR_PENDING: u32 = BASE + 0x280;
const PRIORITY: u32 = BASE + 0x400;
const SYSTEM_PRIORITY: u32 = BASE + 0xd1c;
const SYSTEM_PRIORITY_END: u32 = SYSTEM_PRIORITY + 7;
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
    system_priority: [u8; 8],
    priority_mask: u8,
    pub uart_irq: Option<u8>,
}

impl Nvic {
    pub fn new(priority_mask: u8, uart_irq: Option<u8>) -> Self {
        Self {
            vector_base: None,
            enabled: [0; BANKS],
            pending: [0; BANKS],
            priority: [0; IRQ_LINES],
            system_priority: [0; 8],
            priority_mask,
            uart_irq,
        }
    }

    pub const RANGES: [(u32, u32); 3] = [
        (ENABLE, PRIORITY_END + 1),
        (VTOR, VTOR + 4),
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

    pub fn set_uart_level(&mut self, high: bool, active: &ActiveExceptions) {
        if let Some(line) = self.uart_irq {
            self.set_line(line, high, active);
        }
    }

    pub fn set_line(&mut self, line: u8, high: bool, active: &ActiveExceptions) {
        if high && usize::from(line) < IRQ_LINES && !active.contains(16 + line) {
            self.pending[usize::from(line / 32)] |= 1 << (line % 32);
        }
    }

    pub fn priority(&self, exception: u8) -> u8 {
        match exception {
            11 => self.system_priority[3],
            14 => self.system_priority[6],
            15 => self.system_priority[7],
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

    pub fn read(&self, address: u32, width: usize) -> Option<u32> {
        if width == 4 {
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
                SET_PENDING => self.pending[bank] |= value,
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
            if !system || matches!(offset + index, 3 | 6 | 7) {
                *priority = value & self.priority_mask;
            }
        }
        true
    }
}
