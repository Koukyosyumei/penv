use std::ops::Range;

use crate::stm32_flash::KeyLock;

const ACR_MASK: u32 = 0x1f0f;
const CR_LOCK: u32 = 1 << 31;
const CR_START: u32 = 1 << 16;
const CR_MASS_ERASE: u32 = 1 << 2;
const CR_SECTOR_ERASE: u32 = 1 << 1;
const SR_EOP: u32 = 1;
const SR_WRPERR: u32 = 1 << 4;
const ALL_SECTORS: u32 = 0x0fff;
const OPT_LOCK: u32 = 1;
const OPT_START: u32 = 1 << 1;
const OPT_MASK: u32 = 0x0fff_ffec;

pub struct F4Flash {
    base: u32,
    option_base: u32,
    option_size: u32,
    flash: Range<u32>,
    access: u32,
    status: u32,
    control: u32,
    options: u32,
    option_bytes: u32,
    key: KeyLock,
    option_key: KeyLock,
}

impl F4Flash {
    pub const MODEL: &str = "MTD.STM32F4_FlashController";
    pub const SIZE: u32 = 0x400;
    pub const BANK_SIZE: u32 = 0x10_0000;

    pub fn new(base: u32, option_base: u32, option_size: u32, flash: Range<u32>) -> Self {
        Self {
            base,
            option_base,
            option_size,
            flash,
            access: 0,
            status: 0,
            control: 0,
            options: 0x0fff_aaec,
            option_bytes: 0x0fff_aaec,
            key: KeyLock::new([0x4567_0123, 0xcdef_89ab]),
            option_key: KeyLock::new([0x0819_2a3b, 0x4c5d_6e7f]),
        }
    }

    pub fn reset(&mut self) {
        self.access = 0;
        self.status = 0;
        self.control = 0;
        self.options = self.option_bytes;
        self.key.lock();
        self.option_key.lock();
    }

    pub fn mappings(&self) -> [(u32, u32); 2] {
        [
            (self.base, Self::SIZE),
            (self.option_base, self.option_size),
        ]
    }

    pub fn read(&self, address: u32, width: usize) -> Option<u32> {
        let offset = address.wrapping_sub(self.base);
        let option_offset = address.wrapping_sub(self.option_base);
        let value = if offset < Self::SIZE {
            match offset & !3 {
                0x00 => self.access,
                0x0c => self.status,
                0x10 => self.control | if self.key.locked { CR_LOCK } else { 0 },
                0x14 => self.options | u32::from(self.option_key.locked),
                _ => 0,
            }
        } else if option_offset < self.option_size {
            match option_offset & !3 {
                0x00 => self.option_bytes & 0xffff,
                0x08 => self.option_bytes >> 16,
                _ => 0,
            }
        } else {
            return None;
        };
        Some((value >> ((address & 3) * 8)) & (u32::MAX >> (32 - width * 8)))
    }

    pub fn contains(&self, address: u32) -> bool {
        self.mappings()
            .into_iter()
            .any(|(base, size)| address.wrapping_sub(base) < size)
    }

    pub fn write(&mut self, address: u32, width: usize, value: u32) -> Option<Range<u32>> {
        let offset = address.wrapping_sub(self.base);
        if offset >= Self::SIZE {
            return None;
        }
        let shift = (offset & 3) * 8;
        let bits = value << shift;
        let mask = (u32::MAX >> (32 - width * 8)) << shift;
        match offset & !3 {
            0x00 => self.access = (self.access & !mask) | (bits & mask & ACR_MASK),
            0x04 if width == 4 => self.key.consume(value),
            0x08 if width == 4 => self.option_key.consume(value),
            0x0c => self.status &= !(bits & mask),
            0x10 if !self.key.locked => {
                self.control = (self.control & !mask) | (bits & mask & 0x0300_037f);
                if bits & CR_LOCK != 0 {
                    self.key.lock();
                }
                if bits & CR_START != 0 {
                    return self.erase();
                }
            }
            0x14 if !self.option_key.locked => {
                self.options = (self.options & !mask) | (bits & mask & OPT_MASK);
                if bits & OPT_START != 0 {
                    self.option_bytes = self.options;
                }
                if bits & OPT_LOCK != 0 {
                    self.option_key.lock();
                }
            }
            _ => {}
        }
        None
    }

    fn erase(&mut self) -> Option<Range<u32>> {
        let (range, sectors) = if self.control & CR_MASS_ERASE != 0 {
            (self.flash.clone(), ALL_SECTORS)
        } else if self.control & CR_SECTOR_ERASE != 0 {
            let sector = (self.control >> 3) & 15;
            (self.sector(sector)?, 1 << sector)
        } else {
            return None;
        };
        if (self.option_bytes >> 16) & sectors != sectors {
            self.status |= SR_WRPERR;
            return None;
        }
        self.status |= SR_EOP;
        Some(range)
    }

    fn sector(&self, number: u32) -> Option<Range<u32>> {
        let (offset, size) = match number {
            0..=3 => (number * 0x4000, 0x4000),
            4 => (0x10000, 0x10000),
            5..=11 => ((number - 4) * 0x20000, 0x20000),
            _ => return None,
        };
        let start = self.flash.start + offset;
        Some(start..start + size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::Bus;

    #[test]
    fn sector_erase_requires_unlock_and_updates_only_the_selected_sector() {
        const FLASH: u32 = 0x0800_0000;
        const BASE: u32 = 0x4002_3c00;
        const OPTIONS: u32 = 0x1fff_c000;
        let mut bus = Bus::default();
        bus.map_ram(FLASH, F4Flash::BANK_SIZE as usize).unwrap();
        bus.map_f4_flash(F4Flash::new(
            BASE,
            OPTIONS,
            0x100,
            FLASH..FLASH + F4Flash::BANK_SIZE,
        ))
        .unwrap();

        let mut output = Vec::new();
        let erase = CR_START | CR_SECTOR_ERASE | (4 << 3);
        bus.write(BASE + 0x10, 4, erase, &mut output).unwrap();
        assert_eq!(bus.read::<4>(FLASH + 0x10000).unwrap(), 0);

        bus.write(BASE + 4, 4, 0x4567_0123, &mut output).unwrap();
        bus.write(BASE + 4, 4, 0xcdef_89ab, &mut output).unwrap();
        bus.write(BASE + 0x10, 4, erase | CR_LOCK, &mut output)
            .unwrap();
        assert_eq!(bus.read::<4>(FLASH + 0xfffc).unwrap(), 0);
        assert_eq!(bus.read::<4>(FLASH + 0x10000).unwrap(), u32::MAX);
        assert_eq!(bus.read::<4>(FLASH + 0x1fffc).unwrap(), u32::MAX);
        assert_eq!(bus.read::<4>(FLASH + 0x20000).unwrap(), 0);
        assert_eq!(bus.read::<4>(BASE + 0x0c).unwrap(), SR_EOP);
        assert_ne!(bus.read::<4>(BASE + 0x10).unwrap() & CR_LOCK, 0);
        bus.write(BASE + 0x0c, 1, SR_EOP, &mut output).unwrap();
        assert_eq!(bus.read::<4>(BASE + 0x0c).unwrap(), 0);
    }
}
