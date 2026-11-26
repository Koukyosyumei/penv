use std::{fs, path::Path};

use crate::resd::{SampleBlock, Samples};

struct MultiFrequency {
    blocks: Vec<SampleBlock<[i32; 3]>>,
    current: Option<usize>,
    started_at: u64,
}

impl MultiFrequency {
    fn select(&mut self, rate: u32, now: u64) {
        let start = self.current.map_or(0, |index| index + 1);
        if let Some((index, _)) = self.blocks.iter().enumerate().skip(start)
            .find(|(_, block)| block.period.is_some_and(|period| 1_000_000_000u64.div_ceil(period) == u64::from(rate)))
        {
            self.current = Some(index);
            self.started_at = now;
        }
    }

    fn sample_at(&self, now: u64) -> Option<[i32; 3]> {
        let block = self.blocks.get(self.current?)?;
        let timestamp = block.start.saturating_add(now.saturating_sub(self.started_at));
        block.samples.sample_at(timestamp, true)
    }
}

pub(crate) struct Lis2dw12 {
    registers: [u8; 64],
    selected: Option<u8>,
    address: u8,
    acceleration: [i32; 3],
    defaults: [i32; 3],
    samples: Option<(u64, Samples<[i32; 3]>)>,
    multi: Option<MultiFrequency>,
    rate: u32,
}

impl Default for Lis2dw12 {
    fn default() -> Self {
        let mut registers = [0; 64];
        registers[0x0f] = 0x44;
        registers[0x21] = 4;
        Self {
            registers,
            selected: None,
            address: 0,
            acceleration: [0; 3],
            defaults: [0; 3],
            samples: None,
            multi: None,
            rate: 0,
        }
    }
}

impl Lis2dw12 {
    pub(crate) fn start_write(&mut self) {
        self.selected = None;
    }

    pub(crate) fn write(&mut self, value: u8, now_ns: u64) {
        let Some(address) = self.selected else {
            self.address = value;
            self.selected = Some(value);
            return;
        };
        if address == 0x21 && value & 0x40 != 0 {
            let defaults = self.defaults;
            *self = Self::default();
            self.defaults = defaults;
            return;
        }
        if let Some(register) = self.registers.get_mut(usize::from(address))
            && matches!(address, 0x20..=0x25 | 0x2e | 0x30..=0x36 | 0x3c..=0x3f)
        {
            *register = value;
        }
        if address == 0x20 {
            const RATES: [u32; 10] = [0, 2, 13, 25, 50, 100, 200, 400, 800, 1600];
            self.set_rate(RATES.get(usize::from(value >> 4)).copied().unwrap_or(0), now_ns);
        }
        if self.registers[0x21] & 4 != 0 {
            let next = address.wrapping_add(1);
            self.selected = Some(next);
            self.address = next;
        }
    }

    pub(crate) fn read(&mut self, target: &str, now_ns: u64) -> u8 {
        let address = self.address;
        if self.registers[0x21] & 4 != 0 {
            self.address = address.wrapping_add(1);
        }
        let active = self.registers[0x20] & 0xf0 != 0;
        match address {
            0x27 => u8::from(active),
            0x28..=0x2d if active => {
                if address == 0x28 {
                    self.update_sample(now_ns);
                }
                let mode = self.registers[0x20] & 0x0f;
                let scale = 1 << ((self.registers[0x25] >> 4) & 3);
                let (shift, sensitivity, limit) = if mode == 0 {
                    (4, 976 * scale, 2048)
                } else {
                    (2, 244 * scale, 8192)
                };
                let axis = usize::from((address - 0x28) / 2);
                let steps = (self.acceleration[axis] / sensitivity).clamp(-limit, limit - 1);
                let raw = (steps << shift) as u16;
                crate::monitor_log::record(-1, &format!(
                    "{target}: Conversion done with sensitivity: {:.3}, result: 0x{raw:04X}",
                    f64::from(sensitivity) / 1000.0
                ));
                raw.to_le_bytes()[usize::from(address & 1)]
            }
            _ => self.registers.get(usize::from(address)).copied().unwrap_or(0),
        }
    }

    pub(crate) fn update_sample(&mut self, now_ns: u64) {
        if let Some(sample) = self.multi.as_ref().and_then(|stream| stream.sample_at(now_ns)) {
            self.acceleration = sample;
        } else if let Some((start, samples)) = &self.samples
            && let Some(sample) = samples.sample_at(now_ns.saturating_sub(*start), false)
        {
            self.acceleration = sample;
        } else if self.registers[0x2e] & 0xe0 != 0 {
            self.acceleration = self.defaults;
        }
    }

    pub(crate) fn feed(&mut self, path: &Path, now_ns: u64, multi_frequency: bool) -> Result<(), String> {
        let bytes = fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
        let decode = |bytes: [u8; 12]| {
            let (axes, _) = bytes.as_chunks::<4>();
            std::array::from_fn(|axis| i32::from_le_bytes(axes[axis]))
        };
        let describe = |error| format!("{}: {error}", path.display());
        if multi_frequency {
            let blocks = Samples::decode_blocks(&bytes, 2, 0, decode).map_err(describe)?;
            if blocks.is_empty() || blocks.iter().any(|block| block.period.is_none()) {
                return Err(describe("expected periodic acceleration blocks"));
            }
            let mut stream = MultiFrequency { blocks, current: None, started_at: now_ns };
            stream.select(self.rate, now_ns);
            self.samples = None;
            self.multi = Some(stream);
        } else {
            let samples = Samples::decode(&bytes, 2, decode).map_err(describe)?;
            self.multi = None;
            self.samples = Some((now_ns, samples));
        }
        self.update_sample(now_ns);
        Ok(())
    }

    pub(crate) fn set_rate(&mut self, rate: u32, now_ns: u64) {
        if rate != self.rate && let Some(stream) = &mut self.multi {
            stream.select(rate, now_ns);
        }
        self.rate = rate;
        self.update_sample(now_ns);
    }

    pub(crate) fn axis(&mut self, axis: usize, now_ns: u64) -> f64 {
        self.update_sample(now_ns);
        f64::from(self.acceleration[axis]) / 1_000_000.0
    }

    pub(crate) fn set_axis(&mut self, axis: usize, value: i32, default: bool) {
        if default {
            self.defaults[axis] = value;
        } else {
            self.acceleration[axis] = value;
        }
    }
}
