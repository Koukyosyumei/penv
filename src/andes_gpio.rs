pub(crate) struct AndesGpio {
    pub base: u32,
    pub irq: Option<(usize, u32)>,
    input: u32,
    output: u32,
    direction: u32,
    enable: u32,
    pending: u32,
    mode_registers: [u32; 4],
    modes: [u8; 32],
}

impl AndesGpio {
    pub const SIZE: u32 = 0x1000;

    pub(crate) fn reset(&mut self) {
        *self = Self::new(self.base, self.irq);
    }

    pub fn new(base: u32, irq: Option<(usize, u32)>) -> Self {
        Self {
            base,
            irq,
            input: 0,
            output: 0,
            direction: 0,
            enable: 0,
            pending: 0,
            mode_registers: [0; 4],
            modes: [3; 32],
        }
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    pub fn output(&self) -> u32 {
        self.output
    }

    pub fn interrupt(&self) -> bool {
        self.pending != 0
    }

    pub fn drive_input(&mut self, pin: u8, value: bool) {
        let mask = 1 << pin;
        if self.direction & mask != 0 {
            return;
        }
        let previous = self.input & mask != 0;
        self.input = (self.input & !mask) | (u32::from(value) << pin);
        if self.enable & mask != 0 {
            let triggered = match self.modes[usize::from(pin)] {
                5 => previous && !value,
                6 => !previous && value,
                7 => previous != value,
                _ => false,
            };
            if triggered {
                self.pending |= mask;
            }
        }
        self.refresh_levels();
    }

    fn refresh_levels(&mut self) {
        let mut candidates = self.enable & !self.direction;
        while candidates != 0 {
            let pin = candidates.trailing_zeros() as usize;
            let mask = 1 << pin;
            candidates &= candidates - 1;
            if matches!(self.modes[pin], 2 | 3)
                && ((self.input & mask != 0) == (self.modes[pin] == 2))
            {
                self.pending |= mask;
            }
        }
    }

    pub fn read(&self, address: u32) -> u32 {
        match address - self.base {
            0 => 0x0203_1000,
            0x20 => self.input,
            0x24 => self.output,
            0x28 => self.direction,
            0x50 => self.enable,
            offset @ (0x54 | 0x58 | 0x5c | 0x60) => {
                self.mode_registers[((offset - 0x54) / 4) as usize]
            }
            0x64 => self.pending,
            _ => 0,
        }
    }

    pub fn write(&mut self, address: u32, value: u32) {
        match address - self.base {
            0x24 => self.output = value,
            0x28 => self.direction = value,
            0x2c => self.output &= !value,
            0x30 => self.output |= value,
            0x50 => self.enable = value,
            offset @ (0x54 | 0x58 | 0x5c | 0x60) => {
                let index = ((offset - 0x54) / 4) as usize;
                self.mode_registers[index] = value & 0x7777_7777;
                for channel in 0..8 {
                    let pin = index * 8 + channel;
                    let mode = ((value >> (channel * 4)) & 7) as u8;
                    match mode {
                        0 => self.enable &= !(1 << pin),
                        2 | 3 | 5 | 6 | 7 => {
                            self.modes[pin] = mode;
                            self.enable |= 1 << pin;
                        }
                        _ => {}
                    }
                }
            }
            0x64 => self.pending = 0,
            _ => {}
        }
        self.refresh_levels();
    }
}
