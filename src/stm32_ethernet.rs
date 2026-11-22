use std::collections::VecDeque;

use crate::{
    bus::EthernetDma,
    ethernet::{Mapping, Region, append_fcs, insert_ipv4_checksums},
};

const SIZE: u32 = 0x1400;
const OWN: u32 = 1 << 31;
const TX_COMPLETE: u32 = 1;
const TX_UNAVAILABLE: u32 = 1 << 2;
const RX_COMPLETE: u32 = 1 << 6;
const RX_UNAVAILABLE: u32 = 1 << 7;
const BUS_ERROR: u32 = 1 << 13;
const ABNORMAL: u32 = 1 << 15;
const NORMAL: u32 = 1 << 16;
const MAX_FRAME: usize = 2048;
const DESCRIPTOR_BUDGET: usize = 128;
const MAX_QUEUED_TX: usize = 64;

#[derive(Default)]
struct Ring {
    base: u32,
    current: u32,
    buffer: u32,
}

impl Ring {
    fn set_base(&mut self, value: u32) {
        self.base = value & !3;
        self.current = self.base;
    }

    fn advance(&mut self, chained: bool, end: bool, next: u32, stride: u32) {
        self.current = if chained {
            next & !3
        } else if end {
            self.base
        } else {
            self.current.wrapping_add(stride)
        };
    }
}

struct Registers {
    mac: u32,
    mac_high: u32,
    mac_low: u32,
    mii_address: u32,
    mii_data: u16,
    bus_mode: u32,
    operation: u32,
    status: u32,
    irq_enable: u32,
    rx: Ring,
    tx: Ring,
    misc: [u32; 24],
}

impl Default for Registers {
    fn default() -> Self {
        Self {
            mac: 0x8000,
            mac_high: 0x8000_ffff,
            mac_low: u32::MAX,
            mii_address: 0,
            mii_data: 0,
            bus_mode: 0x0002_0100,
            operation: 0,
            status: 0,
            irq_enable: 0,
            rx: Ring::default(),
            tx: Ring::default(),
            misc: [0; 24],
        }
    }
}

struct Descriptor {
    status: u32,
    control: u32,
    first: u32,
    second: u32,
}

impl Descriptor {
    fn read(dma: &EthernetDma<'_>, address: u32) -> Option<Self> {
        let bytes = dma.read(address, 16)?;
        let (words, _) = bytes.as_chunks::<4>();
        let [status, control, first, second] =
            <[_; 4]>::try_from(words).ok()?.map(u32::from_le_bytes);
        Some(Self {
            status,
            control,
            first,
            second,
        })
    }

    fn buffers(&self, chained: bool) -> [(u32, usize); 2] {
        [
            (self.first, (self.control & 0x1fff) as usize),
            (
                self.second,
                if chained {
                    0
                } else {
                    ((self.control >> 16) & 0x1fff) as usize
                },
            ),
        ]
    }
}

pub struct Stm32Ethernet {
    pub name: String,
    pub mapping: Mapping,
    pub irq: Option<u8>,
    pub phys: Vec<(u8, [u16; 32])>,
    registers: Registers,
    partial: Vec<u8>,
    partial_flags: Option<u32>,
    outgoing: VecDeque<Vec<u8>>,
}

impl Stm32Ethernet {
    pub fn new(name: String, base: u32, irq: Option<u8>) -> Self {
        Self {
            name,
            mapping: Mapping {
                base,
                size: SIZE,
                region: Region::Registers,
            },
            irq,
            phys: Vec::new(),
            registers: Registers::default(),
            partial: Vec::new(),
            partial_flags: None,
            outgoing: VecDeque::new(),
        }
    }

    pub fn interrupt(&self) -> Option<u8> {
        (self.registers.status & self.registers.irq_enable & (NORMAL | ABNORMAL) != 0)
            .then_some(self.irq)
            .flatten()
    }

    pub fn take_frame(&mut self) -> Option<Vec<u8>> {
        self.outgoing.pop_front()
    }

    fn offset(&self, address: u32, width: usize) -> Option<u32> {
        let offset = address.checked_sub(self.mapping.base)?;
        (width == 4 && offset < SIZE && offset & 3 == 0).then_some(offset)
    }

    pub fn read(&self, address: u32, width: usize) -> Option<u32> {
        let offset = self.offset(address, width)?;
        let r = &self.registers;
        Some(match offset {
            0 => r.mac,
            0x10 => r.mii_address,
            0x14 => u32::from(r.mii_data),
            0x40 => r.mac_high,
            0x44 => r.mac_low,
            0x04..=0x5c => r.misc[(offset / 4) as usize],
            0x1000 => r.bus_mode,
            0x100c => r.rx.base,
            0x1010 => r.tx.base,
            0x1014 => {
                r.status
                    | (u32::from(r.operation & (1 << 1) != 0) * (4 << 17))
                    | (u32::from(r.operation & (1 << 13) != 0) * (6 << 20))
            }
            0x1018 => r.operation,
            0x101c => r.irq_enable,
            0x1048 => r.tx.current,
            0x104c => r.rx.current,
            0x1050 => r.tx.buffer,
            0x1054 => r.rx.buffer,
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
        let Some(offset) = self.offset(address, width) else {
            return false;
        };
        let r = &mut self.registers;
        match offset {
            0 => {
                r.mac = value;
                self.transmit(dma);
            }
            0x10 => {
                r.mii_address = value & 0xfffe;
                if value & 1 != 0 {
                    let address = ((value >> 11) & 31) as u8;
                    let index = ((value >> 6) & 31) as usize;
                    let phy = self.phys.iter_mut().find(|(id, _)| *id == address);
                    if value & 2 == 0 {
                        r.mii_data = phy.map_or(0xffff, |(_, words)| words[index]);
                    } else if let Some((_, words)) = phy {
                        words[index] = if index == 0 {
                            r.mii_data & 0x7dff
                        } else {
                            r.mii_data
                        };
                    }
                }
            }
            0x14 => r.mii_data = value as u16,
            0x40 => r.mac_high = (value & 0xffff) | (1 << 31),
            0x44 => r.mac_low = value,
            0x04..=0x5c => r.misc[(offset / 4) as usize] = value,
            0x1000 if value & 1 != 0 => {
                *r = Registers::default();
                self.partial.clear();
                self.partial_flags = None;
                self.outgoing.clear();
            }
            0x1000 => r.bus_mode = value,
            0x1004 => self.transmit(dma),
            0x100c => r.rx.set_base(value),
            0x1010 => r.tx.set_base(value),
            0x1014 => r.status &= !(value & 0x1ffff),
            0x1018 => {
                r.operation = value & !(1 << 20);
                self.transmit(dma);
            }
            0x101c => r.irq_enable = value & 0x1ffff,
            _ => {}
        }
        self.refresh_summary();
        true
    }

    fn stride(&self) -> u32 {
        let mode = self.registers.bus_mode;
        let words = if mode & (1 << 7) != 0 { 8 } else { 4 };
        4 * (words + ((mode >> 2) & 31))
    }

    fn refresh_summary(&mut self) {
        let r = &mut self.registers;
        let enabled = r.status & r.irq_enable;
        if enabled & (TX_COMPLETE | TX_UNAVAILABLE | RX_COMPLETE) != 0 {
            r.status |= NORMAL;
        }
        if enabled & (RX_UNAVAILABLE | BUS_ERROR) != 0 {
            r.status |= ABNORMAL;
        }
    }

    fn transmit(&mut self, dma: &mut EthernetDma<'_>) {
        if self.registers.mac & (1 << 3) == 0 || self.registers.operation & (1 << 13) == 0 {
            return;
        }
        let stride = self.stride();
        for _ in 0..DESCRIPTOR_BUDGET {
            let ring = &mut self.registers.tx;
            let address = ring.current;
            let Some(desc) = Descriptor::read(dma, address) else {
                self.registers.status |= BUS_ERROR;
                break;
            };
            if desc.status & OWN == 0 {
                self.registers.status |= TX_UNAVAILABLE;
                break;
            }
            if desc.status & (1 << 28) != 0 {
                self.partial.clear();
                self.partial_flags = Some(desc.status);
            }
            let chained = desc.status & (1 << 20) != 0;
            let mut error = self.partial_flags.is_none();
            for (buffer, length) in desc.buffers(chained) {
                if length == 0 || error {
                    continue;
                }
                ring.buffer = buffer;
                if self.partial.len() + length <= MAX_FRAME
                    && let Some(bytes) = dma.read(buffer, length)
                {
                    self.partial.extend_from_slice(bytes);
                } else {
                    error = true;
                    self.partial_flags = None;
                }
            }
            if !dma.write(
                address,
                &((desc.status & !OWN) | (u32::from(error) << 15)).to_le_bytes(),
            ) {
                self.registers.status |= BUS_ERROR;
                break;
            }
            ring.advance(chained, desc.status & (1 << 21) != 0, desc.second, stride);
            if desc.status & (1 << 29) != 0 {
                if let Some(flags) = self.partial_flags.take()
                    && self.partial.len() >= 14
                    && self.outgoing.len() < MAX_QUEUED_TX
                {
                    let mut frame = std::mem::take(&mut self.partial);
                    insert_ipv4_checksums(&mut frame, (flags >> 22) & 3);
                    if flags & (1 << 26) == 0 {
                        frame.resize(frame.len().max(60), 0);
                    }
                    if flags & (1 << 27) == 0 {
                        append_fcs(&mut frame);
                    }
                    self.outgoing.push_back(frame);
                }
                if desc.status & (1 << 30) != 0 {
                    self.registers.status |= TX_COMPLETE;
                }
            }
        }
    }

    pub fn receive(&mut self, frame: &[u8], dma: &mut EthernetDma<'_>) {
        if self.registers.mac & (1 << 2) == 0 || self.registers.operation & (1 << 1) == 0 {
            return;
        }
        let frame = if self.registers.mac & (1 << 25) != 0
            && frame.len() >= 18
            && u16::from_be_bytes([frame[12], frame[13]]) >= 1536
        {
            &frame[..frame.len() - 4]
        } else {
            frame
        };
        let stride = self.stride();
        let mut written = 0;
        for _ in 0..DESCRIPTOR_BUDGET {
            let ring = &mut self.registers.rx;
            let address = ring.current;
            let Some(desc) = Descriptor::read(dma, address) else {
                self.registers.status |= BUS_ERROR;
                break;
            };
            if desc.status & OWN == 0 {
                self.registers.status |= RX_UNAVAILABLE;
                break;
            }
            let mut status = u32::from(written == 0) << 9;
            let chained = desc.control & (1 << 14) != 0;
            let mut fault = false;
            for (buffer, capacity) in desc.buffers(chained) {
                let length = capacity.min(frame.len() - written);
                if length == 0 {
                    continue;
                }
                ring.buffer = buffer;
                if !dma.write(buffer, &frame[written..written + length]) {
                    fault = true;
                    break;
                }
                written += length;
            }
            ring.advance(chained, desc.control & (1 << 15) != 0, desc.second, stride);
            let complete = written == frame.len();
            if complete {
                status |= (1 << 8) | ((written as u32) << 16);
            } else if fault
                || Descriptor::read(dma, ring.current).is_none_or(|next| next.status & OWN == 0)
                || ring.current == address
            {
                status |= (1 << 8) | (1 << 15) | (1 << 14);
            }
            if !dma.write(address, &status.to_le_bytes()) || fault {
                self.registers.status |= BUS_ERROR;
                break;
            }
            if complete {
                if desc.control & OWN == 0 {
                    self.registers.status |= RX_COMPLETE;
                }
                break;
            }
            if status & (1 << 15) != 0 {
                self.registers.status |= RX_UNAVAILABLE;
                break;
            }
        }
        self.refresh_summary();
    }
}
