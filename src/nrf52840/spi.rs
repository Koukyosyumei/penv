use std::collections::VecDeque;

pub(super) const READY: u32 = 1 << 2;

pub(super) struct Spi {
    loopback: bool,
    enabled: bool,
    ready: bool,
    interrupts: u32,
    pub(super) raised: u32,
    pins: [u32; 3],
    frequency: u32,
    config: u32,
    transmit: u8,
    receive: VecDeque<u8>,
}

impl Spi {
    pub(super) fn new(loopback: bool) -> Self {
        Self {
            loopback,
            enabled: false,
            ready: false,
            interrupts: 0,
            raised: 0,
            pins: [u32::MAX; 3],
            frequency: 0x0400_0000,
            config: 0,
            transmit: 0,
            receive: VecDeque::new(),
        }
    }

    pub(super) fn reset(&mut self) {
        *self = Self::new(self.loopback);
    }

    pub(super) fn pending(&self) -> bool {
        self.enabled && self.ready && self.interrupts & READY != 0
    }

    fn raise_ready(&mut self) {
        self.ready = true;
        self.raised |= READY;
    }

    pub(super) fn read(&mut self, offset: u32) -> Option<u32> {
        Some(match offset {
            0x108 => u32::from(self.ready),
            0x304 | 0x308 => self.interrupts,
            0x500 => u32::from(self.enabled),
            0x508 | 0x50c | 0x510 => self.pins[((offset - 0x508) / 4) as usize],
            0x518 => {
                let value = self.receive.pop_front().unwrap_or(0);
                if !self.receive.is_empty() {
                    self.raise_ready();
                }
                u32::from(value)
            }
            0x51c => u32::from(self.transmit),
            0x524 => self.frequency,
            0x554 => self.config,
            _ => return None,
        })
    }

    pub(super) fn write(&mut self, offset: u32, value: u32) -> bool {
        match offset {
            0x108 => self.ready = value & 1 != 0,
            0x304 => self.interrupts |= value & READY,
            0x308 => self.interrupts &= !value,
            0x500 if matches!(value, 0 | 1) => self.enabled = value != 0,
            0x508 | 0x50c | 0x510 => {
                self.pins[((offset - 0x508) / 4) as usize] = value;
            }
            0x51c => {
                self.transmit = value as u8;
                if self.enabled && self.loopback && self.receive.len() < 2 {
                    self.receive.push_back(self.transmit);
                    if self.receive.len() == 1 {
                        self.raise_ready();
                    }
                }
            }
            0x524 => self.frequency = value,
            0x554 => self.config = value & 7,
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unattached_spi_updates_txd_without_completing_a_transfer() {
        let mut spi = Spi::new(false);
        assert!(spi.write(0x500, 1));
        assert!(spi.write(0x304, READY));
        assert!(spi.write(0x51c, 0x1a5));
        assert_eq!(spi.read(0x51c), Some(0xa5));
        assert_eq!(spi.read(0x108), Some(0));
        assert_eq!(spi.read(0x518), Some(0));
        assert!(!spi.pending());
        assert!(!spi.write(0x500, 7));
    }

    #[test]
    fn loopback_is_double_buffered_and_irq_tracks_enable_and_ready() {
        let mut spi = Spi::new(true);
        assert!(spi.write(0x51c, 0xff));
        assert_eq!(spi.read(0x108), Some(0));
        assert!(spi.write(0x500, 1));
        assert!(spi.write(0x51c, 0x12));
        assert!(spi.write(0x51c, 0x34));
        assert!(spi.write(0x51c, 0xff));
        assert_eq!(spi.read(0x108), Some(1));
        assert!(!spi.pending());
        assert!(spi.write(0x304, READY));
        assert!(spi.pending());
        assert!(spi.write(0x108, 0));
        assert_eq!(spi.read(0x518), Some(0x12));
        assert!(spi.pending());
        assert!(spi.write(0x108, 0));
        assert_eq!(spi.read(0x518), Some(0x34));
        assert_eq!(spi.read(0x518), Some(0));
        assert!(!spi.pending());
        spi.reset();
        assert_eq!(spi.read(0x500), Some(0));
        assert!(spi.write(0x500, 1));
        assert!(spi.write(0x51c, 0x56));
        assert_eq!(spi.read(0x518), Some(0x56));
    }
}
