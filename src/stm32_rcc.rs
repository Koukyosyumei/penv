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
