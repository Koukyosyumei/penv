use std::collections::VecDeque;

use super::Adxl372;

pub(super) const READY: u32 = 1 << 2;

pub(crate) struct SpiMuxPort {
    pub gpio: u32,
    pub pin: u32,
    pub active_low: bool,
    pub responses: VecDeque<u8>,
}

pub(super) struct Spi {
    loopback: bool,
    adxl: Option<(u32, u32, Box<Adxl372>)>,
    selected: bool,
    mux: Vec<SpiMuxPort>,
    mux_selection: Option<usize>,
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
            adxl: None,
            selected: false,
            mux: Vec::new(),
            mux_selection: None,
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
        let mut adxl = self.adxl.take();
        if let Some((_, _, device)) = &mut adxl {
            device.reset();
        }
        let mut mux = std::mem::take(&mut self.mux);
        for port in &mut mux {
            port.responses.clear();
        }
        *self = Self::new(self.loopback);
        self.mux = mux;
        self.adxl = adxl;
    }

    pub(super) fn attach_adxl(&mut self, gpio: u32, pin: u32, device: Adxl372) {
        self.adxl = Some((gpio, pin, Box::new(device)));
    }

    pub(super) fn attach_mux(&mut self, ports: Vec<SpiMuxPort>) {
        self.mux = ports;
    }

    pub(super) fn adxl(&mut self, name: &str) -> Option<&mut Adxl372> {
        let (_, _, device) = self.adxl.as_mut()?;
        (device.name == name).then_some(device)
    }

    pub(super) fn sample_cs(&mut self, bus: &crate::bus::Bus) {
        let mut selected = self.mux.iter().enumerate().filter(|(_, port)| {
            let high = bus.nrf_gpio_driven_high(port.gpio) & (1 << port.pin) != 0;
            high != port.active_low
        });
        self.mux_selection = selected
            .next()
            .filter(|_| selected.next().is_none())
            .map(|(index, _)| index);
        if let Some((gpio, pin, device)) = &mut self.adxl {
            self.selected = bus.nrf_gpio_driven_high(*gpio) & (1 << *pin) == 0;
            if !self.selected {
                device.exchange(0, false);
            }
        }
    }

    pub(super) fn release_cs(&mut self, gpio: u32, pins: u32) {
        if let Some((base, pin, device)) = &mut self.adxl
            && *base == gpio
            && pins & (1 << *pin) != 0
        {
            device.exchange(0, false);
        }
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
                if self.enabled && self.receive.len() < 2 {
                    let response = if let Some((_, _, device)) = &mut self.adxl {
                        device.exchange(self.transmit, self.selected)
                    } else if !self.mux.is_empty() {
                        self.mux_selection.map_or(0xff, |index| {
                            self.mux[index].responses.pop_front().unwrap_or(0)
                        })
                    } else if self.loopback {
                        self.transmit
                    } else {
                        return true;
                    };
                    self.receive.push_back(response);
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
