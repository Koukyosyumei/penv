use crate::bus::{Bus, BusError, CORTEX_M_STEP_HZ};

const RX_UPDATED: u32 = 1 << 1;
const STOPPED: u32 = 1 << 2;
const TX_UPDATED: u32 = 1 << 5;
const EVENTS: u32 = RX_UPDATED | STOPPED | TX_UPDATED;

struct Transfer {
    steps: u64,
    period: u64,
    length: usize,
    rx: Option<u32>,
    tx: Option<u32>,
}

pub(super) struct I2s {
    loopback: bool,
    enabled: bool,
    config: [u32; 10],
    pins: [u32; 5],
    rx: u32,
    tx: u32,
    words: u32,
    events: u32,
    interrupts: u32,
    pub(super) raised: u32,
    active: Option<Transfer>,
}

impl I2s {
    pub(super) fn new(loopback: bool) -> Self {
        Self {
            loopback,
            enabled: false,
            config: [0, 0, 1, 1, 0x2000_0000, 6, 1, 0, 0, 0],
            pins: [u32::MAX; 5],
            rx: 0,
            tx: 0,
            words: 0,
            events: 0,
            interrupts: 0,
            raised: 0,
            active: None,
        }
    }

    pub(super) fn reset(&mut self) {
        *self = Self::new(self.loopback);
    }

    pub(super) fn pending(&self) -> bool {
        self.events & self.interrupts != 0
    }

    pub(super) fn event(offset: u32) -> Option<u32> {
        match offset {
            0x104 => Some(RX_UPDATED),
            0x108 => Some(STOPPED),
            0x114 => Some(TX_UPDATED),
            _ => None,
        }
    }

    fn raise(&mut self, events: u32) {
        self.events |= events;
        self.raised |= events;
    }

    fn period(&self) -> Option<u64> {
        let [mode, rx, tx, mcken, frequency, ratio, width, _, _, channels] = self.config;
        if mode != 0 || mcken == 0 || (rx | tx) == 0 || self.words == 0 {
            return None;
        }
        let divisor = match frequency {
            0x20000000 => 8,
            0x18000000 => 10,
            0x16000000 => 11,
            0x11000000 => 15,
            0x10000000 => 16,
            0x0c000000 => 21,
            0x0b000000 => 23,
            0x08800000 => 30,
            0x08400000 => 31,
            0x08000000 => 32,
            0x06000000 => 42,
            0x04100000 => 63,
            0x020c0000 => 125,
            _ => return None,
        };
        let ratio = *[32, 48, 64, 96, 128, 192, 256, 384, 512].get(ratio as usize)?;
        let samples_per_word = *[4, 2, 1].get(width as usize)?;
        let channels = *[2, 1, 1].get(channels as usize)?;
        let samples = u64::from(self.words) * samples_per_word;
        Some((CORTEX_M_STEP_HZ * samples * ratio * divisor).div_ceil(32_000_000 * channels))
    }

    fn latch(&mut self, period: u64, length: usize, rx: bool, tx: bool) {
        self.active = Some(Transfer {
            steps: period,
            period,
            length,
            rx: rx.then_some(self.rx),
            tx: tx.then_some(self.tx),
        });
        self.raise((if rx { RX_UPDATED } else { 0 }) | (if tx { TX_UPDATED } else { 0 }));
    }

    pub(super) fn tick(&mut self) -> bool {
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
        let length = active.length;
        let tx = active
            .tx
            .map(|address| Self::dma_address(address, length))
            .transpose()?;
        let rx = active
            .rx
            .map(|address| Self::dma_address(address, length))
            .transpose()?;
        let data = tx
            .map(|address| bus.memory_slice(address, length))
            .transpose()?;
        let loopback = self.loopback && rx.is_some();
        let samples = data.filter(|_| loopback).map(<[u8]>::to_vec);
        if let Some(address) = rx {
            let destination = bus.memory(address, length)?;
            if let Some(samples) = samples {
                destination.copy_from_slice(&samples);
            } else {
                destination.fill(0);
            }
        }
        self.latch(active.period, length, rx.is_some(), tx.is_some());
        Ok(())
    }

    fn dma_address(address: u32, length: usize) -> Result<u32, BusError> {
        if address & 3 != 0 {
            return Err(BusError::Misaligned(address, 4));
        }
        super::dma_address(address, length)
    }

    pub(super) fn read(&self, offset: u32) -> Option<u32> {
        Some(match offset {
            0x000 | 0x004 => 0,
            0x300 | 0x304 | 0x308 => self.interrupts,
            0x500 => self.enabled.into(),
            0x504..=0x528 => self.config[((offset - 0x504) / 4) as usize],
            0x538 => self.rx,
            0x540 => self.tx,
            0x550 => self.words,
            0x560..=0x570 => self.pins[((offset - 0x560) / 4) as usize],
            _ => (self.events & Self::event(offset)? != 0).into(),
        })
    }

    pub(super) fn write(&mut self, offset: u32, value: u32) -> bool {
        match offset {
            0x000 => {
                if value & 1 != 0 && self.enabled && self.active.is_none() {
                    let Some(period) = self.period() else {
                        return false;
                    };
                    self.latch(
                        period,
                        self.words as usize * 4,
                        self.config[1] != 0,
                        self.config[2] != 0,
                    );
                }
            }
            0x004 => {
                if value & 1 != 0 {
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
                    self.active = None;
                }
            }
            0x504..=0x528 if self.active.is_none() => {
                let masks = [1, 1, 1, 1, u32::MAX, 15, 3, 1, 1, 3];
                let index = ((offset - 0x504) / 4) as usize;
                self.config[index] = value & masks[index];
            }
            0x538 => self.rx = value,
            0x540 => self.tx = value,
            0x550 if self.active.is_none() => self.words = value & 0x3fff,
            0x560..=0x570 => self.pins[((offset - 0x560) / 4) as usize] = value,
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
    fn invalid_tx_range_does_not_modify_rx() {
        let mut bus = Bus::default();
        let ram = 0x2000_0000;
        bus.map_ram(ram, 16).unwrap();
        bus.memory(ram, 8).unwrap().fill(0xa5);
        let mut i2s = I2s::new(true);
        assert!(i2s.write(0x500, 1));
        assert!(i2s.write(0x508, 1));
        assert!(i2s.write(0x538, ram));
        assert!(i2s.write(0x540, ram + 12));
        assert!(i2s.write(0x550, 2));
        assert!(i2s.write(0, 1));
        assert!(i2s.complete(&mut bus).is_err());
        assert_eq!(bus.memory_slice(ram, 8).unwrap(), &[0xa5; 8]);
    }

    #[test]
    fn disconnected_rx_writes_only_the_configured_word_count() {
        let mut bus = Bus::default();
        let ram = 0x2000_0000;
        bus.map_ram(ram, 8).unwrap();
        bus.memory(ram, 8).unwrap().fill(0xa5);
        let mut i2s = I2s::new(false);
        assert!(i2s.write(0x500, 1));
        assert!(i2s.write(0x508, 1));
        assert!(i2s.write(0x50c, 0));
        assert!(i2s.write(0x538, ram));
        assert!(i2s.write(0x550, 1));
        assert!(i2s.write(0, 1));
        for _ in 0..6399 {
            assert!(!i2s.tick());
        }
        assert!(i2s.tick());
        i2s.complete(&mut bus).unwrap();
        assert_eq!(
            bus.memory_slice(ram, 8).unwrap(),
            &[0, 0, 0, 0, 0xa5, 0xa5, 0xa5, 0xa5]
        );
        assert!(i2s.write(4, 1));
        assert!(!i2s.tick());
    }
}
