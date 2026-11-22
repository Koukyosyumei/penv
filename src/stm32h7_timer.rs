use crate::{machine::VIRTUAL_STEPS_PER_SECOND, stm32_uart::Stm32Irq};

const CEN: u32 = 1;
const UDIS: u32 = 1 << 1;
const URS: u32 = 1 << 2;
const OPM: u32 = 1 << 3;
const DIR: u32 = 1 << 4;
const ARPE: u32 = 1 << 7;

/// STM32 general-purpose timer counter, update and output-compare registers.
pub struct Timer {
    pub base: u32,
    pub clock: Option<u8>,
    pub irq: Option<Stm32Irq>,
    frequency: u32,
    limit: u32,
    phase: u64,
    divider: u32,
    counter: u32,
    active_arr: u32,
    active_psc: u32,
    registers: [u32; 18],
}

impl Timer {
    pub const MODEL: &str = "Timers.STM32_Timer";
    pub const SIZE: u32 = 0x400;

    pub fn new(base: u32, frequency: u32, limit: u32, clock: Option<u8>) -> Self {
        let mut timer = Self {
            base,
            frequency,
            limit,
            clock,
            irq: None,
            phase: 0,
            divider: 0,
            counter: 0,
            active_arr: limit,
            active_psc: 0,
            registers: [0; 18],
        };
        timer.registers[0x2c / 4] = limit;
        timer
    }

    pub fn reset(&mut self) {
        let irq = self.irq;
        *self = Self::new(self.base, self.frequency, self.limit, self.clock);
        self.irq = irq;
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    pub fn interrupt(&self) -> Option<Stm32Irq> {
        self.irq
            .filter(|_| self.registers[3] & self.registers[4] & 0xff != 0)
    }

    pub fn read(&self, address: u32) -> u32 {
        match address - self.base {
            0x14 => 0,
            0x24 => self.counter,
            offset => self
                .registers
                .get((offset / 4) as usize)
                .copied()
                .unwrap_or(0),
        }
    }

    pub fn write(&mut self, address: u32, value: u32) {
        let offset = address - self.base;
        let index = (offset / 4) as usize;
        match offset {
            0x00 => {
                self.registers[0] = value & 0x3ff;
                if value & ARPE == 0 {
                    self.active_arr = self.registers[0x2c / 4];
                }
            }
            0x10 => self.registers[index] &= value,
            0x14 => {
                self.registers[4] |= value & 0xfe;
                if value & 1 != 0 {
                    self.divider = 0;
                    self.reload_period();
                    self.counter = if self.registers[0] & DIR == 0 {
                        0
                    } else {
                        self.active_arr
                    };
                    if self.registers[0] & (UDIS | URS) == 0 {
                        self.registers[4] |= 1;
                    }
                }
            }
            0x24 => self.counter = value & self.limit,
            0x28 => self.registers[index] = value & 0xffff,
            0x2c => {
                self.registers[index] = value & self.limit;
                if self.registers[0] & ARPE == 0 {
                    self.active_arr = self.registers[index];
                }
            }
            0x34..=0x40 => self.registers[index] = value & self.limit,
            _ => {
                if let Some(register) = self.registers.get_mut(index) {
                    *register = value;
                }
            }
        }
    }

    fn reload_period(&mut self) {
        if self.registers[0] & UDIS == 0 {
            self.active_arr = self.registers[0x2c / 4];
            self.active_psc = self.registers[0x28 / 4];
        }
    }

    pub fn tick(&mut self) {
        if self.registers[0] & CEN == 0 || self.active_arr == 0 {
            return;
        }
        let step_hz = u64::from(VIRTUAL_STEPS_PER_SECOND);
        self.phase += u64::from(self.frequency);
        let cycles = self.phase / step_hz;
        self.phase %= step_hz;
        for _ in 0..cycles {
            if self.divider < self.active_psc {
                self.divider += 1;
                continue;
            }
            self.divider = 0;
            let down = self.registers[0] & DIR != 0;
            let overflow = if down {
                self.counter == 0
            } else {
                self.counter >= self.active_arr
            };
            if overflow {
                self.reload_period();
                self.counter = if down { self.active_arr } else { 0 };
                if self.registers[0] & UDIS == 0 {
                    self.registers[4] |= 1;
                    if self.registers[0] & OPM != 0 {
                        self.registers[0] &= !CEN;
                        break;
                    }
                }
            } else if down {
                self.counter -= 1;
            } else {
                self.counter += 1;
            }
            for channel in 0..4 {
                let mode = self.registers[6 + channel / 2] >> (channel % 2 * 8);
                if mode & 3 == 0 && self.counter == self.registers[13 + channel] {
                    self.registers[4] |= 2 << channel;
                }
            }
        }
    }
}
