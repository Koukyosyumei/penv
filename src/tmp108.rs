/// TMP108 register protocol with a fixed ambient temperature.
pub struct Tmp108 {
    pointer: usize,
    registers: [u16; 4],
}

impl Tmp108 {
    pub const MODEL: &str = "Sensors.TMP108";

    pub fn new(celsius: f64) -> Self {
        let units = (celsius * 16.0).trunc() as i16;
        Self {
            pointer: 0,
            registers: [(units as u16) << 4, 0x2210, 0x8000, 0x7ff0],
        }
    }

    pub fn read(&self) -> [u8; 2] {
        self.registers[self.pointer].to_be_bytes()
    }

    pub fn write(&mut self, bytes: &[u8]) {
        let Some((&pointer, data)) = bytes.split_first() else {
            return;
        };
        self.pointer = usize::from(pointer & 3);
        if self.pointer == 0 {
            return;
        }
        let mut register = self.read();
        for (dst, src) in register.iter_mut().zip(data) {
            *dst = *src;
        }
        let mut value = u16::from_be_bytes(register);
        if self.pointer == 1 {
            value &= 0xffb0;
            if value & 0x0300 == 0x0100 {
                // The one-shot conversion samples the fixed ambient input and returns to shutdown.
                value &= !0x0300;
            }
        } else {
            value &= 0xfff0;
        }
        self.registers[self.pointer] = value;
    }

    pub fn stop(&mut self) {
        self.pointer = 0;
    }
}
