use std::ops::Range;

const KEY1: u32 = 0x4567_0123;
const KEY2: u32 = 0xcdef_89ab;
const PROGRAM: u32 = 1;
const PAGE_ERASE: u32 = 1 << 1;
const MASS_ERASE: u32 = 1 << 2;
const OPTION_PROGRAM: u32 = 1 << 4;
const OPTION_ERASE: u32 = 1 << 5;
const OPTION_WRITE_ENABLE: u32 = 1 << 9;
const ERROR_IRQ: u32 = 1 << 10;
const COMPLETE_IRQ: u32 = 1 << 12;
const OPTION_RELOAD: u32 = 1 << 13;
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
    first_option_key: bool,
    reload_requested: bool,
    option_bytes: [u16; 8],
    options_loaded: [u8; 8],
}

impl Stm32F0Flash {
    pub const BASE: u32 = 0x4002_2000;
    pub const SIZE: u32 = 0x100;
    pub const OPTION_BASE: u32 = 0x1fff_f800;
    pub const OPTION_SIZE: u32 = 16;
    pub const IRQ: u8 = 3;
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
            first_option_key: false,
            reload_requested: false,
            option_bytes: [
                0x55aa, 0x00ff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff, 0xffff,
            ],
            options_loaded: [0xaa, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff],
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
            0x1c => {
                let rdp = match self.options_loaded[0] {
                    0xaa => 0,
                    0xcc => 2 << 1,
                    _ => 1 << 1,
                };
                rdp | (u32::from_le_bytes(self.options_loaded[..4].try_into().unwrap())
                    & 0xffff_7700)
            }
            0x20 => self.write_protection(),
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
            8 if self.control & LOCK == 0 => {
                if self.first_option_key && value == KEY2 {
                    self.control |= OPTION_WRITE_ENABLE;
                }
                self.first_option_key = value == KEY1;
            }
            0x0c => self.status &= !(value & (PROGRAM_ERROR | PROTECTION_ERROR | COMPLETE)),
            0x10 if self.control & LOCK == 0 => {
                let option_enable = value & self.control & OPTION_WRITE_ENABLE;
                self.control = (value
                    & (PROGRAM
                        | PAGE_ERASE
                        | MASS_ERASE
                        | OPTION_PROGRAM
                        | OPTION_ERASE
                        | LOCK
                        | ERROR_IRQ
                        | COMPLETE_IRQ))
                    | option_enable;
                if value & LOCK != 0 {
                    self.control &= !OPTION_WRITE_ENABLE;
                    self.first_option_key = false;
                } else if value & OPTION_RELOAD != 0 {
                    self.reload_requested = true;
                } else if value & START != 0 {
                    if self.control & OPTION_ERASE != 0 {
                        if option_enable != 0 {
                            self.option_bytes.fill(u16::MAX);
                            self.finish(true);
                        } else {
                            self.status |= PROTECTION_ERROR;
                        }
                    } else {
                        return self.erase_range();
                    }
                }
            }
            0x14 if self.control & LOCK == 0 => self.address = value,
            _ => {}
        }
        None
    }

    fn erase_range(&mut self) -> Option<Range<u32>> {
        match self.control & (PAGE_ERASE | MASS_ERASE) {
            MASS_ERASE if self.write_protection() == u32::MAX => Some(self.bank.clone()),
            MASS_ERASE => {
                self.status |= PROTECTION_ERROR;
                None
            }
            PAGE_ERASE if self.bank.contains(&self.address) => {
                if self.protected(self.address) {
                    self.status |= PROTECTION_ERROR;
                    return None;
                }
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
        if self.control & LOCK != 0 || self.control & PROGRAM == 0 || self.protected(address) {
            self.status |= PROTECTION_ERROR;
        } else if width != 2 || address & 1 != 0 || old & value != value {
            self.status |= PROGRAM_ERROR;
        } else {
            self.finish(true);
            return Some(value);
        }
        None
    }

    pub fn contains_options(&self, address: u32, width: usize) -> bool {
        (address.wrapping_sub(Self::OPTION_BASE) as usize)
            .checked_add(width)
            .is_some_and(|end| end <= Self::OPTION_SIZE as usize)
    }

    pub fn read_options(&self, address: u32, width: usize) -> u32 {
        let offset = (address - Self::OPTION_BASE) as usize;
        let mut value = 0;
        for lane in 0..width {
            let byte = offset + lane;
            let word = self.option_bytes[byte / 2];
            value |= u32::from(word.to_le_bytes()[byte % 2]) << (lane * 8);
        }
        value
    }

    pub fn program_option(&mut self, address: u32, width: usize, value: u32) {
        let required = OPTION_WRITE_ENABLE | OPTION_PROGRAM;
        if self.control & (LOCK | required) != required {
            self.status |= PROTECTION_ERROR;
            return;
        }
        if width != 2 || address & 1 != 0 {
            self.status |= PROGRAM_ERROR;
            return;
        }
        let option = &mut self.option_bytes[((address - Self::OPTION_BASE) / 2) as usize];
        if *option != u16::MAX {
            self.status |= PROTECTION_ERROR;
            return;
        }
        let value = value as u8;
        *option = u16::from_le_bytes([value, !value]);
        self.finish(true);
    }

    pub fn take_reload_request(&mut self) -> bool {
        std::mem::take(&mut self.reload_requested)
    }

    pub fn reset(&mut self, option_reload: bool) {
        self.access = 0;
        self.control = LOCK;
        self.status = 0;
        self.address = 0;
        self.first_key = false;
        self.first_option_key = false;
        self.reload_requested = false;
        if option_reload {
            self.reload_options();
        }
    }

    fn reload_options(&mut self) {
        self.options_loaded = self.option_bytes.map(|word| word as u8);
    }

    fn write_protection(&self) -> u32 {
        u32::from_le_bytes(self.options_loaded[4..].try_into().unwrap())
    }

    fn protected(&self, address: u32) -> bool {
        let sector = (address - self.bank.start) / (Self::PAGE_SIZE * 2);
        self.write_protection() & (1 << sector) == 0
    }

    pub fn interrupt_pending(&self) -> bool {
        (self.control & COMPLETE_IRQ != 0 && self.status & COMPLETE != 0)
            || (self.control & ERROR_IRQ != 0
                && self.status & (PROGRAM_ERROR | PROTECTION_ERROR) != 0)
    }

    pub fn finish(&mut self, success: bool) {
        self.status |= if success { COMPLETE } else { PROGRAM_ERROR };
    }
}

pub struct KeyLock {
    keys: [u32; 2],
    pub locked: bool,
    first_key: bool,
}

impl KeyLock {
    pub fn new(keys: [u32; 2]) -> Self {
        Self {
            keys,
            locked: true,
            first_key: false,
        }
    }

    pub fn consume(&mut self, value: u32) {
        if !self.locked {
            return;
        }
        if self.first_key && value == self.keys[1] {
            self.locked = false;
        }
        self.first_key = value == self.keys[0];
    }

    pub fn lock(&mut self) {
        self.locked = true;
        self.first_key = false;
    }
}
