pub(super) struct Lsm330 {
    values: [f64; 3],
    gyroscope: bool,
    register: u8,
    addressed: bool,
    control: u8,
    range: i32,
}

impl Lsm330 {
    pub(super) fn new(values: [f64; 3], gyroscope: bool) -> Self {
        Self {
            values,
            gyroscope,
            register: 0,
            addressed: false,
            control: 0,
            range: if gyroscope { 250 } else { 2 },
        }
    }

    pub(super) fn reset(&mut self) {
        *self = Self::new(self.values, self.gyroscope);
    }

    pub(super) fn start_write(&mut self) {
        self.addressed = false;
    }

    fn control_register(&self) -> u8 {
        if self.gyroscope { 0x23 } else { 0x24 }
    }

    pub(super) fn write(&mut self, byte: u8) {
        if !self.addressed {
            self.register = byte & 0x7f;
            self.addressed = true;
        } else if self.register == self.control_register() {
            self.control = byte & if self.gyroscope { 0xf0 } else { 0x38 };
            let range = if self.gyroscope {
                Some([250, 500, 2000, 2000][usize::from((byte >> 4) & 3)])
            } else {
                [2, 4, 6, 8, 16].get(usize::from((byte >> 3) & 7)).copied()
            };
            if let Some(range) = range {
                self.range = range;
            }
        }
    }

    pub(super) fn read(&self) -> u8 {
        match self.register {
            0x28..=0x2d => {
                let position = usize::from(self.register - 0x28);
                let sensitivity = 65536 / (2 * self.range) - 1;
                ((self.values[position / 2] * f64::from(sensitivity)) as i16).to_le_bytes()
                    [position % 2]
            }
            register if register == self.control_register() => self.control,
            _ => 0,
        }
    }
}
