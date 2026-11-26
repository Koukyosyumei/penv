#[derive(Default)]
struct Lock {
    primed: bool,
    unlocked: bool,
}

impl Lock {
    fn key(&mut self, value: u32, keys: [u32; 2]) {
        if !self.unlocked {
            if self.primed && value == keys[1] {
                self.unlocked = true;
                self.primed = false;
            } else {
                self.primed = value == keys[0];
            }
        }
    }
}

pub(crate) struct Stm32l0Flash {
    pub base: u32,
    pub signature: (u32, u32),
    flash: (u32, u32),
    eeprom: (u32, u32),
    access: u32,
    control: u32,
    power: Lock,
    pecr: Lock,
    program: Lock,
    option: Lock,
}

impl Stm32l0Flash {
    pub const SIZE: u32 = 0x400;

    pub fn new(base: u32, signature: (u32, u32), flash: (u32, u32), eeprom: (u32, u32)) -> Self {
        Self {
            base,
            signature,
            flash,
            eeprom,
            access: 0,
            control: 0,
            power: Lock::default(),
            pecr: Lock::default(),
            program: Lock::default(),
            option: Lock::default(),
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new(self.base, self.signature, self.flash, self.eeprom);
    }

    pub fn contains(&self, addr: u32) -> bool {
        addr.wrapping_sub(self.base) < Self::SIZE
            || addr.wrapping_sub(self.signature.0) < self.signature.1
    }

    pub fn read(&self, addr: u32) -> u32 {
        if addr.wrapping_sub(self.signature.0) < self.signature.1 {
            return match addr - self.signature.0 {
                0x50 => 0x0030_3000,
                0x54 => 0x3030_3030,
                0x7c => self.flash.1 / 1024,
                _ => 0,
            };
        }
        match addr - self.base {
            0 => self.access,
            4 => {
                self.control
                    | u32::from(!self.pecr.unlocked)
                    | (u32::from(!self.program.unlocked) << 1)
                    | (u32::from(!self.option.unlocked) << 2)
            }
            0x18 => 0xc,
            _ => 0,
        }
    }

    pub fn write(&mut self, addr: u32, value: u32) {
        match addr.wrapping_sub(self.base) {
            0 => {
                let mut access = value & 0x73;
                if !self.power.unlocked {
                    access = (access & !0x10) | (self.access & 0x10);
                } else if self.access & 0x10 != 0 && access & 0x10 == 0 {
                    self.power = Lock::default();
                }
                if access & 0x20 != 0 {
                    access &= !0x42;
                }
                self.access = access;
            }
            4 if self.pecr.unlocked => {
                if value & 1 != 0 {
                    self.pecr = Lock::default();
                    self.program = Lock::default();
                    self.option = Lock::default();
                    self.control = 0;
                } else {
                    if value & 2 != 0 {
                        self.program = Lock::default();
                    }
                    if value & 4 != 0 {
                        self.option = Lock::default();
                    }
                    self.control = value & 0x208;
                }
            }
            8 => self.power.key(value, [0x04152637, 0xfafbfcfd]),
            0xc => self.pecr.key(value, [0x89abcdef, 0x02030405]),
            0x10 => self.program.key(value, [0x8c9daebf, 0x13141516]),
            0x14 => self.option.key(value, [0xfbead9c8, 0x24252627]),
            _ => {}
        }
    }

    pub fn erase_range(&self, addr: u32) -> Option<(u32, usize)> {
        if self.control & 0x200 == 0 {
            return None;
        }
        let (base, size, unit) = if self.control & 8 != 0 {
            (self.flash.0, self.flash.1, 128)
        } else {
            (self.eeprom.0, self.eeprom.1, 4)
        };
        let offset = addr.wrapping_sub(base);
        (offset < size).then(|| (base + (offset & !(unit - 1)), unit as usize))
    }
}

#[cfg(test)]
mod tests {
    use super::Stm32l0Flash;
    use crate::bus::Bus;

    #[test]
    fn locked_control_preserves_data_and_unlocked_erase_clears_only_selected_page() {
        let mut bus = Bus::default();
        bus.map_ram(0x0800_0000, 1024).unwrap();
        bus.map_ram(0x0808_0000, 64).unwrap();
        bus.map_stm32l0_flash(Stm32l0Flash::new(
            0x4002_2000, (0x1ff8_0000, 0x100), (0x0800_0000, 1024), (0x0808_0000, 64),
        )).unwrap();
        let mut sink = std::io::sink();
        for (address, value) in [
            (0x4002_2004, 0x208), (0x0800_0000, 0x1234), (0x0800_0080, 0x4567),
            (0x4002_200c, 0x89abcdef), (0x4002_200c, 0x02030405), (0x4002_2004, 0x208),
        ] {
            bus.write(address, 4, value, &mut sink).unwrap();
        }
        assert_eq!(bus.read::<4>(0x0800_0000).unwrap(), 0x1234);
        bus.write(0x0800_0004, 4, u32::MAX, &mut sink).unwrap();
        assert_eq!(bus.read::<4>(0x0800_0000).unwrap(), 0);
        assert_eq!(bus.read::<4>(0x0800_0004).unwrap(), 0);
        assert_eq!(bus.read::<4>(0x0800_0080).unwrap(), 0x4567);
        assert_eq!(bus.read::<4>(0x1ff8_007c).unwrap(), 1);
    }
}
