pub(crate) type Interrupt = (usize, u32);

pub(crate) struct Gpio {
    pub(crate) base: u32,
    pub(crate) irq: Option<Interrupt>,
    config: [u8; 32],
    input: u32,
    output: u32,
    latched: u32,
}

impl Gpio {
    pub(crate) const SIZE: u32 = 0xa4;

    pub(crate) fn new(base: u32, irq: Option<Interrupt>) -> Self {
        Self {
            base,
            irq,
            config: [0; 32],
            input: 0,
            output: 0,
            latched: 0,
        }
    }

    pub(crate) fn contains(&self, address: u32) -> bool {
        address & 3 == 0 && address.wrapping_sub(self.base) < Self::SIZE
    }

    fn enabled_pins(&self, flag: u8) -> u32 {
        self.config
            .iter()
            .enumerate()
            .fold(0, |mask, (pin, config)| {
                mask | (u32::from(config & flag != 0) << pin)
            })
    }

    pub(crate) fn output(&self) -> u32 {
        self.output & self.enabled_pins(1)
    }

    pub(crate) fn read(&self, address: u32) -> u32 {
        match address - self.base {
            offset @ 0..=0x7c => u32::from(self.config[(offset / 4) as usize]),
            0x90 => self.input & self.enabled_pins(2),
            0xa0 => self.output(),
            _ => 0,
        }
    }

    pub(crate) fn write(&mut self, address: u32, value: u32) {
        match address - self.base {
            offset @ 0..=0x7c => {
                let config = &mut self.config[(offset / 4) as usize];
                let trigger = if (value >> 5) & 7 <= 4 {
                    value as u8 & 0xe0
                } else {
                    *config & 0xe0
                };
                *config = (value as u8 & 15) | trigger;
            }
            0x80 => self.latched &= !value,
            0xa0 => {
                let outputs = self.enabled_pins(1);
                self.output = (self.output & !outputs) | (value & outputs);
            }
            _ => {}
        }
        self.refresh_level_interrupts();
    }

    pub(crate) fn drive_input(&mut self, pin: u8, level: bool) {
        let bit = 1 << pin;
        let before = self.input & bit != 0;
        self.input = (self.input & !bit) | if level { bit } else { 0 };
        let config = self.config[usize::from(pin)];
        if config & 0xa == 0xa && before != level {
            match (config >> 5, level) {
                (2, true) | (3, false) | (4, _) => self.latched |= bit,
                _ => {}
            }
        }
        self.refresh_level_interrupts();
    }

    fn refresh_level_interrupts(&mut self) {
        for (pin, config) in self.config.iter().copied().enumerate() {
            let bit = 1 << pin;
            if config & 0xa == 0xa
                && matches!((config >> 5, self.input & bit != 0), (0, true) | (1, false))
            {
                self.latched |= bit;
            }
        }
    }

    pub(crate) fn interrupt(&self) -> bool {
        self.latched != 0
    }
}

pub(crate) struct Timer {
    pub(crate) base: u32,
    pub(crate) irq: Option<Interrupt>,
    frequency: u64,
    limit: u32,
    counter: u32,
    control: u8,
    divisor: u16,
    phase: u128,
    pending: bool,
    irq_level: bool,
}

impl Timer {
    pub(crate) const SIZE: u32 = 0x1c;

    pub(crate) fn new(base: u32, frequency: u64, irq: Option<Interrupt>) -> Self {
        Self {
            base,
            irq,
            frequency,
            limit: u32::MAX,
            counter: u32::MAX,
            control: 2,
            divisor: 1,
            phase: 0,
            pending: false,
            irq_level: false,
        }
    }

    pub(crate) fn contains(&self, address: u32) -> bool {
        address & 3 == 0 && address.wrapping_sub(self.base) < Self::SIZE
    }

    pub(crate) fn read(&self, address: u32) -> u32 {
        match address - self.base {
            0 => self.limit,
            4 => self.counter,
            8 => u32::from(self.control),
            12 => u32::from(self.divisor / 2).max(1).ilog2(),
            20 => u32::from(self.pending),
            24 => u32::from(self.pending && self.control & 2 != 0),
            _ => 0,
        }
    }

    pub(crate) fn write(&mut self, address: u32, value: u32) {
        match address - self.base {
            0 => {
                self.limit = value;
                self.counter = value;
                if self.control & 4 != 0 {
                    self.control |= 1;
                }
            }
            8 => self.control = value as u8 & 7,
            12 => {
                self.divisor = 2 << (value & 15).min(9);
                self.phase = 0;
            }
            16 => {
                self.pending = false;
                self.irq_level = false;
            }
            _ => {}
        }
    }

    pub(crate) fn tick(&mut self, step_hz: u32) {
        if self.control & 1 == 0 {
            return;
        }
        let period = u128::from(step_hz) * u128::from(self.divisor);
        self.phase += u128::from(self.frequency);
        let ticks = self.phase / period;
        self.phase %= period;
        if ticks == 0 {
            return;
        }
        if ticks < u128::from(self.counter.max(1)) {
            self.counter -= ticks as u32;
            return;
        }
        self.pending = true;
        self.irq_level |= self.control & 2 != 0;
        if self.control & 4 != 0 {
            self.control &= !1;
            self.counter = 0;
        } else {
            let excess = ticks - u128::from(self.counter.max(1));
            self.counter = self.limit - (excess % u128::from(self.limit.max(1))) as u32;
        }
    }

    pub(crate) fn interrupt(&self) -> bool {
        self.irq_level
    }
}
