pub struct Bmc050 {
    registers: [u8; 0x53],
    write_data: u8,
    response: Vec<u8>,
}

impl Default for Bmc050 {
    fn default() -> Self {
        let mut device = Self {
            registers: [0; 0x53],
            write_data: 0,
            response: Vec::new(),
        };
        device.reset();
        device
    }
}

impl Bmc050 {
    pub fn reset(&mut self) {
        self.registers.fill(0);
        for (register, value) in [
            (0x00, 0x03),
            (0x01, 0x21),
            (0x0f, 0x03),
            (0x10, 0x1f),
            (0x22, 0x09),
            (0x23, 0x30),
            (0x24, 0x81),
            (0x25, 0x0f),
            (0x26, 0xc0),
            (0x28, 0x14),
            (0x2a, 0x04),
            (0x2b, 0x0a),
            (0x2c, 0x18),
            (0x2d, 0x08),
            (0x2e, 0x08),
            (0x2f, 0x10),
            (0x32, 0x70),
            (0x33, 0x04),
            (0x4b, 0x01),
            (0x4c, 0x06),
            (0x4d, 0x3f),
            (0x4e, 0x07),
        ] {
            self.registers[register] = value;
        }
        self.write_data = 0;
    }

    pub fn write(&mut self, packet: &[u8]) {
        let [operation, register, rest @ ..] = packet else {
            return;
        };
        if let [value] = rest {
            self.write_data = *value;
        }
        match operation {
            0xfd => self.write_register(*register, self.write_data),
            0xfc => {
                let value = if *register == 0x40 {
                    if self.registers[0x4b] & 1 == 1 {
                        0x32
                    } else {
                        0
                    }
                } else {
                    self.registers
                        .get(usize::from(*register))
                        .copied()
                        .unwrap_or(0)
                };
                let crc = packet.iter().fold(value, |crc, byte| crc ^ byte);
                self.response = vec![value, crc];
            }
            _ => {}
        }
    }

    pub fn read(&self, count: usize) -> Vec<u8> {
        if self.response.len() < count {
            vec![0; count]
        } else {
            self.response.clone()
        }
    }

    fn write_register(&mut self, register: u8, value: u8) {
        let stored = match register {
            0x14 if value == 0xb6 => {
                self.reset();
                return;
            }
            0x0f => {
                if matches!(value, 3 | 5 | 8 | 12) {
                    value
                } else {
                    3
                }
            }
            0x36 if value & 0x80 != 0 => 0,
            0x4b => value & 0x87,
            0x10..=0x11
            | 0x13
            | 0x16..=0x17
            | 0x19..=0x1b
            | 0x1e
            | 0x20..=0x28
            | 0x2a..=0x2f
            | 0x32..=0x34
            | 0x36..=0x3d
            | 0x4c..=0x52 => value,
            _ => return,
        };
        self.registers[usize::from(register)] = stored;
    }
}
