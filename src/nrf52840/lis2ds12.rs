pub(super) struct Lis2ds12 {
    acceleration: [f64; 3],
    register: u8,
    written: usize,
    ctrl1: u8,
    ctrl4: u8,
    data_ready: bool,
    gpio: u32,
    pin: u8,
}

impl Lis2ds12 {
    pub(super) fn new(acceleration: [f64; 3], gpio: u32, pin: u8) -> Self {
        Self {
            acceleration,
            register: 0,
            written: 0,
            ctrl1: 0,
            ctrl4: 1,
            data_ready: false,
            gpio,
            pin,
        }
    }

    pub(super) fn reset(&mut self) {
        *self = Self::new(self.acceleration, self.gpio, self.pin);
    }

    pub(super) fn irq(&self) -> (u32, u8, bool) {
        (self.gpio, self.pin, self.ctrl4 & 1 != 0 && self.data_ready)
    }

    pub(super) fn start_write(&mut self) {
        self.written = 0;
    }

    pub(super) fn finish_write(&mut self) {
        if self.written == 1 {
            self.data_ready = true;
        }
        self.written = 0;
    }

    pub(super) fn write(&mut self, byte: u8) {
        if self.written == 0 {
            self.register = byte;
        } else {
            match self.register {
                0x20 => self.ctrl1 = byte,
                0x23 => self.ctrl4 = byte,
                _ => {}
            }
        }
        self.written += 1;
    }

    pub(super) fn read(&mut self) -> u8 {
        self.data_ready = false;
        match self.register {
            0x0f => 0x43,
            0x20 => self.ctrl1,
            0x23 => self.ctrl4,
            0x28..=0x2d => {
                let position = usize::from(self.register - 0x28);
                let gain = 0.061 * [1.0, 8.0, 2.0, 4.0][usize::from((self.ctrl1 >> 2) & 3)];
                let bytes = ((self.acceleration[position / 2] * 1000.0 / gain / 9.806650) as i16)
                    .to_le_bytes();
                self.register += u8::from(self.register < 0x2d);
                let low_mask = match self.ctrl1 >> 4 {
                    8.. => 0xc0,
                    5..=7 if self.ctrl1 & 2 != 0 => 0xf0,
                    _ => 0xfa,
                };
                bytes[position % 2] & if position % 2 == 0 { low_mask } else { 0xff }
            }
            _ => 0,
        }
    }
}
