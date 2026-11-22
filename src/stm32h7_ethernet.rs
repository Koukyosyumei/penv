use std::collections::VecDeque;

use crate::{
    bus::EthernetDma,
    ethernet::{Mapping, Region, append_fcs, has_valid_fcs, insert_ipv4_checksums},
};

pub const MAC_SIZE: u32 = 0xc00;
const OWN: u32 = 1 << 31;
const FIRST: u32 = 1 << 29;
const LAST: u32 = 1 << 28;
const CONTEXT: u32 = 1 << 30;
const TX_COMPLETE: u32 = 1;
const TX_UNAVAILABLE: u32 = 1 << 2;
const RX_COMPLETE: u32 = 1 << 6;
const RX_UNAVAILABLE: u32 = 1 << 7;
const BUS_ERROR: u32 = 1 << 12;
const ABNORMAL: u32 = 1 << 14;
const NORMAL: u32 = 1 << 15;
const RING_BUDGET: usize = 1024;
const MAX_PACKET: usize = 2048;
const FIFO_DEPTH: usize = 32;

#[derive(Default)]
struct Ring {
    base: u32,
    current: u32,
    tail: u32,
    length: u32,
    buffer: u32,
}

impl Ring {
    fn set_base(&mut self, address: u32) {
        self.base = address & !3;
        self.current = self.base;
    }

    fn advance(&mut self, stride: u32) {
        let offset = self.current.wrapping_sub(self.base).wrapping_add(stride);
        self.current = self
            .base
            .wrapping_add(offset % ((self.length + 1) * stride));
    }
}

struct Registers {
    mac: [u32; (MAC_SIZE / 4) as usize],
    mtl: [u32; 128],
    dma: [u32; 128],
    tx: Ring,
    rx: Ring,
    status: u32,
}

impl Default for Registers {
    fn default() -> Self {
        let mut registers = Self {
            mac: [0; (MAC_SIZE / 4) as usize],
            mtl: [0; 128],
            dma: [0; 128],
            tx: Ring::default(),
            rx: Ring::default(),
            status: 0,
        };
        registers.mac[0x300 / 4] = 0x8000_ffff;
        registers.mac[0x304 / 4] = u32::MAX;
        registers.mtl[0x100 / 4] = 0x70008;
        registers.mtl[0x130 / 4] = 0x01f0_0000;
        registers
    }
}

#[derive(Default)]
struct PendingTx {
    data: Vec<u8>,
    flags: Option<u32>,
}

struct Received {
    bytes: Vec<u8>,
    crc_error: bool,
}

pub struct Stm32h7Ethernet {
    pub name: String,
    pub mappings: Vec<Mapping>,
    pub phys: Vec<(u8, [u16; 32])>,
    irq: Option<u8>,
    registers: Box<Registers>,
    pending: PendingTx,
    outgoing: VecDeque<Vec<u8>>,
    incoming: VecDeque<Received>,
    rx_offset: usize,
}

impl Stm32h7Ethernet {
    pub fn new(name: String, mappings: Vec<Mapping>, irq: Option<u8>) -> Self {
        Self {
            name,
            mappings,
            phys: Vec::new(),
            irq,
            registers: Box::default(),
            pending: PendingTx::default(),
            outgoing: VecDeque::new(),
            incoming: VecDeque::new(),
            rx_offset: 0,
        }
    }

    fn offset(&self, address: u32, width: usize) -> Option<(Region, usize)> {
        if width != 4 || address & 3 != 0 {
            return None;
        }
        self.mappings.iter().find_map(|mapping| {
            let offset = address.checked_sub(mapping.base)?;
            (offset < mapping.size).then_some((mapping.region, offset as usize))
        })
    }

    pub fn interrupt(&self) -> Option<u8> {
        self.irq.filter(|_| self.irq_pending())
    }

    fn irq_pending(&self) -> bool {
        self.registers.status & self.registers.dma[0x134 / 4] & (NORMAL | ABNORMAL) != 0
    }

    pub fn take_frame(&mut self) -> Option<Vec<u8>> {
        self.outgoing.pop_front()
    }

    pub fn read(&self, address: u32, width: usize) -> Option<u32> {
        let (region, offset) = self.offset(address, width)?;
        let r = &self.registers;
        Some(match (region, offset) {
            (Region::Registers, 0x110) => 0x3142,
            (Region::Registers, _) => bank_read(&r.mac, offset),
            (Region::Mtl, _) => bank_read(&r.mtl, offset),
            (Region::Dma, 0x008) => u32::from(self.irq_pending()),
            (Region::Dma, 0x114) => r.tx.base,
            (Region::Dma, 0x11c) => r.rx.base,
            (Region::Dma, 0x120) => r.tx.tail,
            (Region::Dma, 0x128) => r.rx.tail,
            (Region::Dma, 0x12c) => r.tx.length,
            (Region::Dma, 0x144) => r.tx.current,
            (Region::Dma, 0x14c) => r.rx.current,
            (Region::Dma, 0x154) => r.tx.buffer,
            (Region::Dma, 0x15c) => r.rx.buffer,
            (Region::Dma, 0x160) => r.status,
            (Region::Dma, _) => bank_read(&r.dma, offset),
            _ => 0,
        })
    }

    pub fn write(
        &mut self,
        address: u32,
        width: usize,
        value: u32,
        dma: &mut EthernetDma<'_>,
    ) -> bool {
        let Some((region, offset)) = self.offset(address, width) else {
            return false;
        };
        let r = &mut self.registers;
        match (region, offset) {
            (Region::Registers, 0x000) => {
                r.mac[0] = value;
                self.transmit(dma);
                self.move_received(dma);
            }
            (Region::Registers, 0x110) => {}
            (Region::Registers, 0x200) => self.mdio(value),
            (Region::Registers, 0x300) => r.mac[offset / 4] = 0x8000_0000 | (value & 0xffff),
            (Region::Registers, _) => bank_write(&mut r.mac, offset, value),
            (Region::Mtl, _) => bank_write(&mut r.mtl, offset, value),
            (Region::Dma, 0x000) if value & 1 != 0 => {
                **r = Registers::default();
                self.pending = PendingTx::default();
                self.outgoing.clear();
                self.incoming.clear();
                self.rx_offset = 0;
            }
            (Region::Dma, 0x104) => {
                r.dma[offset / 4] = value;
                self.transmit(dma);
            }
            (Region::Dma, 0x108) => {
                r.dma[offset / 4] = value;
                self.move_received(dma);
            }
            (Region::Dma, 0x114) => r.tx.set_base(value),
            (Region::Dma, 0x11c) => {
                r.rx.set_base(value);
                self.rx_offset = 0;
            }
            (Region::Dma, 0x120) => {
                r.tx.tail = value & !3;
                self.transmit(dma);
            }
            (Region::Dma, 0x128) => {
                r.rx.tail = value & !3;
                self.move_received(dma);
            }
            (Region::Dma, 0x12c) => r.tx.length = value & 0x3ff,
            (Region::Dma, 0x130) => {
                r.rx.length = value & 0x3ff;
                r.dma[offset / 4] = value & 0x00ff_03ff;
            }
            (Region::Dma, 0x160) => r.status &= !(value & 0xffff),
            (Region::Dma, 0x008 | 0x144 | 0x14c | 0x154 | 0x15c) => {}
            (Region::Dma, _) => bank_write(&mut r.dma, offset, value),
            _ => {}
        }
        self.refresh_interrupt();
        true
    }

    fn mdio(&mut self, value: u32) {
        let r = &mut self.registers;
        r.mac[0x200 / 4] = value & !1;
        if value & 3 != 1 {
            return;
        }
        let address = ((value >> 21) & 31) as u8;
        let index = ((value >> 16) & 31) as usize;
        let phy = self.phys.iter_mut().find(|(id, _)| *id == address);
        match (value >> 2) & 3 {
            1 => {
                if let Some((_, words)) = phy {
                    let data = r.mac[0x204 / 4] as u16;
                    words[index] = if index == 0 { data & 0x7dff } else { data };
                }
            }
            3 => r.mac[0x204 / 4] = u32::from(phy.map_or(u16::MAX, |(_, words)| words[index])),
            _ => {}
        }
    }

    fn stride(&self) -> u32 {
        16 + 4 * ((self.registers.dma[0x100 / 4] >> 18) & 7)
    }

    fn refresh_interrupt(&mut self) {
        let r = &mut self.registers;
        let enabled = r.status & r.dma[0x134 / 4];
        if enabled & (TX_COMPLETE | TX_UNAVAILABLE | RX_COMPLETE) != 0 {
            r.status |= NORMAL;
        }
        if enabled & (RX_UNAVAILABLE | BUS_ERROR) != 0 {
            r.status |= ABNORMAL;
        }
    }

    fn transmit(&mut self, dma: &mut EthernetDma<'_>) {
        let stride = self.stride();
        let r = &mut self.registers;
        if r.mac[0] & 2 == 0 || r.dma[0x104 / 4] & 1 == 0 {
            return;
        }
        for _ in 0..RING_BUDGET {
            let address = r.tx.current;
            if address == r.tx.tail {
                break;
            }
            let Some([first, second, control, flags]) = dma.read_descriptor(address) else {
                r.status |= BUS_ERROR;
                break;
            };
            if flags & OWN == 0 {
                r.status |= TX_UNAVAILABLE;
                break;
            }
            let mut failed = false;
            if flags & CONTEXT == 0 {
                if flags & FIRST != 0 {
                    self.pending.data.clear();
                    self.pending.flags = Some(flags);
                }
                for (buffer, length) in [
                    (first, control & 0x3fff),
                    (second, (control >> 16) & 0x3fff),
                ] {
                    if length == 0 || self.pending.flags.is_none() {
                        continue;
                    }
                    r.tx.buffer = buffer;
                    let length = length as usize;
                    if self.pending.data.len() + length <= MAX_PACKET
                        && let Some(data) = dma.read(buffer, length)
                    {
                        self.pending.data.extend_from_slice(data);
                    } else {
                        self.pending.flags = None;
                        failed = true;
                    }
                }
            }
            let writeback = (flags & (FIRST | LAST | CONTEXT)) | (u32::from(failed) << 15);
            if !dma.write_descriptor(address, [first, second, control, writeback]) {
                r.status |= BUS_ERROR;
                break;
            }
            r.tx.advance(stride);
            if flags & (LAST | CONTEXT) == LAST {
                if let Some(first_flags) = self.pending.flags.take()
                    && self.pending.data.len() >= 14
                    && self.outgoing.len() < FIFO_DEPTH
                {
                    let mut frame = std::mem::take(&mut self.pending.data);
                    insert_ipv4_checksums(&mut frame, (first_flags >> 16) & 3);
                    match (first_flags >> 26) & 3 {
                        0 => {
                            frame.resize(frame.len().max(60), 0);
                            append_fcs(&mut frame);
                        }
                        1 => append_fcs(&mut frame),
                        3 if frame.len() >= 18 => {
                            frame.truncate(frame.len() - 4);
                            append_fcs(&mut frame);
                        }
                        _ => {}
                    }
                    self.outgoing.push_back(frame);
                }
                if control & OWN != 0 {
                    r.status |= TX_COMPLETE;
                }
            }
        }
    }

    pub fn receive(&mut self, frame: &[u8], dma: &mut EthernetDma<'_>) {
        if self.registers.mac[0] & 1 == 0 || self.incoming.len() == FIFO_DEPTH {
            return;
        }
        let mut length = frame.len();
        if self.registers.mac[0] & (1 << 21) != 0 && length >= 18 {
            length -= 4;
        }
        self.incoming.push_back(Received {
            bytes: frame[..length].to_vec(),
            crc_error: self.registers.mac[1] & (1 << 16) == 0 && !has_valid_fcs(frame),
        });
        self.move_received(dma);
        self.refresh_interrupt();
    }

    fn move_received(&mut self, dma: &mut EthernetDma<'_>) {
        let stride = self.stride();
        let r = &mut self.registers;
        let capacity = ((r.dma[0x108 / 4] >> 1) & 0x3ffc) as usize;
        let alternate = ((r.dma[0x130 / 4] >> 16) & 0xff) as usize;
        if r.mac[0] & 1 == 0 || r.dma[0x108 / 4] & 1 == 0 || capacity == 0 {
            return;
        }
        for _ in 0..RING_BUDGET {
            let Some(frame) = self.incoming.front() else {
                break;
            };
            let address = r.rx.current;
            if address == r.rx.tail {
                r.status |= RX_UNAVAILABLE;
                break;
            }
            let Some([first, _, second, flags]) = dma.read_descriptor(address) else {
                r.status |= BUS_ERROR;
                break;
            };
            if flags & OWN == 0 {
                r.status |= RX_UNAVAILABLE;
                break;
            }
            let start = self.rx_offset;
            let (buffer, capacity) = if flags & (1 << 24) != 0 && first != 0 {
                (first, if alternate == 0 { capacity } else { alternate })
            } else if flags & (1 << 25) != 0 && second != 0 {
                (second, capacity)
            } else {
                if !dma.write_descriptor(address, [first, 0, second, flags & !OWN]) {
                    r.status |= BUS_ERROR;
                    break;
                }
                r.rx.advance(stride);
                continue;
            };
            let end = frame.bytes.len().min(start + capacity);
            r.rx.buffer = buffer;
            let failed = !dma.write(buffer, &frame.bytes[start..end]);
            let mut status = if start == 0 { FIRST } else { 0 };
            let complete = end == frame.bytes.len() || failed;
            if complete {
                status |= LAST | (if failed { start } else { end }) as u32;
                if frame.crc_error {
                    status |= (1 << 24) | (1 << 15);
                }
            }
            if failed {
                status |= 1 << 15;
            }
            if !dma.write_descriptor(address, [0, 0, 0, status]) {
                r.status |= BUS_ERROR;
                break;
            }
            r.rx.advance(stride);
            if complete {
                self.incoming.pop_front();
                self.rx_offset = 0;
                if flags & CONTEXT != 0 {
                    r.status |= RX_COMPLETE;
                }
            } else {
                self.rx_offset = end;
            }
            if failed {
                r.status |= BUS_ERROR;
                break;
            }
        }
    }
}

fn bank_read(bank: &[u32], offset: usize) -> u32 {
    bank.get(offset / 4).copied().unwrap_or(0)
}

fn bank_write(bank: &mut [u32], offset: usize, value: u32) {
    if let Some(word) = bank.get_mut(offset / 4) {
        *word = value;
    }
}
