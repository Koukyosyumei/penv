use crate::{
    bus::EthernetDma, litex_ethernet::LiteXEthernet, stm32_ethernet::Stm32Ethernet,
    stm32h7_ethernet::Stm32h7Ethernet,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Region {
    Registers,
    Buffer,
    Phy,
    Dma,
    Mtl,
}

pub struct Mapping {
    pub base: u32,
    pub size: u32,
    pub region: Region,
}

pub enum Controller {
    LiteX(LiteXEthernet),
    Stm32(Stm32Ethernet),
    Stm32h7(Stm32h7Ethernet),
}

impl Controller {
    pub fn name(&self) -> &str {
        match self {
            Self::LiteX(device) => &device.name,
            Self::Stm32(device) => &device.name,
            Self::Stm32h7(device) => &device.name,
        }
    }

    pub fn mappings(&self) -> &[Mapping] {
        match self {
            Self::LiteX(device) => &device.mappings,
            Self::Stm32(device) => std::slice::from_ref(&device.mapping),
            Self::Stm32h7(device) => &device.mappings,
        }
    }

    pub fn vex_irq(&self) -> u32 {
        match self {
            Self::LiteX(device) => device.interrupt(),
            Self::Stm32(_) | Self::Stm32h7(_) => 0,
        }
    }

    pub fn nvic_irq(&self) -> Option<u8> {
        match self {
            Self::Stm32(device) => device.interrupt(),
            Self::Stm32h7(device) => device.interrupt(),
            Self::LiteX(_) => None,
        }
    }

    pub fn read(&self, address: u32, width: usize) -> Option<u32> {
        match self {
            Self::LiteX(device) => device.read(address, width),
            Self::Stm32(device) => device.read(address, width),
            Self::Stm32h7(device) => device.read(address, width),
        }
    }

    pub fn write(
        &mut self,
        address: u32,
        width: usize,
        value: u32,
        dma: &mut EthernetDma<'_>,
    ) -> bool {
        match self {
            Self::LiteX(device) => device.write(address, width, value),
            Self::Stm32(device) => device.write(address, width, value, dma),
            Self::Stm32h7(device) => device.write(address, width, value, dma),
        }
    }

    pub fn take_frame(&mut self) -> Option<Vec<u8>> {
        match self {
            Self::LiteX(device) => device.take_frame(),
            Self::Stm32(device) => device.take_frame(),
            Self::Stm32h7(device) => device.take_frame(),
        }
    }

    pub fn receive(&mut self, frame: &[u8], dma: &mut EthernetDma<'_>) {
        match self {
            Self::LiteX(device) => device.receive(frame),
            Self::Stm32(device) => device.receive(frame, dma),
            Self::Stm32h7(device) => device.receive(frame, dma),
        }
    }
}

pub fn append_fcs(frame: &mut Vec<u8>) {
    frame.extend_from_slice(&fcs(frame).to_le_bytes());
}

pub fn has_valid_fcs(frame: &[u8]) -> bool {
    frame.len() >= 18 && {
        let (data, trailer) = frame.split_at(frame.len() - 4);
        trailer == fcs(data).to_le_bytes()
    }
}

fn fcs(frame: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for &byte in frame.iter() {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}

pub fn insert_ipv4_checksums(frame: &mut [u8], mode: u32) {
    if mode == 0 || frame.len() < 14 {
        return;
    }
    let mut network = 14;
    let mut protocol = u16::from_be_bytes([frame[12], frame[13]]);
    while matches!(protocol, 0x8100 | 0x88a8) && frame.len() >= network + 4 {
        protocol = u16::from_be_bytes([frame[network + 2], frame[network + 3]]);
        network += 4;
    }
    if protocol != 0x0800 || frame.len() < network + 20 {
        return;
    }
    let packet = &mut frame[network..];
    let header_length = usize::from(packet[0] & 15) * 4;
    let total_length = usize::from(u16::from_be_bytes([packet[2], packet[3]]));
    if packet[0] >> 4 != 4
        || header_length < 20
        || total_length < header_length
        || total_length > packet.len()
    {
        return;
    }
    let (header, transport) = packet[..total_length].split_at_mut(header_length);
    header[10..12].fill(0);
    let checksum = finish_checksum(checksum_sum(header));
    header[10..12].copy_from_slice(&checksum.to_be_bytes());
    let fragment = u16::from_be_bytes([header[6], header[7]]) & 0x3fff;
    if mode == 1 || fragment != 0 {
        return;
    }
    let protocol = header[9];
    let offset = match protocol {
        1 if transport.len() >= 4 => 2,
        6 if transport.len() >= 20 => 16,
        17 if transport.len() >= 8 => 6,
        _ => return,
    };
    let seed = if protocol == 1 {
        0
    } else if mode == 3 {
        checksum_sum(&header[12..20]) + u32::from(protocol) + transport.len() as u32
    } else {
        u32::from(u16::from_be_bytes([
            transport[offset],
            transport[offset + 1],
        ]))
    };
    transport[offset..offset + 2].fill(0);
    let checksum = finish_checksum(seed + checksum_sum(transport));
    let checksum = if protocol == 17 && checksum == 0 {
        u16::MAX
    } else {
        checksum
    };
    transport[offset..offset + 2].copy_from_slice(&checksum.to_be_bytes());
}

fn checksum_sum(data: &[u8]) -> u32 {
    data.chunks(2)
        .map(|word| {
            u32::from(u16::from_be_bytes([
                word[0],
                word.get(1).copied().unwrap_or(0),
            ]))
        })
        .sum()
}

fn finish_checksum(mut sum: u32) -> u16 {
    while sum > 0xffff {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}
