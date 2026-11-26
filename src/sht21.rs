use crate::{crc::CrcEngine, pseudorandom::Pseudorandom};
use std::io;

const WRITE: u8 = 0xfd;
const READ: u8 = 0xfc;
const USER_RESET: u8 = 0x3a;

pub struct Sht21 {
    user: u8,
    register_data: u8,
    measurement: [u8; 2],
    response: Vec<u8>,
    random: Pseudorandom,
    pub mean_humidity: f64,
}

impl Default for Sht21 {
    fn default() -> Self {
        Self::new(0x21)
    }
}

impl Sht21 {
    pub fn new(seed: i32) -> Self {
        Self {
            user: USER_RESET,
            register_data: 0,
            measurement: [0; 2],
            response: Vec::new(),
            random: Pseudorandom::new(seed),
            mean_humidity: 75.0,
        }
    }

    pub fn reset(&mut self) {
        self.user = USER_RESET;
        self.register_data = 0;
        self.measurement = [0; 2];
    }

    pub fn write(&mut self, packet: &[u8]) -> io::Result<()> {
        let [mode, register, ..] = *packet else {
            return Ok(());
        };
        if let [_, _, value] = *packet {
            self.register_data = value;
        }

        match (mode, register) {
            (WRITE, 0xfe) => self.reset(),
            (WRITE, 0xe6) => self.user = self.register_data,
            (WRITE, 0xe3 | 0xf3) => {
                let temperature = self.sample(25.0, 1.0)?;
                let raw = ((temperature + 46.85) * 65536.0 / 17572.0).round_ties_even() as u16;
                let low_bits = match self.user & 0x81 {
                    0 => 6,
                    1 => 4,
                    0x80 => 5,
                    _ => 3,
                };
                self.measurement = pack(raw, low_bits, 0);
                if register == 0xf3 {
                    self.send_measurement(packet);
                }
            }
            (WRITE, 0xe5 | 0xf5) => {
                let humidity = self.sample(self.mean_humidity, 5.0)?.clamp(1.0, 99.0);
                let raw = ((humidity + 6.0) * 65536.0 / 125.0).round_ties_even() as u16;
                let low_bits = match self.user & 0x81 {
                    0 => 4,
                    1 => 0,
                    0x80 => 2,
                    _ => 3,
                };
                self.measurement = pack(raw, low_bits, 2);
                if register == 0xf5 {
                    self.send_measurement(packet);
                }
            }
            (READ, 0xe3 | 0xe5 | 0xf3 | 0xf5) => self.send_measurement(packet),
            (READ, 0xe7) => self.send(packet, &[self.user]),
            (READ, 0xfa) => self.send(packet, &[0; 9]),
            (READ, 0xfc) => self.send(packet, &[0; 7]),
            (READ, _) => self.send(packet, &[0; 3]),
            _ => {}
        }
        Ok(())
    }

    pub fn read(&self) -> &[u8] {
        &self.response
    }

    fn sample(&self, mean: f64, sigma: f64) -> io::Result<f64> {
        let unit = |number| f64::from(number) / f64::from(i32::MAX);
        let sign = if (unit(self.random.next()?) * 10.0) as u8 > 5 {
            1.0
        } else {
            -1.0
        };
        Ok(mean * (1.0 + sign * unit(self.random.next()?) / (2.0 * sigma)))
    }

    fn send_measurement(&mut self, packet: &[u8]) {
        let mut crc = CrcEngine::new(0x31, 8, false, false, 0, 0).unwrap();
        crc.update(&self.measurement);
        self.send(
            packet,
            &[self.measurement[0], self.measurement[1], crc.value() as u8],
        );
    }

    fn send(&mut self, packet: &[u8], response: &[u8]) {
        let checksum = packet
            .iter()
            .chain(response)
            .fold(0, |sum, byte| sum ^ byte);
        self.response.clear();
        self.response.extend_from_slice(response);
        self.response.push(checksum);
    }
}

fn pack(raw: u16, low_bits: u32, status: u8) -> [u8; 2] {
    let low = if low_bits == 0 {
        0
    } else {
        ((raw & ((1 << low_bits) - 1)) << (8 - low_bits)) as u8
    };
    [(raw >> low_bits) as u8, low | status]
}
