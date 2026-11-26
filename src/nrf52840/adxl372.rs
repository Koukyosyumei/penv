pub(super) struct Adxl372 {
    pub(super) name: String,
    pub(super) acceleration: [i16; 3],
    registers: [u8; 128],
    address: u8,
    addressed: bool,
    reading: bool,
}

impl Adxl372 {
    pub(super) fn new(name: String, acceleration: [i16; 3]) -> Self {
        Self {
            name,
            acceleration,
            registers: [0; 128],
            address: 0,
            addressed: false,
            reading: false,
        }
    }

    pub(super) fn reset(&mut self) {
        *self = Self::new(std::mem::take(&mut self.name), self.acceleration);
    }

    pub(super) fn start_write(&mut self) {
        self.addressed = false;
    }

    pub(super) fn write(&mut self, byte: u8) {
        if !self.addressed {
            self.address = byte & 127;
            self.addressed = true;
        } else {
            match self.address {
                0x41 if byte == 0x52 => self.reset(),
                0x20..=0x40 => self.registers[self.address as usize] = byte,
                _ => {}
            }
            self.advance();
        }
    }

    pub(super) fn read(&mut self) -> u8 {
        let value = match self.address {
            0 => 0xad,
            1 => 0x1d,
            2 => 0xfa,
            4 => 1,
            register @ (0x08..=0x0d | 0x15..=0x1a) => {
                let index = register - if register < 0x15 { 0x08 } else { 0x15 };
                (self.acceleration[usize::from(index / 2)] << 4).to_be_bytes()
                    [usize::from(index % 2)]
            }
            _ => self.registers[self.address as usize],
        };
        self.advance();
        value
    }

    fn advance(&mut self) {
        self.address = self.address.wrapping_add(1) & 127;
    }

    pub(super) fn exchange(&mut self, byte: u8, selected: bool) -> u8 {
        if !selected {
            self.addressed = false;
            return 0;
        }
        if !self.addressed {
            self.address = byte >> 1;
            self.addressed = true;
            self.reading = byte & 1 != 0;
            0
        } else if self.reading {
            self.read()
        } else {
            self.write(byte);
            0
        }
    }
}
