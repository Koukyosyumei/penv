// VEML7700 register protocol and static light input.
pub struct Veml7700 {
    registers: [u16; 4],
    illuminance: f64,
    selected: Option<u8>,
}

impl Veml7700 {
    pub const MODEL: &str = "Sensors.VEML7700";

    pub fn new(illuminance: f64) -> Self {
        Self {
            registers: [1, 0, 0, 0],
            illuminance,
            selected: None,
        }
    }

    pub fn write(&mut self, data: &[u8]) {
        match data {
            [address] => self.selected = Some(*address),
            [address, low, rest @ ..] if rest.len() <= 1 => {
                self.selected = None;
                let value = u16::from_le_bytes([*low, rest.first().copied().unwrap_or(0)]);
                match self.registers.get_mut(usize::from(*address)) {
                    Some(config) if *address == 0 => {
                        let integration = (value >> 6) & 15;
                        let mask = if matches!(integration, 0..=3 | 8 | 12) {
                            0x1bf3
                        } else {
                            0x1833
                        };
                        *config = (*config & !mask) | (value & mask);
                    }
                    Some(register) => *register = if *address == 3 { value & 7 } else { value },
                    None => {}
                }
            }
            _ => {}
        }
    }

    pub fn read(&mut self) -> [u8; 2] {
        let config = self.registers[0];
        let integration_ms = match (config >> 6) & 15 {
            1 => 200.0,
            2 => 400.0,
            3 => 800.0,
            8 => 50.0,
            12 => 25.0,
            _ => 100.0,
        };
        let gain = [1.0, 2.0, 0.125, 0.25][usize::from((config >> 11) & 3)];
        let counts = (self.illuminance * gain * integration_ms / 5.76)
            .round_ties_even()
            .clamp(0.0, f64::from(u16::MAX)) as u16;
        let value = match self.selected.take() {
            Some(address @ 0..=3) => self.registers[usize::from(address)],
            Some(4) => counts,
            Some(6) if config & 2 != 0 => {
                (u16::from(counts > self.registers[1]) << 14)
                    | (u16::from(counts > self.registers[2]) << 15)
            }
            Some(7) => 0xc481,
            _ => 0,
        };
        value.to_le_bytes()
    }

    pub fn stop(&mut self) {
        self.selected = None;
    }
}
