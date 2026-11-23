use crate::{machine::VIRTUAL_STEPS_PER_SECOND, stm32_uart::Stm32Irq};

const CEN: u32 = 1;
const UDIS: u32 = 1 << 1;
const URS: u32 = 1 << 2;
const OPM: u32 = 1 << 3;
const DIR: u32 = 1 << 4;
const ARPE: u32 = 1 << 7;

#[derive(Clone, Copy)]
pub enum TimerClock {
    Apb1Low(u8),
    Apb2(u8),
}

impl TimerClock {
    pub const ALIASES: [(&str, Self); 12] = [
        ("TIM2", Self::Apb1Low(0)),
        ("TIM3", Self::Apb1Low(1)),
        ("TIM4", Self::Apb1Low(2)),
        ("TIM5", Self::Apb1Low(3)),
        ("TIM6", Self::Apb1Low(4)),
        ("TIM7", Self::Apb1Low(5)),
        ("TIM12", Self::Apb1Low(6)),
        ("TIM13", Self::Apb1Low(7)),
        ("TIM14", Self::Apb1Low(8)),
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

    pub fn apb2_mask() -> u32 {
        Self::ALIASES
            .iter()
            .fold(0, |mask, (_, clock)| match *clock {
                Self::Apb2(bit) => mask | (1 << bit),
                Self::Apb1Low(_) => mask,
            })
    }
}

/// STM32 general-purpose timer counter, update and output-compare registers.
pub struct Timer {
    pub base: u32,
    pub clock: Option<TimerClock>,
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

    pub fn new(base: u32, frequency: u32, limit: u32, clock: Option<TimerClock>) -> Self {
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
