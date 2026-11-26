use crate::bus::{Bus, BusError};

#[derive(Default)]
pub(super) struct Aar {
    enabled: bool,
    count: u32,
    keys: u32,
    packet: u32,
    scratch: u32,
    status: u32,
    events: u32,
    interrupts: u32,
    pub(super) raised: u32,
}

impl Aar {
    pub(super) fn pending(&self) -> bool {
        self.events & self.interrupts != 0
    }

    fn resolve(&self, bus: &Bus) -> Result<Option<u32>, BusError> {
        if self.count == 0 {
            return Ok(None);
        }
        let address = super::dma_address(self.packet, 9)?;
        let packet = bus.memory_slice(address, 9)?;
        let address = &packet[3..];
        if address[5] & 0xc0 != 0x40 {
            return Ok(None);
        }

        let length = self.count as usize * 16;
        let keys = bus.memory_slice(super::dma_address(self.keys, length)?, length)?;
        let mut input = [0; 16];
        for (dest, source) in input[13..].iter_mut().zip(address[3..].iter().rev()) {
            *dest = *source;
        }
        for (index, key) in keys.chunks_exact(16).enumerate() {
            let mut key: [u8; 16] = key.try_into().unwrap();
            key.reverse();
            let result = super::ecb::aes128(key, input);
            if result[13..].iter().rev().eq(&address[..3]) {
                return Ok(Some(index as u32));
            }
        }
        Ok(None)
    }

    pub(super) fn read(&self, offset: u32) -> Option<u32> {
        Some(match offset {
            0 | 8 => 0,
            0x100 | 0x104 | 0x108 => (self.events >> ((offset - 0x100) / 4)) & 1,
            0x304 | 0x308 => self.interrupts,
            0x400 => self.status,
            0x500 => self.enabled.into(),
            0x504 => self.count,
            0x508 => self.keys,
            0x510 => self.packet,
            0x514 => self.scratch,
            _ => return None,
        })
    }

    pub(super) fn write(&mut self, offset: u32, value: u32, bus: &Bus) -> Result<bool, BusError> {
        match offset {
            0 if self.enabled && value & 1 != 0 => {
                let found = self.resolve(bus)?;
                self.status = found.unwrap_or(0);
                let events = 1 | if found.is_some() { 2 } else { 4 };
                self.events |= events;
                self.raised |= events;
            }
            0 | 8 => {}
            0x100 | 0x104 | 0x108 => {
                super::set_event(&mut self.events, 1 << ((offset - 0x100) / 4), value);
            }
            0x304 => self.interrupts |= value & 7,
            0x308 => self.interrupts &= !value,
            0x500 if value <= 1 => self.enabled = value == 1,
            0x504 if value <= 16 => self.count = value,
            0x508 => self.keys = value,
            0x510 => self.packet = value,
            0x514 => self.scratch = value,
            _ => return Ok(false),
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bluetooth_ah_vector_resolves_second_key_and_latches_irq() {
        let mut bus = Bus::default();
        let ram = 0x2000_0000;
        bus.map_ram(ram, 64).unwrap();
        // Packet contains three header bytes followed by hash || prand, least significant byte first.
        bus.memory(ram, 9)
            .unwrap()
            .copy_from_slice(&[0, 0, 0, 0xaa, 0xfb, 0x0d, 0x94, 0x81, 0x70]);
        let mut irk = [
            0xec, 0x02, 0x34, 0xa3, 0x57, 0xc8, 0xad, 0x05, 0x34, 0x10, 0x10, 0xa6, 0x0a, 0x39,
            0x7d, 0x9b,
        ];
        irk.reverse();
        bus.memory(ram + 32, 16).unwrap().copy_from_slice(&irk);
        let mut aar = Aar::default();
        for (offset, value) in [
            (0x500, 1),
            (0x504, 2),
            (0x508, ram + 16),
            (0x510, ram),
            (0x304, 2),
            (0, 1),
        ] {
            assert!(aar.write(offset, value, &bus).unwrap());
        }
        assert_eq!(aar.read(0x400), Some(1));
        assert_eq!(aar.read(0x100), Some(1));
        assert_eq!(aar.read(0x104), Some(1));
        assert!(aar.pending());
        assert!(aar.write(0x104, 0, &bus).unwrap());
        assert!(!aar.pending());
        bus.memory(ram + 3, 1).unwrap()[0] ^= 1;
        assert!(aar.write(0, 1, &bus).unwrap());
        assert_eq!(aar.read(0x108), Some(1));
    }
}
