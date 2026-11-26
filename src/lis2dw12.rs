use std::{fs, path::Path};

use crate::resd::Samples;

pub(crate) struct Lis2dw12 {
    registers: [u8; 64],
    selected: Option<u8>,
    address: u8,
    acceleration: [i32; 3],
    defaults: [i32; 3],
    samples: Option<(u64, Samples<[i32; 3]>)>,
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
        }
    }
}

impl Lis2dw12 {
    pub(crate) fn start_write(&mut self) {
        self.selected = None;
    }

    pub(crate) fn write(&mut self, value: u8) {
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
        if let Some((start, samples)) = &self.samples
            && let Some(sample) = samples.sample_at(now_ns.saturating_sub(*start), false)
        {
            self.acceleration = sample;
        } else if self.registers[0x2e] & 0xe0 != 0 {
            self.acceleration = self.defaults;
        }
    }

    pub(crate) fn feed(&mut self, path: &Path, now_ns: u64) -> Result<(), String> {
        let bytes = fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
        let samples = Samples::decode(&bytes, 2, |bytes: [u8; 12]| {
            let (axes, _) = bytes.as_chunks::<4>();
            std::array::from_fn(|axis| i32::from_le_bytes(axes[axis]))
        }).map_err(|error| format!("{}: {error}", path.display()))?;
        self.samples = Some((now_ns, samples));
        self.update_sample(now_ns);
        Ok(())
    }

    pub(crate) fn set_axis(&mut self, axis: usize, value: i32, default: bool) {
        if default {
            self.defaults[axis] = value;
        } else {
            self.acceleration[axis] = value;
        }
    }
}
