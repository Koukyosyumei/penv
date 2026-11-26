pub(crate) struct EgisSyscon {
    pub base: u32,
    pub name: String,
    pub devices: [u32; 3],
    registers: [u32; 12],
}

impl EgisSyscon {
    pub const SIZE: u32 = 0x1000;
    const DIVIDERS: [u32; 7] = [1, 2, 3, 4, 6, 8, 16];

    const RESET_REGISTERS: [u32; 12] = [
        1,
        0x41,
        0,
        0x40,
        0x7000_0000,
        0x3f_ffff,
        0x3f_ffff,
        0xbffc_0000,
        0,
        0,
        0,
        0,
    ];

    pub fn new(base: u32, name: String, devices: [u32; 3]) -> Self {
        let mut registers = Self::RESET_REGISTERS;
        registers[7] |= 1 << 13;
        Self {
            base,
            name,
            devices,
            registers,
        }
    }

    pub fn reset(&mut self, cause: EgisReset) {
        let previous = self.registers;
        self.registers = Self::RESET_REGISTERS;
        match cause {
            EgisReset::Warm => {
                for index in [3, 4, 7] {
                    self.registers[index] = previous[index];
                }
            }
            EgisReset::Cold => {}
            EgisReset::Watchdog => self.registers[7] |= 1 << 12,
        }
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    pub fn root_clock(&self) -> u32 {
        let control = self.registers[1];
        let root = if control & 1 == 0 {
            102_560 / 3
        } else {
            205_000_000
        };
        root / Self::DIVIDERS[((control >> 4) & 7) as usize]
    }

    pub fn apb_clock(&self) -> u32 {
        self.root_clock() / 2 / Self::DIVIDERS[((self.registers[1] >> 1) & 7) as usize]
    }

    pub fn reset_vector(&self) -> u32 {
        self.registers[4]
    }

    pub fn read(&self, address: u32) -> u32 {
        let offset = address - self.base;
        match offset {
            0x00..=0x2c if offset & 3 == 0 => self.registers[(offset / 4) as usize],
            _ => 0,
        }
    }

    pub fn write(&mut self, address: u32, value: u32) -> SysconChange {
        match address - self.base {
            0x04 if (value >> 4) & 7 < 7 && (value >> 1) & 7 < 7 => {
                self.registers[1] = value & 0x7f;
                SysconChange::Clock(self.apb_clock())
            }
            0x0c => {
                if value & 4 != 0 {
                    SysconChange::Reset(EgisReset::Warm)
                } else if value & 2 != 0 {
                    SysconChange::Reset(EgisReset::Cold)
                } else {
                    SysconChange::None
                }
            }
            0x10 => {
                self.registers[4] = value;
                SysconChange::None
            }
            0x14 | 0x18 => {
                let index = ((address - self.base) / 4) as usize;
                self.registers[index] = value & 0x3f_ffff;
                SysconChange::None
            }
            0x1c => {
                self.registers[7] = (value & 0x7ffc_77fd) | (1 << 31);
                SysconChange::None
            }
            _ => SysconChange::None,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum EgisReset {
    Cold,
    Warm,
    Watchdog,
}

pub(crate) enum SysconChange {
    None,
    Clock(u32),
    Reset(EgisReset),
}

#[cfg(test)]
mod tests {
    use super::{EgisSyscon, SysconChange};

    #[test]
    fn clock_sources_and_dividers_reconfigure_the_apb_frequency() {
        let mut syscon = EgisSyscon::new(0, "syscon".into(), [0; 3]);
        assert_eq!(syscon.apb_clock(), 17_083_333);
        assert_eq!(syscon.reset_vector(), 0x7000_0000);
        assert!(matches!(syscon.write(4, 0x12), SysconChange::Clock(4273)));
        assert_eq!(syscon.root_clock(), 17_093);
        assert_eq!(syscon.apb_clock(), 4_273);
    }
}

pub(crate) enum EgisExtra {
    Smu2 {
        base: u32,
    },
    Entropy {
        base: u32,
        control: u32,
        key: [u32; 4],
        sequence: crate::pseudorandom::Sequence,
    },
}

impl EgisExtra {
    pub fn smu2(base: u32) -> Self {
        Self::Smu2 { base }
    }

    pub fn entropy(base: u32) -> Self {
        Self::Entropy {
            base,
            control: 0,
            key: [0; 4],
            sequence: crate::pseudorandom::Sequence::new(0),
        }
    }

    pub fn base(&self) -> u32 {
        match self {
            Self::Smu2 { base } | Self::Entropy { base, .. } => *base,
        }
    }

    pub fn size(&self) -> u32 {
        match self {
            Self::Smu2 { .. } => 0x1000,
            Self::Entropy { .. } => 0x10_0000,
        }
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base()) < self.size()
    }

    fn smu2_has_register(offset: u32) -> bool {
        matches!(
            offset,
            0x100 | 0x104 | 0x110 | 0x114 | 0x200 | 0x300 | 0x304 | 0x500 | 0x600
        ) || ((0x400..=0x424).contains(&offset) && offset & 3 == 0)
    }

    pub fn read(&mut self, address: u32) -> u32 {
        let offset = address - self.base();
        match self {
            Self::Smu2 { .. } => {
                if !Self::smu2_has_register(offset) {
                    crate::monitor_log::record(
                        2,
                        &format!("SMU2: Unhandled read from offset {offset:#x}"),
                    );
                }
                0
            }
            Self::Entropy {
                control,
                key,
                sequence,
                ..
            } => match offset {
                0x400 => 0x400,
                0x1000 => *control,
                0x1004 => u32::MAX,
                0x1010 | 0x1014 | 0x1018 | 0x101c => key[((offset - 0x1010) / 4) as usize],
                0x1030 => 0x87,
                0x1080 => sequence.next() as u32,
                _ => 0,
            },
        }
    }

    pub fn reset(&mut self) {
        if let Self::Entropy { control, key, .. } = self {
            *control = 0;
            *key = [0; 4];
        }
    }

    pub fn write(&mut self, address: u32, value: u32) {
        let offset = address - self.base();
        match self {
            Self::Smu2 { .. } => {
                if !Self::smu2_has_register(offset) {
                    crate::monitor_log::record(
                        2,
                        &format!("SMU2: Unhandled write to offset {offset:#x}"),
                    );
                }
            }
            Self::Entropy { control, key, .. } => match offset {
                0x1000 => *control = value,
                0x1010 | 0x1014 | 0x1018 | 0x101c => key[((offset - 0x1010) / 4) as usize] = value,
                _ => {}
            },
        }
    }
}
