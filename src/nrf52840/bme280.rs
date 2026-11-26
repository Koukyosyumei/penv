pub(crate) struct Bme280 {
    registers: [u8; 256],
    register: u8,
    addressed: bool,
    temperature: f64,
    humidity: f64,
    pressure: f64,
}

impl Default for Bme280 {
    fn default() -> Self {
        let mut registers = [0; 256];
        registers[0xd0] = 0x60;
        for (address, calibration) in [(0x88, 32768u16), (0x8a, 32768), (0x8e, 1), (0x90, 1), (0x9c, 16384)] {
            registers[address..address + 2].copy_from_slice(&calibration.to_le_bytes());
        }
        registers[0xe1..0xe8].copy_from_slice(&[0x69, 1, 0, 20, 0x21, 3, 30]);
        let mut device = Self {
            registers,
            register: 0,
            addressed: false,
            temperature: 0.0,
            humidity: 0.0,
            pressure: 0.0,
        };
        device.update_measurements();
        device
    }
}

impl Bme280 {
    pub(crate) fn start_write(&mut self) {
        self.addressed = false;
    }

    pub(crate) fn write(&mut self, value: u8) {
        if !self.addressed {
            self.register = value;
            self.addressed = true;
            return;
        }
        match self.register {
            0xe0 if value == 0xb6 => {
                let (temperature, humidity, pressure) = (self.temperature, self.humidity, self.pressure);
                *self = Self::default();
                (self.temperature, self.humidity, self.pressure) = (temperature, humidity, pressure);
                self.update_measurements();
            }
            0x88..=0xa0 | 0xa1 | 0xe1..=0xf1 | 0xf2..=0xf5 => self.registers[usize::from(self.register)] = value,
            _ => {}
        }
        self.addressed = false;
    }

    pub(crate) fn set_property(&mut self, property: &str, value: &str) -> Result<(), String> {
        let value = value.parse::<f64>().ok().filter(|value| value.is_finite())
            .ok_or_else(|| format!("invalid BME280 {property} value {value:?}"))?;
        match property {
            "Temperature" => self.temperature = value,
            "Humidity" => self.humidity = value,
            "Pressure" => self.pressure = value,
            _ => return Err(format!("unknown BME280 property {property:?}")),
        }
        self.update_measurements();
        Ok(())
    }

    fn update_measurements(&mut self) {
        let calibration = |address| {
            u16::from_le_bytes([self.registers[address], self.registers[address + 1]])
        };
        let t1 = i64::from(calibration(0x88));
        let t2 = i64::from(calibration(0x8a) as i16);
        let p1 = i64::from(calibration(0x8e));
        let p2 = i64::from(calibration(0x90) as i16);
        let p8 = i64::from(calibration(0x9c) as i16);
        let temperature = (((self.temperature * 25_600.0 - 128.0) / (5 * t2) as f64
            * 2048.0 + (t1 * 2) as f64) * 8.0).round_ties_even() as i32;
        let scale = ((2i64 << 47) + (i64::from(temperature) / 8 - t1 * 2) * t2 / 2048 - 128_000)
            .wrapping_mul(p2).wrapping_mul(4096).wrapping_mul(p1) / (2i64 << 33);
        let pressure = (1_048_576.0 - (self.pressure - 52.0) * f64::from(2 << 27)
            / (p8 + 1) as f64 * scale as f64 / (3125 * (2i64 << 31)) as f64 * 2.0)
            .round_ties_even() as i32;
        let humidity = (20650.0 + 17900.0 * self.humidity / 100.0) as u16;
        self.registers[0xf7..0xfa].copy_from_slice(&(pressure << 4).to_be_bytes()[1..]);
        self.registers[0xfa..0xfd].copy_from_slice(&(temperature << 4).to_be_bytes()[1..]);
        self.registers[0xfd..0xff].copy_from_slice(&humidity.to_be_bytes());
    }

    pub(crate) fn read(&mut self) -> u8 {
        let value = self.registers[usize::from(self.register)];
        self.register = self.register.wrapping_add(1);
        value
    }
}
