use crate::ethernet::{Mapping, Region, append_fcs};

use std::collections::VecDeque;

pub const SLOT_SIZE: usize = 0x800;

pub struct LiteXEthernet {
    pub name: String,
    pub mappings: Vec<Mapping>,
    pub irq_line: Option<u32>,
    pub phy: Option<Phy>,
    csr32: bool,
    buffer: Vec<u8>,
    rx_lengths: Vec<usize>,
    rx_ready: VecDeque<usize>,
    tx_lengths: Vec<usize>,
    tx_slot: usize,
    rx_enabled: bool,
    tx_enabled: bool,
    tx_pending: bool,
    outgoing: Option<Vec<u8>>,
}

impl LiteXEthernet {
    pub fn new(name: String, csr32: bool, rx_slots: usize, tx_slots: usize) -> Self {
        Self {
            name,
            mappings: Vec::new(),
            irq_line: None,
            phy: None,
            csr32,
            buffer: vec![0; (rx_slots + tx_slots) * SLOT_SIZE],
            rx_lengths: vec![0; rx_slots],
            rx_ready: VecDeque::with_capacity(rx_slots),
            tx_lengths: vec![0; tx_slots],
            tx_slot: 0,
            rx_enabled: false,
            tx_enabled: false,
            tx_pending: false,
            outgoing: None,
        }
    }

    pub fn interrupt(&self) -> u32 {
        if (self.rx_enabled && !self.rx_ready.is_empty()) || (self.tx_enabled && self.tx_pending) {
            self.irq_line.map_or(0, |line| 1 << line)
        } else {
            0
        }
    }

    pub fn take_frame(&mut self) -> Option<Vec<u8>> {
        self.outgoing.take()
    }

    pub fn receive(&mut self, frame: &[u8]) {
        let Some(slot) = self.rx_lengths.iter().position(|&length| length == 0) else {
            return;
        };
        let length = frame.len();
        if length > SLOT_SIZE {
            return;
        }
        let start = slot * SLOT_SIZE;
        self.buffer[start..start + length].copy_from_slice(frame);
        self.rx_lengths[slot] = length;
        self.rx_ready.push_back(slot);
    }

    fn transmit(&mut self) {
        let length = self.tx_lengths[self.tx_slot];
        if (14..=SLOT_SIZE - 4).contains(&length) {
            let start = (self.rx_lengths.len() + self.tx_slot) * SLOT_SIZE;
            let mut frame = self.buffer[start..start + length].to_vec();
            frame.resize(length.max(60), 0);
            append_fcs(&mut frame);
            self.outgoing = Some(frame);
        }
        self.tx_pending = true;
    }

    fn locate(&self, address: u32, width: usize) -> Option<(Region, usize)> {
        self.mappings.iter().find_map(|mapping| {
            let offset = address.checked_sub(mapping.base)?;
            (u64::from(offset) + width as u64 <= u64::from(mapping.size))
                .then_some((mapping.region, offset as usize))
        })
    }

    pub fn read(&self, address: u32, width: usize) -> Option<u32> {
        let (region, offset) = self.locate(address, width)?;
        Some(match region {
            Region::Buffer => {
                let mut bytes = [0; 4];
                if let Some(data) = self.buffer.get(offset..offset + width) {
                    bytes[..width].copy_from_slice(data);
                }
                u32::from_le_bytes(bytes)
            }
            Region::Registers => self.read_csr(offset & !3) >> (8 * (offset & 3)),
            Region::Phy => u32::from(offset == 8 && self.phy.as_ref().is_none_or(|phy| phy.input)),
            Region::Dma | Region::Mtl => return None,
        })
    }

    pub fn write(&mut self, address: u32, width: usize, value: u32) -> bool {
        let Some((region, offset)) = self.locate(address, width) else {
            return false;
        };
        match region {
            Region::Buffer => {
                if let Some(data) = self.buffer.get_mut(offset..offset + width) {
                    data.copy_from_slice(&value.to_le_bytes()[..width]);
                }
            }
            Region::Registers if offset & 3 == 0 => self.write_csr(offset, value),
            Region::Phy if offset == 4 => {
                if let Some(phy) = &mut self.phy {
                    phy.write(value);
                }
            }
            _ => {}
        }
        true
    }

    fn read_csr(&self, offset: usize) -> u32 {
        let register = (self.csr32, offset);
        let rx_slot = self.rx_ready.front().copied().unwrap_or(0);
        let rx_length = self.rx_lengths[rx_slot] as u32;
        let tx_length = self.tx_lengths[self.tx_slot] as u32;
        match register {
            (_, 0) => rx_slot as u32,
            (true, 4) => rx_length,
            (false, 4 | 8 | 12 | 16) => (rx_length >> ((16 - offset) * 2)) & 0xff,
            (true, 0x0c | 0x10) | (false, 0x24 | 0x28) => u32::from(!self.rx_ready.is_empty()),
            (true, 0x14) | (false, 0x2c) => u32::from(self.rx_enabled),
            (true, 0x1c) | (false, 0x34) => 1,
            (true, 0x24) | (false, 0x3c) => self.tx_slot as u32,
            (true, 0x28) => tx_length,
            (false, 0x40) => tx_length >> 8,
            (false, 0x44) => tx_length & 0xff,
            (true, 0x2c | 0x30) | (false, 0x48 | 0x4c) => u32::from(self.tx_pending),
            (true, 0x34) | (false, 0x50) => u32::from(self.tx_enabled),
            _ => 0,
        }
    }

    fn write_csr(&mut self, offset: usize, value: u32) {
        match (self.csr32, offset) {
            (true, 0x10) | (false, 0x28) if value & 1 != 0 => {
                if let Some(slot) = self.rx_ready.pop_front() {
                    self.rx_lengths[slot] = 0;
                }
            }
            (true, 0x14) | (false, 0x2c) => self.rx_enabled = value & 1 != 0,
            (true, 0x18) | (false, 0x30) if value & 1 != 0 => self.transmit(),
            (true, 0x24) | (false, 0x3c) => {
                let slot = value as usize;
                self.tx_slot = if slot < self.tx_lengths.len() {
                    slot
                } else {
                    0
                };
            }
            (true, 0x28) => self.tx_lengths[self.tx_slot] = value as usize & 0xffff,
            (false, 0x40) => {
                let length = &mut self.tx_lengths[self.tx_slot];
                *length = (*length & 0xff) | ((value as usize & 0xff) << 8);
            }
            (false, 0x44) => {
                let length = &mut self.tx_lengths[self.tx_slot];
                *length = (*length & 0xff00) | (value as usize & 0xff);
            }
            (true, 0x30) | (false, 0x4c) if value & 1 != 0 => self.tx_pending = false,
            (true, 0x34) | (false, 0x50) => self.tx_enabled = value & 1 != 0,
            _ => {}
        }
    }
}

pub struct Phy {
    pub address: u8,
    pub registers: [u16; 32],
    clock: bool,
    input: bool,
    bits: u8,
    command: u32,
    reply: Option<(u16, u8)>,
}

impl Phy {
    pub fn new(address: u8) -> Self {
        let mut registers = [0; 32];
        registers[0] = 0x3100;
        registers[1] = 0x782d;
        registers[4] = 0x01e1;
        registers[5] = 0x41e1;
        Self {
            address,
            registers,
            clock: false,
            input: true,
            bits: 0,
            command: 0,
            reply: None,
        }
    }

    fn write(&mut self, value: u32) {
        let clock = value & 1 != 0;
        let rising = clock && !self.clock;
        self.clock = clock;
        if !rising {
            return;
        }
        if let Some((data, bit)) = self.reply {
            self.input = bit == 17 || (bit < 16 && data & (1 << bit) != 0);
            self.reply = bit.checked_sub(1).map(|next| (data, next));
            return;
        }
        let data = value & 4 != 0;
        // A Clause 22 frame starts with 01 after the optional preamble.
        if self.bits == 0 && data {
            self.input = true;
            return;
        }
        self.command = (self.command << 1) | u32::from(data);
        self.bits += 1;
        if self.bits == 14 && self.command >> 10 == 0b0110 {
            let address = ((self.command >> 5) & 31) as u8;
            let register = (self.command & 31) as usize;
            let data = if address == self.address {
                self.registers[register]
            } else {
                u16::MAX
            };
            self.reply = Some((data, 17));
            self.input = true;
            self.bits = 0;
            self.command = 0;
        } else if self.bits == 32 {
            let address = ((self.command >> 23) & 31) as u8;
            let register = ((self.command >> 18) & 31) as usize;
            if self.command >> 28 == 0b0101 && address == self.address {
                self.registers[register] = self.command as u16;
            }
            self.bits = 0;
            self.command = 0;
        }
    }
}
