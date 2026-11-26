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
    pub name: Option<String>,
    frequency: u32,
    initial_frequency: u32,
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
            name: None,
            frequency,
            initial_frequency: frequency,
            // The REPL limit defines the width by its highest set bit.
            counter_mask: u32::MAX >> initial_limit.leading_zeros(),
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
        let name = self.name.take();
        *self = Self::new(
            self.base,
            self.initial_frequency,
            self.initial_limit,
            self.routes,
        );
        self.name = name;
        self.gpio_outputs = outputs;
        self.dma_routes = dma;
    }

    pub fn set_frequency(&mut self, frequency: u32) {
        self.frequency = frequency;
        self.phase = 0;
        self.prescaler = 0;
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

    pub(crate) fn quiet_steps(&self, limit: u64) -> u64 {
        if self.output_changes != 0 || self.capture_levels != self.capture_next || self.dma_requests != 0 {
            return 0;
        }
        if self.registers[0] & ENABLE == 0 || self.active_arr == 0 || self.frequency == 0 {
            return limit;
        }
        let ticks = u64::from(self.registers[10].saturating_sub(self.prescaler)) + 1;
        let threshold = u128::from(ticks) * u128::from(CORTEX_M_STEP_HZ);
        let steps = threshold.saturating_sub(u128::from(self.phase)).div_ceil(u128::from(self.frequency));
        limit.min(steps.saturating_sub(1) as u64)
    }

    pub(crate) fn advance_quiet(&mut self, steps: u64) {
        if self.registers[0] & ENABLE != 0 && self.active_arr != 0 {
            let total = u128::from(self.phase) + u128::from(self.frequency) * u128::from(steps);
            let cycles = total / u128::from(CORTEX_M_STEP_HZ);
            self.phase = (total % u128::from(CORTEX_M_STEP_HZ)) as u64;
            self.prescaler += cycles as u32;
        }
    }

    pub fn tick(&mut self) -> u32 {
        self.advance(1)
    }

    fn advance(&mut self, steps: u32) -> u32 {
        if self.registers[0] & ENABLE != 0 && self.active_arr != 0 {
            self.phase += u64::from(self.frequency) * u64::from(steps);
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

#[cfg(test)]
mod tests {
    use super::*;

    const COMPARE_IRQ: u32 = 1 << 15;
    const CC1: u32 = 1 << 1;
    const CNT: u32 = 0x24;
    const CCR1: u32 = 0x34;
    const SR: u32 = 0x10;

    const UPDATE_IRQ: u32 = 1 << 15;

    fn limit_timer(frequency: u32, period: u32, direction: u32, interrupt: bool) -> Stm32Timer {
        let mut timer = Stm32Timer::new(0, 48_000_000, u32::MAX, [UPDATE_IRQ, 0, 0, 0, 0, 0, 0, 0]);
        timer.set_frequency(frequency);
        timer.write(0x2c, period - 1);
        timer.write(0, direction | UPDATE_OVERFLOW_ONLY);
        timer.write(0x14, 1);
        timer.write(0x0c, u32::from(interrupt));
        timer.write(0, direction | UPDATE_OVERFLOW_ONLY | ENABLE);
        timer
    }

    #[test]
    fn renode_stm32_limit_timer_monotonic_and_bounded() {
        for direction in [0, DOWN] {
            let mut timer = limit_timer(100, 100_000, direction, false);
            for _ in 0..100 {
                let old = timer.read(CNT);
                timer.advance(CORTEX_M_STEP_HZ as u32);
                if direction == DOWN {
                    assert!(timer.read(CNT) < old);
                } else {
                    assert!(timer.read(CNT) > old);
                }
            }

            let mut timer = limit_timer(1, 100, direction, false);
            for _ in 0..99 {
                timer.advance(CORTEX_M_STEP_HZ as u32);
            }
            let samples = if direction == DOWN {
                [0, 99, 98]
            } else {
                [99, 0, 1]
            };
            for expected in samples {
                assert_eq!(timer.read(CNT), expected);
                timer.advance(CORTEX_M_STEP_HZ as u32);
            }
        }
    }

    #[test]
    fn renode_stm32_limit_timer_update_irq_gates_and_precision() {
        for interrupt in [false, true] {
            let mut timer = limit_timer(1, 10, DOWN, interrupt);
            for _ in 0..9 {
                assert_eq!(timer.advance(CORTEX_M_STEP_HZ as u32), 0);
            }
            assert_eq!(
                timer.advance(CORTEX_M_STEP_HZ as u32),
                if interrupt { UPDATE_IRQ } else { 0 }
            );
            assert_eq!(timer.read(SR) & 1, 1);
            if !interrupt {
                assert_eq!(timer.advance(CORTEX_M_STEP_HZ as u32), 0);
            }
        }

        for (frequency, microseconds) in [(1_000_000, 10), (10_000_000, 1)] {
            let mut timer = limit_timer(frequency, 4, DOWN, true);
            let mut events = 0;
            for _ in 0..microseconds * (CORTEX_M_STEP_HZ / 1_000_000) {
                if timer.tick() == UPDATE_IRQ {
                    events += 1;
                    timer.write(SR, !1);
                }
            }
            assert_eq!(events, 2);
            assert_eq!(timer.read(CNT), 1); // Generic LimitTimer represents this as 2.
        }
    }

    fn comparing_timer(period: u32, compare: u32) -> Stm32Timer {
        let mut timer = Stm32Timer::new(
            0,
            1_000_000,
            u16::MAX as u32,
            [0, COMPARE_IRQ, 0, 0, 0, 0, 0, 0],
        );
        timer.write(0x2c, period - 1);
        timer.write(CCR1, compare);
        timer.write(0x0c, CC1);
        timer.write(0, ENABLE);
        timer
    }

    fn advance_us(timer: &mut Stm32Timer, microseconds: u64) -> u32 {
        let mut events = 0;
        for _ in 0..microseconds * (CORTEX_M_STEP_HZ / 1_000_000) {
            if timer.tick() & COMPARE_IRQ != 0 {
                assert_eq!(timer.read(SR) & CC1, CC1);
                events += 1;
                timer.write(SR, !CC1);
            }
        }
        events
    }

    #[test]
    fn renode_stm32_timer_compare_change_while_running() {
        let mut timer = comparing_timer(65536, 65535);
        for (elapsed, count, compare, events) in [
            (65535, 65535, 65535, 1),
            (1, 0, 65535, 0),
            (16304, 16304, 65535, 0),
        ] {
            assert_eq!(advance_us(&mut timer, elapsed), events);
            assert_eq!((timer.read(CNT), timer.read(CCR1)), (count, compare));
        }
        timer.write(CCR1, 0);
        assert_eq!(
            (timer.read(CNT), timer.read(CCR1), timer.read(SR) & CC1),
            (16304, 0, 0)
        );
        for (elapsed, count, events) in [
            (65535 - 16304, 65535, 0),
            (1, 0, 1),
            (65535, 65535, 0),
            (1, 0, 1),
        ] {
            assert_eq!(advance_us(&mut timer, elapsed), events);
            assert_eq!((timer.read(CNT), timer.read(CCR1)), (count, 0));
        }
    }

    #[test]
    fn renode_stm32_timer_compare_clear_preserves_compare_and_rearms() {
        let mut timer = comparing_timer(300, 200);
        assert_eq!(advance_us(&mut timer, 100), 0);
        assert_eq!((timer.read(CNT), timer.read(CCR1)), (100, 200));
        timer.write(CNT, 0);
        assert_eq!((timer.read(CNT), timer.read(CCR1)), (0, 200));

        let mut timer = comparing_timer(300, 100);
        assert_eq!(advance_us(&mut timer, 200), 1);
        timer.write(CNT, 0);
        assert_eq!(advance_us(&mut timer, 100), 1);
        assert_eq!((timer.read(CNT), timer.read(CCR1)), (100, 100));
    }

    #[test]
    fn renode_stm32_timer_compare_same_counter_write_is_not_an_event() {
        let mut timer = comparing_timer(300, 100);
        assert_eq!(timer.read(SR) & CC1, 0);
        assert_eq!(advance_us(&mut timer, 100), 1);
        timer.write(CNT, 100);
        assert_eq!(timer.read(SR) & CC1, 0);
        assert_eq!(advance_us(&mut timer, 300), 1);
        assert_eq!((timer.read(CNT), timer.read(CCR1)), (100, 100));
    }

    #[test]
    fn stm32_timer_register_width_follows_initial_limit_highest_bit() {
        let mut timer = Stm32Timer::new(0, 1_000_000, 0x1ff, [0; 8]);
        for offset in [CNT, 0x2c, CCR1] {
            timer.write(offset, 0xffff);
            assert_eq!(timer.read(offset), 0x1ff);
        }
    }

    // STM32F4_Discovery.robot: Should Fire Update Event When Counting Up/Down.
    #[test]
    fn renode_tim2_update_irq_repeats_in_both_directions() {
        const TIM2_IRQ: u32 = 1 << 15;
        for direction in [0, DOWN] {
            let mut timer = Stm32Timer::new(
                0,
                CORTEX_M_STEP_HZ as u32,
                u32::MAX,
                [TIM2_IRQ, 0, 0, 0, 0, 0, 0, 0],
            );
            timer.write(0, direction | UPDATE_OVERFLOW_ONLY);
            timer.write(0x28, 1); // two input cycles per counter tick
            timer.write(0x2c, 3); // four counter values
            timer.write(0x14, 1); // establish the initial counter without raising UIF
            timer.write(0x0c, 1); // UIE
            timer.write(0, direction | UPDATE_OVERFLOW_ONLY | ENABLE);
            for _ in 0..3 {
                for _ in 0..7 {
                    assert_eq!(timer.tick(), 0);
                }
                assert_eq!(timer.tick(), TIM2_IRQ);
                assert_ne!(timer.read(0x10) & 1, 0);
                assert_eq!(timer.read(0x24), if direction == DOWN { 3 } else { 0 });
                timer.write(0x10, !1);
                assert_eq!(timer.read(0x10) & 1, 0);
            }
        }
    }
}
