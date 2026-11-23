pub(super) struct Lsm303 {
    values: [f64; 3],
    irq_wiring: Option<(u32, [u8; 2])>,
    register: u8,
    increment: bool,
    written: usize,
    config: [u8; 4],
    status: u8,
}

impl Lsm303 {
    pub(super) fn new(values: [f64; 3], irq_wiring: Option<(u32, [u8; 2])>) -> Self {
        let accel = irq_wiring.is_some();
        Self {
            values,
            irq_wiring,
            register: 0,
            increment: false,
            written: 0,
            config: if accel { [0x43, 0, 0x3f, 0x3f] } else { [0; 4] },
            status: if accel { 8 } else { 1 },
        }
    }

    pub(super) fn reset(&mut self) {
        *self = Self::new(self.values, self.irq_wiring);
    }

    pub(super) fn start_write(&mut self) {
        self.written = 0;
    }

    pub(super) fn finish_write(&mut self) {
        if self.written == 1 {
            self.status = if self.irq_wiring.is_some() {
                match self.config[0] {
                    0..=15 => 0,
                    config if config & 7 == 7 => 8,
                    config => config & 7,
                }
            } else {
                u8::from(self.config[2] & 3 < 2)
            };
        }
        self.written = 0;
    }

    fn irq_source(&self, index: usize) -> u8 {
        let mut bits = 0;
        for axis in 0..3 {
            if self.status & (8 | (1 << axis.min(1))) != 0 {
                bits |= 3 << (axis * 2);
            }
        }
        self.config[index + 2] & bits
    }

    pub(super) fn irqs(&self) -> [Option<(u32, u8, bool)>; 2] {
        match self.irq_wiring {
            Some((gpio, pins)) => {
                std::array::from_fn(|i| Some((gpio, pins[i], self.irq_source(i) & 0x2a != 0)))
            }
            None => [None; 2],
        }
    }

    pub(super) fn write(&mut self, byte: u8) {
        let accel = self.irq_wiring.is_some();
        if self.written == 0 {
            self.register = if accel { byte & 0x7f } else { byte };
            self.increment = !accel || byte > 0x80;
        } else {
            if accel {
                match self.register {
                    0x20 => self.config[0] = byte,
                    0x23 => self.config[1] = byte,
                    0x30 => self.config[2] = byte,
                    0x34 => self.config[3] = byte,
                    _ => {}
                }
            } else if self.register <= 2 {
                self.config[usize::from(self.register)] = byte;
            }
            self.advance();
        }
        self.written += 1;
    }

    pub(super) fn read(&mut self) -> u8 {
        let value = if self.irq_wiring.is_some() {
            match self.register {
                0x0f => 0x33,
                0x20 => self.config[0],
                0x23 => self.config[1],
                0x27 => self.status,
                0x28..=0x2d => {
                    let position = usize::from(self.register - 0x28);
                    let sensitivity = [1.0, 2.0, 4.0, 12.0][usize::from((self.config[1] >> 4) & 3)];
                    let raw = ((self.values[position / 2].trunc() * 16384.0)
                        / (9.806650 * sensitivity)) as i16;
                    raw.to_le_bytes()[position % 2]
                }
                0x30 => self.config[2],
                0x31 => 0x40 | self.irq_source(0),
                0x34 => self.config[3],
                0x35 => 0x40 | self.irq_source(1),
                _ => 0,
            }
        } else {
            match self.register {
                register @ 0..=2 => self.config[usize::from(register)],
                3..=8 => {
                    if self.register == 3 {
                        self.status |= 2;
                    }
                    let position = usize::from(self.register - 3);
                    let gain = [980.0, 1100.0, 760.0, 600.0, 400.0, 355.0, 295.0, 205.0]
                        [usize::from(self.config[1] >> 5)];
                    let axis = [0, 2, 1][position / 2];
                    ((self.values[axis] * gain) as i16).to_be_bytes()[position % 2]
                }
                9 => self.status,
                _ => 0,
            }
        };
        self.advance();
        value
    }

    fn advance(&mut self) {
        if self.increment {
            self.register = self.register.wrapping_add(1);
        }
    }
}
