/// IIS2MDC register interface with a configured stationary magnetic field.
pub struct Iis2mdc {
    pointer: u8,
    field_nt: [i32; 3],
    registers: [u8; 34],
}

impl Iis2mdc {
    pub const MODEL: &str = "Sensors.IIS2MDC";

    pub fn new(field_nt: [i32; 3]) -> Self {
        let mut registers = [0; 34];
        registers[0x60 - 0x45] = 3;
        registers[0x63 - 0x45] = 0xe0;
        Self {
            pointer: 0,
            field_nt,
            registers,
        }
    }

    pub fn read_byte(&mut self) -> u8 {
        let register = self.pointer;
        self.pointer = self.pointer.wrapping_add(1);
        match register {
            0x4f => 0x40,
            0x67 if self.registers[0x60 - 0x45] & 3 == 0 => 0xf,
            0x68..=0x6d => {
                let offset = usize::from(register - 0x68);
                let sample = (self.field_nt[offset / 2] / 150)
                    .clamp(i32::from(i16::MIN), i32::from(i16::MAX))
                    as i16;
                let bytes = if self.registers[0x62 - 0x45] & 8 == 0 {
                    sample.to_le_bytes()
                } else {
                    sample.to_be_bytes()
                };
                bytes[offset % 2]
            }
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
                0x60 if value & 0x20 != 0 => *self = Self::new(self.field_nt),
                0x45..=0x4a | 0x60..=0x63 | 0x65..=0x66 => {
                    self.registers[usize::from(register - 0x45)] = value;
                }
                _ => {}
            }
        }
    }
}
