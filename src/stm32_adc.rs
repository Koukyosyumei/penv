use crate::bus::CORTEX_M_STEP_HZ;

const READY: u32 = 1;
const EOC: u32 = 1 << 2;
const EOS: u32 = 1 << 3;
const OVERRUN: u32 = 1 << 4;
const WATCHDOG: u32 = 1 << 7;
const EXTERNAL_TRIGGER: u32 = 3 << 10;
const OVERWRITE: u32 = 1 << 12;
const CONTINUOUS: u32 = 1 << 13;
const WAIT: u32 = 1 << 14;
const DISCONTINUOUS: u32 = 1 << 16;
const CHANNEL_MASK: u32 = (1 << 19) - 1;

pub struct Stm32F0Adc {
    pub base: u32,
    pub irq: Option<u8>,
    pub dma_request: u8,
    reference: f64,
    inputs: [u32; 19],
    trigger_frequency: u32,
    phase: u64,
    enabled: bool,
    started: bool,
    remaining: u32,
    status: u32,
    interrupt_enable: u32,
    config: u32,
    config2: u32,
    sample_time: u32,
    selected: u32,
    watchdog_thresholds: u32,
    common: u32,
    data: u32,
}

impl Stm32F0Adc {
    pub const SIZE: u32 = 0x400;

    pub fn new(base: u32, irq: Option<u8>, reference: f64, trigger_frequency: u32) -> Self {
        Self {
            base,
            irq,
            dma_request: 0,
            reference,
            trigger_frequency,
            inputs: [0; 19],
            phase: 0,
            enabled: false,
            started: false,
            remaining: 0,
            status: 0,
            interrupt_enable: 0,
            config: 0,
            config2: 0,
            sample_time: 0,
            selected: 0,
            watchdog_thresholds: 0x0fff_0000,
            common: 0,
            data: 0,
        }
    }

    pub fn reset(&mut self) {
        let inputs = self.inputs;
        let request = self.dma_request;
        *self = Self::new(self.base, self.irq, self.reference, self.trigger_frequency);
        self.inputs = inputs;
        self.dma_request = request;
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    pub fn validate_input(&self, channel: usize, volts: f64) -> Result<(), String> {
        if channel >= self.inputs.len() {
            return Err("STM32 ADC channel must be in 0–18".into());
        }
        if !volts.is_finite() || volts < 0.0 {
            return Err("STM32 ADC input voltage must be finite and nonnegative".into());
        }
        Ok(())
    }

    pub fn set_input(&mut self, channel: usize, volts: f64) -> Result<(), String> {
        self.validate_input(channel, volts)?;
        self.inputs[channel] =
            ((volts / self.reference).min(1.0) * f64::from(u32::MAX)).round() as u32;
        Ok(())
    }

    pub fn read(&mut self, address: u32) -> u32 {
        match address - self.base {
            0x00 => self.status,
            0x04 => self.interrupt_enable,
            0x08 => u32::from(self.enabled) | (u32::from(self.started) << 2),
            0x0c => self.config,
            0x10 => self.config2,
            0x14 => self.sample_time,
            0x20 => self.watchdog_thresholds,
            0x28 => self.selected,
            0x40 => {
                let data = self.data;
                self.acknowledge(EOC);
                data
            }
            0x308 => self.common,
            _ => 0,
        }
    }

    pub fn write(&mut self, address: u32, value: u32) {
        match address - self.base {
            0x00 => self.acknowledge(value),
            0x04 => self.interrupt_enable = value & 0x9f,
            0x08 => {
                if value & 1 != 0 && !self.enabled {
                    self.enabled = true;
                    self.status |= READY;
                }
                if value & 2 != 0 {
                    self.enabled = false;
                }
                if !self.enabled || value & 0x10 != 0 {
                    self.remaining = 0;
                    self.started = false;
                } else if value & 4 != 0 && !self.started && self.selected != 0 {
                    self.started = true;
                    self.phase = 0;
                    if self.config & EXTERNAL_TRIGGER == 0 {
                        self.start_sample();
                    }
                }
                // Calibration completes immediately and command bits read clear.
            }
            0x0c if !self.started => self.config = value & 0x7cc1_fdff,
            0x10 => self.config2 = value & (3 << 30),
            0x14 => self.sample_time = value & 7,
            0x20 => self.watchdog_thresholds = value & 0x0fff_0fff,
            0x28 if !self.started => self.selected = value & CHANNEL_MASK,
            0x308 => self.common = value & (7 << 22),
            _ => {}
        }
    }

    pub fn pending_dma(&self) -> u8 {
        if self.config & 1 != 0 && self.status & (EOC | OVERRUN) == EOC {
            self.dma_request
        } else {
            0
        }
    }

    pub fn tick(&mut self) -> bool {
        if self.started && self.config & (EXTERNAL_TRIGGER | CONTINUOUS) != 0 {
            self.phase += u64::from(self.trigger_frequency);
            if self.phase >= CORTEX_M_STEP_HZ {
                self.phase %= CORTEX_M_STEP_HZ;
                if self.config & WAIT == 0 || self.status & EOC == 0 {
                    self.start_sample();
                }
            }
        }
        self.status & self.interrupt_enable != 0
    }

    fn acknowledge(&mut self, flags: u32) {
        let next = flags & self.status & EOC != 0 && self.started && self.remaining != 0;
        self.status &= !flags;
        if next && self.config & DISCONTINUOUS == 0 {
            self.sample();
        }
    }

    fn start_sample(&mut self) {
        if self.remaining == 0 {
            self.remaining = self.selected;
        }
        self.sample();
    }

    fn sample(&mut self) {
        let channel = if self.config & 4 == 0 {
            self.remaining.trailing_zeros()
        } else {
            31 - self.remaining.leading_zeros()
        };
        self.remaining &= !(1 << channel);
        let resolution = 12 - ((self.config >> 3) & 3) * 2;
        let input = u64::from(self.inputs[channel as usize]);
        let maximum = u64::from(u32::MAX);
        let sample = ((input * ((1 << resolution) - 1) + maximum / 2) / maximum) as u32;
        if self.status & EOC != 0 {
            self.status |= OVERRUN;
        }
        if self.status & OVERRUN == 0 || self.config & OVERWRITE != 0 {
            self.data = if self.config & (1 << 5) != 0 {
                sample << (16 - resolution)
            } else {
                sample
            };
        }
        self.status |= EOC | (1 << 1);
        if self.config & (1 << 23) != 0
            && (self.config & (1 << 22) == 0 || (self.config >> 26) & 31 == channel)
            && (sample < self.watchdog_thresholds & 0xfff
                || sample > self.watchdog_thresholds >> 16)
        {
            self.status |= WATCHDOG;
        }
        if self.remaining == 0 {
            self.status |= EOS;
            self.started &= self.config & (EXTERNAL_TRIGGER | CONTINUOUS) != 0;
        } else if self.config & (DISCONTINUOUS | EXTERNAL_TRIGGER) == DISCONTINUOUS {
            self.started = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renode_stm32f072_adc_sample_values_drive_eoc_irq() {
        // STM32F072b.robot: Should Read ADC, with the board REPL's ADC settings.
        let mut adc = Stm32F0Adc::new(0, Some(12), 3.3, 1000);
        adc.write(0x04, EOC);
        adc.write(0x28, 1 << 1);
        adc.write(0x08, 1);
        for (volts, expected) in [(0.6, 745), (1.2, 1489)] {
            adc.set_input(1, volts).unwrap();
            assert!(!adc.tick());
            adc.write(0x08, 4);
            assert!(adc.tick());
            assert_eq!(adc.read(0x00) & (EOC | EOS), EOC | EOS);
            assert_eq!(adc.read(0x40), expected);
            assert!(!adc.tick());
            adc.write(0x00, EOS);
        }
    }
}

#[derive(Default)]
struct AdcSampleQueue {
    queued: std::collections::VecDeque<(u32, u32)>,
    repeated: Option<u32>,
}

impl AdcSampleQueue {
    fn next(&mut self) -> u32 {
        let Some((value, count)) = self.queued.front_mut() else {
            return self.repeated.unwrap_or(0);
        };
        let sample = *value;
        *count -= 1;
        if *count == 0 {
            self.queued.pop_front();
        }
        sample
    }
}

pub(crate) struct Stm32Adc {
    pub base: u32,
    pub name: String,
    samples: [AdcSampleQueue; 19],
    pub irq: Option<u8>,
    status: u32,
    control1: u32,
    control2: u32,
    sequence: [u32; 3],
    channel_index: usize,
    phase: u64,
    converting: bool,
    interrupt: bool,
    data: u32,
}

impl Stm32Adc {
    pub const SIZE: u32 = 0x50;

    pub fn new(base: u32, name: String, irq: Option<u8>) -> Self {
        Self {
            base, name, irq, samples: std::array::from_fn(|_| AdcSampleQueue::default()), status: 0, control1: 0, control2: 0, sequence: [0; 3],
            channel_index: 0, phase: 0, converting: false, interrupt: false, data: 0,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new(self.base, std::mem::take(&mut self.name), self.irq);
    }

    pub fn feed_sample(&mut self, value: u32, channel: u32, repeat: i32) -> Result<(), String> {
        let queue = self.samples.get_mut(channel as usize)
            .ok_or_else(|| format!("invalid STM32 ADC channel {channel}"))?;
        queue.repeated = (repeat == -1).then_some(value);
        if repeat > 0 {
            queue.queued.push_back((value, repeat as u32));
        }
        Ok(())
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    pub fn read(&mut self, address: u32) -> u32 {
        match address - self.base {
            0 => self.status,
            4 => self.control1,
            8 => self.control2,
            offset @ (0x2c | 0x30 | 0x34) => self.sequence[(offset as usize - 0x2c) / 4],
            0x4c => {
                self.status &= !2;
                self.interrupt = false;
                self.data
            }
            _ => 0,
        }
    }

    pub fn write(&mut self, address: u32, value: u32) {
        match address - self.base {
            0 => self.status = value & 2,
            4 => self.control1 = value & 0x120,
            8 => {
                self.control2 = value & 0x703;
                if value & (1 << 30) != 0 && self.control2 & 1 != 0 {
                    self.converting = true;
                }
            }
            offset @ (0x2c | 0x30 | 0x34) => {
                let index = (offset as usize - 0x2c) / 4;
                self.sequence[index] = value & if index == 0 { 0xff_ffff } else { 0x3fff_ffff };
            }
            _ => {}
        }
    }

    pub fn tick(&mut self, rate: u32) -> bool {
        if !self.converting {
            return self.interrupt;
        }
        // The upstream sampling clock completes one conversion per 100 microseconds.
        self.phase += 10_000;
        if self.phase < u64::from(rate) {
            return self.interrupt;
        }
        self.phase %= u64::from(rate);
        let length = ((self.sequence[0] >> 20) & 15) as usize + 1;
        let index = self.channel_index % length;
        let shift = (index % 6) * 5;
        let channel = ((self.sequence[2 - index / 6] >> shift) & 31) as usize;
        self.data = self.samples.get_mut(channel).map_or(0, AdcSampleQueue::next);
        let scan_active = self.control1 & (1 << 8) != 0 && self.channel_index + 1 < length;
        if !scan_active || self.control2 & (1 << 10) != 0 {
            self.status |= 2;
            self.interrupt |= self.control1 & (1 << 5) != 0;
        } else {
            self.status &= !2;
        }
        self.channel_index = (self.channel_index + 1) % length;
        self.converting = scan_active || self.control2 & 2 != 0;
        self.interrupt
    }
}

#[cfg(test)]
mod regular_adc_tests {
    use super::Stm32Adc;

    #[test]
    fn scans_sampled_channels_and_signals_the_end_of_the_sequence() {
        let mut adc = Stm32Adc::new(0, "adc".into(), Some(12));
        adc.feed_sample(0x123, 3, 1).unwrap();
        adc.feed_sample(0x456, 7, -1).unwrap();
        adc.write(4, 0x120);
        adc.write(0x2c, 1 << 20);
        adc.write(0x34, (7 << 5) | 3);
        adc.write(8, (1 << 30) | 1);
        assert!(!adc.tick(20_000));
        assert!(!adc.tick(20_000));
        assert_eq!(adc.read(0x4c), 0x123);
        assert!(!adc.tick(20_000));
        assert!(adc.tick(20_000));
        assert_eq!(adc.read(0x4c), 0x456);
        assert!(!adc.tick(20_000));
    }
}
