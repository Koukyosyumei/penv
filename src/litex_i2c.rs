use std::collections::VecDeque;

pub struct Sensor {
    pub name: String,
    pub address: u8,
    pub model: u8,
    pub temperature: i32,
    pub humidity: u32,
}

#[derive(Default)]
enum Phase {
    #[default]
    Idle,
    Address,
    AddressAck,
    Write,
    WriteAck,
    Read,
    ReadAck,
}

pub struct I2c {
    signals: u32,
    phase: Phase,
    byte: u8,
    bits: u8,
    selected: bool,
    reading: bool,
    incoming: Vec<u8>,
    outgoing: VecDeque<bool>,
    sensor: Option<Sensor>,
    response: Vec<u8>,
}

impl I2c {
    pub fn new(sensor: Option<Sensor>) -> Self {
        Self {
            signals: 5,
            phase: Phase::Idle,
            byte: 0,
            bits: 0,
            selected: false,
            reading: false,
            incoming: Vec::new(),
            outgoing: VecDeque::new(),
            sensor,
            response: Vec::new(),
        }
    }

    pub fn sensor_mut(&mut self, name: &str) -> Option<&mut Sensor> {
        self.sensor.as_mut().filter(|sensor| sensor.name == name)
    }

    pub fn read(&self, offset: u32) -> u32 {
        match offset {
            0 => self.signals,
            4 => u32::from(self.signals & 2 != 0 || self.outgoing.front().copied().unwrap_or(true)),
            _ => 0,
        }
    }

    pub fn write(&mut self, offset: u32, value: u32) {
        if offset != 0 {
            return;
        }
        let last = self.signals;
        self.signals = value & 7;
        let data = self.signals & 4 != 0;
        match (last & 1 != 0, self.signals & 1 != 0) {
            (true, true) if (last ^ self.signals) & 4 != 0 => {
                self.finish_write();
                self.phase = if data { Phase::Idle } else { Phase::Address };
                self.incoming.clear();
                self.outgoing.clear();
                self.selected = false;
                self.reading = false;
                self.bits = 0;
                self.byte = 0;
            }
            (false, true) => self.rising(data),
            (true, false) if self.signals & 2 == 0 => {
                self.outgoing.pop_front();
            }
            _ => {}
        }
    }

    fn rising(&mut self, data: bool) {
        match self.phase {
            Phase::Address | Phase::Write => {
                self.byte = (self.byte << 1) | u8::from(data);
                self.bits += 1;
                if self.bits != 8 {
                    return;
                }
                self.bits = 0;
                if matches!(self.phase, Phase::Address) {
                    self.selected = self
                        .sensor
                        .as_ref()
                        .is_some_and(|sensor| sensor.address == self.byte >> 1);
                    self.reading = self.byte & 1 != 0;
                    self.phase = Phase::AddressAck;
                } else {
                    // The modeled commands consume at most two bytes.
                    if self.incoming.len() < 3 {
                        self.incoming.push(self.byte);
                    }
                    self.phase = Phase::WriteAck;
                }
                self.byte = 0;
            }
            Phase::AddressAck => {
                self.outgoing.push_back(!self.selected);
                self.phase = if !self.selected {
                    Phase::Idle
                } else if self.reading {
                    for byte in self.response.drain(..) {
                        self.outgoing
                            .extend((0..8).rev().map(|bit| byte & (1 << bit) != 0));
                    }
                    Phase::Read
                } else {
                    Phase::Write
                };
            }
            Phase::WriteAck => {
                self.outgoing.push_back(false);
                self.phase = Phase::Write;
            }
            Phase::Read => {
                self.bits += 1;
                if self.bits == 8 {
                    self.phase = Phase::ReadAck;
                }
            }
            Phase::ReadAck => {
                self.bits = 0;
                self.phase = Phase::Read;
            }
            Phase::Idle => {}
        }
    }

    fn finish_write(&mut self) {
        let Some(sensor) = self
            .sensor
            .as_ref()
            .filter(|_| self.selected && !self.reading)
        else {
            return;
        };
        match self.incoming.as_slice() {
            [0xe0 | 0xe3 | 0xf3] => {
                let raw = ((sensor.temperature + 4685) * 65536 / 17572) as u16;
                self.response.extend(raw.to_be_bytes());
            }
            [0xe5 | 0xf5] => {
                let raw = ((sensor.humidity + 600) * 65536 / 12500) as u16;
                self.response.extend(raw.to_be_bytes());
            }
            [0xfa, 0x0f] => self.response.extend([0; 8]),
            [0xfc, 0xc9] => self.response.extend([sensor.model, 0, 0, 0, 0, 0]),
            [0xfe] => self.response.clear(),
            _ => {}
        }
    }
}
