pub(super) struct Gpio {
    out: u32,
    direction: u32,
    config: [u32; 32],
    latch: u32,
    detect_mode: u32,
    pins: usize,
}

impl Gpio {
    pub(super) fn new(pins: usize) -> Self {
        Self {
            out: 0,
            direction: 0,
            config: [2; 32],
            latch: 0,
            detect_mode: 0,
            pins,
        }
    }

    pub(super) fn reset(&mut self) {
        *self = Self::new(self.pins);
    }

    fn mask(&self) -> u32 {
        u32::MAX >> (32 - self.pins)
    }

    fn input(&self) -> u32 {
        self.config[..self.pins]
            .iter()
            .enumerate()
            .fold(0, |input, (pin, cfg)| {
                let bit = 1 << pin;
                let high = cfg & 2 == 0
                    && if self.direction & bit != 0 {
                        self.out & bit != 0
                    } else {
                        cfg & 12 == 12
                    };
                input | if high { bit } else { 0 }
            })
    }

    fn detect(&self) -> u32 {
        let input = self.input();
        self.config[..self.pins]
            .iter()
            .enumerate()
            .fold(0, |sense, (pin, cfg)| {
                let bit = 1 << pin;
                let set = cfg & 2 == 0
                    && match (cfg >> 16) & 3 {
                        2 => input & bit != 0,
                        3 => input & bit == 0,
                        _ => false,
                    };
                sense | if set { bit } else { 0 }
            })
    }

    pub(super) fn read(&self, offset: u32) -> Option<u32> {
        Some(match offset {
            4 | 8 | 12 => self.out,
            16 => self.input(),
            20 | 24 | 28 => self.direction,
            32 => self.latch,
            36 => self.detect_mode,
            0x200..=0x27c => {
                let pin = ((offset - 0x200) / 4) as usize;
                if pin >= self.pins {
                    0
                } else {
                    self.config[pin] | ((self.direction >> pin) & 1)
                }
            }
            _ => return None,
        })
    }

    pub(super) fn write(&mut self, offset: u32, value: u32) -> bool {
        let mask = self.mask();
        match offset {
            4 => self.out = value & mask,
            8 => self.out |= value & mask,
            12 => self.out &= !value,
            20 => self.direction = value & mask,
            24 => self.direction |= value & mask,
            28 => self.direction &= !value,
            32 => self.latch &= !value,
            36 => self.detect_mode = value & 1,
            0x200..=0x27c => {
                let pin = ((offset - 0x200) / 4) as usize;
                if pin < self.pins {
                    self.config[pin] = value & 0x0003_070e;
                    self.direction = (self.direction & !(1 << pin)) | ((value & 1) << pin);
                }
            }
            _ => return false,
        }
        self.latch |= self.detect();
        true
    }
}

impl Gpio {
    pub(super) fn sample(&self) -> (u32, bool) {
        let detect = if self.detect_mode == 0 {
            self.detect()
        } else {
            self.latch
        };
        (self.input(), detect != 0)
    }

    pub(super) fn task_pin(&mut self, pin: u32, action: u32) {
        if pin < self.pins as u32 {
            let bit = 1 << pin;
            self.direction |= bit;
            match action {
                1 => self.out |= bit,
                2 => self.out &= !bit,
                3 => self.out ^= bit,
                _ => {}
            }
            self.latch |= self.detect();
        }
    }
}

#[derive(Default)]
pub(super) struct Gpiote {
    pub(super) ports: [u32; 2],
    config: [u32; 8],
    levels: [u32; 2],
    detect: bool,
    events: u32,
    interrupts: u32,
}

impl Gpiote {
    pub(super) fn reset(&mut self) {
        *self = Self {
            ports: self.ports,
            ..Self::default()
        };
    }

    pub(super) fn sample(&mut self, samples: [(u32, bool); 2]) {
        let current = samples.map(|sample| sample.0);
        for (index, &config) in self.config.iter().enumerate() {
            if config & 3 == 1 {
                let port = ((config >> 13) & 1) as usize;
                let bit = 1 << ((config >> 8) & 31);
                let old = self.levels[port] & bit != 0;
                let new = current[port] & bit != 0;
                if old != new && ((config >> 16) & 3) & if new { 1 } else { 2 } != 0 {
                    self.events |= (1 << index) | (1 << 31);
                }
            }
        }
        self.levels = current;
        let detect = samples.iter().any(|sample| sample.1);
        if detect && !self.detect {
            self.events |= 1 << 31;
        }
        self.detect = detect;
    }

    pub(super) fn pending(&self) -> bool {
        self.events & self.interrupts != 0
    }

    pub(super) fn read(&self, offset: u32) -> Option<u32> {
        Some(match offset {
            0x000..=0x01c | 0x030..=0x04c | 0x060..=0x07c => 0,
            0x100..=0x11c => (self.events >> ((offset - 0x100) / 4)) & 1,
            0x17c => self.events >> 31,
            0x304 | 0x308 => self.interrupts,
            0x510..=0x52c => self.config[((offset - 0x510) / 4) as usize],
            _ => return None,
        })
    }

    pub(super) fn write(&mut self, offset: u32, value: u32, bus: &mut crate::bus::Bus) -> bool {
        const EVENTS: u32 = 0x8000_00ff;
        match offset {
            0x000..=0x01c | 0x030..=0x04c | 0x060..=0x07c => {
                let config = self.config[((offset % 0x30) / 4) as usize];
                if value & 1 != 0 && config & 3 == 3 {
                    let action = match offset {
                        0x030..=0x04c => 1,
                        0x060..=0x07c => 2,
                        _ => (config >> 16) & 3,
                    };
                    let port = self.ports[((config >> 13) & 1) as usize];
                    bus.nrf_gpio_task(port, (config >> 8) & 31, action);
                }
            }
            0x100..=0x11c | 0x17c => {
                let shift = if offset == 0x17c {
                    31
                } else {
                    (offset - 0x100) / 4
                };
                self.events = (self.events & !(1 << shift)) | ((value & 1) << shift);
            }
            0x304 => self.interrupts |= value & EVENTS,
            0x308 => self.interrupts &= !value,
            0x510..=0x52c if value & 3 != 2 => {
                let port = ((value >> 13) & 1) as usize;
                let pin = (value >> 8) & 31;
                self.config[((offset - 0x510) / 4) as usize] = value & 0x0013_3f03;
                if value & 3 == 3 {
                    let action = if value & (1 << 20) == 0 { 2 } else { 1 };
                    bus.nrf_gpio_task(self.ports[port], pin, action);
                } else if value & 3 == 1 {
                    let bit = 1 << pin;
                    self.levels[port] = (self.levels[port] & !bit)
                        | (bus.nrf_gpio_sample(self.ports[port]).0 & bit);
                }
            }
            _ => return false,
        }
        true
    }
}
