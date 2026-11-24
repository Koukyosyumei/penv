pub struct Bmc050 {
    registers: [u8; 0x53],
    write_data: u8,
    response: Vec<u8>,
    orientation: u8,
    random: u64,
}

impl Default for Bmc050 {
    fn default() -> Self {
        let mut device = Self {
            registers: [0; 0x53],
            write_data: 0,
            response: Vec::new(),
            orientation: 0,
            random: 1,
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
        self.orientation = 0;
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
                let mut response = self.read_payload(*register);
                let checksum = packet
                    .iter()
                    .chain(&response)
                    .fold(0, |crc, byte| crc ^ byte);
                response.push(checksum);
                self.response = response;
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

    fn random_word(&mut self) -> u64 {
        self.random ^= self.random << 13;
        self.random ^= self.random >> 7;
        self.random ^= self.random << 17;
        self.random
    }

    fn sample(&mut self, mean: f64, sigma: f64) -> u16 {
        let sign = if self.random_word() % 10 > 5 {
            1.0
        } else {
            -1.0
        };
        let fraction = (self.random_word() >> 11) as f64 / (1u64 << 53) as f64;
        (mean * (1.0 + sign * fraction / (2.0 * sigma))).round_ties_even() as u16
    }

    fn read_payload(&mut self, register: u8) -> Vec<u8> {
        let positive = matches!(self.orientation, 0 | 4 | 5);
        match register {
            0x02..=0x07 => {
                let axes = if register <= 0x03 { 3 } else { 1 };
                let mut frame = Vec::with_capacity(axes * 2 + 1);
                for _ in 0..axes {
                    let value = self.sample(if positive { 900.0 } else { 100.0 }, 10.0) & 0x3ff;
                    frame.extend([((value & 3) << 6) as u8 | 1, (value >> 2) as u8]);
                }
                if register <= 0x03 || register >= 0x06 {
                    self.orientation = (self.orientation + 1) % 6;
                }
                frame
            }
            0x08 => vec![self.sample(25.0, 1.0) as u8],
            0x40 => vec![if self.registers[0x4b] & 1 != 0 {
                0x32
            } else {
                0
            }],
            0x42..=0x49 => {
                let axes = if register <= 0x43 {
                    3
                } else {
                    usize::from(register <= 0x47)
                };
                let mut frame = Vec::with_capacity(9);
                for _ in 0..axes {
                    let value = self.sample(if positive { 7900.0 } else { 290.0 }, 40.0) & 0x1fff;
                    frame.extend([((value & 0x1f) << 3) as u8 | 1, (value >> 5) as u8]);
                }
                if register <= 0x43 || register >= 0x48 {
                    let value = self.sample(50.0, 10.0);
                    frame.extend([((value & 0x3f) << 2) as u8, (value >> 6) as u8]);
                }
                frame
            }
            _ => vec![self
                .registers
                .get(usize::from(register))
                .copied()
                .unwrap_or(0)],
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
