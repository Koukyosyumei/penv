//! LSM6DSO user-bank I²C interface and the accelerometer/gyroscope data-ready outputs.

use crate::machine::VIRTUAL_STEPS_PER_SECOND;

const RATES: [u32; 12] = [0, 12, 26, 52, 104, 208, 416, 833, 1660, 3330, 6660, 5];

pub fn input_axis(property: &str) -> Option<usize> {
    [
        "DefaultAccelerationX",
        "DefaultAccelerationY",
        "DefaultAccelerationZ",
        "DefaultAngularRateX",
        "DefaultAngularRateY",
        "DefaultAngularRateZ",
    ]
    .iter()
    .position(|axis| *axis == property)
}

pub struct Lsm6dso {
    pointer: u8,
    registers: [u8; 128],
    route: Option<(u32, u8)>,
    default: [f64; 6],
    samples: Vec<[f64; 6]>,
    start: u64,
    frequency: u32,
    phase: [u32; 2],
    output: [i16; 6],
    ready: u8,
}

impl Lsm6dso {
    pub const MODEL: &str = "Sensors.LSM6DSO_IMU";

    pub fn new(default: [f64; 6], route: Option<(u32, u8)>) -> Self {
        let mut sensor = Self {
            pointer: 0,
            registers: [0; 128],
            route,
            default,
            samples: Vec::new(),
            start: 0,
            frequency: 12,
            phase: [0; 2],
            output: [0; 6],
            ready: 0,
        };
        sensor.reset();
        sensor
    }

    fn reset(&mut self) {
        self.pointer = 0;
        self.registers.fill(0);
        self.registers[0x12] = 4;
        self.registers[0x18] = 0xe0;
        self.phase = [0; 2];
        self.output.fill(0);
        self.ready = 0;
    }

    pub fn load_samples(
        &mut self,
        path: &std::path::Path,
        start: u64,
        frequency: u32,
    ) -> Result<(), String> {
        let csv = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let path = path.display();
        let mut samples = Vec::new();
        let mut lines = csv.lines();
        if lines.next() != Some("accel_x,accel_y,accel_z,gyro_x,gyro_y,gyro_z") {
            return Err(format!("{path}: expected LSM6DSO CSV header"));
        }
        for (line, sample) in lines.enumerate() {
            let invalid = || format!("{path}:{}: expected six integer samples", line + 2);
            let values = sample
                .split(',')
                .map(|value| value.trim().parse::<i32>())
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| invalid())?;
            let [ax, ay, az, gx, gy, gz] = values.as_slice() else {
                return Err(invalid());
            };
            samples.push([
                f64::from(*ax) / 1e6,
                f64::from(*ay) / 1e6,
                f64::from(*az) / 1e6,
                f64::from(*gx) / 1e5 * 180.0 / std::f64::consts::PI,
                f64::from(*gy) / 1e5 * 180.0 / std::f64::consts::PI,
                f64::from(*gz) / 1e5 * 180.0 / std::f64::consts::PI,
            ]);
        }
        self.samples = samples;
        self.start = start;
        self.frequency = frequency;
        Ok(())
    }

    pub fn set_input(&mut self, axis: usize, value: f64) {
        self.default[axis] = value;
    }

    pub fn tick(&mut self, step: u64) -> Option<(u32, u8, bool)> {
        for (group, control, mask) in [(0, 0x10, 1), (1, 0x11, 2)] {
            self.phase[group] += RATES
                .get(usize::from(self.registers[control] >> 4))
                .copied()
                .unwrap_or(0);
            if self.phase[group] < VIRTUAL_STEPS_PER_SECOND {
                continue;
            }
            self.phase[group] -= VIRTUAL_STEPS_PER_SECOND;
            let input = step
                .checked_sub(self.start)
                .and_then(|elapsed| elapsed.checked_mul(u64::from(self.frequency)))
                .and_then(|scaled| {
                    usize::try_from(scaled / u64::from(VIRTUAL_STEPS_PER_SECOND)).ok()
                })
                .and_then(|index| self.samples.get(index))
                .unwrap_or(&self.default);
            let fs = usize::from((self.registers[control] >> 2) & 3);
            let gain = if group == 0 {
                let range = if fs == 1 && self.registers[0x17] & 2 != 0 {
                    2.0
                } else {
                    [2.0, 16.0, 4.0, 8.0][fs]
                };
                32768.0 / range
            } else if self.registers[control] & 2 != 0 {
                1000.0 / 4.375
            } else {
                1000.0 / [8.75, 17.5, 35.0, 70.0][fs]
            };
            for axis in group * 3..group * 3 + 3 {
                self.output[axis] = (input[axis] * gain).round_ties_even() as i16;
            }
            self.ready |= mask;
        }
        self.route.map(|(base, pin)| {
            let active = self.registers[0x0d] & self.ready != 0;
            (base, pin, active ^ (self.registers[0x12] & 0x20 != 0))
        })
    }

    fn advance(&mut self) {
        if self.registers[0x12] & 4 != 0 {
            self.pointer = self.pointer.wrapping_add(1) & 0x7f;
        }
    }

    pub fn read_byte(&mut self) -> u8 {
        let address = self.pointer;
        self.advance();
        if address != 1 && self.registers[1] & 0xc0 != 0 {
            return 0;
        }
        match address {
            0x0f => 0x6c,
            0x1e => self.ready,
            0x20..=0x21 => 0,
            0x22..=0x2d => {
                let offset = usize::from(address - 0x22);
                let axis = (offset / 2 + 3) % 6;
                if offset & 1 != 0 {
                    self.ready &= if axis < 3 { !1 } else { !2 };
                }
                self.output[axis].to_le_bytes()[offset & 1]
            }
            _ => self.registers[usize::from(address)],
        }
    }

    pub fn write(&mut self, bytes: &[u8]) {
        let Some((&address, bytes)) = bytes.split_first() else {
            return;
        };
        self.pointer = address & 0x7f;
        for &byte in bytes {
            let address = self.pointer;
            if address == 0x12 && byte & 1 != 0 && self.registers[1] & 0xc0 == 0 {
                self.reset();
                break;
            }
            if address == 1
                || self.registers[1] & 0xc0 == 0
                    && matches!(address, 0x02 | 0x07..=0x0e | 0x10..=0x19 | 0x56..=0x5f | 0x62)
            {
                self.registers[usize::from(address)] = byte;
            }
            self.advance();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interrupt_follows_output_data_rate_and_clears_after_read() {
        let mut imu = Lsm6dso::new([1.0, -1.0, 0.0, 0.0, 0.0, 0.0], Some((0x58020800, 13)));
        imu.write(&[0x0d, 1]);
        imu.write(&[0x10, 0x10]);
        for step in 1..=83_333 {
            assert_eq!(imu.tick(step), Some((0x58020800, 13, false)));
        }
        assert_eq!(imu.tick(83_334), Some((0x58020800, 13, true)));
        imu.write(&[0x28]);
        assert_eq!(
            i16::from_le_bytes([imu.read_byte(), imu.read_byte()]),
            16384
        );
        assert_eq!(imu.tick(83_335), Some((0x58020800, 13, false)));
        imu.write(&[0x2a]);
        assert_eq!(
            i16::from_le_bytes([imu.read_byte(), imu.read_byte()]),
            -16384
        );
    }
}
