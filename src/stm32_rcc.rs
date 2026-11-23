const RESET_FLAGS: u32 = 0xfe80_0000;

pub struct Stm32F0Rcc {
    pub base: u32,
    registers: [u32; 14],
}

impl Stm32F0Rcc {
    pub const SIZE: u32 = 0x100;

    // Register fields implemented by Renode's STM32F0 RCC, in offset order.
    const WRITE_MASKS: [u32; 14] = [
        0x010d_fffb,
        0xff3f_c7f3,
        0x00bf_3fbf,
        0x0047_5a21,
        0x10fe_4932,
        0x005e_0055,
        0x0047_5a21,
        0x10fe_4932,
        0x0001_831f,
        0xff80_0003,
        0x005e_0000,
        0x0000_000f,
        0x0000_0193,
        0x0000_fffd,
    ];

    pub fn new(base: u32) -> Self {
        let mut registers = [0; 14];
        registers[0] = 0x83;
        registers[5] = 0x14;
        registers[8] = 0x18;
        Self { base, registers }
    }

    pub fn reset(&mut self, option_reload: bool) {
        let flags = self.registers[9] & RESET_FLAGS;
        *self = Self::new(self.base);
        self.registers[9] = flags | if option_reload { 1 << 25 } else { 1 << 28 };
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    pub fn read(&self, address: u32) -> u32 {
        let index = ((address - self.base) / 4) as usize;
        let value = self.registers.get(index).copied().unwrap_or(0);
        match index {
            0 => value | ((value & 0x0101_0000) << 1), // PLL and HSE ready
            1 => value | ((value & 3) << 2),           // selected system clock
            13 => value | ((value & 1) << 1),          // HSI14 ready
            _ => value,
        }
    }

    pub fn write(&mut self, address: u32, value: u32) {
        let index = ((address - self.base) / 4) as usize;
        if let Some(register) = self.registers.get_mut(index) {
            if index == 9 {
                let flags = if value & (1 << 24) == 0 {
                    *register & RESET_FLAGS
                } else {
                    0
                };
                *register = flags | (value & 3);
            } else {
                *register = value & Self::WRITE_MASKS[index];
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_status_tracks_source_selection_and_enable_bits() {
        let mut rcc = Stm32F0Rcc::new(0x4002_1000);
        let base = rcc.base;
        assert_eq!(rcc.read(base), 0x83);
        assert_eq!(rcc.read(base + 0x14), 0x14);
        assert_eq!(rcc.read(base + 0x20), 0x18);
        rcc.write(base, 0x0101_0083);
        assert_eq!(rcc.read(base), 0x0303_0083);
        rcc.write(base, 0x0202_0083);
        assert_eq!(rcc.read(base), 0x83);
        rcc.write(base + 4, 2);
        assert_eq!(rcc.read(base + 4), 0xa);
        rcc.write(base + 0x34, 1);
        assert_eq!(rcc.read(base + 0x34), 3);
    }
}

/// The clock control registers used during STM32F4 firmware startup.
pub struct Stm32F4Rcc {
    pub base: u32,
    registers: [u32; 36],
}

impl Stm32F4Rcc {
    pub const SIZE: u32 = 0x100;

    pub fn new(base: u32) -> Self {
        let mut registers = [0; 36];
        registers[0] = 0x481;
        registers[1] = 0x2400_3010;
        registers[0x74 / 4] = 0x0e00_0000;
        Self { base, registers }
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    pub fn read(&self, address: u32) -> u32 {
        let offset = address - self.base;
        let value = self
            .registers
            .get((offset / 4) as usize)
            .copied()
            .unwrap_or(0);
        match offset {
            0 => value | ((value & 0x1501_0001) << 1),
            8 => value | ((value & 3) << 2),
            0x70 | 0x74 => value | ((value & 1) << 1),
            _ => value,
        }
    }

    pub fn write(&mut self, address: u32, value: u32) {
        let offset = address - self.base;
        let mask = match offset {
            0 => 0x150d_fff9,
            4 => 0x0f43_7fff,
            8 => 0xffff_fcf3,
            0x10..=0x24 | 0x30..=0x44 | 0x50..=0x64 | 0x78..=0x8c => u32::MAX,
            0x70 => 0x0001_830d,
            0x74 => 0xff00_0001,
            _ => 0,
        };
        if let Some(register) = self.registers.get_mut((offset / 4) as usize) {
            *register = value & mask;
        }
    }
}

#[cfg(test)]
#[test]
fn renode_neonkey_rcc_reports_clock_ready_and_source() {
    let mut rcc = Stm32F4Rcc::new(0);
    assert_eq!(rcc.read(0), 0x483);
    assert_eq!(rcc.read(4), 0x2400_3010);
    rcc.write(0, 0x0101_0081);
    assert_eq!(rcc.read(0), 0x0303_0083);
    rcc.write(8, 2);
    assert_eq!(rcc.read(8), 10);
    rcc.write(0x44, 1 << 4);
    assert_eq!(rcc.read(0x44), 1 << 4);
}
