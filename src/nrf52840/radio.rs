use crate::bus::{Bus, BusError};

const READY: u32 = 1;
const ADDRESS: u32 = 1 << 1;
const PAYLOAD: u32 = 1 << 2;
const END: u32 = 1 << 3;
const DISABLED: u32 = 1 << 4;
const DEVICE_MATCH: u32 = 1 << 5;
const DEVICE_MISS: u32 = 1 << 6;
const RSSI_END: u32 = 1 << 7;
const BIT_MATCH: u32 = 1 << 10;
const CRC_OK: u32 = 1 << 12;
const TX_READY: u32 = 1 << 21;
const RX_READY: u32 = 1 << 22;
const PHY_END: u32 = 1 << 27;
const EVENTS: u32 = READY
    | ADDRESS
    | PAYLOAD
    | END
    | DISABLED
    | DEVICE_MATCH
    | DEVICE_MISS
    | RSSI_END
    | BIT_MATCH
    | CRC_OK
    | TX_READY
    | RX_READY
    | PHY_END;
const SHORTS: u32 =
    (1 << 0) | (1 << 1) | (1 << 4) | (1 << 6) | (1 << 8) | (1 << 18) | (1 << 19) | (1 << 20);

#[derive(Clone, Copy)]
enum State {
    Disabled = 0,
    RxIdle = 2,
    Rx = 3,
    TxIdle = 10,
}

struct Packet {
    config: [u32; 4],
    bytes: Vec<u8>,
}

pub(super) struct Radio {
    loopback: bool,
    powered: bool,
    state: State,
    events: u32,
    interrupts: u32,
    pub(super) raised: u32,
    shorts: u32,
    pointer: u32,
    config: [u32; 4],
    tx_power: u32,
    errata: u32,
    mode_config: u32,
    link_config: [u32; 11],
    device_addresses: [u32; 17],
    bit_compare: u32,
    bit_counter: bool,
    crc_ok: bool,
    packet: Option<Packet>,
}

impl Radio {
    pub(super) fn new(loopback: bool) -> Self {
        Self {
            loopback,
            powered: true,
            state: State::Disabled,
            events: 0,
            interrupts: 0,
            raised: 0,
            shorts: 0,
            pointer: 0,
            config: [0; 4],
            tx_power: 0,
            errata: 0,
            mode_config: 0x200,
            link_config: [0; 11],
            device_addresses: [0; 17],
            bit_compare: 0,
            bit_counter: false,
            crc_ok: false,
            packet: None,
        }
    }

    pub(super) fn reset(&mut self) {
        *self = Self::new(self.loopback);
    }

    pub(super) fn pending(&self) -> bool {
        self.events & self.interrupts != 0
    }

    pub(super) fn event(offset: u32) -> Option<u32> {
        let bit = 1u32.checked_shl(offset.checked_sub(0x100)? / 4)?;
        (bit & EVENTS != 0).then_some(bit)
    }

    pub(super) fn task(offset: u32) -> bool {
        matches!(offset, 0 | 4 | 8 | 12 | 16 | 0x14 | 0x18 | 0x1c | 0x20)
    }

    fn raise(&mut self, mask: u32) {
        self.events |= mask;
        self.raised |= mask;
    }

    fn disable(&mut self) {
        self.state = State::Disabled;
        self.raise(DISABLED);
    }

    fn finish_packet(&mut self, bytes: usize) {
        self.raise(ADDRESS | PAYLOAD | END | PHY_END);
        if self.shorts & (1 << 4) != 0 {
            self.raise(RSSI_END);
        }
        if (self.bit_counter || self.shorts & (1 << 6) != 0)
            && self.bit_compare as usize <= bytes * 8
        {
            self.raise(BIT_MATCH);
        }
        self.bit_counter = false;
        if self.shorts & ((1 << 1) | (1 << 20)) != 0 {
            self.disable();
        }
    }

    fn packet_bytes<'a>(&self, bus: &'a Bus) -> Result<&'a [u8], BusError> {
        let [_, _, pcnf0, pcnf1] = self.config;
        let length_bits = pcnf0 & 15;
        let s0 = ((pcnf0 >> 8) & 1) as usize;
        let s1 = (pcnf0 >> 16) & 15;
        let header = s0
            + length_bits.div_ceil(8) as usize
            + if s1 != 0 {
                s1.div_ceil(8) as usize
            } else {
                ((pcnf0 >> 20) & 1) as usize
            };
        let data = if length_bits == 0 {
            0
        } else {
            let address = super::dma_address(self.pointer, header)?;
            let bytes = bus.memory_slice(address, header)?;
            let length = u32::from(bytes[s0])
                | if length_bits > 8 {
                    u32::from(bytes[s0 + 1]) << 8
                } else {
                    0
                };
            length & ((1 << length_bits) - 1)
        };
        let payload = (data + ((pcnf1 >> 8) & 255)).min(pcnf1 & 255) as usize;
        let size = header + payload;
        if size == 0 {
            return Ok(&[]);
        }
        let address = super::dma_address(self.pointer, size)?;
        bus.memory_slice(address, size)
    }

    fn start(&mut self, bus: &mut Bus) -> Result<(), BusError> {
        match self.state {
            State::TxIdle => {
                let bytes = self.packet_bytes(bus)?;
                if self.loopback {
                    self.packet = Some(Packet {
                        config: self.config,
                        bytes: bytes.to_vec(),
                    });
                }
                self.finish_packet(bytes.len());
            }
            State::RxIdle => {
                self.crc_ok = false;
                if let Some(packet) = &self.packet
                    && packet.config == self.config
                {
                    if !packet.bytes.is_empty() {
                        let address = super::dma_address(self.pointer, packet.bytes.len())?;
                        bus.memory(address, packet.bytes.len())?
                            .copy_from_slice(&packet.bytes);
                    }
                    let bytes = packet.bytes.len();
                    self.packet = None;
                    self.crc_ok = true;
                    self.raise(CRC_OK);
                    self.finish_packet(bytes);
                } else {
                    self.state = State::Rx;
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub(super) fn read(&self, offset: u32) -> Option<u32> {
        Some(match offset {
            offset if Self::task(offset) => 0,
            0x200 => self.shorts,
            0x304 | 0x308 => self.interrupts,
            0x400 => self.crc_ok.into(),
            0x504 => self.pointer,
            0x508 => self.config[0],
            0x50c => self.tx_power,
            0x510..=0x518 => self.config[((offset - 0x510) / 4) as usize + 1],
            0x548 => 10,
            0x550 => self.state as u32,
            0x51c..=0x53c => self.link_config[((offset - 0x51c) / 4) as usize],
            0x544 => self.link_config[9],
            0x554 => self.link_config[10],
            0x560 => self.bit_compare,
            0x600..=0x640 => self.device_addresses[((offset - 0x600) / 4) as usize],
            0x650 => self.mode_config,
            0x774 => self.errata,
            0xffc => self.powered.into(),
            _ => (self.events & Self::event(offset)? != 0).into(),
        })
    }

    pub(super) fn write(
        &mut self,
        offset: u32,
        value: u32,
        bus: &mut Bus,
    ) -> Result<bool, BusError> {
        let trigger = self.powered && value & 1 != 0;
        match offset {
            0 | 4 if trigger => {
                let tx = offset == 0;
                self.state = if tx { State::TxIdle } else { State::RxIdle };
                let (ready, shortcut) = if tx {
                    (TX_READY, 1 << 18)
                } else {
                    (RX_READY, 1 << 19)
                };
                self.raise(READY | ready);
                if self.shorts & (1 | shortcut) != 0 {
                    self.start(bus)?;
                }
            }
            8 if trigger => self.start(bus)?,
            12 if trigger => {
                if matches!(self.state, State::Rx) {
                    self.state = State::RxIdle;
                }
            }
            16 if trigger => self.disable(),
            0x14 if trigger => self.raise(RSSI_END),
            0x1c | 0x20 if trigger => self.bit_counter = offset == 0x1c,
            offset if Self::task(offset) => {}
            0x200 if value & !SHORTS == 0 => self.shorts = value,
            0x304 => self.interrupts |= value & EVENTS,
            0x308 => self.interrupts &= !value,
            0x504 => self.pointer = value,
            0x508 => self.config[0] = value & 0x17f,
            0x50c => self.tx_power = value & 255,
            // Zephyr applies the nRF52840 RADIO anomalies 102, 106 and 107 workaround.
            0x774 => self.errata = value,
            0x600..=0x640 => {
                self.device_addresses[((offset - 0x600) / 4) as usize] = if offset < 0x620 {
                    value
                } else {
                    value & 0xffff
                };
            }
            0x650 => self.mode_config = value & 0x301,
            0x51c..=0x53c => {
                const MASKS: [u32; 9] = [
                    u32::MAX,
                    u32::MAX,
                    u32::MAX,
                    u32::MAX,
                    7,
                    0xff,
                    0x303,
                    0x00ff_ffff,
                    0x00ff_ffff,
                ];
                let index = ((offset - 0x51c) / 4) as usize;
                self.link_config[index] = value & MASKS[index];
            }
            0x544 => self.link_config[9] = value & 0xff,
            0x554 => self.link_config[10] = value & 127,
            0x560 => self.bit_compare = value,
            0x510 => self.config[1] = value & 15,
            0x514 if value & !0x67df_010f == 0 => self.config[2] = value,
            0x518 if value & !0x0307_ffff == 0 => self.config[3] = value,
            0xffc => {
                if value & 1 == 0 {
                    self.reset();
                }
                self.powered = value & 1 != 0;
            }
            _ => {
                let Some(bit) = Self::event(offset) else {
                    return Ok(false);
                };
                super::set_event(&mut self.events, bit, value);
            }
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_loopback_does_not_deliver_a_packet() {
        let mut bus = Bus::default();
        let mut radio = Radio::new(false);
        assert!(radio.write(0, 1, &mut bus).unwrap());
        assert!(radio.write(8, 1, &mut bus).unwrap());
        assert_eq!(radio.read(0x10c), Some(1));
        assert!(radio.write(0x10c, 0, &mut bus).unwrap());
        assert!(radio.write(4, 1, &mut bus).unwrap());
        assert!(radio.write(8, 1, &mut bus).unwrap());
        assert_eq!(radio.read(0x10c), Some(0));
        assert_eq!(radio.read(0x400), Some(0));
    }

    #[test]
    fn failed_rx_dma_preserves_packet_and_power_cycle_clears_it() {
        let mut bus = Bus::default();
        let ram = 0x2000_0000;
        bus.map_ram(ram, 8).unwrap();
        bus.memory(ram, 4).unwrap().copy_from_slice(&[3, 1, 2, 3]);
        let mut radio = Radio::new(true);
        for (offset, value) in [(0x504, ram), (0x514, 8), (0x518, 8), (0, 1), (8, 1)] {
            assert!(radio.write(offset, value, &mut bus).unwrap());
        }
        assert!(radio.write(0x10c, 0, &mut bus).unwrap());
        assert!(radio.write(4, 1, &mut bus).unwrap());
        assert!(radio.write(0x504, ram + 5, &mut bus).unwrap());
        assert!(radio.write(8, 1, &mut bus).is_err());
        assert_eq!(radio.read(0x10c), Some(0));
        assert_eq!(bus.memory_slice(ram + 4, 4).unwrap(), &[0; 4]);
        assert!(radio.write(0x504, ram + 4, &mut bus).unwrap());
        assert!(radio.write(8, 1, &mut bus).unwrap());
        assert_eq!(bus.memory_slice(ram + 4, 4).unwrap(), &[3, 1, 2, 3]);

        assert!(radio.write(0, 1, &mut bus).unwrap());
        assert!(radio.write(8, 1, &mut bus).unwrap());
        assert!(radio.packet.is_some());
        assert!(radio.write(0xffc, 0, &mut bus).unwrap());
        assert!(radio.packet.is_none());
        assert_eq!(radio.read(0x10c), Some(0));
        assert!(radio.write(0, 1, &mut bus).unwrap());
        assert_eq!(radio.read(0x550), Some(0));
    }
}
