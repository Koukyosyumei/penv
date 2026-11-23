/// LPS25HB byte registers and I²C subaddress protocol for polling.
pub struct Lps25hb {
    pointer: Option<u8>,
    increment: bool,
    registers: [u8; 0x3b],
}

impl Lps25hb {
    pub const MODEL: &str = "Sensors.LPS25HB";

    pub fn new() -> Self {
        let mut registers = [0; 0x3b];
        registers[0x10] = 0x1b;
        Self {
            pointer: None,
            increment: false,
            registers,
        }
    }

    pub fn set_temperature(&mut self, temperature: f64) -> bool {
        if !(-30.0..=105.0).contains(&temperature) {
            return false;
        }
        let value = ((temperature - 42.5) * 480.0) as i32 as i16;
        self.registers[0x2b..=0x2c].copy_from_slice(&value.to_le_bytes());
        true
    }

    pub fn set_pressure(&mut self, pressure: f64) -> bool {
        if !(260.0..=1260.0).contains(&pressure) {
            return false;
        }
        let bytes = ((pressure * 4096.0) as u32).to_le_bytes();
        self.registers[0x28..=0x2a].copy_from_slice(&bytes[..3]);
        true
    }

    pub fn stop(&mut self) {
        self.pointer = None;
        self.increment = false;
    }

    fn next_register(&mut self) -> Option<usize> {
        let register = self.pointer?;
        if self.increment {
            self.pointer = Some(register.wrapping_add(1));
        }
        Some(usize::from(register))
    }

    pub fn read_byte(&mut self) -> u8 {
        if self.registers[0x21] & 8 != 0 {
            return 0xff;
        }
        match self.next_register() {
            Some(0x0f) => 0xbd,
            Some(0x27) => 3,
            Some(index) => self.registers.get(index).copied().unwrap_or(0),
            None => 0xff,
        }
    }

    pub fn write(&mut self, mut bytes: &[u8]) {
        if self.registers[0x21] & 8 != 0 {
            return;
        }
        if self.pointer.is_none() {
            let Some((&subaddress, data)) = bytes.split_first() else {
                return;
            };
            self.pointer = Some(subaddress & 0x7f);
            self.increment = subaddress & 0x80 != 0;
            bytes = data;
        }
        for &value in bytes {
            match self.next_register() {
                Some(0x21) => {
                    if value & 0x84 != 0 {
                        self.registers[0x10] = 0x1b;
                        self.stop();
                    }
                    self.registers[0x21] = value & !0x84;
                }
                Some(
                    index @ (0x08..=0x0a
                    | 0x10
                    | 0x20
                    | 0x22..=0x25
                    | 0x2e
                    | 0x30..=0x31
                    | 0x39..=0x3a),
                ) => self.registers[index] = value,
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follows_datasheet_pressure_vector_and_ignores_out_of_range_updates() {
        let mut sensor = Lps25hb::new();
        assert!(sensor.set_pressure(1023.346923828125));
        assert!(!sensor.set_pressure(200.0));
        assert!(!sensor.set_pressure(1300.0));
        sensor.write(&[0xa8]);
        assert_eq!(
            [sensor.read_byte(), sensor.read_byte(), sensor.read_byte()],
            [0x8d, 0xf5, 0x3f]
        );
        sensor.stop();
        assert!(sensor.set_temperature(-10.0));
        assert!(!sensor.set_temperature(-200.0));
        assert!(!sensor.set_temperature(200.0));
        sensor.write(&[0xab]);
        assert_eq!(
            i16::from_le_bytes([sensor.read_byte(), sensor.read_byte()]),
            -25200
        );
    }
}
