const IRQ_PINS: [&[(usize, usize)]; 12] = [
    &[
        (0, 6),
        (1, 7),
        (2, 8),
        (4, 9),
        (5, 10),
        (6, 11),
        (8, 12),
        (9, 13),
        (10, 14),
        (15, 13),
    ],
    &[(0, 2), (1, 1), (4, 1), (5, 0)],
    &[(2, 3), (3, 2), (5, 1), (6, 0), (8, 3), (12, 3), (13, 2)],
    &[(0, 4), (1, 6), (2, 5), (4, 9), (5, 8)],
    &[
        (0, 0),
        (1, 5),
        (2, 4),
        (3, 14),
        (4, 15),
        (8, 7),
        (9, 6),
        (10, 5),
        (11, 4),
        (14, 9),
        (15, 8),
    ],
    &[(8, 1), (9, 2), (10, 3), (11, 15), (12, 14)],
    &[(15, 7)],
    &[(6, 7), (7, 8), (8, 11), (9, 10)],
    &[(0, 11), (1, 12), (4, 14), (6, 0), (8, 15)],
    &[(5, 8), (6, 9), (7, 10), (8, 11)],
    &[(8, 6), (9, 5), (10, 4)],
    &[],
];

pub(crate) struct Protection {
    pub(crate) base: u32,
    pub(crate) name: String,
    pub(crate) size: u32,
    offset: u32,
    locked: bool,
    enabled: bool,
}

impl Protection {
    pub(crate) fn new(base: u32, name: String, ra8: bool) -> Self {
        Self {
            base,
            name,
            size: if ra8 { 0x20 } else { 0x10 },
            offset: if ra8 { 0x14 } else { 3 },
            locked: true,
            enabled: false,
        }
    }

    pub(crate) fn reset(&mut self) {
        self.locked = true;
        self.enabled = false;
    }

    pub(crate) fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < self.size
    }

    pub(crate) fn read(&self, address: u32) -> u32 {
        if address == self.base + self.offset {
            (u32::from(self.enabled) << 6) | (u32::from(self.locked) << 7)
        } else {
            0
        }
    }

    pub(crate) fn write(&mut self, address: u32, value: u32) {
        if address == self.base + self.offset {
            if !self.locked {
                self.enabled = value & 0x40 != 0;
            }
            self.locked = value & 0x80 != 0;
        }
    }

    pub(crate) fn enabled(&self) -> bool {
        self.enabled
    }
}

pub(crate) struct Port {
    pub(crate) base: u32,
    pub(crate) pfs: u32,
    pub(crate) name: String,
    pub(crate) protection: String,
    mask: u16,
    routes: [Option<usize>; 16],
    output: u16,
    direction: u16,
    input: u16,
    configuration: [u16; 16],
    irq_levels: u16,
}

impl Port {
    pub(crate) const SIZE: u32 = 0x20;
    pub(crate) const PFS_SIZE: u32 = 0x40;

    pub(crate) fn new(
        base: u32,
        pfs: u32,
        name: String,
        protection: String,
        port: usize,
        pins: usize,
        outputs: [Option<usize>; 16],
    ) -> Result<Self, String> {
        let mapping = IRQ_PINS.get(port).ok_or("invalid RA8 GPIO port number")?;
        if !(1..=16).contains(&pins) {
            return Err("invalid GPIO pin count".into());
        }
        let mut routes = [None; 16];
        for &(pin, irq) in *mapping {
            routes[pin] = outputs[irq];
        }
        Ok(Self {
            base,
            pfs,
            name,
            protection,
            mask: u16::MAX >> (16 - pins),
            routes,
            output: 0,
            direction: 0,
            input: 0,
            configuration: [0; 16],
            irq_levels: 0,
        })
    }

    pub(crate) fn reset(&mut self) {
        self.output = 0;
        self.direction = 0;
        self.input = 0;
        self.configuration.fill(0);
        self.irq_levels = 0;
    }

    pub(crate) fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
            || address.wrapping_sub(self.pfs) < Self::PFS_SIZE
    }

    fn pin_input(&self) -> u16 {
        self.input | (self.output & self.direction)
    }

    pub(crate) fn set_input(&mut self, pin: usize, high: bool) {
        if pin >= 16 || self.mask & (1 << pin) == 0 {
            return;
        }
        if high {
            self.input |= 1 << pin;
        } else {
            self.input &= !(1 << pin);
        }
    }

    pub(crate) fn sample_irqs(&mut self, mut drive: impl FnMut(usize, bool)) {
        let mut levels = 0;
        for (pin, route) in self.routes.iter().enumerate() {
            let bit = 1 << pin;
            if self.input & bit != 0
                && self.direction & bit == 0
                && self.configuration[pin] & 0x4000 != 0
            {
                levels |= bit;
            }
            if (self.irq_levels ^ levels) & bit != 0
                && let Some(event) = route
            {
                drive(*event, levels & bit != 0);
            }
        }
        self.irq_levels = levels;
    }

    fn read_word(&self, offset: u32) -> u32 {
        u32::from(match offset {
            0 => self.output,
            2 => self.direction,
            6 => self.pin_input(),
            _ => 0,
        })
    }

    fn write_word(&mut self, offset: u32, value: u16) {
        let value = value & self.mask;
        match offset {
            0 => self.output = value,
            2 => self.direction = value,
            8 => self.output &= !value,
            10 => self.output |= value,
            _ => {}
        }
    }

    pub(crate) fn read(&self, address: u32, width: usize) -> u32 {
        if address.wrapping_sub(self.base) < Self::SIZE {
            let offset = address - self.base;
            return match width {
                2 if offset % 2 == 0 => self.read_word(offset),
                4 if offset % 4 == 0 => (self.read_word(offset) << 16) | self.read_word(offset + 2),
                _ => 0,
            };
        }
        let offset = address - self.pfs;
        let pin = (offset / 4) as usize;
        let bit = 1 << pin;
        if (offset % 4) as usize + width != 4 || self.mask & bit == 0 {
            return 0;
        }
        let flags = u16::from(self.output & bit != 0)
            | (u16::from(self.pin_input() & bit != 0) << 1)
            | (u16::from(self.direction & bit != 0) << 2);
        let value = self.configuration[pin] | flags;
        u32::from(if width == 1 { value & 0xff } else { value })
    }

    pub(crate) fn write(&mut self, address: u32, width: usize, value: u32, pfs_enabled: bool) {
        if address.wrapping_sub(self.base) < Self::SIZE {
            let offset = address - self.base;
            match width {
                2 if offset % 2 == 0 => self.write_word(offset, value as u16),
                4 if offset % 4 == 0 => {
                    self.write_word(offset, (value >> 16) as u16);
                    self.write_word(offset + 2, value as u16);
                }
                _ => {}
            }
            return;
        }
        let offset = address - self.pfs;
        let pin = (offset / 4) as usize;
        let bit = 1 << pin;
        if !pfs_enabled || (offset % 4) as usize + width != 4 || self.mask & bit == 0 {
            return;
        }
        self.output = (self.output & !bit) | if value & 1 != 0 { bit } else { 0 };
        self.direction = (self.direction & !bit) | if value & 4 != 0 { bit } else { 0 };
        let mask = if width == 1 { 0x50 } else { 0x4050 };
        self.configuration[pin] = (self.configuration[pin] & !mask) | (value as u16 & mask);
    }
}
