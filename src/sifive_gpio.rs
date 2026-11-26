#[derive(Clone, Debug)]
pub(crate) struct SiFiveGpio {
    pub base: u32,
    pub routes: [Option<u8>; 32],
    registers: [u32; 14],
    pins: u32,
    connections: u32,
}

impl SiFiveGpio {
    pub const SIZE: u32 = 0x1000;
    const INPUT_ENABLE: usize = 1;
    const OUTPUT_ENABLE: usize = 2;
    const OUTPUT_VALUE: usize = 3;
    const RISE_ENABLE: usize = 6;
    const RISE_PENDING: usize = 7;
    const FALL_ENABLE: usize = 8;
    const FALL_PENDING: usize = 9;
    const HIGH_ENABLE: usize = 10;
    const HIGH_PENDING: usize = 11;
    const LOW_ENABLE: usize = 12;
    const LOW_PENDING: usize = 13;

    pub fn new(base: u32, routes: [Option<u8>; 32]) -> Self {
        Self {
            base,
            routes,
            registers: [0; 14],
            pins: 0,
            connections: 0,
        }
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    pub fn read(&self, address: u32, width: usize) -> u32 {
        let offset = address - self.base;
        if width != 4 || offset % 4 != 0 {
            return 0;
        }
        match (offset / 4) as usize {
            0 => self.pins & self.registers[Self::INPUT_ENABLE],
            4 | 5 => 0,
            register => self.registers.get(register).copied().unwrap_or(0),
        }
    }

    pub fn write(&mut self, address: u32, width: usize, value: u32) {
        let offset = address - self.base;
        if width != 4 || offset % 4 != 0 {
            return;
        }
        match (offset / 4) as usize {
            Self::INPUT_ENABLE => self.registers[Self::INPUT_ENABLE] = value,
            Self::OUTPUT_ENABLE => self.registers[Self::OUTPUT_ENABLE] = value,
            Self::OUTPUT_VALUE => {
                self.registers[Self::OUTPUT_VALUE] = value;
                self.drive_outputs(value);
            }
            register @ (Self::RISE_PENDING
            | Self::FALL_PENDING
            | Self::HIGH_PENDING
            | Self::LOW_PENDING) => {
                self.registers[register] &= !value;
                self.refresh_interrupts();
            }
            register @ (Self::RISE_ENABLE
            | Self::FALL_ENABLE
            | Self::HIGH_ENABLE
            | Self::LOW_ENABLE) => {
                self.registers[register] = value;
                self.refresh_interrupts();
            }
            _ => {}
        }
    }

    fn drive_outputs(&mut self, value: u32) {
        for pin in 0..32 {
            let mask = 1 << pin;
            if self.registers[Self::OUTPUT_ENABLE] & mask == 0 {
                continue;
            }
            let previous = self.pins;
            self.pins = (previous & !mask) | (value & mask);
            let enabled = self.registers[Self::RISE_ENABLE]
                | self.registers[Self::FALL_ENABLE]
                | self.registers[Self::HIGH_ENABLE]
                | self.registers[Self::LOW_ENABLE];
            if enabled & mask != 0 {
                self.registers[Self::RISE_PENDING] |= !previous & self.pins & mask;
                self.registers[Self::FALL_PENDING] |= previous & !self.pins & mask;
                self.refresh_interrupts();
            } else {
                self.connections = (self.connections & !mask) | (value & mask);
            }
        }
    }

    fn refresh_interrupts(&mut self) {
        self.registers[Self::HIGH_PENDING] = self.pins;
        self.registers[Self::LOW_PENDING] = !self.pins;
        self.connections = (self.registers[Self::RISE_ENABLE] & self.registers[Self::RISE_PENDING])
            | (self.registers[Self::FALL_ENABLE] & self.registers[Self::FALL_PENDING])
            | (self.registers[Self::HIGH_ENABLE] & self.registers[Self::HIGH_PENDING])
            | (self.registers[Self::LOW_ENABLE] & self.registers[Self::LOW_PENDING]);
    }

    pub fn line(&self, pin: usize) -> bool {
        self.connections & (1 << pin) != 0
    }
}
