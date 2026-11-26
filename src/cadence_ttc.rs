use std::time::Duration;

use crate::bus_translation::NativePeripheral;

pub struct CadenceTtc {
    frequency: u64,
    timers: [Counter; 3],
}

#[derive(Default)]
struct Counter {
    clock_control: u32,
    control: u32,
    value: u32,
    interval: u32,
    interrupt_status: u32,
    interrupt_enable: u32,
    phase: u128,
}

impl Counter {
    fn limit(&self) -> u32 {
        if self.control & 2 == 0 {
            u32::MAX
        } else {
            self.interval
        }
    }

    fn count_down(&self) -> bool {
        self.control & 4 != 0
    }

    fn advance(&mut self, frequency: u64, duration: Duration) {
        if self.control & 1 != 0 || duration.is_zero() {
            return;
        }
        let divisor = if self.clock_control & 1 == 0 {
            1
        } else {
            2 << ((self.clock_control >> 1) & 0xf)
        };
        let denominator = divisor * 1_000_000_000u128;
        // Divide the duration into whole seconds and subsecond nanoseconds
        // so the intermediate stays in range for any u64 frequency.
        let whole_cycles = u128::from(duration.as_secs()) * u128::from(frequency);
        let subsecond = (whole_cycles % divisor) * 1_000_000_000
            + u128::from(duration.subsec_nanos()) * u128::from(frequency)
            + self.phase;
        let ticks = whole_cycles / divisor + subsecond / denominator;
        self.phase = subsecond % denominator;
        if ticks == 0 {
            return;
        }

        let limit = self.limit();
        let remaining = if self.count_down() {
            self.value
        } else {
            limit.saturating_sub(self.value)
        };
        if ticks < u128::from(remaining) {
            if self.count_down() {
                self.value -= ticks as u32;
            } else {
                self.value += ticks as u32;
            }
            return;
        }

        self.interrupt_status |= if self.control & 2 != 0 { 1 } else { 0x10 };
        let position = ((ticks - u128::from(remaining)) % u128::from(limit.max(1))) as u32;
        self.value = if self.count_down() {
            limit - position
        } else {
            position
        };
    }
}

impl CadenceTtc {
    pub fn new(frequency: u64) -> Self {
        let mut device = Self {
            frequency,
            timers: Default::default(),
        };
        device.reset();
        device
    }

    pub fn reset(&mut self) {
        self.timers = std::array::from_fn(|_| Counter {
            control: 1,
            ..Default::default()
        });
    }

    pub fn set_counter_value(&mut self, index: usize, value: u32) {
        self.timers[index].value = value;
    }

    pub fn irq(&self, index: usize) -> bool {
        let timer = &self.timers[index];
        timer.interrupt_enable & timer.interrupt_status != 0
    }

    pub fn advance(&mut self, duration: Duration) {
        for counter in &mut self.timers {
            counter.advance(self.frequency, duration);
        }
    }

    fn register(offset: u64) -> Option<(u64, usize)> {
        (offset % 4 == 0 && offset < 0x6c)
            .then_some((offset - offset % 0xc, (offset % 0xc / 4) as usize))
    }

    pub fn read_register(&mut self, offset: u64) -> u32 {
        let Some((register, index)) = Self::register(offset) else {
            return 0;
        };
        let counter = &mut self.timers[index];
        match register {
            0x00 => counter.clock_control,
            0x0c => counter.control,
            0x18 => counter.value,
            0x24 => counter.interval,
            0x54 => std::mem::take(&mut counter.interrupt_status),
            0x60 => counter.interrupt_enable,
            _ => 0,
        }
    }

    pub fn write_register(&mut self, offset: u64, value: u32) {
        let Some((register, index)) = Self::register(offset) else {
            return;
        };
        let counter = &mut self.timers[index];
        match register {
            0x00 => {
                if (counter.clock_control ^ value) & 0x1f != 0 {
                    counter.phase = 0;
                }
                counter.clock_control = value & 0x7f;
            }
            0x0c => {
                counter.control = value & 0x6f;
                if value & 0x10 != 0 {
                    counter.value = if counter.count_down() {
                        counter.limit()
                    } else {
                        0
                    };
                    counter.phase = 0;
                }
            }
            0x24 => counter.interval = value,
            0x60 => counter.interrupt_enable = value & 0x3f,
            _ => {}
        }
    }
}

impl NativePeripheral for CadenceTtc {
    fn read(&mut self, _: Option<&str>, offset: u64) -> u64 {
        self.read_register(offset).into()
    }

    fn write(&mut self, _: Option<&str>, offset: u64, value: u64) {
        self.write_register(offset, value as u32);
    }
}
