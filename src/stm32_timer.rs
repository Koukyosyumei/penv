use crate::bus::CORTEX_M_STEP_HZ;

const ENABLE: u32 = 1;
const UPDATE_DISABLE: u32 = 1 << 1;
const UPDATE_OVERFLOW_ONLY: u32 = 1 << 2;
const ONE_PULSE: u32 = 1 << 3;
const DOWN: u32 = 1 << 4;
const CENTER: u32 = 3 << 5;
const ARR_PRELOAD: u32 = 1 << 7;

pub struct TimerPin {
    pub port: u32,
    pub pin: u8,
    pub function: u8,
}

pub struct Stm32Timer {
    pub base: u32,
    frequency: u32,
    initial_limit: u32,
    counter_mask: u32,
    routes: [u32; 8],
    pub dma_routes: [u8; 8],
    dma_requests: u8,
    pub gpio_outputs: Vec<(u8, TimerPin)>,
    output_reference: u8,
    output_enable: u8,
    output_levels: u8,
    output_changes: u8,
    capture_levels: u8,
    pub capture_next: u8,
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
            gpio_outputs: Vec::new(),
            output_reference: 0,
            output_enable: 0,
            output_levels: 0,
            output_changes: 0,
            capture_levels: 0,
            capture_next: 0,
            phase: 0,
            prescaler: 0,
            counter: 0,
            initial_limit,
            arr: initial_limit,
            active_arr: initial_limit,
            repetition: 0,
            registers: [0; 18],
        }
    }

    pub fn reset(&mut self) {
        let outputs = std::mem::take(&mut self.gpio_outputs);
        let dma = self.dma_routes;
        *self = Self::new(self.base, self.frequency, self.initial_limit, self.routes);
        self.gpio_outputs = outputs;
        self.dma_routes = dma;
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
        if matches!(
            offset,
            0x00 | 0x14 | 0x18 | 0x1c | 0x20 | 0x24 | 0x2c | 0x34..=0x40
        ) {
            self.update_outputs(0);
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
            while self.phase >= CORTEX_M_STEP_HZ
                && self.registers[0] & ENABLE != 0
                && self.active_arr != 0
            {
                self.phase -= CORTEX_M_STEP_HZ;
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

        let mut matches = 0;
        for channel in 0..4 {
            if self.channel_mode(channel) & 3 == 0 && self.counter == self.registers[13 + channel] {
                matches |= 1 << channel;
            }
        }
        if matches != 0 && !matches!(((control & CENTER) >> 5, down), (1, false) | (2, true)) {
            self.raise_events(u32::from(matches) << 1);
        }
        self.update_outputs(matches);
    }

    fn channel_mode(&self, channel: usize) -> u32 {
        (self.registers[6 + channel / 2] >> ((channel % 2) * 8)) & 0xff
    }

    fn update_outputs(&mut self, matches: u8) {
        let mut enabled = 0;
        let mut levels = 0;
        for channel in 0..4 {
            let mode = self.channel_mode(channel);
            let flags = (self.registers[8] >> (channel * 4)) & 0xf;
            if mode & 3 != 0 || flags & 1 == 0 {
                continue;
            }
            let bit = 1 << channel;
            let old = self.output_reference & bit != 0;
            let matched = matches & bit != 0;
            let compare = self.registers[13 + channel];
            let pwm = if self.registers[0] & DOWN == 0 {
                self.counter < compare
            } else {
                self.counter <= compare
            };
            let high = match (mode >> 4) & 7 {
                1 if matched => true,
                2 if matched => false,
                3 if matched => !old,
                4 => false,
                5 => true,
                6 => pwm,
                7 => !pwm,
                _ => old,
            };
            self.output_reference = (self.output_reference & !bit) | (u8::from(high) << channel);
            enabled |= bit;
            levels |= u8::from(high ^ (flags & 2 != 0)) << channel;
        }
        self.output_changes |= (self.output_enable ^ enabled) | (self.output_levels ^ levels);
        self.output_enable = enabled;
        self.output_levels = levels;
    }

    pub fn take_output_changes(&mut self) -> u8 {
        std::mem::take(&mut self.output_changes)
    }

    pub fn pin_output(&self, channel: u8) -> Option<bool> {
        let bit = 1 << channel;
        (self.output_enable & bit != 0).then_some(self.output_levels & bit != 0)
    }

    pub fn apply_capture_inputs(&mut self) {
        let changed = self.capture_levels ^ self.capture_next;
        self.capture_levels = self.capture_next;
        if changed == 0 {
            return;
        }
        for channel in 0..4 {
            let input = match self.channel_mode(channel) & 3 {
                1 => channel,
                2 => channel ^ 1,
                _ => continue,
            };
            let bit = 1 << input;
            let flags = (self.registers[8] >> (channel * 4)) & 0xf;
            if flags & 1 == 0 || changed & bit == 0 {
                continue;
            }
            let high = self.capture_levels & bit != 0;
            if flags & 8 == 0 && high == (flags & 2 != 0) {
                continue;
            }
            let signal = 1 << (channel + 1);
            if self.registers[4] & signal != 0 {
                self.registers[4] |= 1 << (channel + 9);
            }
            self.registers[13 + channel] = self.counter;
            self.raise_events(signal);
        }
    }
}
