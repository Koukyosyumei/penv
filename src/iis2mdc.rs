/// IIS2MDC register interface with a stationary zero magnetic field.
pub struct Iis2mdc {
    pointer: u8,
    registers: [u8; 34],
}

impl Iis2mdc {
    pub const MODEL: &str = "Sensors.IIS2MDC";

    pub fn new() -> Self {
        let mut registers = [0; 34];
        registers[0x60 - 0x45] = 3;
        registers[0x63 - 0x45] = 0xe0;
        Self {
            pointer: 0,
            registers,
        }
    }

    pub fn read_byte(&mut self) -> u8 {
        let register = self.pointer;
        self.pointer = self.pointer.wrapping_add(1);
        match register {
            0x4f => 0x40,
            0x67 if self.registers[0x60 - 0x45] & 3 == 0 => 0xf,
            0x45..=0x66 => self.registers[usize::from(register - 0x45)],
            _ => 0,
        }
    }

    pub fn write(&mut self, bytes: &[u8]) {
        let Some((&pointer, data)) = bytes.split_first() else {
            return;
        };
        self.pointer = pointer;
        for &value in data {
            let register = self.pointer;
            self.pointer = self.pointer.wrapping_add(1);
            match register {
                0x60 if value & 0x20 != 0 => *self = Self::new(),
                0x45..=0x4a | 0x60..=0x63 | 0x65..=0x66 => {
                    self.registers[usize::from(register - 0x45)] = value;
                }
                _ => {}
            }
        }
    }
}
