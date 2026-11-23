use crate::resd_illuminance::IlluminanceSamples;

// VEML7700 register protocol and light input.
pub struct Veml7700 {
    registers: [u16; 4],
    illuminance: f64,
    samples: Option<IlluminanceSamples>,
    selected: Option<u8>,
}

impl Veml7700 {
    pub const MODEL: &str = "Sensors.VEML7700";

    pub fn new(illuminance: f64) -> Self {
        Self {
            registers: [1, 0, 0, 0],
            illuminance,
            samples: None,
            selected: None,
        }
    }

    pub fn set_illuminance(&mut self, value: f64) -> Result<(), String> {
        if !value.is_finite() || value < 0.0 {
            return Err("expected a nonnegative finite illuminance".into());
        }
        if self.samples.is_some() {
            return Err("cannot set illuminance while using a RESD stream".into());
        }
        self.illuminance = value;
        Ok(())
    }

    pub fn feed_illuminance(&mut self, path: &std::path::Path) -> Result<(), String> {
        let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
        self.samples = Some(
            IlluminanceSamples::decode(&bytes)
                .map_err(|error| format!("{}: {error}", path.display()))?,
        );
        Ok(())
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

    pub fn read(&mut self, nanoseconds: u64) -> [u8; 2] {
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
        let illuminance = self
            .samples
            .as_ref()
            .map_or(self.illuminance, |samples| samples.lux_at(nanoseconds));
        let counts = (illuminance * gain * integration_ms / 5.76)
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
