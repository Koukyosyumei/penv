const CALIBRATION: [u8; 22] = [
    0x1b, 0xcb, 0xfb, 0xcb, 0xc6, 0x91, 0x7b, 0xa8, 0x7f, 0x75, 0x5a, 0x71, 0x15, 0x7a, 0x00, 0x38,
    0x80, 0x00, 0xdd, 0xf9, 0x0b, 0x34,
];

pub(super) struct Bmp180 {
    temperature: f64,
    pressure: u32,
    register: u8,
    addressed: bool,
    control: u8,
    sample: [u8; 3],
}

impl Bmp180 {
    pub(super) fn new(temperature: f64, pressure: u32) -> Self {
        Self {
            temperature,
            pressure,
            register: 0,
            addressed: false,
            control: 0,
            sample: [0x80, 0, 0],
        }
    }

    pub(super) fn reset(&mut self) {
        *self = Self::new(self.temperature, self.pressure);
    }

    pub(super) fn start_write(&mut self) {
        self.addressed = false;
    }

    pub(super) fn write(&mut self, byte: u8) {
        if !self.addressed {
            self.register = byte;
            self.addressed = true;
        } else if self.register == 0xe0 && byte == 0xb6 {
            self.reset();
        } else if self.register == 0xf4 {
            self.control = byte & !0x20;
            match byte & 0x1f {
                0x0e => {
                    let ac5 = 0x7f75;
                    let ac6 = 0x5a71;
                    let b5 = (self.temperature * 10.0) as i32 * 16 - 8;
                    let md = 0x0b34;
                    let mc = -8711;
                    let delta = (b5 + md).pow(2) - (mc << 13);
                    let x2 = ((f64::from(b5 + md) + f64::from(delta).sqrt()) as i32) >> 1;
                    self.sample[..2]
                        .copy_from_slice(&((((b5 - x2) << 15) / ac5 + ac6) as u16).to_be_bytes());
                }
                0x14 => {
                    let pressure = self.pressure << (8 - (byte >> 6));
                    self.sample.copy_from_slice(&pressure.to_be_bytes()[1..]);
                }
                _ => {}
            }
        }
    }

    pub(super) fn read(&mut self) -> u8 {
        let value = match self.register {
            0xaa..=0xbf => CALIBRATION[usize::from(self.register - 0xaa)],
            0xd0 => 0x55,
            0xf4 => self.control,
            0xf6..=0xf8 => self.sample[usize::from(self.register - 0xf6)],
            _ => 0,
        };
        if matches!(self.register, 0xaa..=0xbe | 0xf6..=0xf7) {
            self.register += 1;
        }
        value
    }
}
