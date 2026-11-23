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
const RATE_BOOST: u32 = 1 << 20;
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
    | RATE_BOOST
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
    Tx = 11,
}

struct Packet {
    config: [u32; 4],
    bytes: Vec<u8>,
}

pub(crate) struct AirFrame {
    channel: u32,
    mode: u32,
    access_address: (u32, u8, u32),
    bytes: Vec<u8>,
    remaining: u64,
}

struct AirTx {
    until_address: u64,
    remaining: u64,
    length: usize,
    frame: Option<AirFrame>,
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
    device_match_index: u32,
    bit_compare: u32,
    bit_counter: bool,
    crc_ok: bool,
    packet: Option<Packet>,
    medium: bool,
    air_tx: Option<AirTx>,
    air_rx: Option<(u64, usize)>,
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
            device_match_index: 0,
            bit_compare: 0,
            bit_counter: false,
            crc_ok: false,
            packet: None,
            medium: false,
            air_tx: None,
            air_rx: None,
        }
    }

    pub(super) fn reset(&mut self) {
        let medium = self.medium;
        *self = Self::new(self.loopback);
        self.medium = medium;
    }

    pub(super) fn attach_medium(&mut self) {
        self.medium = true;
    }

    fn access_address(&self, index: u32) -> (u32, u8, u32) {
        let length = (self.config[3] >> 16) & 7;
        let base = self.link_config[usize::from(index != 0)];
        let prefix = (self.link_config[(2 + index / 4) as usize] >> ((index % 4) * 8)) as u8;
        let mask = if length == 0 {
            0
        } else {
            u32::MAX << (32 - length.min(4) * 8)
        };
        (base & mask, prefix, length)
    }

    pub(super) fn tick_air(&mut self) -> Option<AirFrame> {
        if let Some((remaining, length)) = &mut self.air_rx {
            *remaining -= 1;
            if *remaining == 0 {
                let length = *length;
                self.air_rx = None;
                self.state = State::RxIdle;
                self.crc_ok = true;
                self.raise(CRC_OK);
                self.finish_packet(length, false);
            }
        }
        let tx = self.air_tx.as_mut()?;
        tx.remaining -= 1;
        if tx.remaining == 0 {
            let length = tx.length;
            self.air_tx = None;
            self.state = State::TxIdle;
            self.finish_packet(length, false);
        } else if tx.frame.is_some() {
            tx.until_address -= 1;
            if tx.until_address == 0 {
                let mut frame = tx.frame.take().unwrap();
                frame.remaining = tx.remaining;
                self.begin_packet();
                return Some(frame);
            }
        }
        None
    }

    pub(super) fn receive_air(&mut self, frame: AirFrame, bus: &mut Bus) -> Result<(), BusError> {
        if !matches!(self.state, State::Rx)
            || self.air_rx.is_some()
            || self.config[0] != frame.channel
            || self.config[1] != frame.mode
            || !(0..8).any(|index| {
                self.link_config[5] & (1 << index) != 0
                    && self.access_address(index) == frame.access_address
            })
        {
            return Ok(());
        }
        let header = self.header_length();
        let length = frame
            .bytes
            .len()
            .min(header + (self.config[3] & 255) as usize);
        if length != 0 {
            let address = super::dma_address(self.pointer, length)?;
            bus.memory(address, length)?
                .copy_from_slice(&frame.bytes[..length]);
        }
        self.match_device(&frame.bytes, header);
        self.air_rx = Some((frame.remaining, length));
        self.begin_packet();
        Ok(())
    }

    fn match_device(&mut self, bytes: &[u8], header: usize) {
        let enabled = self.device_addresses[16] & 0xff;
        if enabled == 0 {
            return;
        }
        let Some(address) = bytes.get(header..header + 6) else {
            return;
        };
        let base = u32::from_le_bytes(address[..4].try_into().unwrap());
        let prefix = u32::from(u16::from_le_bytes(address[4..].try_into().unwrap()));
        let random = bytes[0] >> 6 & 1;
        for index in 0..8 {
            if enabled & (1 << index) != 0
                && (self.device_addresses[16] >> (8 + index)) & 1 == u32::from(random)
                && self.device_addresses[index] == base
                && self.device_addresses[index + 8] == prefix
            {
                self.device_match_index = index as u32;
                self.raise(DEVICE_MATCH);
                return;
            }
        }
        self.raise(DEVICE_MISS);
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
        self.air_tx = None;
        self.air_rx = None;
        self.raise(DISABLED);
    }

    fn begin_packet(&mut self) {
        self.raise(ADDRESS);
        if self.shorts & (1 << 4) != 0 {
            self.raise(RSSI_END);
        }
    }

    fn finish_packet(&mut self, bytes: usize, synchronous: bool) {
        if synchronous {
            self.begin_packet();
        }
        self.raise(PAYLOAD | END | PHY_END);
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

    fn header_length(&self) -> usize {
        let pcnf0 = self.config[2];
        let length_bits = pcnf0 & 15;
        let s0 = ((pcnf0 >> 8) & 1) as usize;
        let s1 = (pcnf0 >> 16) & 15;
        s0 + length_bits.div_ceil(8) as usize
            + if s1 != 0 {
                s1.div_ceil(8) as usize
            } else {
                ((pcnf0 >> 20) & 1) as usize
            }
    }

    fn packet_bytes<'a>(&self, bus: &'a Bus) -> Result<&'a [u8], BusError> {
        let [_, _, pcnf0, pcnf1] = self.config;
        let length_bits = pcnf0 & 15;
        let s0 = ((pcnf0 >> 8) & 1) as usize;
        let header = self.header_length();
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
                if self.medium {
                    let steps_per_us = crate::bus::CORTEX_M_STEP_HZ / 1_000_000;
                    let rate = if self.config[1] == 4 { 2 } else { 1 };
                    let until_address = steps_per_us * (40 + 40 / rate);
                    let after_address = steps_per_us * ((bytes.len() + 3) as u64 * 8) / rate;
                    self.air_tx = Some(AirTx {
                        until_address,
                        remaining: until_address + after_address,
                        length: bytes.len(),
                        frame: Some(AirFrame {
                            channel: self.config[0],
                            mode: self.config[1],
                            access_address: self.access_address(self.link_config[4]),
                            bytes: bytes.to_vec(),
                            remaining: 0,
                        }),
                    });
                    self.state = State::Tx;
                    return Ok(());
                }
                if self.loopback {
                    self.packet = Some(Packet {
                        config: self.config,
                        bytes: bytes.to_vec(),
                    });
                }
                self.finish_packet(bytes.len(), true);
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
                    self.finish_packet(bytes, true);
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
            0x410 => self.device_match_index,
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
                    self.air_rx = None;
                    self.state = State::RxIdle;
                } else if matches!(self.state, State::Tx) {
                    self.air_tx = None;
                    self.state = State::TxIdle;
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
            0x514 => self.config[2] = value & 0x67df_010f,
            0x518 => self.config[3] = value & 0x0307_ffff,
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
    fn medium_transfers_sram_between_radios_and_separates_address_from_end() {
        let ram = 0x2000_0000;
        let mut tx_bus = Bus::default();
        let mut rx_bus = Bus::default();
        tx_bus.map_ram(ram, 8).unwrap();
        rx_bus.map_ram(ram, 8).unwrap();
        tx_bus
            .memory(ram, 4)
            .unwrap()
            .copy_from_slice(&[3, 1, 2, 3]);
        let mut tx = Radio::new(false);
        let mut rx = Radio::new(false);
        for (device, bus) in [(&mut tx, &mut tx_bus), (&mut rx, &mut rx_bus)] {
            device.attach_medium();
            for (offset, value) in [(0x504, ram), (0x514, 8), (0x518, 8), (0x530, 1), (0x200, 3)] {
                assert!(device.write(offset, value, bus).unwrap());
            }
        }
        assert!(rx.write(4, 1, &mut rx_bus).unwrap());
        assert!(tx.write(0, 1, &mut tx_bus).unwrap());
        let frame = (0..10_000)
            .find_map(|_| tx.tick_air())
            .expect("TX must reach ADDRESS");
        assert_eq!(tx.read(0x104), Some(1));
        assert_eq!(tx.read(0x10c), Some(0));
        let remaining = frame.remaining;
        rx.receive_air(frame, &mut rx_bus).unwrap();
        assert_eq!(rx.read(0x104), Some(1));
        assert_eq!(rx.read(0x10c), Some(0));
        for _ in 0..remaining {
            assert!(tx.tick_air().is_none());
            assert!(rx.tick_air().is_none());
        }
        assert_eq!(rx_bus.memory_slice(ram, 4).unwrap(), &[3, 1, 2, 3]);
        assert_eq!(rx.read(0x10c), Some(1));
        assert_eq!(rx.read(0x400), Some(1));
        assert_eq!(tx.read(0x10c), Some(1));
    }

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
