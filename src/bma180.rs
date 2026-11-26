use crate::pseudorandom::Pseudorandom;
use std::io;

const READ: u8 = 0xfc;
const WRITE: u8 = 0xfd;
const CTRL0: usize = 0x0d;
const CTRL2: usize = 0x0f;
const ORIENTATIONS: [u8; 6] = [0b111, 0b011, 0b001, 0b000, 0b100, 0b110];

pub struct Bma180 {
    registers: [u8; 0x3b],
    orientation: usize,
    data: u8,
    response: Vec<u8>,
    random: Pseudorandom,
}

impl Default for Bma180 {
    fn default() -> Self {
        Self::new(0x180)
    }
}

impl Bma180 {
    pub fn new(seed: i32) -> Self {
        let mut device = Self {
            registers: [0; 0x3b],
            orientation: 0,
            data: 0,
            response: Vec::new(),
            random: Pseudorandom::new(seed),
        };
        device.reset();
        device
    }

    pub fn reset(&mut self) {
        self.registers[..0x20].fill(0);
        self.registers[0] = 3;
        self.restore_image();
        self.orientation = 0;
        self.data = 0;
    }

    fn restore_image(&mut self) {
        self.registers[0x20..].fill(0);
        for (register, reset) in [
            (0x20, 4),
            (0x26, 0x50),
            (0x27, 0x32),
            (0x29, 0x17),
            (0x2a, 0x50),
        ] {
            self.registers[register] = reset;
        }
    }

    pub fn write(&mut self, packet: &[u8]) -> io::Result<()> {
        let [mode, address, ..] = *packet else {
            return Ok(());
        };
        if let [_, _, data] = *packet {
            self.data = data;
        }
        match mode {
            WRITE => self.write_register(usize::from(address), self.data),
            READ => {
                let mut response = self.read_register(address)?;
                let checksum = packet
                    .iter()
                    .chain(&response)
                    .fold(0, |sum, value| sum ^ value);
                response.push(checksum);
                self.response = response;
            }
            _ => {}
        }
        Ok(())
    }

    pub fn read(&self) -> &[u8] {
        &self.response
    }

    fn write_register(&mut self, address: usize, value: u8) {
        match address {
            0x09..=0x0c | 0x0e..=0x0f => self.registers[address] = value,
            CTRL0 => {
                self.registers[CTRL0] = value;
                if value & 0x10 != 0 && self.registers[CTRL2] & 0xf0 == 0xf0 {
                    self.reset();
                }
                if self.registers[CTRL0] & 0x20 != 0 {
                    self.restore_image();
                    self.registers[CTRL0] &= !0x20;
                }
            }
            0x10 if value == 0xb6 => self.reset(),
            0x20..=0x3a if self.registers[CTRL0] & 0x10 != 0 => self.registers[address] = value,
            _ => {}
        }
    }

    fn read_register(&mut self, address: u8) -> io::Result<Vec<u8>> {
        Ok(match address {
            0x02 | 0x03 => {
                let x = self.acceleration(2)?;
                let y = self.acceleration(1)?;
                let _z = self.acceleration(0)?;
                self.orientation = (self.orientation + 1) % ORIENTATIONS.len();
                // The pinned model puts Y in both the second and third pair.
                [x, y, y].concat()
            }
            0x04 | 0x05 => self.acceleration(1)?.to_vec(),
            0x06 | 0x07 => self.acceleration(0)?.to_vec(),
            0x08 => vec![self.sample(25.0, 1.0)? as u8],
            address => {
                let register = if (0x40..=0x5a).contains(&address) {
                    address - 0x20
                } else {
                    address
                };
                vec![
                    self.registers
                        .get(usize::from(register))
                        .copied()
                        .unwrap_or(0),
                ]
            }
        })
    }

    fn acceleration(&self, axis: u8) -> io::Result<[u8; 2]> {
        let mean = if ORIENTATIONS[self.orientation] & (1 << axis) != 0 {
            16_000.0
        } else {
            300.0
        };
        let raw = self.sample(mean, 50.0)? & 0x3fff;
        Ok([((raw & 0x3f) << 2) as u8 | 1, (raw >> 6) as u8])
    }

    fn sample(&self, mean: f64, sigma: f64) -> io::Result<u16> {
        let unit = |value| f64::from(value) / f64::from(i32::MAX);
        let sign = if (unit(self.random.next()?) * 10.0) as u8 > 5 {
            1.0
        } else {
            -1.0
        };
        Ok(
            (mean * (1.0 + sign * unit(self.random.next()?) / (2.0 * sigma))).round_ties_even()
                as u16,
        )
    }
}
