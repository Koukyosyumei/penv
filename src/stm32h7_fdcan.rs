/// Bosch M_CAN configuration and interrupt registers.
pub struct Fdcan {
    pub base: u32,
    pub lines: [Option<u8>; 2],
    registers: [u32; 64],
}

impl Fdcan {
    pub const MODEL: &str = "CAN.MCAN";
    pub const SIZE: u32 = 0x400;

    pub fn new(base: u32, lines: [Option<u8>; 2]) -> Self {
        let mut registers = [0; 64];
        registers[0] = 0x3301_1200;
        registers[0x18 / 4] = 1; // INIT at reset
        registers[0x44 / 4] = 0x707; // LEC and DLEC have not changed.
        Self {
            base,
            lines,
            registers,
        }
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    pub fn interrupts(&self) -> impl Iterator<Item = u8> + '_ {
        let pending = self.registers[0x50 / 4] & self.registers[0x54 / 4];
        let select = self.registers[0x58 / 4];
        let enable = self.registers[0x5c / 4];
        self.lines
            .into_iter()
            .enumerate()
            .filter_map(move |(index, line)| {
                let mask = if index == 0 { !select } else { select };
                line.filter(|_| enable & (1 << index) != 0 && pending & mask != 0)
            })
    }

    pub fn read(&self, address: u32) -> u32 {
        self.registers
            .get(((address - self.base) / 4) as usize)
            .copied()
            .unwrap_or(0)
    }

    pub fn write(&mut self, address: u32, value: u32) {
        let offset = address - self.base;
        let initialized = self.registers[0x18 / 4] & 3 == 3;
        let Some(register) = self.registers.get_mut((offset / 4) as usize) else {
            return;
        };
        match offset {
            0x00 | 0x04 | 0x40 | 0x44 | 0xcc | 0xd8 | 0xdc => {}
            0x18 => {
                let init = value & 1;
                let changes = if *register & 1 != 0 && init != 0 {
                    value & 2
                } else {
                    0
                };
                let sleep = value & (1 << 4);
                let options = if initialized { value } else { *register };
                *register = init | changes | sleep | (sleep >> 1) | (options & 0xf3e0);
            }
            0x50 => *register &= !value,
            0x54 | 0x58 | 0x5c => *register = value,
            0xd0 => {
                // A transmit request remains pending until cancelled or delivered.
                self.registers[0xcc / 4] |= value;
            }
            0xd4 => {
                let cancelled = self.registers[0xcc / 4] & value;
                self.registers[0xdc / 4] |= cancelled;
                self.registers[0xcc / 4] &= !value;
                if cancelled & self.registers[0xe4 / 4] != 0 {
                    self.registers[0x50 / 4] |= 1 << 10;
                }
            }
            _ if initialized => *register = value,
            _ => {}
        }
    }
}
