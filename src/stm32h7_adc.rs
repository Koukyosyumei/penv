use crate::{machine::VIRTUAL_STEPS_PER_SECOND, stm32_uart::Stm32Irq};

pub const CHANNELS: usize = 19;
const READY: u32 = 1;
const END_SAMPLE: u32 = 1 << 1;
const EOC: u32 = 1 << 2;
const EOS: u32 = 1 << 3;
const OVERRUN: u32 = 1 << 4;
const JEOC: u32 = 1 << 5;
const JEOS: u32 = 1 << 6;
const END_CALIBRATION: u32 = 1 << 11;
const REGULATOR: u32 = 1 << 28;
const REGULATOR_READY: u32 = 1 << 12;
const ADC_FLAGS: u32 = 0x1fff;
const DMA_ENABLE: u32 = 1;
const EXTERNAL: u32 = 3 << 10;
const OVERWRITE: u32 = 1 << 12;
const CONTINUOUS: u32 = 1 << 13;
const WAIT: u32 = 1 << 14;

#[derive(Clone, Copy)]
pub enum AdcClock {
    Adc12,
    Adc3,
}

#[derive(Clone, Copy)]
pub struct AdcRequest {
    pub endpoint: u32,
    pub id: u8,
}

pub struct Adc {
    pub base: u32,
    pub irq: Option<Stm32Irq>,
    pub clock: Option<AdcClock>,
    pub dma: Option<AdcRequest>,
    reference: u32,
    frequency: u32,
    inputs: [u32; CHANNELS],
    phase: u32,
    registers: [u32; 51],
    common: u32,
    secondary_enabled: bool,
    enabled: bool,
    timer_running: bool,
    regular: Option<u8>,
    injected: Option<u8>,
    service_regular: bool,
    service_injected: bool,
    dma_pending: bool,
}

impl Adc {
    pub const SIZE: u32 = 0x400;
    pub const DATA: u32 = 0x40;

    pub fn new(base: u32, reference: u32, frequency: u32) -> Self {
        Self {
            base,
            irq: None,
            clock: None,
            dma: None,
            reference,
            frequency,
            inputs: [0; CHANNELS],
            phase: 0,
            registers: [0; 51],
            common: 0,
            secondary_enabled: false,
            enabled: false,
            timer_running: false,
            regular: None,
            injected: None,
            service_regular: false,
            service_injected: false,
            dma_pending: false,
        }
    }

    pub fn reset(&mut self) {
        let mut reset = Self::new(self.base, self.reference, self.frequency);
        reset.irq = self.irq;
        reset.clock = self.clock;
        reset.dma = self.dma;
        reset.inputs = self.inputs;
        *self = reset;
    }

    pub fn set_input(&mut self, channel: usize, microvolts: u32) {
        self.inputs[channel] = microvolts;
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    pub fn interrupt(&self) -> Option<Stm32Irq> {
        let pending = self.registers[0] | self.regulator_ready();
        self.irq.filter(|_| pending & self.registers[1] != 0)
    }

    fn regulator_ready(&self) -> u32 {
        if self.registers[2] & REGULATOR != 0 {
            REGULATOR_READY
        } else {
            0
        }
    }

    pub fn take_request(&mut self) -> Option<(AdcRequest, u32)> {
        if std::mem::take(&mut self.dma_pending) {
            self.dma.map(|request| (request, self.registers[16]))
        } else {
            None
        }
    }

    pub fn data_read(&mut self) {
        self.registers[0] &= !EOC;
        self.dma_pending = false;
        self.service_regular = self.regular.is_some();
    }

    pub fn tick(&mut self) {
        if std::mem::take(&mut self.service_regular) {
            self.sample_regular();
        }
        if std::mem::take(&mut self.service_injected) {
            self.sample_injected();
        }
        if !self.timer_running {
            return;
        }
        let phase = u64::from(self.phase) + u64::from(self.frequency);
        self.phase = (phase % u64::from(VIRTUAL_STEPS_PER_SECOND)) as u32;
        if phase >= u64::from(VIRTUAL_STEPS_PER_SECOND)
            && self.regular.is_none()
            && (self.registers[3] & WAIT == 0 || self.registers[0] & EOC == 0)
        {
            self.start_regular();
        }
    }

    pub fn read(&mut self, address: u32) -> u32 {
        match address - self.base {
            0 => self.registers[0] | self.regulator_ready(),
            8 => {
                self.registers[2]
                    | (0x3f << 22)
                    | u32::from(self.enabled)
                    | (u32::from(self.regular.is_some() || self.timer_running) << 2)
                    | (u32::from(self.injected.is_some()) << 3)
            }
            Self::DATA => {
                let value = self.registers[16];
                self.data_read();
                value
            }
            0x108 => u32::from(self.secondary_enabled),
            0x308 => self.common,
            offset if adc_register(offset) => self.registers[(offset / 4) as usize],
            _ => 0,
        }
    }

    pub fn write(&mut self, address: u32, value: u32) {
        match address - self.base {
            0 => {
                if self.registers[0] & value & EOC != 0 {
                    self.data_read();
                }
                if self.registers[0] & value & JEOC != 0 {
                    self.service_injected = self.injected.is_some();
                }
                self.registers[0] &= !(value & ADC_FLAGS);
            }
            4 => self.registers[1] = value & ADC_FLAGS,
            8 => self.control(value),
            0x0c => {
                self.registers[3] = value;
                if value & (CONTINUOUS | EXTERNAL) == 0 {
                    self.timer_running = false;
                }
            }
            0x1c => self.registers[7] = value & ((1 << CHANNELS) - 1),
            Self::DATA | 0x80..=0x8c => {}
            0x108 => {
                if value & 2 != 0 {
                    self.secondary_enabled = false;
                } else if value & 1 != 0 {
                    self.secondary_enabled = true;
                }
            }
            0x308 => self.common = value & 0x01ff_0000,
            offset @ (0x20 | 0x24 | 0xb0 | 0xb4 | 0xb8 | 0xbc) => {
                self.registers[(offset / 4) as usize] = value & 0x03ff_ffff;
            }
            offset if adc_register(offset) => self.registers[(offset / 4) as usize] = value,
            _ => {}
        }
    }

    fn control(&mut self, value: u32) {
        self.registers[2] = value & 0x7001_0300;
        if value >> 31 != 0 && !self.enabled {
            self.registers[0] |= END_CALIBRATION;
        }
        if value & 2 != 0 {
            self.enabled = false;
            self.stop_regular();
            self.injected = None;
            self.service_injected = false;
            return;
        }
        if value & 1 != 0 && !self.enabled {
            self.enabled = true;
            self.registers[0] |= READY;
        }
        if !self.enabled {
            return;
        }
        if value & (1 << 4) != 0 {
            self.stop_regular();
        } else if value & (1 << 2) != 0 && self.regular.is_none() && !self.timer_running {
            if self.registers[3] & (EXTERNAL | CONTINUOUS) != 0 {
                self.phase = 0;
                self.timer_running = true;
            } else {
                self.start_regular();
            }
        }
        if value & (1 << 5) != 0 {
            self.injected = None;
            self.service_injected = false;
        } else if value & (1 << 3) != 0 && self.injected.is_none() {
            self.injected = Some(0);
            self.sample_injected();
        }
    }

    fn stop_regular(&mut self) {
        self.timer_running = false;
        self.regular = None;
        self.service_regular = false;
        self.dma_pending = false;
    }

    fn start_regular(&mut self) {
        self.regular = Some(0);
        self.sample_regular();
    }

    fn sample_regular(&mut self) {
        let Some(index) = self.regular else { return };
        let slot = u32::from(index) + 1;
        let channel =
            ((self.registers[(12 + slot / 5) as usize] >> (6 * (slot % 5))) & 31) as usize;
        let sample = self.convert(channel);
        if self.registers[0] & EOC != 0 {
            self.registers[0] |= OVERRUN;
        }
        if self.registers[0] & OVERRUN == 0 || self.registers[3] & OVERWRITE != 0 {
            self.registers[16] = sample;
        }
        self.registers[0] |= EOC | END_SAMPLE;
        let length = (self.registers[12] & 15) as u8;
        if index == length {
            self.registers[0] |= EOS;
            self.regular = None;
        } else {
            self.regular = Some(index + 1);
        }
        self.watchdogs(channel, sample);
        self.dma_pending = self.registers[3] & DMA_ENABLE != 0 && self.registers[0] & OVERRUN == 0;
    }

    fn sample_injected(&mut self) {
        let Some(index) = self.injected else { return };
        let sequence = self.registers[0x4c / 4];
        let channel = ((sequence >> (9 + 6 * index)) & 31) as usize;
        self.registers[32 + usize::from(index)] = self.convert(channel);
        self.registers[0] |= JEOC;
        if u32::from(index) == sequence & 3 {
            self.injected = None;
            self.registers[0] |= JEOS;
        } else {
            self.injected = Some(index + 1);
        }
    }

    fn convert(&self, channel: usize) -> u32 {
        let bits = [16, 14, 12, 8][((self.registers[3] >> 2) & 3) as usize];
        let voltage = self
            .inputs
            .get(channel)
            .copied()
            .unwrap_or(0)
            .min(self.reference);
        let value = (f64::from(voltage) / f64::from(self.reference) * f64::from((1u32 << bits) - 1))
            .round_ties_even() as u32;
        if self.registers[3] & (1 << 5) != 0 {
            value << (16 - bits)
        } else {
            value
        }
    }

    fn watchdogs(&mut self, channel: usize, sample: u32) {
        let config = self.registers[3];
        let first = config & (1 << 23) != 0
            && (config & (1 << 22) == 0 || ((config >> 26) & 31) as usize == channel);
        let enabled = [
            first,
            self.registers[40] & (1 << channel) != 0,
            self.registers[41] & (1 << channel) != 0,
        ];
        for (index, ((low, high), enable)) in [(8, 9), (44, 45), (46, 47)]
            .into_iter()
            .zip(enabled)
            .enumerate()
        {
            if enable && (sample < self.registers[low] || sample > self.registers[high]) {
                self.registers[0] |= 1 << (7 + index);
            }
        }
    }
}

fn adc_register(offset: u32) -> bool {
    matches!(
        offset,
        0x04 | 0x0c..=0x24 | 0x30..=0x3c | 0x4c | 0x60..=0x6c | 0x80..=0x8c | 0xa0..=0xa4 | 0xb0..=0xc8
    ) && offset & 3 == 0
}
