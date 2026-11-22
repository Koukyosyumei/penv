use std::ops::Range;

const KEY1: u32 = 0x4567_0123;
const KEY2: u32 = 0xcdef_89ab;
const PROGRAM: u32 = 1;
const PAGE_ERASE: u32 = 1 << 1;
const MASS_ERASE: u32 = 1 << 2;
const START: u32 = 1 << 6;
const LOCK: u32 = 1 << 7;
const PROGRAM_ERROR: u32 = 1 << 2;
const PROTECTION_ERROR: u32 = 1 << 4;
const COMPLETE: u32 = 1 << 5;

pub struct Stm32F0Flash {
    pub base: u32,
    bank: Range<u32>,
    access: u32,
    control: u32,
    status: u32,
    address: u32,
    first_key: bool,
}

impl Stm32F0Flash {
    pub const BASE: u32 = 0x4002_2000;
    pub const SIZE: u32 = 0x100;
    const PAGE_SIZE: u32 = 2048;

    pub fn new(bank: Range<u32>) -> Self {
        Self {
            base: Self::BASE,
            bank,
            access: 0,
            control: LOCK,
            status: 0,
            address: 0,
            first_key: false,
        }
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    pub fn contains_bank(&self, address: u32) -> bool {
        self.bank.contains(&address)
    }

    pub fn read(&self, address: u32) -> u32 {
        match address - self.base {
            0 => self.access | ((self.access & 0x10) << 1),
            0x0c => self.status,
            0x10 => self.control,
            0x14 => self.address,
            0x20 => u32::MAX,
            _ => 0,
        }
    }

    pub fn write(&mut self, address: u32, value: u32) -> Option<Range<u32>> {
        match address - self.base {
            0 => self.access = value & 0x17,
            4 if self.control & LOCK != 0 => {
                if self.first_key && value == KEY2 {
                    self.control &= !LOCK;
                }
                self.first_key = value == KEY1;
            }
            0x0c => self.status &= !(value & (PROGRAM_ERROR | PROTECTION_ERROR | COMPLETE)),
            0x10 if self.control & LOCK == 0 => {
                self.control = value & (PROGRAM | PAGE_ERASE | MASS_ERASE | LOCK | 0x1400);
                if value & START != 0 && value & LOCK == 0 {
                    return self.erase_range();
                }
            }
            0x14 if self.control & LOCK == 0 => self.address = value,
            _ => {}
        }
        None
    }

    fn erase_range(&mut self) -> Option<Range<u32>> {
        match self.control & (PAGE_ERASE | MASS_ERASE) {
            MASS_ERASE => Some(self.bank.clone()),
            PAGE_ERASE if self.bank.contains(&self.address) => {
                let start = self.address & !(Self::PAGE_SIZE - 1);
                Some(start..start + Self::PAGE_SIZE)
            }
            _ => {
                self.status |= PROGRAM_ERROR;
                None
            }
        }
    }

    pub fn program(&mut self, address: u32, width: usize, old: u16, value: u16) -> Option<u16> {
        if self.control & LOCK != 0 || self.control & PROGRAM == 0 {
            self.status |= PROTECTION_ERROR;
        } else if width != 2 || address & 1 != 0 || old & value != value {
            self.status |= PROGRAM_ERROR;
        } else {
            self.finish(true);
            return Some(value);
        }
        None
    }

    pub fn finish(&mut self, success: bool) {
        self.status |= if success { COMPLETE } else { PROGRAM_ERROR };
    }
}
