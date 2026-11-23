use crate::{machine::VIRTUAL_STEPS_PER_SECOND, stm32_uart::Stm32Irq};

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
    inputs: [bool; 4],
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
            inputs: [false; 4],
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
        let channel = usize::from(channel);
        let mode = self.registers[6 + channel / 2] >> (channel % 2 * 8);
        if mode & 3 == 0 || std::mem::replace(&mut self.inputs[channel], high) == high {
            return;
        }
        let selected = (self.registers[2] >> 4) & 7;
        let falling = self.registers[8] & (2 << (channel * 4)) != 0;
        if channel < 2
            && selected == 5 + channel as u32
            && self.registers[2] & 7 == 4
            && high != falling
        {
            self.counter = 0;
            if self.registers[3] & (1 << 6) != 0 {
                self.registers[4] |= 1 << 6;
            }
        }
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
        self.advance_steps(1);
    }

    fn advance_steps(&mut self, steps: u32) -> u64 {
        if self.registers[0] & CEN == 0 || self.active_arr == 0 {
            return 0;
        }
        let step_hz = u64::from(VIRTUAL_STEPS_PER_SECOND);
        self.phase += u64::from(self.frequency) * u64::from(steps);
        let cycles = self.phase / step_hz;
        self.phase %= step_hz;
        let mut notifications = 0;
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
                    notifications += u64::from(self.registers[3] & 1);
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
                if mode & 3 == 0
                    && self.counter == self.registers[13 + channel]
                    && self.registers[3] & (2 << channel) != 0
                {
                    self.registers[4] |= 2 << channel;
                }
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

        timer.tick();
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
            timer.tick();
            if timer.read(SR) & CC1 != 0 {
                compare_events += 1;
                timer.write(SR, !CC1);
            }
        }
        compare_events
    }

    fn check(timer: &Timer, counter: u32, compare: u32) {
        assert_eq!(timer.read(CNT), counter);
        assert_eq!(timer.read(CCR1), compare);
    }

    #[test]
    fn should_handle_compare_value_change_when_enabled() {
        let mut timer = periodic_timer(65_536, 65_535);
        assert_eq!(advance(&mut timer, 65_535), 1);
        check(&timer, 65_535, 65_535);
        assert_eq!(advance(&mut timer, 1), 0);
        check(&timer, 0, 65_535);

        assert_eq!(advance(&mut timer, 16_304), 0);
        check(&timer, 16_304, 65_535);
        timer.write(CCR1, 0);
        check(&timer, 16_304, 0);
        assert_eq!(timer.read(SR) & CC1, 0);

        for _ in 0..2 {
            let remaining = 65_535 - timer.read(CNT);
            assert_eq!(advance(&mut timer, remaining), 0);
            check(&timer, 65_535, 0);
            assert_eq!(advance(&mut timer, 1), 1);
            check(&timer, 0, 0);
        }
    }

    #[test]
    fn should_clear_when_value_set_to_zero() {
        let mut timer = periodic_timer(300, 200);
        assert_eq!(advance(&mut timer, 100), 0);
        check(&timer, 100, 200);
        timer.write(CNT, 0);
        check(&timer, 0, 200);
    }

    #[test]
    fn should_generate_compare_event_at_compare_after_clear() {
        let mut timer = periodic_timer(300, 100);
        advance(&mut timer, 200);
        timer.write(CNT, 0);
        assert_eq!(advance(&mut timer, 100), 1);
        check(&timer, 100, 100);
    }

    #[test]
    fn should_not_generate_additional_compare_event() {
        let mut timer = periodic_timer(300, 100);
        assert_eq!(timer.read(SR) & CC1, 0);
        assert_eq!(advance(&mut timer, 100), 1);
        timer.write(CNT, 100);
        assert_eq!(timer.read(SR) & CC1, 0);
        assert_eq!(advance(&mut timer, 300), 1);
        check(&timer, 100, 100);
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
