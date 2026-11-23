pub(super) struct Bme280 {
    registers: [u8; 256],
    register: u8,
    addressed: bool,
}

impl Default for Bme280 {
    fn default() -> Self {
        let mut registers = [0; 256];
        registers[0xd0] = 0x60;
        Self {
            registers,
            register: 0,
            addressed: false,
        }
    }
}

impl Bme280 {
    pub(super) fn start_write(&mut self) {
        self.addressed = false;
    }

    pub(super) fn write(&mut self, value: u8) {
        if !self.addressed {
            self.register = value;
            self.addressed = true;
            return;
        }
        match self.register {
            0xe0 if value == 0xb6 => *self = Self::default(),
            0xf2 | 0xf4 | 0xf5 => self.registers[usize::from(self.register)] = value,
            _ => {}
        }
        self.addressed = false;
    }

    pub(super) fn read(&mut self) -> u8 {
        let value = self.registers[usize::from(self.register)];
        self.register = self.register.wrapping_add(1);
        value
    }
}
