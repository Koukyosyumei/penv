use crate::bus::STM32_STEP_HZ;

const ENABLE: u32 = 1;
const UPDATE_DISABLE: u32 = 1 << 1;
const UPDATE_OVERFLOW_ONLY: u32 = 1 << 2;
const ONE_PULSE: u32 = 1 << 3;
const DOWN: u32 = 1 << 4;
const CENTER: u32 = 3 << 5;
const ARR_PRELOAD: u32 = 1 << 7;

pub struct Stm32Timer {
    pub base: u32,
    frequency: u32,
    counter_mask: u32,
    routes: [u32; 8],
    pub dma_routes: [u8; 8],
    dma_requests: u8,
    phase: u64,
    prescaler: u32,
    counter: u32,
    arr: u32,
    active_arr: u32,
    repetition: u32,
    registers: [u32; 18],
}

impl Stm32Timer {
    pub const SIZE: u32 = 0x400;

    pub fn new(base: u32, frequency: u32, initial_limit: u32, routes: [u32; 8]) -> Self {
        Self {
            base,
            frequency,
            counter_mask: if initial_limit > 0xffff {
                u32::MAX
            } else {
                0xffff
            },
            routes,
            dma_routes: [0; 8],
            dma_requests: 0,
            phase: 0,
            prescaler: 0,
            counter: 0,
            arr: initial_limit,
            active_arr: initial_limit,
            repetition: 0,
            registers: [0; 18],
        }
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    pub fn read(&self, address: u32) -> u32 {
        match address - self.base {
            0x14 => 0,
            0x24 => self.counter,
            0x2c => self.arr,
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
            0 => {
                let previous = self.registers[0];
                let direction = if previous & CENTER != 0 {
                    previous
                } else {
                    value
                };
                self.registers[0] = (value & 0x3ef) | (direction & DOWN);
                if value & ARR_PRELOAD == 0 {
                    self.active_arr = self.arr;
                }
            }
            0x04 | 0x08 | 0x18 | 0x1c | 0x20 | 0x44 => self.registers[index] = value & 0xffff,
            0x0c => self.registers[index] = value & 0x7fff,
            0x10 => self.registers[index] &= value,
            0x14 => {
                self.raise_events(value & 0xfe);
                if value & 1 != 0 && self.registers[0] & UPDATE_DISABLE == 0 {
                    self.active_arr = self.arr;
                    self.counter = if self.registers[0] & DOWN == 0 {
                        0
                    } else {
                        self.active_arr
                    };
                    self.phase = 0;
                    self.prescaler = 0;
                    self.repetition = self.registers[12];
                    if self.registers[0] & UPDATE_OVERFLOW_ONLY == 0 {
                        self.raise_events(1);
                    }
                }
            }
            0x24 => self.counter = value & self.counter_mask,
            0x28 => {
                self.registers[index] = value & 0xffff;
                self.prescaler = 0;
            }
            0x2c => {
                self.arr = value & self.counter_mask;
                if self.registers[0] & ARR_PRELOAD == 0 {
                    self.active_arr = self.arr;
                }
            }
            0x30 => self.registers[index] = value & 0xff,
            0x34..=0x40 => self.registers[index] = value & self.counter_mask,
            _ => {}
        }
    }

    pub fn write_halfword(&mut self, address: u32, value: u32) {
        let word = address & !3;
        let shift = (address & 2) * 8;
        let mask = 0xffff << shift;
        let retained = if word - self.base == 0x10 {
            u32::MAX
        } else {
            self.read(word)
        };
        self.write(word, (retained & !mask) | ((value & 0xffff) << shift));
    }

    pub fn tick(&mut self) -> u32 {
        if self.registers[0] & ENABLE != 0 && self.active_arr != 0 {
            self.phase += u64::from(self.frequency);
            while self.phase >= STM32_STEP_HZ
                && self.registers[0] & ENABLE != 0
                && self.active_arr != 0
            {
                self.phase -= STM32_STEP_HZ;
                if self.prescaler >= self.registers[10] {
                    self.prescaler = 0;
                    self.count();
                } else {
                    self.prescaler += 1;
                }
            }
        }
        let mut pending = (self.registers[3] & self.registers[4]) & 0xff;
        let mut irqs = 0;
        while pending != 0 {
            irqs |= self.routes[pending.trailing_zeros() as usize];
            pending &= pending - 1;
        }
        irqs
    }

    pub fn take_dma_requests(&mut self) -> u8 {
        std::mem::take(&mut self.dma_requests)
    }

    fn raise_events(&mut self, events: u32) {
        self.registers[4] |= events;
        let mut enabled = events & (self.registers[3] >> 8) & 0x7f;
        while enabled != 0 {
            self.dma_requests |= self.dma_routes[enabled.trailing_zeros() as usize];
            enabled &= enabled - 1;
        }
    }

    fn count(&mut self) {
        let control = self.registers[0];
        let down = control & DOWN != 0;
        let center = control & CENTER != 0;
        let boundary = if down {
            self.counter <= u32::from(center)
        } else {
            self.counter >= self.active_arr - u32::from(center)
        };
        if boundary {
            if center {
                self.counter = if down { 0 } else { self.active_arr };
                self.registers[0] ^= DOWN;
            } else {
                self.counter = if down { self.active_arr } else { 0 };
            }
            if control & UPDATE_DISABLE == 0 {
                if self.repetition == 0 {
                    self.repetition = self.registers[12];
                    self.active_arr = self.arr;
                    if down && !center {
                        self.counter = self.active_arr;
                    }
                    self.raise_events(1);
                    if control & ONE_PULSE != 0 {
                        self.registers[0] &= !ENABLE;
                        self.phase = 0;
                    }
                } else {
                    self.repetition -= 1;
                }
            }
        } else if down {
            self.counter -= 1;
        } else {
            self.counter += 1;
        }

        if !matches!(((control & CENTER) >> 5, down), (1, false) | (2, true)) {
            for channel in 0..4 {
                let mode = self.registers[6 + channel / 2] >> ((channel % 2) * 8);
                if mode & 3 == 0 && self.counter == self.registers[13 + channel] {
                    self.raise_events(1 << (channel + 1));
                }
            }
        }
    }
}
