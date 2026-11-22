use std::ops::Range;

use crate::stm32_flash::KeyLock;

const SECTOR_SIZE: u32 = 0x20000;
const EOP: u32 = 1 << 16;
const PROGRAM_SEQUENCE: u32 = 1 << 18;
const INCONSISTENCY: u32 = 1 << 21;
const OPERATION_ERROR: u32 = 1 << 22;
const INTERRUPTS: u32 = EOP | PROGRAM_SEQUENCE | INCONSISTENCY | OPERATION_ERROR;
const LOCK: u32 = 1;
const PROGRAM: u32 = 1 << 1;
const CONTROL_MASK: u32 = 0x1fef_873f;

struct Bank {
    range: Range<u32>,
    lock: KeyLock,
    control: u32,
    status: u32,
    protection_current: u8,
    protection_program: u8,
    written: u32,
    next_address: u32,
}

impl Bank {
    fn new(range: Range<u32>) -> Self {
        Self {
            range,
            lock: KeyLock::new([0x4567_0123, 0xcdef_89ab]),
            control: 0x30,
            status: 0,
            protection_current: 0xff,
            protection_program: 0xff,
            written: 0,
            next_address: 0,
        }
    }

    fn read(&self, register: u32) -> u32 {
        match register {
            0x0c => self.control | u32::from(self.lock.locked),
            0x10 => self.status | (u32::from(self.written != 0) << 1),
            0x38 => u32::from(self.protection_current),
            0x3c => u32::from(self.protection_program),
            _ => 0,
        }
    }

    fn control(&mut self, value: u32) -> Option<Range<u32>> {
        if self.lock.locked {
            return None;
        }
        if (self.control ^ value) & PROGRAM != 0 {
            self.written = 0;
        }
        self.control = value & CONTROL_MASK & !LOCK;
        if value & LOCK != 0 {
            self.lock.lock();
        }
        if value & (1 << 6) != 0 && self.control & PROGRAM != 0 {
            self.finish_program();
        }
        if value & (1 << 7) == 0 {
            return None;
        }
        if value & (1 << 3) != 0 {
            return self.erase(self.range.clone());
        }
        if value & (1 << 2) != 0 {
            let sector = (value >> 8) & 7;
            if self.protection_current & (1 << sector) == 0 {
                self.status |= OPERATION_ERROR;
                return None;
            }
            let offset = sector * SECTOR_SIZE;
            if offset + SECTOR_SIZE > self.range.end - self.range.start {
                self.status |= OPERATION_ERROR;
                return None;
            }
            let start = self.range.start + offset;
            return self.erase(start..start + SECTOR_SIZE);
        }
        None
    }

    fn erase(&mut self, range: Range<u32>) -> Option<Range<u32>> {
        if range.end > self.range.end {
            self.status |= OPERATION_ERROR;
            return None;
        }
        self.status |= EOP;
        Some(range)
    }

    fn note_program(&mut self, address: u32, width: u32) {
        if self.control & PROGRAM == 0 || self.status & INCONSISTENCY != 0 {
            self.status |= PROGRAM_SEQUENCE;
            return;
        }
        if self.written != 0 && address != self.next_address {
            self.status |= INCONSISTENCY;
            return;
        }
        self.next_address = address.wrapping_add(width);
        self.written += width;
        if self.written >= 32 {
            self.finish_program();
        }
    }

    fn finish_program(&mut self) {
        self.written = 0;
        self.status |= EOP;
    }
}

pub struct FlashController {
    pub base: u32,
    irq: u8,
    access: u32,
    banks: [Bank; 2],
    option_lock: KeyLock,
    options: u32,
    option_current: u32,
    option_program: u32,
}

impl FlashController {
    pub const SIZE: u32 = 0x1000;

    pub fn new(base: u32, irq: u8, banks: [Range<u32>; 2]) -> Self {
        Self {
            base,
            irq,
            access: 0x37,
            banks: banks.map(Bank::new),
            option_lock: KeyLock::new([0x0819_2a3b, 0x4c5d_6e7f]),
            options: 0,
            option_current: 0x0406_aaf0,
            option_program: 0x0406_aaf0,
        }
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    pub fn interrupt(&self) -> Option<u8> {
        self.banks
            .iter()
            .any(|bank| bank.control & bank.status & INTERRUPTS != 0)
            .then_some(self.irq)
    }

    pub fn read(&self, address: u32) -> u32 {
        let offset = address - self.base;
        match offset {
            0 => self.access,
            0x18 => self.options | u32::from(self.option_lock.locked),
            0x1c => self.option_current,
            0x20 => self.option_program,
            offset if offset < 0x200 => self.banks[(offset / 0x100) as usize].read(offset & 0xff),
            _ => 0,
        }
    }

    pub fn write(&mut self, address: u32, value: u32) -> [Option<Range<u32>>; 2] {
        let offset = address - self.base;
        match offset {
            0 => self.access = value & 0x3f,
            0x08 => self.option_lock.consume(value),
            0x18 => {
                self.options = value & 0xc000_0000;
                if value & 1 != 0 {
                    self.option_lock.lock();
                }
                if !self.option_lock.locked && value & 2 != 0 {
                    self.option_current = self.option_program;
                    for bank in &mut self.banks {
                        bank.protection_current = bank.protection_program;
                    }
                }
                if !self.option_lock.locked && value & (1 << 4) != 0 {
                    return self
                        .banks
                        .each_mut()
                        .map(|bank| bank.erase(bank.range.clone()));
                }
            }
            0x20 if !self.option_lock.locked => self.option_program = value,
            offset if offset < 0x200 => {
                let index = (offset / 0x100) as usize;
                let bank = &mut self.banks[index];
                match offset & 0xff {
                    0x04 => bank.lock.consume(value),
                    0x0c => {
                        let mut result = [None, None];
                        result[index] = bank.control(value);
                        return result;
                    }
                    0x14 => bank.status &= !(value & INTERRUPTS),
                    0x3c if !self.option_lock.locked => bank.protection_program = value as u8,
                    _ => {}
                }
            }
            _ => {}
        }
        [None, None]
    }

    pub fn note_program(&mut self, address: u32, width: usize) {
        if self.banks.iter().any(|bank| bank.control & PROGRAM != 0)
            && let Some(bank) = self
                .banks
                .iter_mut()
                .find(|bank| bank.range.contains(&address))
        {
            bank.note_program(address, width as u32);
        }
    }
}
