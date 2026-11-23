use std::collections::{BTreeMap, VecDeque};

use crate::stm32_uart::Stm32Irq;

const PE: u32 = 1;
const START: u32 = 1 << 8;
const STOP: u32 = 1 << 9;
const ACK: u32 = 1 << 10;
const POS: u32 = 1 << 11;
const SWRST: u32 = 1 << 15;

const ITERREN: u32 = 1 << 8;
const ITEVTEN: u32 = 1 << 9;
const ITBUFEN: u32 = 1 << 10;

const SB: u32 = 1;
const ADDR: u32 = 1 << 1;
const BTF: u32 = 1 << 2;
const RXNE: u32 = 1 << 6;
const TXE: u32 = 1 << 7;
const AF: u32 = 1 << 10;

const MSL: u32 = 1;
const BUSY: u32 = 1 << 1;
const TRA: u32 = 1 << 2;
const RESET_REGISTERS: [u32; 9] = [0, 0, 0, 0, 0, 0, 0, 0, 2];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Idle,
    Address,
    Transmit(u8),
    Receive(u8),
    Nack,
}

enum Target {
    Echo(Vec<u8>),
    Hs3001(Hs3001),
}

#[derive(Default)]
struct Hs3001 {
    temperature: f64,
    humidity: f64,
}

impl Hs3001 {
    fn measurement(&self) -> [u8; 4] {
        // The pinned Renode HS3001 model quantizes the physical value before
        // applying the 14-bit transfer function, including for negative values.
        fn quantize(value: f64) -> f64 {
            if value == 0.0 {
                return 0.0;
            }
            let divisor = (16384.0 / value).floor();
            if divisor == 0.0 {
                0.0
            } else {
                16384.0 / divisor
            }
        }
        fn encode(value: f64) -> u16 {
            value.clamp(0.0, 16383.0).round_ties_even() as u16
        }

        let humidity = encode(quantize(self.humidity) * 16383.0 / 100.0);
        let temperature = encode((quantize(self.temperature) + 40.0) * 16383.0 / 165.0);
        let [rh_hi, rh_lo] = humidity.to_be_bytes();
        let [temp_hi, temp_lo] = (temperature << 2).to_be_bytes();
        [rh_hi, rh_lo, temp_hi, temp_lo]
    }
}

pub struct F1I2c {
    pub base: u32,
    pub event: Option<Stm32Irq>,
    pub error: Option<Stm32Irq>,
    targets: BTreeMap<u8, Target>,
    registers: [u32; 9],
    phase: Phase,
    status: u32,
    saw_sr1: bool,
    transferred: usize,
    receive: VecDeque<u8>,
    last_byte: bool,
}

impl F1I2c {
    pub const MODEL: &str = "I2C.STM32F1_I2C";
    pub const ECHO_MODEL: &str = "Mocks.EchoI2CDevice";
    pub const HS3001_MODEL: &str = "Sensors.HS3001";
    pub const SIZE: u32 = 0x400;

    pub fn new(base: u32) -> Self {
        Self {
            base,
            event: None,
            error: None,
            targets: BTreeMap::new(),
            registers: RESET_REGISTERS,
            phase: Phase::Idle,
            status: 0,
            saw_sr1: false,
            transferred: 0,
            receive: VecDeque::new(),
            last_byte: false,
        }
    }

    pub fn reset(&mut self) {
        self.registers = RESET_REGISTERS;
        self.begin(Phase::Idle);
        self.status = 0;
    }

    pub fn attach_echo(&mut self, address: u8) -> bool {
        self.attach(address, Target::Echo(Vec::new()))
    }

    pub fn attach_hs3001(&mut self, address: u8) -> bool {
        self.attach(address, Target::Hs3001(Hs3001::default()))
    }

    fn attach(&mut self, address: u8, device: Target) -> bool {
        let std::collections::btree_map::Entry::Vacant(target) = self.targets.entry(address) else {
            return false;
        };
        target.insert(device);
        true
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    pub fn interrupts(&self) -> impl Iterator<Item = Stm32Irq> {
        let control = self.registers[1];
        let status = self.status();
        let event = control & ITEVTEN != 0
            && (status & (SB | ADDR | BTF) != 0
                || (control & ITBUFEN != 0 && status & (RXNE | TXE) != 0));
        let error = control & ITERREN != 0 && status & AF != 0;
        [self.event.filter(|_| event), self.error.filter(|_| error)]
            .into_iter()
            .flatten()
    }

    fn status(&self) -> u32 {
        self.status
            | (u32::from(!self.receive.is_empty()) * RXNE)
            | (u32::from(self.receive.len() == 2) * BTF)
    }

    fn begin(&mut self, phase: Phase) {
        self.phase = phase;
        self.status &= AF;
        self.saw_sr1 = false;
        self.transferred = 0;
        self.receive.clear();
        self.last_byte = false;
    }

    fn fill_receive(&mut self, limit: usize) {
        let Phase::Receive(address) = self.phase else {
            return;
        };
        let measurement;
        let data: &[u8] = match &self.targets[&address] {
            Target::Echo(data) => data,
            Target::Hs3001(sensor) => {
                measurement = sensor.measurement();
                &measurement
            }
        };
        while self.receive.len() < limit {
            self.receive
                .push_back(data.get(self.transferred).copied().unwrap_or(0));
            self.transferred += 1;
        }
    }

    fn read_data(&mut self) -> u32 {
        let byte = self.receive.pop_front().unwrap_or(0);
        if self.status & ADDR == 0
            && (self.registers[0] & ACK != 0 || std::mem::take(&mut self.last_byte))
        {
            self.fill_receive(2);
        }
        u32::from(byte)
    }

    pub fn read(&mut self, address: u32, width: usize) -> u32 {
        let offset = address - self.base;
        let value = match offset & !3 {
            0x10 if offset == 0x10 => self.read_data(),
            0x14 => {
                if offset == 0x14 {
                    self.saw_sr1 = self.status & (SB | ADDR) != 0;
                }
                self.status()
            }
            0x18 => {
                let value = if self.phase == Phase::Idle {
                    0
                } else {
                    MSL | BUSY | (u32::from(matches!(self.phase, Phase::Transmit(_))) * TRA)
                };
                if offset == 0x18 && self.status & ADDR != 0 && std::mem::take(&mut self.saw_sr1) {
                    self.status &= !ADDR;
                    if matches!(self.phase, Phase::Transmit(_)) {
                        self.status |= TXE;
                    } else {
                        self.fill_receive(if self.registers[0] & (ACK | POS) == 0 {
                            1
                        } else {
                            2
                        });
                    }
                }
                value
            }
            _ => self
                .registers
                .get((offset / 4) as usize)
                .copied()
                .unwrap_or(0),
        };
        (value >> ((offset & 3) * 8)) & (u32::MAX >> (32 - width * 8))
    }

    pub fn write(&mut self, address: u32, width: usize, value: u32) {
        let offset = address - self.base;
        let shift = (offset & 3) * 8;
        let lanes = (u32::MAX >> (32 - width * 8)) << shift;
        let bits = (value << shift) & lanes;
        let mask = match offset & !3 {
            0 => 0xbffb,
            4 => 0x1f3f,
            8 => 0xc3ff,
            0x0c => 0xff,
            0x10 if offset == 0x10 => {
                self.write_data(bits as u8);
                return;
            }
            0x14 => {
                self.status &= !((!bits) & lanes & AF);
                return;
            }
            0x1c => 0xcfff,
            0x20 => 0x3f,
            _ => return,
        };
        let register = &mut self.registers[(offset / 4) as usize];
        let previous = *register;
        *register = (previous & !lanes) | (bits & mask);
        if offset & !3 == 0 {
            self.apply_control(previous);
        }
    }

    fn apply_control(&mut self, previous: u32) {
        let control = self.registers[0];
        self.registers[0] &= !(START | STOP);
        if control & SWRST != 0 {
            self.reset();
            self.registers[0] = SWRST;
        } else if control & PE == 0 {
            self.begin(Phase::Idle);
            self.status = 0;
        } else if control & STOP != 0 {
            // A receiver may issue STOP before consuming the last buffered bytes.
            self.phase = Phase::Idle;
            self.status &= AF;
            self.saw_sr1 = false;
            self.last_byte = false;
        } else if control & START != 0 {
            self.begin(Phase::Address);
            self.status |= SB;
        } else if previous & ACK != 0
            && control & (ACK | POS) == 0
            && matches!(self.phase, Phase::Receive(_))
            && self.status & ADDR == 0
        {
            self.last_byte = !self.receive.is_empty();
        }
    }

    fn write_data(&mut self, value: u8) {
        match self.phase {
            Phase::Address if std::mem::take(&mut self.saw_sr1) => {
                let address = value >> 1;
                self.status &= !SB;
                if self.targets.contains_key(&address) {
                    self.phase = if value & 1 == 0 {
                        Phase::Transmit(address)
                    } else {
                        Phase::Receive(address)
                    };
                    self.status |= ADDR;
                } else {
                    self.phase = Phase::Nack;
                    self.status |= AF;
                }
            }
            Phase::Transmit(address) if self.status & ADDR == 0 => {
                if let Target::Echo(data) = self.targets.get_mut(&address).unwrap() {
                    if self.transferred == 0 {
                        data.clear();
                    }
                    data.push(value);
                }
                self.transferred += 1;
                self.status |= TXE | BTF;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn address(i2c: &mut F1I2c, byte: u32, control: u32) {
        i2c.write(0, 2, PE | START | control);
        assert_eq!(i2c.read(0x14, 2), SB);
        i2c.write(0x10, 2, byte);
    }

    #[test]
    fn echo_repeated_start_reads_twenty_bytes_and_zero_padding() {
        // Payload and response length from the Renode Zedboard echo test.
        let mut i2c = F1I2c::new(0);
        assert!(i2c.attach_echo(0x10));
        address(&mut i2c, 0x20, 0);
        assert_eq!(i2c.read(0x18, 2), MSL | BUSY | TRA);
        assert_eq!(i2c.read(0x14, 2), ADDR);
        assert_eq!(i2c.read(0x18, 2), MSL | BUSY | TRA);
        assert_eq!(i2c.read(0x14, 2), TXE);
        for byte in 1..=20 {
            i2c.write(0x10, 2, byte);
            assert_eq!(i2c.read(0x14, 2), TXE | BTF);
        }

        address(&mut i2c, 0x21, ACK);
        assert_eq!(i2c.read(0x14, 2), ADDR);
        assert_eq!(i2c.read(0x18, 2), MSL | BUSY);
        for byte in 1..=18 {
            assert_eq!(i2c.read(0x14, 2), RXNE | BTF);
            assert_eq!(i2c.read(0x10, 2), byte);
        }
        // Three bytes remain. Clear ACK, read one byte, then STOP and drain two.
        i2c.write(0, 2, PE);
        assert_eq!(i2c.read(0x10, 2), 19);
        assert_eq!(i2c.read(0x14, 2), RXNE | BTF);
        i2c.write(0, 2, PE | STOP);
        assert_eq!(i2c.read(0x10, 2), 20);
        assert_eq!(i2c.read(0x10, 2), 0);
        assert_eq!(i2c.read(0x14, 2), 0);
        assert_eq!(i2c.read(0x18, 2), 0);
    }

    #[test]
    fn nack_stays_on_error_line_until_cleared_and_reset_restores_registers() {
        let mut i2c = F1I2c::new(0);
        i2c.event = Some(Stm32Irq::Nvic(31));
        i2c.error = Some(Stm32Irq::Nvic(32));
        i2c.write(4, 4, ITERREN | ITEVTEN | ITBUFEN | 16);
        i2c.write(0x20, 2, 17);
        address(&mut i2c, 0x20, 0);
        assert_eq!(i2c.read(0x14, 4), AF);
        assert_eq!(i2c.read(0x14, 4), AF);
        assert!(matches!(i2c.interrupts().next(), Some(Stm32Irq::Nvic(32))));
        i2c.write(0x14, 1, 0);
        assert_eq!(i2c.read(0x14, 2), AF);
        i2c.write(0x14, 2, !AF);
        assert!(i2c.interrupts().next().is_none());
        i2c.write(0, 2, SWRST);
        assert_eq!(i2c.read(0x18, 2), 0);
        assert_eq!(i2c.read(4, 2), 0);
        assert_eq!(i2c.read(0x20, 2), 2);
    }
}
