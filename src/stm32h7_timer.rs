use crate::stm32_uart::Stm32Irq;

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
    capture_edges: [u8; 4],
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
            capture_edges: [0; 4],
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

    fn channel_enable(&self, channel: usize) -> u32 {
        self.registers[8] >> (channel * 4)
    }

    fn encoder_mode(&self) -> u32 {
        self.registers[2] & 7
    }

    fn active_edge(&self, channel: usize, previous: bool, high: bool) -> bool {
        let ccer = self.channel_enable(channel);
        previous != high && (ccer & 0xa == 0xa || high != (ccer & 2 != 0))
    }

    pub fn sample_inputs(&mut self, levels: [bool; 4]) {
        let previous = std::mem::replace(&mut self.input, levels);
        if self.registers[0] & CEN == 0 || previous == levels {
            return;
        }

        for channel in 0..4 {
            let mode = self.channel_mode(channel);
            let input = match mode & 3 {
                1 => channel,
                2 => channel ^ 1,
                _ => continue,
            };
            if self.channel_enable(channel) & 1 == 0
                || !self.active_edge(channel, previous[input], levels[input])
            {
                continue;
            }
            self.capture_edges[channel] += 1;
            if self.capture_edges[channel] >= 1 << ((mode >> 2) & 3) {
                self.capture_edges[channel] = 0;
                let flag = 2 << channel;
                if self.registers[4] & flag != 0 {
                    self.registers[4] |= flag << 8;
                }
                self.registers[13 + channel] = self.counter;
                self.registers[4] |= flag;
            }
        }

        match self.encoder_mode() {
            mode @ 1..=3 => self.sample_encoder(mode, previous, levels),
            4 => {
                let input = match (self.registers[2] >> 4) & 7 {
                    5 => 0,
                    6 => 1,
                    _ => return,
                };
                if self.active_edge(input, previous[input], levels[input]) {
                    self.counter = 0;
                    self.divider = 0;
                    self.reload_period();
                    self.registers[4] |= 1 << 6;
                    if self.registers[0] & (UDIS | URS) == 0 {
                        self.registers[4] |= 1;
                    }
                }
            }
            _ => {}
        }
    }

    fn sample_encoder(&mut self, mode: u32, previous: [bool; 4], levels: [bool; 4]) {
        let [changed_a, changed_b] = [previous[0] != levels[0], previous[1] != levels[1]];
        if changed_a == changed_b || self.active_arr == 0 || self.registers[8] & 0x11 != 0x11 {
            return;
        }
        let a = levels[0] ^ (self.channel_enable(0) & 2 != 0);
        let b = levels[1] ^ (self.channel_enable(1) & 2 != 0);
        let down = (a == b) == changed_a;
        self.registers[0] = (self.registers[0] & !DIR) | (u32::from(down) * DIR);
        if mode == 3 || (mode == 1) == changed_a {
            self.count_clock();
        }
    }

    pub fn read_halfword(&mut self, address: u32) -> u32 {
        (self.read(address & !3) >> ((address & 2) * 8)) & 0xffff
    }

    pub fn write_halfword(&mut self, address: u32, value: u32) {
        let aligned = address & !3;
        let offset = aligned - self.base;
        let shift = (address & 2) * 8;
        let mask = 0xffff << shift;
        let unchanged = match offset {
            0x10 => u32::MAX, // Status bits are cleared by writing zero.
            0x14 => 0,        // EGR is write-only and its bits trigger events.
            0x24 => self.counter,
            _ => self
                .registers
                .get((offset / 4) as usize)
                .copied()
                .unwrap_or(0),
        };
        self.write(aligned, (unchanged & !mask) | ((value & 0xffff) << shift));
    }

    pub fn read(&mut self, address: u32) -> u32 {
        match address - self.base {
            0x14 => 0,
            0x24 => self.counter,
            offset @ (0x34..=0x40) => {
                let channel = ((offset - 0x34) / 4) as usize;
                if self.channel_mode(channel) & 3 != 0 {
                    self.registers[4] &= !(2 << channel);
                }
                self.registers[13 + channel]
            }
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
            0x20 => {
                self.registers[index] = value;
                for channel in 0..4 {
                    if self.channel_enable(channel) & 1 == 0 {
                        self.capture_edges[channel] = 0;
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

    pub fn tick(&mut self, step_hz: u32) {
        if self.registers[0] & CEN == 0
            || self.active_arr == 0
            || matches!(self.encoder_mode(), 1..=3)
        {
            return;
        }
        let step_hz = u64::from(step_hz);
        self.phase += u64::from(self.frequency);
        let cycles = self.phase / step_hz;
        self.phase %= step_hz;
        for _ in 0..cycles {
            self.count_clock();
            if self.registers[0] & CEN == 0 {
                break;
            }
        }
    }

    fn count_clock(&mut self) {
        if self.divider < self.active_psc {
            self.divider += 1;
            return;
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
                    return;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paired_capture_latches_pulse_and_period_before_reset() {
        let mut timer = Timer::new(0, 1_000_000, 0xffff, None);
        timer.write(0x08, (5 << 4) | 4);
        timer.write(0x18, (2 << 8) | 1);
        timer.write(0x20, 0x31);
        timer.write(0, CEN | URS);

        timer.tick(1_000_000);
        timer.sample_inputs([true, false, false, false]);
        assert_eq!(timer.read(0x24), 0);
        assert_eq!(timer.read(0x34), 1);
        for _ in 0..3 {
            timer.tick(1_000_000);
        }
        timer.sample_inputs([false; 4]);
        assert_eq!(timer.read(0x38), 3);
        for _ in 0..7 {
            timer.tick(1_000_000);
        }
        timer.sample_inputs([true, false, false, false]);
        assert_eq!(timer.read(0x34), 10);
        assert_eq!(timer.read(0x24), 0);
        assert_eq!(timer.read(0x10) & 1, 0);
    }

    #[test]
    fn encoder_counts_valid_phases_in_both_directions_and_wraps() {
        let mut timer = Timer::new(0, 1_000_000, 0xffff, None);
        timer.write(0x08, 3);
        timer.write(0x18, 0x101);
        timer.write(0x20, 0x11);
        timer.write(0x2c, 3);
        timer.write(0, CEN);
        timer.tick(1_000_000);
        assert_eq!(timer.read(0x24), 0);
        let forward = [[true, false], [true, true], [false, true], [false, false]];
        for (edge, [a, b]) in forward.into_iter().enumerate() {
            timer.sample_inputs([a, b, false, false]);
            assert_eq!(timer.read(0x24), (edge as u32 + 1) % 4);
            assert_eq!(timer.read(0) & DIR, 0);
        }
        timer.sample_inputs([false, true, false, false]);
        assert_eq!(timer.read(0x24), 3);
        assert_eq!(timer.read(0) & DIR, DIR);
        timer.sample_inputs([true, false, false, false]); // Both phases changed.
        assert_eq!(timer.read(0x24), 3);
    }

    #[test]
    fn advanced_timer_routes_input_capture_and_update_to_distinct_interrupts() {
        let mut timer = Timer::new(0, 1_000_000, 0xffff, None);
        timer.irq[2] = Some(Stm32Irq::Nvic(25));
        timer.irq[4] = Some(Stm32Irq::Nvic(27));
        timer.write(0x0c, 3);
        timer.write(0x18, 1);
        timer.write(0x20, 1);
        timer.write(0, CEN);
        timer.tick(crate::machine::VIRTUAL_STEPS_PER_SECOND);
        timer.sample_inputs([true, false, false, false]);
        assert!(matches!(
            timer.interrupts().next(),
            Some(Stm32Irq::Nvic(27))
        ));
        assert_eq!(timer.read(0x34), 1);
        assert!(timer.interrupts().next().is_none());
        timer.write(0x10, 0);
        timer.write(0x14, 1);
        assert!(matches!(
            timer.interrupts().next(),
            Some(Stm32Irq::Nvic(25))
        ));
    }
}
