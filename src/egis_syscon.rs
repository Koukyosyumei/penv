pub(crate) struct EgisSyscon {
    pub base: u32,
    pub name: String,
    pub devices: [u32; 3],
    registers: [u32; 12],
}

impl EgisSyscon {
    pub const SIZE: u32 = 0x1000;
    const DIVIDERS: [u32; 7] = [1, 2, 3, 4, 6, 8, 16];

    pub fn new(base: u32, name: String, devices: [u32; 3]) -> Self {
        Self {
            base,
            name,
            devices,
            registers: [
                1,
                0x41,
                0,
                0x40,
                0x7000_0000,
                0x3f_ffff,
                0x3f_ffff,
                0xbffc_2000,
                0,
                0,
                0,
                0,
            ],
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
                if value & 6 != 0 {
                    SysconChange::Reset
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

pub(crate) enum SysconChange {
    None,
    Clock(u32),
    Reset,
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
