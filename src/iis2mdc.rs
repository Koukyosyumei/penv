/// IIS2MDC register interface with a static or timestamped magnetic field.
pub struct Iis2mdc {
    pointer: u8,
    field_nt: [i32; 3],
    samples: Vec<(u64, [i32; 3])>,
    next_sample: usize,
    ready: bool,
    route: Option<(u32, u8)>,
    registers: [u8; 34],
}

impl Iis2mdc {
    pub const MODEL: &str = "Sensors.IIS2MDC";

    pub fn new(field_nt: [i32; 3]) -> Self {
        let mut registers = [0; 34];
        registers[0x60 - 0x45] = 3;
        registers[0x63 - 0x45] = 0xe0;
        Self {
            pointer: 0,
            field_nt,
            samples: Vec::new(),
            next_sample: 0,
            ready: false,
            route: None,
            registers,
        }
    }

    pub fn load_samples(&mut self, path: &str) -> Result<(), String> {
        let csv = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
        let mut lines = csv.lines();
        if lines.next()
            != Some(
                "timestamp,magnetic_flux_density_x,magnetic_flux_density_y,magnetic_flux_density_z",
            )
        {
            return Err(format!("{path}: expected IIS2MDC CSV header"));
        }
        for (index, line) in lines.enumerate() {
            let invalid = || format!("{path}:{}: invalid IIS2MDC sample", index + 2);
            let columns: Vec<_> = line.split(',').collect();
            let [time, x, y, z] = columns.as_slice() else {
                return Err(invalid());
            };
            let step = time.trim().parse::<u64>().map_err(|_| invalid())? / 1000;
            if self
                .samples
                .last()
                .is_some_and(|(previous, _)| *previous >= step)
            {
                return Err(invalid());
            }
            let field = [x, y, z].map(|axis| axis.trim().parse::<i32>());
            let [Ok(x), Ok(y), Ok(z)] = field else {
                return Err(invalid());
            };
            self.samples.push((step, [x, y, z]));
        }
        Ok(())
    }

    pub fn feed_resd(&mut self, path: &std::path::Path) -> Result<(), String> {
        let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
        let samples = crate::resd::Samples::decode(&bytes, 8, |bytes: [u8; 12]| {
            let words: &[[u8; 4]; 3] = bytes.as_chunks::<4>().0.try_into().unwrap();
            words.map(i32::from_le_bytes)
        })
        .map_err(|error| format!("{}: {error}", path.display()))?;
        self.samples = samples
            .into_points()
            .into_iter()
            .map(|(time, field)| (time / 1000, field))
            .collect();
        self.next_sample = 0;
        Ok(())
    }

    pub fn set_route(&mut self, base: u32, pin: u8) {
        self.route = Some((base, pin));
    }

    pub fn tick(&mut self, step: u64) -> Option<(u32, u8, bool)> {
        while let Some(&(time, field)) = self.samples.get(self.next_sample) {
            if time > step {
                break;
            }
            self.field_nt = field;
            self.ready = true;
            self.next_sample += 1;
        }
        self.route.map(|(base, pin)| {
            let enabled = self.registers[0x62 - 0x45] & 1 != 0;
            (base, pin, enabled && self.ready)
        })
    }

    pub(crate) fn next_sample_time(&self) -> Option<u64> {
        self.samples.get(self.next_sample).map(|(time, _)| *time)
    }

    pub fn read_byte(&mut self) -> u8 {
        let register = self.pointer;
        self.pointer = self.pointer.wrapping_add(1);
        match register {
            0x4f => 0x40,
            0x67 if self.ready
                || self.samples.is_empty() && self.registers[0x60 - 0x45] & 3 == 0 =>
            {
                0xf
            }
            0x68..=0x6d => {
                if register == 0x6d {
                    self.ready = false;
                }
                let offset = usize::from(register - 0x68);
                let sample = (self.field_nt[offset / 2] / 150)
                    .clamp(i32::from(i16::MIN), i32::from(i16::MAX))
                    as i16;
                let bytes = if self.registers[0x62 - 0x45] & 8 == 0 {
                    sample.to_le_bytes()
                } else {
                    sample.to_be_bytes()
                };
                bytes[offset % 2]
            }
            0x45..=0x66 => self.registers[usize::from(register - 0x45)],
            _ => 0,
        }
    }

    pub fn write(&mut self, bytes: &[u8]) {
        let Some((&pointer, data)) = bytes.split_first() else {
            return;
        };
        self.pointer = pointer;
        for &value in data {
            let register = self.pointer;
            self.pointer = self.pointer.wrapping_add(1);
            match register {
                0x60 if value & 0x20 != 0 => {
                    self.registers = Self::new(self.field_nt).registers;
                    self.pointer = 0;
                    self.ready = false;
                }
                0x45..=0x4a | 0x60..=0x63 | 0x65..=0x66 => {
                    self.registers[usize::from(register - 0x45)] = value;
                }
                _ => {}
            }
        }
    }
}
