use crate::bus::{Bus, BusError, CORTEX_M_STEP_HZ};

const STARTED: u32 = 1;
const STOPPED: u32 = 2;
const END: u32 = 4;
const EVENTS: u32 = STARTED | STOPPED | END;

struct Buffer {
    address: u32,
    samples: usize,
    steps: u64,
}

pub(super) struct Pdm {
    enabled: bool,
    running: bool,
    active: Option<Buffer>,
    events: u32,
    interrupts: u32,
    pub(super) raised: u32,
    clock: u32,
    mode: u32,
    ratio: u32,
    gain: [u32; 2],
    pins: [u32; 2],
    address: u32,
    count: u32,
    input: Vec<u8>,
    sample: usize,
}

impl Default for Pdm {
    fn default() -> Self {
        Self {
            enabled: false,
            running: false,
            active: None,
            events: 0,
            interrupts: 0,
            raised: 0,
            clock: 0x0840_0000,
            mode: 0,
            ratio: 0,
            gain: [40; 2],
            pins: [u32::MAX; 2],
            address: 0,
            count: 0,
            input: Vec::new(),
            sample: 0,
        }
    }
}

impl Pdm {
    pub(super) fn reset(&mut self) {
        *self = Self {
            input: std::mem::take(&mut self.input),
            ..Self::default()
        };
    }

    pub(super) fn set_input(&mut self, input: Vec<u8>) {
        self.input = input;
        self.sample = 0;
    }

    pub(super) fn pending(&self) -> bool {
        self.events & self.interrupts != 0
    }

    pub(super) fn event(offset: u32) -> Option<u32> {
        match offset {
            0x100 => Some(STARTED),
            0x104 => Some(STOPPED),
            0x108 => Some(END),
            _ => None,
        }
    }

    fn raise(&mut self, mask: u32) {
        self.events |= mask;
        self.raised |= mask;
    }

    fn clock_divisor(value: u32) -> Option<u64> {
        Some(match value {
            0x0800_0000 => 32,
            0x0840_0000 => 31,
            0x0880_0000 => 30,
            0x0980_0000 => 26,
            0x0a00_0000 => 25,
            0x0a80_0000 => 24,
            _ => return None,
        })
    }

    fn latch(&mut self) {
        if self.count == 0 {
            self.active = None;
            return;
        }
        let divisor = Self::clock_divisor(self.clock).unwrap();
        let ratio = if self.ratio == 0 { 64 } else { 80 };
        let channels = if self.mode & 1 == 0 { 2 } else { 1 };
        let steps = (u64::from(self.count) * ratio * divisor * CORTEX_M_STEP_HZ)
            .div_ceil(32_000_000 * channels);
        self.active = Some(Buffer {
            address: self.address,
            samples: self.count as usize,
            steps,
        });
    }

    pub(super) fn tick(&mut self) -> bool {
        if !self.running {
            return false;
        }
        if self.active.is_none() {
            self.latch();
        }
        let Some(active) = &mut self.active else {
            return false;
        };
        active.steps = active.steps.saturating_sub(1);
        active.steps == 0
    }

    pub(super) fn complete(&mut self, bus: &mut Bus) -> Result<(), BusError> {
        let Some(active) = &self.active else {
            return Ok(());
        };
        let length = active.samples * 2;
        let address = super::dma_address(active.address, length)?;
        if address & 3 != 0 {
            return Err(BusError::Misaligned(address, 4));
        }
        let destination = bus.memory(address, length)?;
        let swap = if self.mode == 2 { 1 } else { 0 };
        for sample in destination.chunks_exact_mut(2) {
            let index = (self.sample ^ swap).saturating_mul(2);
            if let Some(data) = self.input.get(index..index.saturating_add(2)) {
                sample.copy_from_slice(data);
            } else {
                sample.fill(0);
            }
            self.sample = self.sample.saturating_add(1);
        }
        self.raise(END);
        self.latch();
        self.raise(STARTED);
        Ok(())
    }

    pub(super) fn read(&self, offset: u32) -> Option<u32> {
        Some(match offset {
            0x000 | 0x004 => 0,
            0x300 | 0x304 | 0x308 => self.interrupts,
            0x500 => self.enabled.into(),
            0x504 => self.clock,
            0x508 => self.mode,
            0x518 | 0x51c => self.gain[((offset - 0x518) / 4) as usize],
            0x520 => self.ratio,
            0x540 | 0x544 => self.pins[((offset - 0x540) / 4) as usize],
            0x560 => self.address,
            0x564 => self.count,
            _ => (self.events & Self::event(offset)? != 0).into(),
        })
    }

    pub(super) fn write(&mut self, offset: u32, value: u32) -> bool {
        match offset {
            0x000 => {
                if value & 1 != 0 && self.enabled && !self.running {
                    self.running = true;
                    self.latch();
                    self.raise(STARTED);
                }
            }
            0x004 => {
                if value & 1 != 0 {
                    self.running = false;
                    self.active = None;
                    self.raise(STOPPED);
                }
            }
            0x300 => self.interrupts = value & EVENTS,
            0x304 => self.interrupts |= value & EVENTS,
            0x308 => self.interrupts &= !value,
            0x500 => {
                self.enabled = value & 1 != 0;
                if !self.enabled {
                    self.running = false;
                    self.active = None;
                }
            }
            0x504 if !self.running && Self::clock_divisor(value).is_some() => self.clock = value,
            0x508 if !self.running => self.mode = value & 3,
            0x518 | 0x51c => self.gain[((offset - 0x518) / 4) as usize] = value & 0x7f,
            0x520 if !self.running => self.ratio = value & 1,
            0x540 | 0x544 => self.pins[((offset - 0x540) / 4) as usize] = value,
            0x560 => self.address = value,
            0x564 => self.count = value & 0x7fff,
            _ => {
                let Some(bit) = Self::event(offset) else {
                    return false;
                };
                super::set_event(&mut self.events, bit, value);
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_without_count_arms_dma_and_reset_rewinds_the_pcm() {
        let mut bus = Bus::default();
        let ram = 0x2000_0000;
        bus.map_ram(ram, 8).unwrap();
        let mut pdm = Pdm::default();
        pdm.set_input(vec![0x34, 0x92]);
        for _ in 0..2 {
            assert!(pdm.write(0x500, 1));
            assert!(pdm.write(0x304, STARTED));
            assert!(pdm.write(0, 1));
            assert!(pdm.pending());
            assert!(!pdm.tick());
            assert!(pdm.write(0x100, 0));
            assert!(pdm.write(0x560, ram));
            assert!(pdm.write(0x564, 1));
            for _ in 0..3099 {
                assert!(!pdm.tick());
            }
            assert!(pdm.tick());
            pdm.complete(&mut bus).unwrap();
            assert_eq!(bus.read_ram::<2>(ram).unwrap(), 0x9234);
            assert_eq!(pdm.read(0x108), Some(1));
            pdm.reset();
            assert!(!pdm.pending());
            bus.memory(ram, 2).unwrap().fill(0);
        }
    }

    #[test]
    fn invalid_dma_buffer_keeps_pcm_and_guest_ram_intact() {
        let mut bus = Bus::default();
        let ram = 0x2000_0000;
        bus.map_ram(ram, 4).unwrap();
        bus.memory(ram, 4).unwrap().fill(0x55);
        let mut pdm = Pdm::default();
        pdm.set_input(vec![1, 2]);
        assert!(pdm.write(0x560, ram));
        assert!(pdm.write(0x564, 3));
        assert!(pdm.write(0x500, 1));
        assert!(pdm.write(0, 1));
        assert!(pdm.complete(&mut bus).is_err());
        assert_eq!(pdm.sample, 0);
        assert_eq!(pdm.read(0x108), Some(0));
        assert_eq!(bus.memory_slice(ram, 4).unwrap(), &[0x55; 4]);
        assert!(pdm.write(4, 1));
        assert!(!pdm.tick());
        assert_eq!(pdm.read(0x104), Some(1));
    }
}
