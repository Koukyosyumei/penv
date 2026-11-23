use std::collections::VecDeque;

pub(super) struct Lsm9ds1 {
    magnetic: bool,
    samples: VecDeque<[f64; 3]>,
    sample: [f64; 3],
    fifo_depth: u8,
    register: u8,
    addressed: bool,
    register_increment: bool,
    controls: [u8; 3],
}

impl Lsm9ds1 {
    pub(super) fn new(magnetic: bool) -> Self {
        Self {
            magnetic,
            samples: VecDeque::new(),
            sample: [0.0; 3],
            fifo_depth: 0,
            register: 0,
            addressed: false,
            register_increment: false,
            controls: [0, 0, 4],
        }
    }

    pub(super) fn reset(&mut self) {
        *self = Self {
            samples: std::mem::take(&mut self.samples),
            sample: self.sample,
            ..Self::new(self.magnetic)
        };
    }

    pub(super) fn imu_samples(&mut self) -> Option<&mut VecDeque<[f64; 3]>> {
        (!self.magnetic).then_some(&mut self.samples)
    }

    pub(super) fn start_write(&mut self) {
        self.addressed = false;
    }

    pub(super) fn write(&mut self, byte: u8) {
        if !self.addressed {
            self.register = byte & 0x7f;
            self.register_increment = byte & 0x80 != 0;
            self.addressed = true;
            return;
        }
        if self.magnetic {
            if self.register == 0x21 {
                self.controls[0] = byte & 0x60;
            }
        } else {
            match self.register {
                0x10 => self.controls[0] = byte & 0xfb,
                0x20 => self.controls[1] = byte,
                0x22 => self.controls[2] = byte,
                _ => {}
            }
        }
        self.advance();
    }

    pub(super) fn read(&mut self) -> u8 {
        let value = if self.magnetic {
            match self.register {
                0x0f => 0x3d,
                0x21 => self.controls[0],
                _ => 0,
            }
        } else {
            match self.register {
                0x0f => 0x68,
                0x10 => self.controls[0],
                0x15 => ((-25i16 * 16).to_le_bytes())[0],
                0x16 => ((-25i16 * 16).to_le_bytes())[1],
                0x20 => self.controls[1],
                0x22 => self.controls[2],
                0x28..=0x2d => {
                    if self.register == 0x28 {
                        let next = self.samples.pop_front();
                        self.fifo_depth = self.fifo_depth.saturating_sub(u8::from(next.is_some()));
                        self.sample = next.unwrap_or([0.0; 3]);
                    }
                    let pos = usize::from(self.register - 0x28);
                    let sensitivity =
                        [16384.0, 2048.0, 8192.0, 4096.0][usize::from((self.controls[1] >> 3) & 3)];
                    ((self.sample[pos / 2] * sensitivity) as i16).to_le_bytes()[pos % 2]
                }
                0x2f => {
                    if self.fifo_depth == 0 {
                        self.fifo_depth = 1;
                        0
                    } else {
                        self.fifo_depth
                    }
                }
                _ => 0,
            }
        };
        self.advance();
        value
    }

    fn advance(&mut self) {
        if (self.magnetic && self.register_increment)
            || (!self.magnetic && self.controls[2] & 4 != 0)
        {
            self.register = self.register.wrapping_add(1) & 0x7f;
        }
    }
}
