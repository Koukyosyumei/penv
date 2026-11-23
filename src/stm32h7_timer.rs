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
    pub irq: [Option<Stm32Irq>; 5],
    input: [bool; 4],
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
            irq: [None; 5],
            input: [false; 4],
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

    pub fn set_frequency(&mut self, frequency: u32) {
        self.frequency = frequency;
    }

    pub fn reset(&mut self) {
        let irq = self.irq;
        *self = Self::new(self.base, self.frequency, self.limit, self.clock);
        self.irq = irq;
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    pub fn interrupts(&self) -> impl Iterator<Item = Stm32Irq> + '_ {
        let pending = self.registers[3] & self.registers[4] & 0xff;
        let masks = [0xff, 1 << 7, 1, (1 << 5) | (1 << 6), 0x1e];
        self.irq
            .into_iter()
            .zip(masks)
            .filter_map(move |(irq, mask)| irq.filter(|_| pending & mask != 0))
    }

    fn channel_mode(&self, channel: usize) -> u32 {
        self.registers[6 + channel / 2] >> (channel % 2 * 8)
    }

    pub fn output(&self, channel: usize) -> Option<bool> {
        let ccer = self.registers[8] >> (channel * 4);
        if ccer & 1 == 0 || self.channel_mode(channel) & 3 != 0 {
            return None;
        }
        if matches!(self.base, 0x4001_0000 | 0x4001_0400) && self.registers[17] & (1 << 15) == 0 {
            return None;
        }
        let high = match (self.channel_mode(channel) >> 4) & 7 {
            4 => false,
            5 => true,
            6 => self.counter < self.registers[13 + channel],
            7 => self.counter >= self.registers[13 + channel],
            _ => return None,
        };
        Some(high ^ (ccer & 2 != 0))
    }

    pub fn sample_input(&mut self, channel: usize, high: bool) {
        let previous = std::mem::replace(&mut self.input[channel], high);
        let ccer = self.registers[8] >> (channel * 4);
        if self.registers[0] & CEN == 0
            || self.channel_mode(channel) & 3 != 1
            || ccer & 1 == 0
            || previous == high
        {
            return;
        }
        if ccer & 0xa == 0xa || high != (ccer & 2 != 0) {
            self.registers[13 + channel] = self.counter;
            self.registers[4] |= 2 << channel;
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advanced_timer_routes_input_capture_and_update_to_distinct_interrupts() {
        let mut timer = Timer::new(0, 1_000_000, 0xffff, None);
        timer.irq[2] = Some(Stm32Irq::Nvic(25));
        timer.irq[4] = Some(Stm32Irq::Nvic(27));
        timer.write(0x0c, 3);
        timer.write(0x18, 1);
        timer.write(0x20, 1);
        timer.write(0, CEN);
        timer.tick();
        timer.sample_input(0, true);
        assert_eq!(timer.read(0x34), 1);
        assert!(matches!(
            timer.interrupts().next(),
            Some(Stm32Irq::Nvic(27))
        ));
        timer.write(0x10, 0);
        timer.write(0x14, 1);
        assert!(matches!(
            timer.interrupts().next(),
            Some(Stm32Irq::Nvic(25))
        ));
    }
}
