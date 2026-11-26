#[cfg(test)]
use crate::machine::VIRTUAL_STEPS_PER_SECOND;

use crate::stm32_uart::Stm32Irq;

const CEN: u32 = 1;
const UDIS: u32 = 1 << 1;
const URS: u32 = 1 << 2;
const OPM: u32 = 1 << 3;
const DIR: u32 = 1 << 4;
const ARPE: u32 = 1 << 7;

#[derive(Clone, Copy, PartialEq)]
pub enum TimerClock {
    Apb1Low(u8),
    Apb2(u8),
}

impl TimerClock {
    pub const ALIASES: [(&str, Self); 14] = [
        ("TIM2", Self::Apb1Low(0)),
        ("TIM3", Self::Apb1Low(1)),
        ("TIM4", Self::Apb1Low(2)),
        ("TIM5", Self::Apb1Low(3)),
        ("TIM6", Self::Apb1Low(4)),
        ("TIM7", Self::Apb1Low(5)),
        ("TIM12", Self::Apb1Low(6)),
        ("TIM13", Self::Apb1Low(7)),
        ("TIM14", Self::Apb1Low(8)),
        ("TIM1", Self::Apb2(0)),
        ("TIM8", Self::Apb2(1)),
        ("TIM15", Self::Apb2(16)),
        ("TIM16", Self::Apb2(17)),
        ("TIM17", Self::Apb2(18)),
    ];

    pub fn from_alias(alias: &str) -> Result<Self, String> {
        Self::ALIASES
            .iter()
            .find_map(|&(name, clock)| (name == alias).then_some(clock))
            .ok_or_else(|| format!("unsupported STM32 timer RCC alias {alias}"))
    }

    pub fn alias(self) -> Option<&'static str> {
        Self::ALIASES
            .iter()
            .find_map(|&(name, clock)| (self == clock).then_some(name))
    }

    pub fn apb2_mask() -> u32 {
        Self::ALIASES
            .iter()
            .fold(0, |mask, (_, clock)| match *clock {
                Self::Apb2(bit) => mask | (1 << bit),
                Self::Apb1Low(_) => mask,
            })
    }
}

#[derive(Clone, Copy)]
pub enum TimerOutput {
    Combined,
    Break,
    Update,
    Trigger,
    Commutation,
    CaptureCompare,
}

impl TimerOutput {
    pub fn parse(output: &str) -> Result<Self, String> {
        match output.trim() {
            "" | "IRQ" => Ok(Self::Combined),
            "BreakInterrupt" => Ok(Self::Break),
            "UpdateInterrupt" => Ok(Self::Update),
            "TriggerInterrupt" => Ok(Self::Trigger),
            "CommutationInterrupt" => Ok(Self::Commutation),
            "CaptureCompareInterrupt" => Ok(Self::CaptureCompare),
            _ => Err(format!("unsupported STM32 timer output {output}")),
        }
    }

    fn mask(self) -> u32 {
        match self {
            Self::Combined => 0x5f,
            Self::Update => 1,
            Self::Trigger => 1 << 6,
            Self::CaptureCompare => 0x1e,
            // The upstream model wires these outputs but always drives them low.
            Self::Break | Self::Commutation => 0,
        }
    }
}

/// STM32 general-purpose timer counter, update and output-compare registers.
pub struct Timer {
    pub name: String,
    pub base: u32,
    pub clock: Option<TimerClock>,
    routes: [Option<Stm32Irq>; 6],
    input: [bool; 4],
    capture_edges: [u8; 4],
    pub frequency: u32,
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

    pub fn new(
        name: String,
        base: u32,
        frequency: u32,
        limit: u32,
        clock: Option<TimerClock>,
    ) -> Self {
        let mut timer = Self {
            name,
            base,
            frequency,
            limit,
            clock,
            routes: [None; 6],
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
        let routes = self.routes;
        *self = Self::new(
            std::mem::take(&mut self.name),
            self.base,
            self.frequency,
            self.limit,
            self.clock,
        );
        self.routes = routes;
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    pub fn route(&mut self, output: TimerOutput, irq: Stm32Irq) -> Result<(), String> {
        let route = &mut self.routes[output as usize];
        if route.is_some() {
            return Err("duplicate STM32 timer output".into());
        }
        *route = Some(irq);
        Ok(())
    }

    fn uses_advanced_events(&self) -> bool {
        matches!(self.clock, Some(TimerClock::Apb2(0 | 1)))
    }

    pub fn interrupts(&self) -> impl Iterator<Item = Stm32Irq> + '_ {
        let pending = self.registers[3] & self.registers[4];
        [
            TimerOutput::Combined,
            TimerOutput::Break,
            TimerOutput::Update,
            TimerOutput::Trigger,
            TimerOutput::Commutation,
            TimerOutput::CaptureCompare,
        ]
        .into_iter()
        .zip(self.routes)
        .filter_map(move |(output, irq)| {
            irq.filter(|_| {
                let mask =
                    if matches!(output, TimerOutput::Combined) && !self.uses_advanced_events() {
                        0xff
                    } else {
                        output.mask()
                    };
                mask & pending != 0
            })
        })
    }

    pub fn set_input(&mut self, channel: u8, high: bool) {
        let mut levels = self.input;
        levels[usize::from(channel)] = high;
        self.sample_inputs(levels);
    }

    fn pwm_config(&self, channel: u8) -> Option<(u32, bool)> {
        let channel = usize::from(channel);
        let control = self.registers[0];
        let ccer = self.registers[8] >> (channel * 4);
        let mode = (self.registers[6 + channel / 2] >> (channel % 2 * 8)) & 0xff;
        if control & (CEN | DIR | (3 << 5)) != CEN || ccer & 1 == 0 || mode & 3 != 0 {
            return None;
        }
        let mode_inverted = match (mode >> 4) & 7 {
            6 => false,
            7 => true,
            _ => return None,
        };
        Some((
            self.registers[13 + channel],
            mode_inverted ^ (ccer & 2 != 0),
        ))
    }

    /// Average high level for an enabled edge-aligned, up-counting PWM channel.
    pub fn pwm_level(&self, channel: u8) -> Option<f64> {
        let (compare, inverted) = self.pwm_config(channel)?;
        let period = f64::from(self.active_arr) + 1.0;
        let active = (f64::from(compare) / period).min(1.0);
        Some(if inverted { 1.0 - active } else { active })
    }

    pub fn pwm_pin(&self, channel: u8) -> Option<bool> {
        self.pwm_config(channel)
            .map(|(compare, inverted)| (self.counter < compare) ^ inverted)
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
                // Advanced-timer EGR sources other than UG are tagged fields upstream.
                let advanced = self.uses_advanced_events();
                if !advanced {
                    self.registers[4] |= value & 0xfe;
                }
                if value & 1 != 0 && (!advanced || self.registers[0] & UDIS == 0) {
                    self.divider = 0;
                    self.reload_period();
                    self.counter = if self.registers[0] & DIR == 0 {
                        0
                    } else {
                        self.active_arr
                    };
                    if self.registers[0] & (UDIS | URS) == 0
                        && (!advanced || self.registers[3] & 1 != 0)
                    {
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

    pub(crate) fn quiet_steps(&self, limit: u64, step_hz: u32) -> u64 {
        if self.registers[0] & CEN == 0 || self.active_arr == 0
            || matches!(self.encoder_mode(), 1..=3) || self.frequency == 0
        {
            return limit;
        }
        let down = self.registers[0] & DIR != 0;
        let mut counts = if down { self.counter } else { self.active_arr.saturating_sub(self.counter) };
        for channel in 0..4 {
            let mode = self.channel_mode(channel);
            if mode & 3 != 0 {
                continue;
            }
            let compare = self.registers[13 + channel];
            let distance = if down {
                if matches!((mode >> 4) & 7, 6 | 7) && self.counter == compare {
                    return 0;
                }
                self.counter.checked_sub(compare)
            } else {
                compare.checked_sub(self.counter)
            };
            if let Some(distance) = distance.filter(|distance| *distance != 0) {
                counts = counts.min(distance - 1);
            }
        }
        let first = u128::from(self.active_psc.saturating_sub(self.divider)) + 1;
        let ticks = first + u128::from(counts) * (u128::from(self.active_psc) + 1);
        let threshold = ticks * u128::from(step_hz);
        let steps = threshold.saturating_sub(u128::from(self.phase)).div_ceil(u128::from(self.frequency));
        u128::from(limit).min(steps.saturating_sub(1)) as u64
    }

    pub(crate) fn advance_quiet(&mut self, steps: u64, step_hz: u32) {
        if self.registers[0] & CEN != 0 && self.active_arr != 0 && !matches!(self.encoder_mode(), 1..=3) {
            let total = u128::from(self.phase) + u128::from(self.frequency) * u128::from(steps);
            let cycles = total / u128::from(step_hz);
            self.phase = (total % u128::from(step_hz)) as u64;
            let first = u128::from(self.active_psc.saturating_sub(self.divider)) + 1;
            if cycles < first {
                self.divider += cycles as u32;
                return;
            }
            let period = u128::from(self.active_psc) + 1;
            let remaining = cycles - first;
            self.divider = (remaining % period) as u32;
            let counts = 1 + (remaining / period) as u32;
            if self.registers[0] & DIR != 0 {
                self.counter -= counts;
            } else {
                self.counter += counts;
            }
        }
    }

    pub fn tick(&mut self, step_hz: u32) {
        self.advance(1, step_hz);
    }

    #[cfg(test)]
    fn advance_steps(&mut self, steps: u32) -> u64 {
        self.advance(steps, VIRTUAL_STEPS_PER_SECOND)
    }

    fn advance(&mut self, steps: u32, step_hz: u32) -> u64 {
        if self.registers[0] & CEN == 0
            || self.active_arr == 0
            || matches!(self.encoder_mode(), 1..=3)
        {
            return 0;
        }
        let step_hz = u64::from(step_hz);
        self.phase += u64::from(self.frequency) * u64::from(steps);
        let cycles = self.phase / step_hz;
        self.phase %= step_hz;
        let mut notifications = 0;
        for _ in 0..cycles {
            notifications += self.count_clock();
            if self.registers[0] & CEN == 0 {
                break;
            }
        }
        notifications
    }

    fn count_clock(&mut self) -> u64 {
        let mut notifications = 0;
        if self.divider < self.active_psc {
            self.divider += 1;
            return 0;
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
                notifications += u64::from(self.registers[3] & 1);
                if self.registers[0] & OPM != 0 {
                    self.registers[0] &= !CEN;
                    return notifications;
                }
            }
        } else if down {
            self.counter -= 1;
        } else {
            self.counter += 1;
        }
        for channel in 0..4 {
            let mode = self.registers[6 + channel / 2] >> (channel % 2 * 8);
            if mode & 3 == 0
                && self.counter == self.registers[13 + channel]
                && self.registers[3] & (2 << channel) != 0
            {
                self.registers[4] |= 2 << channel;
            }
        }
        notifications
    }
}

#[cfg(test)]
mod limit_timer_cases {
    use super::*;

    const CR1: u32 = 0;
    const CNT: u32 = 0x24;

    fn enabled_timer(frequency: u32, limit: u32, descending: bool) -> Timer {
        let mut timer = Timer::new("timer2".into(), 0, frequency, limit, None);
        if descending {
            timer.write(CNT, limit);
        }
        timer.write(CR1, CEN | if descending { DIR } else { 0 });
        timer
    }

    fn advance_seconds(timer: &mut Timer, seconds: u32) {
        timer.advance_steps(seconds * VIRTUAL_STEPS_PER_SECOND);
    }

    fn periodic_downcounter(frequency: u32, period: u32, events_enabled: bool) -> Timer {
        let mut timer = enabled_timer(frequency, period - 1, true);
        timer
            .route(TimerOutput::Update, Stm32Irq::Nvic(28))
            .unwrap();
        timer.write(0x0c, u32::from(events_enabled));
        timer
    }

    fn assert_precision(frequency: u32, microseconds: u32) {
        let mut timer = periodic_downcounter(frequency, 4, true);
        assert_eq!(timer.advance_steps(microseconds), 2);
        // LimitTimer counts 4..1, while STM32 counts 3..0 for this period.
        assert_eq!(timer.read(CNT) + 1, 2);
    }

    #[test]
    fn upstream_microsecond_precision_events() {
        assert_precision(1_000_000, 10);
    }

    #[test]
    fn upstream_submicrosecond_precision_events() {
        assert_precision(10_000_000, 1);
    }

    #[test]
    fn upstream_alarm_disabled() {
        let mut timer = periodic_downcounter(1, 10, false);
        assert_eq!(timer.advance_steps(11 * VIRTUAL_STEPS_PER_SECOND), 0);
        assert_eq!(timer.interrupts().count(), 0);
    }

    #[test]
    fn upstream_alarm_enabled() {
        let mut timer = periodic_downcounter(1, 10, true);
        assert_eq!(timer.advance_steps(10 * VIRTUAL_STEPS_PER_SECOND), 1);
        assert_eq!(timer.interrupts().count(), 1);
    }

    #[test]
    fn upstream_switch_direction_at_zero() {
        let mut timer = enabled_timer(100, 100_000, false);
        timer
            .route(TimerOutput::Combined, Stm32Irq::Nvic(28))
            .unwrap();
        // Compare at zero observes the descending limit on arrival.
        timer.write(0x34, 0);
        timer.write(0x0c, 1 << 1);
        advance_seconds(&mut timer, 2);
        assert_eq!(timer.read(CNT), 200);
        assert_eq!(timer.interrupts().count(), 0);

        timer.write(CR1, CEN | DIR);
        assert_eq!(timer.read(CNT), 200);
        timer.advance_steps(2 * VIRTUAL_STEPS_PER_SECOND - 1);
        assert_eq!(timer.read(CNT), 1);
        assert_eq!(timer.interrupts().count(), 0);

        timer.tick(VIRTUAL_STEPS_PER_SECOND);
        assert_eq!(timer.read(CNT), 0);
        assert_eq!(timer.read(0x10) & 3, 1 << 1);
        assert_eq!(timer.interrupts().count(), 1);
    }

    #[test]
    fn upstream_should_be_ascending() {
        let mut timer = enabled_timer(100, 100_000, false);
        let mut previous = 0;
        for _ in 0..100 {
            advance_seconds(&mut timer, 1);
            let value = timer.read(CNT);
            assert!(value > previous);
            previous = value;
        }
    }

    #[test]
    fn upstream_should_be_descending() {
        let mut timer = enabled_timer(100, 100_000, true);
        let mut previous = 100_000;
        for _ in 0..100 {
            advance_seconds(&mut timer, 1);
            let value = timer.read(CNT);
            assert!(value < previous);
            previous = value;
        }
    }

    fn assert_limit(descending: bool) {
        let limit = 100;
        let mut timer = enabled_timer(1, limit, descending);
        advance_seconds(&mut timer, limit - 1);
        for _ in 0..3 {
            assert!((0..=i64::from(limit)).contains(&i64::from(timer.read(CNT))));
            advance_seconds(&mut timer, 1);
        }
    }

    #[test]
    fn upstream_should_not_exceed_limit_ascending() {
        assert_limit(false);
    }

    #[test]
    fn upstream_should_not_exceed_limit_descending() {
        assert_limit(true);
    }
}

#[cfg(test)]
mod comparing_timer_cases {
    use super::*;

    const CR1: u32 = 0;
    const DIER: u32 = 0x0c;
    const SR: u32 = 0x10;
    const CNT: u32 = 0x24;
    const ARR: u32 = 0x2c;
    const CCR1: u32 = 0x34;
    const CC1: u32 = 1 << 1;

    fn periodic_timer(period: u32, compare: u32) -> Timer {
        // ComparingTimer names the wrap boundary "limit", while STM32 ARR
        // names the last count before wrap.
        let mut timer = Timer::new("timer3".into(), 0, 1_000_000, 0xffff, None);
        timer.write(ARR, period - 1);
        timer.write(CCR1, compare);
        timer.write(DIER, CC1);
        timer.write(CR1, CEN);
        timer
    }

    fn advance(timer: &mut Timer, microseconds: u32) -> u32 {
        let mut compare_events = 0;
        for _ in 0..microseconds {
            timer.tick(VIRTUAL_STEPS_PER_SECOND);
            if timer.read(SR) & CC1 != 0 {
                compare_events += 1;
                timer.write(SR, !CC1);
            }
        }
        compare_events
    }

    fn check(timer: &mut Timer, counter: u32, compare: u32) {
        assert_eq!(timer.read(CNT), counter);
        assert_eq!(timer.read(CCR1), compare);
    }

    #[test]
    fn should_handle_compare_value_change_when_enabled() {
        let mut timer = periodic_timer(65_536, 65_535);
        assert_eq!(advance(&mut timer, 65_535), 1);
        check(&mut timer, 65_535, 65_535);
        assert_eq!(advance(&mut timer, 1), 0);
        check(&mut timer, 0, 65_535);

        assert_eq!(advance(&mut timer, 16_304), 0);
        check(&mut timer, 16_304, 65_535);
        timer.write(CCR1, 0);
        check(&mut timer, 16_304, 0);
        assert_eq!(timer.read(SR) & CC1, 0);

        for _ in 0..2 {
            let remaining = 65_535 - timer.read(CNT);
            assert_eq!(advance(&mut timer, remaining), 0);
            check(&mut timer, 65_535, 0);
            assert_eq!(advance(&mut timer, 1), 1);
            check(&mut timer, 0, 0);
        }
    }

    #[test]
    fn should_clear_when_value_set_to_zero() {
        let mut timer = periodic_timer(300, 200);
        assert_eq!(advance(&mut timer, 100), 0);
        check(&mut timer, 100, 200);
        timer.write(CNT, 0);
        check(&mut timer, 0, 200);
    }

    #[test]
    fn should_generate_compare_event_at_compare_after_clear() {
        let mut timer = periodic_timer(300, 100);
        advance(&mut timer, 200);
        timer.write(CNT, 0);
        assert_eq!(advance(&mut timer, 100), 1);
        check(&mut timer, 100, 100);
    }

    #[test]
    fn should_not_generate_additional_compare_event() {
        let mut timer = periodic_timer(300, 100);
        assert_eq!(timer.read(SR) & CC1, 0);
        assert_eq!(advance(&mut timer, 100), 1);
        timer.write(CNT, 100);
        assert_eq!(timer.read(SR) & CC1, 0);
        assert_eq!(advance(&mut timer, 300), 1);
        check(&mut timer, 100, 100);
    }

    #[test]
    fn compare_match_only_sets_interrupt_flag_for_an_enabled_channel() {
        let mut timer = periodic_timer(300, 100);
        timer.write(DIER, 0);
        // PWM output can run while its compare interrupt is masked.
        timer.write(0x18, 0x60);
        timer.write(0x20, 1);
        assert_eq!(advance(&mut timer, 100), 0);
        assert_eq!(timer.pwm_pin(0), Some(false));
        timer.write(DIER, CC1);
        assert_eq!(advance(&mut timer, 300), 1);
    }
}

#[cfg(test)]
mod rcc_binding_tests {
    use super::*;
    use crate::{bus::Bus, stm32h7_net_control::NetworkControl};

    #[test]
    fn tim17_uses_apb2_clock_divider_gate_and_reset() {
        const RCC: u32 = 0x5802_4400;
        const TIM17: u32 = 0x4001_4800;
        const ARR: u32 = TIM17 + 0x2c;
        const TIM17_BIT: u32 = 1 << 18;
        let clock = TimerClock::from_alias("TIM17").unwrap();
        let mut rcc = NetworkControl::new(RCC, None, None);
        rcc.write(RCC + 0x1c, (5 << 4) | (6 << 8)).unwrap();
        assert_eq!(rcc.timer_frequency(clock), Some(16_000_000));
        assert_eq!(
            rcc.timer_frequency(TimerClock::Apb1Low(0)),
            Some(32_000_000)
        );

        let mut bus = Bus::new(0x1000).unwrap();
        bus.map_h7_network_control(NetworkControl::new(RCC, None, None))
            .unwrap();
        bus.map_h7_timer(Timer::new(
            "timer17".into(),
            TIM17,
            64_000_000,
            0xffff,
            Some(clock),
        ))
        .unwrap();
        let write = |bus: &mut Bus, address, value| {
            bus.write(address, 4, value, &mut std::io::sink()).unwrap();
        };
        write(&mut bus, RCC + 0xf0, 0);
        write(&mut bus, RCC + 0xe8, TIM17_BIT);
        assert_eq!(bus.read::<4>(ARR).unwrap(), 0);
        write(&mut bus, RCC + 0xf0, TIM17_BIT);
        assert_eq!(bus.read::<4>(ARR).unwrap(), 0xffff);
        write(&mut bus, ARR, 0x42);
        write(&mut bus, RCC + 0x90, TIM17_BIT);
        assert_eq!(bus.read::<4>(ARR).unwrap(), 0x42);
        write(&mut bus, RCC + 0x98, TIM17_BIT);
        assert_eq!(bus.read::<4>(ARR).unwrap(), 0);
        write(&mut bus, RCC + 0x98, 0);
        assert_eq!(bus.read::<4>(ARR).unwrap(), 0xffff);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paired_capture_latches_pulse_and_period_before_reset() {
        let mut timer = Timer::new(String::new(), 0, 1_000_000, 0xffff, None);
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
        let mut timer = Timer::new(String::new(), 0, 1_000_000, 0xffff, None);
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
        let mut timer = Timer::new(String::new(), 0, 1_000_000, 0xffff, None);
        timer
            .route(TimerOutput::Update, Stm32Irq::Nvic(25))
            .unwrap();
        timer
            .route(TimerOutput::CaptureCompare, Stm32Irq::Nvic(27))
            .unwrap();
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
