use crate::stm32_i2c::Eeprom;
use std::collections::BTreeMap;

const START: u8 = 4;
const STOP: u8 = 8;
const NACK: u8 = 16;
const RECEIVE: u8 = 32;
const END: u8 = 64;
const TRANSMIT: u8 = 128;

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Idle,
    Address,
    Write(u8),
    Read(u8, bool),
}

pub(crate) struct Iic {
    pub(crate) base: u32,
    pub(crate) name: String,
    routes: [Option<usize>; 4],
    regs: [u8; 0x18],
    state: State,
    pending_address: Option<(u32, u8)>,
    requested_end: u8,
    irq_levels: u8,
    devices: BTreeMap<u8, Eeprom>,
}

impl Iic {
    pub(crate) const SIZE: u32 = 0x20;

    pub(crate) fn new(base: u32, name: String, routes: [Option<usize>; 4]) -> Self {
        let mut iic = Self {
            base,
            name,
            routes,
            regs: [0; 0x18],
            state: State::Idle,
            pending_address: None,
            requested_end: 0,
            irq_levels: 0,
            devices: BTreeMap::new(),
        };
        iic.reset();
        iic
    }

    pub(crate) fn reset(&mut self) {
        self.regs = [0; 0x18];
        for (offset, value) in [
            (0, 0x1f),
            (2, 8),
            (3, 6),
            (5, 0x72),
            (6, 9),
            (0x10, 0xff),
            (0x11, 0xff),
            (0x16, 0x10),
            (0x17, 0xfd),
        ] {
            self.regs[offset] = value;
        }
        self.state = State::Idle;
        self.pending_address = None;
        self.requested_end = 0;
        self.irq_levels = 0;
    }

    pub(crate) fn attach(&mut self, address: u8, eeprom: Eeprom) -> Result<(), String> {
        if address > 0x7f || self.devices.contains_key(&address) {
            return Err("invalid or duplicate I²C slave address".into());
        }
        self.devices.insert(address, eeprom);
        Ok(())
    }

    pub(crate) fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    fn start(&mut self) {
        self.pending_address = None;
        self.state = State::Address;
        self.regs[9] = (self.regs[9] & !(STOP | RECEIVE | END)) | START | TRANSMIT;
    }

    fn stop(&mut self) {
        self.pending_address = None;
        self.state = State::Idle;
        self.regs[4] &= !8;
        self.regs[9] = (self.regs[9] & !(START | RECEIVE | END | TRANSMIT)) | STOP;
    }

    fn select_address(&mut self, value: u8) {
        let address = value >> 1;
        if let Some(device) = self.devices.get_mut(&address) {
            self.regs[9] &= !NACK;
            if value & 1 != 0 {
                self.state = State::Read(address, true);
                self.regs[9] |= RECEIVE;
            } else {
                device.start_write();
                self.state = State::Write(address);
                self.regs[9] |= TRANSMIT | END;
            }
        } else {
            self.regs[9] |= NACK | END;
        }
    }

    pub(crate) fn tick(&mut self, mut drive: impl FnMut(usize, bool)) {
        if let Some((remaining, value)) = self.pending_address {
            if remaining <= 1 {
                self.pending_address = None;
                self.select_address(value);
            } else {
                self.pending_address = Some((remaining - 1, value));
            }
        }
        let active = self.regs[7] & self.regs[9];
        let mut levels = 0;
        for (index, mask) in [RECEIVE, TRANSMIT, END, START | STOP | NACK]
            .into_iter()
            .enumerate()
        {
            let bit = 1 << index;
            if active & mask != 0 {
                levels |= bit;
            }
            if let Some(event) = self.routes[index] {
                if index < 2 {
                    if (levels & !self.irq_levels) & bit != 0 {
                        drive(event, true);
                        drive(event, false);
                    }
                } else if (levels ^ self.irq_levels) & bit != 0 {
                    drive(event, levels & bit != 0);
                }
            }
        }
        self.irq_levels = levels;
    }

    pub(crate) fn read(&mut self, address: u32) -> u8 {
        let offset = (address - self.base) as usize;
        match offset {
            1 => {
                let flags = if self.state == State::Idle { 0 } else { 0xc0 };
                flags
                    | if matches!(self.state, State::Read(..)) {
                        0
                    } else {
                        0x20
                    }
            }
            0x12 => 0,
            0x13 => {
                self.regs[9] &= !RECEIVE;
                let State::Read(address, dummy) = self.state else {
                    return 0;
                };
                let value = if dummy {
                    0
                } else {
                    self.devices.get_mut(&address).map_or(0, Eeprom::read)
                };
                self.state = State::Read(address, false);
                let end = std::mem::take(&mut self.requested_end);
                if end & 4 != 0 {
                    self.start();
                } else if end & 8 != 0 && !dummy {
                    self.stop();
                } else {
                    self.requested_end = end;
                    self.regs[9] |= RECEIVE;
                    self.irq_levels &= !1;
                }
                value
            }
            _ => self.regs.get(offset).copied().unwrap_or(0),
        }
    }

    pub(crate) fn write(&mut self, address: u32, value: u8, step_hz: u32) {
        let offset = (address - self.base) as usize;
        match offset {
            0 => {
                if value & 0x40 != 0 {
                    if value & 0x80 == 0 {
                        self.reset();
                    } else {
                        self.state = State::Idle;
                        self.pending_address = None;
                        self.requested_end = 0;
                        self.regs[9] = 0;
                    }
                }
                self.regs[0] = (self.regs[0] & 0x1f) | (value & 0xc0);
            }
            1 if self.regs[0] & 0x80 != 0 => {
                if matches!(self.state, State::Read(..)) {
                    self.requested_end |= value & 0xc;
                } else if value & 8 != 0 {
                    self.stop();
                } else if value & 4 != 0 || value & 2 != 0 && self.state == State::Idle {
                    self.start();
                }
            }
            4 => {
                let ack = if value & 0x10 != 0 {
                    value
                } else {
                    self.regs[4]
                };
                self.regs[4] = (value & !8) | (ack & 8);
            }
            9 => self.regs[9] &= value,
            0x12 => {
                self.regs[9] &= !(TRANSMIT | END);
                self.irq_levels &= !2;
                match self.state {
                    State::Address => {
                        self.pending_address = Some((step_hz.div_ceil(10_000), value))
                    }
                    State::Write(address) => {
                        if let Some(device) = self.devices.get_mut(&address) {
                            device.write(value);
                        }
                        self.regs[9] |= TRANSMIT | END;
                    }
                    _ => {}
                }
            }
            2..=8 | 0xa..=0x11 | 0x16..=0x17 => self.regs[offset] = value,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_start_reads_eeprom_and_missing_address_nacks() {
        let mut iic = Iic::new(0, "iic0".into(), [Some(21), Some(22), Some(23), Some(24)]);
        iic.attach(0x50, Eeprom::new(256, 8, true, 8)).unwrap();
        let mut events = Vec::new();
        let clocks = 10_000;
        iic.write(0, 0x80, clocks);
        iic.write(7, 0xfc, clocks);
        iic.write(1, 2, clocks);
        iic.write(0x12, 0xa0, clocks);
        assert_eq!(iic.read(9) & END, 0);
        iic.tick(|event, high| events.push((event, high)));
        assert_eq!(iic.read(9) & END, END);
        iic.write(0x12, 0x34, clocks);
        iic.write(0x12, 0xc5, clocks);
        iic.write(1, 8, clocks);
        iic.write(1, 2, clocks);
        iic.write(0x12, 0xa0, clocks);
        iic.tick(|_, _| {});
        iic.write(0x12, 0x34, clocks);
        iic.write(1, 4, clocks);
        iic.write(0x12, 0xa1, clocks);
        iic.tick(|event, high| events.push((event, high)));
        assert_eq!(iic.read(0x13), 0);
        iic.write(1, 8, clocks);
        assert_eq!(iic.read(0x13), 0xc5);
        assert_eq!(iic.read(9) & STOP, STOP);
        assert!(events.contains(&(21, true)));
        iic.write(1, 2, clocks);
        iic.write(0x12, 0xa2, clocks);
        iic.tick(|_, _| {});
        assert_eq!(iic.read(9) & NACK, NACK);
    }
}
