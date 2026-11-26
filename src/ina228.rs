use crate::resd::Samples;

pub struct Ina228 {
    registers: [u16; 18],
    pointer: Option<u8>,
    stream: [Option<Samples<f64>>; 3],
    values: [f64; 3],
    next_sample: Option<u64>,
    last: u64,
    energy: f64,
    charge: f64,
    response: [u8; 8],
    length: usize,
}

impl Ina228 {
    pub const MODEL: &str = "Sensors.INA228";

    pub fn new() -> Self {
        let mut device = Self {
            registers: [0; 18],
            pointer: None,
            stream: [None, None, None],
            values: [0.0; 3],
            next_sample: None,
            last: 0,
            energy: 0.0,
            charge: 0.0,
            response: [0; 8],
            length: 0,
        };
        device.reset(0);
        device
    }

    fn reset(&mut self, now: u64) {
        self.registers = [
            0, 0xfb68, 0x1000, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x7fff, 0x8000, 0x7fff, 0, 0x7fff, 0xffff,
        ];
        self.pointer = None;
        self.length = 0;
        self.reset_accumulators(now);
    }

    fn reset_accumulators(&mut self, now: u64) {
        self.energy = 0.0;
        self.charge = 0.0;
        self.last = now;
    }

    pub fn feed(
        &mut self,
        path: &std::path::Path,
        input: usize,
        channel: u16,
        now: u64,
    ) -> Result<(), String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let samples = if input == 2 {
            Samples::decode_channel(&bytes, 1, channel, |bytes| {
                f64::from(i32::from_le_bytes(bytes)) / 1000.0
            })
        } else {
            Samples::decode_channel(&bytes, 4, channel, |bytes| {
                f64::from(u32::from_le_bytes(bytes)) / 1e6
            })
        }
        .map_err(|e| format!("{}: {e}", path.display()))?;
        self.stream[input] = Some(samples);
        if self.next_sample.is_none() {
            self.next_sample = Some(now);
        }
        Ok(())
    }

    fn conversion_period(&self) -> u64 {
        const TIMES: [u64; 8] = [50, 84, 150, 280, 540, 1052, 2074, 4120];
        let config = self.registers[1];
        let mode = config >> 12;
        let times = [9, 6, 3].map(|shift| TIMES[usize::from((config >> shift) & 7)]);
        let active: u64 = times
            .iter()
            .enumerate()
            .filter(|(index, _)| mode & (1 << index) != 0)
            .map(|(_, duration)| duration)
            .sum();
        let total = if mode & 8 == 0 || active == 0 {
            times.iter().sum()
        } else {
            active
        };
        total * [1, 4, 16, 64, 128, 256, 512, 1024][usize::from(config & 7)]
    }

    pub fn tick(&mut self, now: u64) {
        if self.next_sample.is_some_and(|next| now >= next) {
            self.next_sample = Some(now + self.conversion_period());
            let samples = self.stream.each_ref().map(|stream| {
                stream
                    .as_ref()
                    .and_then(|stream| stream.sample_at(now.saturating_mul(1000), true))
            });
            if samples[..2].iter().any(Option::is_some) {
                self.accumulate(now);
            }
            for (value, sample) in self.values.iter_mut().zip(samples) {
                if let Some(sample) = sample {
                    *value = sample;
                }
            }
        }
    }

    pub(crate) fn next_sample_time(&self) -> Option<u64> {
        self.next_sample
    }

    fn signed(value: f64, bits: u32) -> i64 {
        let half = (1i64 << (bits - 1)) as f64;
        value.clamp(-half, half - 1.0).round_ties_even() as i64
    }

    fn current(&self) -> i64 {
        let calibration = self.registers[2] & 0x7fff;
        if calibration == 0 {
            return 0;
        }
        let range = if self.registers[0] & 0x10 != 0 {
            4.0
        } else {
            1.0
        };
        let correction = if self.registers[0] & 0x20 != 0 {
            1.0 + (self.values[2] - 25.0) * f64::from(self.registers[3]) / 1e6
        } else {
            1.0
        };
        Self::signed(
            self.values[0] * 13_107_200_000.0 * range / f64::from(calibration) / correction,
            20,
        )
    }

    fn power(&self) -> i64 {
        (self.current() as f64 * self.values[1] / 3.2)
            .clamp(0.0, 16_777_215.0)
            .round_ties_even() as i64
    }

    fn accumulate(&mut self, now: u64) {
        const WRAP: f64 = (1u64 << 40) as f64;
        let elapsed = now.saturating_sub(self.last) as f64 / 1e6;
        self.energy = (self.energy + self.power() as f64 * elapsed / 16.0).rem_euclid(WRAP);
        self.charge = (self.charge + self.current() as f64 * elapsed + WRAP / 2.0).rem_euclid(WRAP)
            - WRAP / 2.0;
        self.last = now;
    }

    pub fn write(&mut self, bytes: &[u8], now: u64) {
        let Some((&register, data)) = bytes.split_first() else {
            return;
        };
        self.pointer = Some(register);
        let Ok(bytes) = <[u8; 2]>::try_from(data) else {
            return;
        };
        let value = u16::from_be_bytes(bytes);
        match register {
            0 if value & 0x8000 != 0 => self.reset(now),
            0 => {
                self.registers[0] = value & 0x3ff0;
                if value & 0x4000 != 0 {
                    self.reset_accumulators(now);
                }
            }
            1 => {
                self.registers[1] = value;
                if self.next_sample.is_some() {
                    self.next_sample = Some(now + self.conversion_period());
                }
            }
            2 | 3 | 0x0b..=0x11 => self.registers[usize::from(register)] = value,
            _ => {}
        }
    }

    pub fn start_read(&mut self, now: u64) {
        let (value, width) = match self.pointer {
            Some(4) => {
                let lsb = if self.registers[0] & 0x10 != 0 {
                    78.125e-9
                } else {
                    312.5e-9
                };
                (Self::signed(self.values[0] / lsb, 20) << 4, 3)
            }
            Some(5) => (Self::signed(self.values[1] / 195.3125e-6, 20) << 4, 3),
            Some(6) => (Self::signed(self.values[2] / 7.8125e-3, 16), 2),
            Some(7) => (self.current() << 4, 3),
            Some(8) => (self.power(), 3),
            Some(9 | 10) => {
                self.accumulate(now);
                (
                    (if self.pointer == Some(9) {
                        self.energy
                    } else {
                        self.charge
                    })
                    .round_ties_even() as i64,
                    5,
                )
            }
            Some(0x0b) => (i64::from(self.registers[0x0b] | 1), 2),
            Some(0x3e) => (0x5449, 2),
            Some(0x3f) => (0x2281, 2),
            Some(reg @ 0..=0x11) => (i64::from(self.registers[usize::from(reg)]), 2),
            _ => (0, 0),
        };
        self.response = value.to_be_bytes();
        self.length = width;
    }

    pub fn read_byte(&self, offset: usize) -> u8 {
        self.response[8 - self.length..]
            .get(offset)
            .copied()
            .unwrap_or(0xff)
    }
}
