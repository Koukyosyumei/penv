use std::{
    collections::BTreeMap,
    fs::File,
    io::{self, Read, Seek, SeekFrom},
};

use crate::bus::Bus;

const SECTOR: u64 = 512;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Region {
    Phy,
    Core,
    Reader,
    Writer,
}

pub struct Registration {
    pub base: u32,
    pub size: u32,
    pub region: Region,
}

#[derive(Default)]
struct Dma {
    address: [u8; 8],
    length: [u8; 4],
    enabled: bool,
}

impl Dma {
    fn read(&self, offset: u32) -> u32 {
        match offset {
            0..=0x1c => u32::from(self.address[(offset / 4) as usize]),
            0x20..=0x2c => u32::from(self.length[((offset - 0x20) / 4) as usize]),
            0x30 => u32::from(self.enabled),
            0x34 => 1,
            _ => 0,
        }
    }

    fn write(&mut self, offset: u32, value: u32) {
        match offset {
            0..=0x1c => self.address[(offset / 4) as usize] = value as u8,
            0x20..=0x2c => self.length[((offset - 0x20) / 4) as usize] = value as u8,
            0x30 => self.enabled = value & 1 != 0,
            _ => {}
        }
    }

    fn read_wide(&self, offset: u32) -> u32 {
        match offset {
            0 | 4 => {
                let index = offset as usize;
                u32::from_be_bytes(self.address[index..index + 4].try_into().unwrap())
            }
            8 => u32::from_be_bytes(self.length),
            0xc => u32::from(self.enabled),
            0x10 => 1,
            _ => 0,
        }
    }

    fn write_wide(&mut self, offset: u32, value: u32) {
        match offset {
            0 | 4 => {
                let index = offset as usize;
                self.address[index..index + 4].copy_from_slice(&value.to_be_bytes());
            }
            8 => self.length = value.to_be_bytes(),
            0xc => self.enabled = value & 1 != 0,
            _ => {}
        }
    }

    fn buffer<'a>(&self, bus: &'a mut Bus) -> Result<&'a mut [u8], ()> {
        let address = u64::from_be_bytes(self.address) as u32;
        let length = u32::from_be_bytes(self.length) as usize;
        bus.memory(address, length).map_err(|_| ())
    }
}

pub struct Card {
    file: File,
    file_size: u64,
    capacity: u64,
    overlay: BTreeMap<u64, Box<[u8; SECTOR as usize]>>,
    state: u32,
    rca: u16,
    app: bool,
    position: u64,
    special: Option<Vec<u8>>,
}

impl Card {
    pub fn new(file: File, capacity: u64) -> Result<Self, String> {
        let file_size = file.metadata().map_err(|error| error.to_string())?.len();
        if capacity < 2048
            || file_size == 0
            || file_size > capacity
            || !capacity.is_multiple_of(SECTOR)
        {
            return Err("SD image must fit in a nonempty sector-aligned card".into());
        }
        Ok(Self {
            file,
            file_size,
            capacity,
            overlay: BTreeMap::new(),
            state: 0,
            rca: 0,
            app: false,
            position: 0,
            special: None,
        })
    }

    fn status(&self) -> u32 {
        (self.state << 9) | (1 << 8) | (u32::from(self.app) << 5)
    }

    fn high_capacity(&self) -> bool {
        self.capacity > (2 << 30)
    }

    fn csd(&self) -> u128 {
        let common = (9 << 80) | (9 << 22) | (0x15 << 84) | (0x32 << 96);
        if self.high_capacity() {
            common | (1 << 126) | (u128::from(self.capacity / 524288 - 1).min(0x3f_ffff) << 48)
        } else {
            let multiplier = (0..=7)
                .find(|&m| self.capacity / (2048 << m) <= 4096)
                .unwrap_or(7);
            let size = (self.capacity / (2048 << multiplier)).saturating_sub(1);
            common | (u128::from(size.min(4095)) << 62) | ((multiplier as u128) << 47)
        }
    }

    fn command(&mut self, command: u8, arg: u32) -> Option<(u8, u128)> {
        let app = std::mem::take(&mut self.app);
        self.special = None;
        let long = |value| Some((2, value));
        let short = |value: u32| Some((1, u128::from(value)));
        let cid = 0xab00_0052_454e_4f44_0000_0000_0001_2800;
        let ocr = 0x80ff_ff80 | (u32::from(self.high_capacity()) << 30);
        if app {
            match command {
                6 | 23 => return short(self.status()),
                13 => {
                    self.special = Some(vec![0; 64]);
                    return short(self.status());
                }
                41 => {
                    self.state = 1;
                    return short(ocr);
                }
                51 => {
                    self.state = 5;
                    self.special = Some(vec![0, 5, 0, 0, 0, 0, 0, 0]);
                    return short(self.status());
                }
                _ => {}
            }
        }
        match command {
            0 => {
                self.state = 0;
                self.rca = 0;
                Some((0, 0))
            }
            1 => short(ocr),
            2 => {
                self.state = 2;
                long(cid)
            }
            3 => {
                self.state = 3;
                self.rca = self.rca.wrapping_add(1).max(1);
                short((u32::from(self.rca) << 16) | self.status())
            }
            6 => {
                let mut result = vec![0; 64];
                result[47] = 1;
                self.special = Some(result);
                short(self.status())
            }
            7 => {
                self.state = if arg >> 16 == u32::from(self.rca) && self.rca != 0 {
                    4
                } else {
                    3
                };
                short(self.status())
            }
            8 => short(arg & 0xfff),
            9 => long(self.csd()),
            10 => long(cid),
            12 => {
                self.state = 4;
                short(self.status())
            }
            13 | 16 | 23 => short(self.status()),
            17 | 18 | 24 | 25 => {
                self.position = u64::from(arg) * if self.high_capacity() { SECTOR } else { 1 };
                self.state = if command <= 18 { 5 } else { 6 };
                short(self.status())
            }
            55 => {
                self.app = true;
                short(self.status())
            }
            _ => None,
        }
    }

    fn check_range(&self, offset: u64, size: usize) -> io::Result<()> {
        if offset
            .checked_add(size as u64)
            .is_none_or(|end| end > self.capacity)
        {
            return Err(io::Error::other("access outside SD card capacity"));
        }
        Ok(())
    }

    fn backing(&mut self, offset: u64, bytes: &mut [u8]) -> io::Result<()> {
        let available = self
            .file_size
            .saturating_sub(offset)
            .min(bytes.len() as u64) as usize;
        if available != 0 {
            self.file.seek(SeekFrom::Start(offset))?;
            self.file.read_exact(&mut bytes[..available])?;
        }
        bytes[available..].fill(0);
        Ok(())
    }

    fn read(&mut self, bytes: &mut [u8]) -> io::Result<()> {
        if let Some(data) = self.special.take() {
            let available = bytes.len().min(data.len());
            bytes[..available].copy_from_slice(&data[..available]);
            bytes[available..].fill(0);
        } else {
            self.check_range(self.position, bytes.len())?;
            self.backing(self.position, bytes)?;
            let mut offset = self.position;
            let mut position = 0;
            while position < bytes.len() {
                let within = (offset % SECTOR) as usize;
                let length = (SECTOR as usize - within).min(bytes.len() - position);
                if let Some(sector) = self.overlay.get(&(offset / SECTOR)) {
                    bytes[position..position + length]
                        .copy_from_slice(&sector[within..within + length]);
                }
                position += length;
                offset += length as u64;
            }
            self.position = offset;
        }
        self.state = 4;
        Ok(())
    }

    fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.check_range(self.position, bytes.len())?;
        let mut offset = self.position;
        let mut position = 0;
        while position < bytes.len() {
            let index = offset / SECTOR;
            let within = (offset % SECTOR) as usize;
            let length = (SECTOR as usize - within).min(bytes.len() - position);
            let mut sector = if let Some(sector) = self.overlay.remove(&index) {
                sector
            } else {
                let mut sector = Box::new([0; SECTOR as usize]);
                if within != 0 || length != sector.len() {
                    self.backing(index * SECTOR, sector.as_mut_slice())?;
                }
                sector
            };
            sector[within..within + length].copy_from_slice(&bytes[position..position + length]);
            self.overlay.insert(index, sector);
            position += length;
            offset += length as u64;
        }
        self.position = offset;
        self.state = 4;
        Ok(())
    }
}

/// SPI-mode SD transport for BIOS initialization and single-block reads.
pub struct SpiCard {
    card: Card,
    packet: [u8; 6],
    received: usize,
    response: std::collections::VecDeque<u8>,
    idle: bool,
    application: bool,
}

impl SpiCard {
    pub fn new(card: Card) -> Self {
        Self {
            card,
            packet: [0; 6],
            received: 0,
            response: std::collections::VecDeque::new(),
            idle: true,
            application: false,
        }
    }

    pub fn deselect(&mut self) {
        self.received = 0;
        self.response.clear();
    }

    pub fn transfer(&mut self, byte: u8) -> u8 {
        if let Some(response) = self.response.pop_front() {
            return response;
        }
        if self.received == 0 && byte & 0xc0 != 0x40 {
            return 0xff;
        }
        self.packet[self.received] = byte;
        self.received += 1;
        if self.received == self.packet.len() {
            self.received = 0;
            self.command();
        }
        0xff
    }

    fn command(&mut self) {
        let cmd = self.packet[0] & 63;
        let arg = u32::from_be_bytes(self.packet[1..5].try_into().unwrap());
        let app = std::mem::take(&mut self.application);
        let status = u8::from(self.idle);
        match cmd {
            0 => {
                self.idle = true;
                self.response.push_back(1);
            }
            1 => {
                self.idle = false;
                self.response.push_back(0);
            }
            8 => {
                self.response.push_back(status);
                self.response.extend((arg & 0xfff).to_be_bytes());
            }
            41 if app => {
                self.idle = false;
                self.response.push_back(0);
            }
            55 => {
                self.application = true;
                self.response.push_back(status);
            }
            58 => {
                self.response.push_back(status);
                let ocr = 0x00ff_8000
                    | (u32::from(!self.idle) << 31)
                    | (u32::from(self.card.high_capacity()) << 30);
                self.response.extend(ocr.to_be_bytes());
            }
            16 if !self.idle && arg == SECTOR as u32 => self.response.push_back(0),
            9 | 10 if !self.idle => {
                let (_, register) = self.card.command(cmd, arg).unwrap();
                self.data(&register.to_be_bytes());
            }
            17 if !self.idle => {
                self.card.command(cmd, arg);
                let mut block = [0; SECTOR as usize];
                if self.card.read(&mut block).is_ok() {
                    self.data(&block);
                } else {
                    self.response.push_back(0x20);
                }
            }
            _ => self.response.push_back(status | 4),
        }
    }

    fn data(&mut self, bytes: &[u8]) {
        self.response.extend([0, 0xff, 0xfe]);
        self.response.extend(bytes);
        let mut crc = 0u16;
        for &byte in bytes {
            crc ^= u16::from(byte) << 8;
            for _ in 0..8 {
                crc = (crc << 1) ^ if crc & 0x8000 != 0 { 0x1021 } else { 0 };
            }
        }
        self.response.extend(crc.to_be_bytes());
    }
}

pub struct Controller {
    pub name: String,
    pub registrations: Vec<Registration>,
    pub card: Option<Card>,
    wide_csr: bool,
    clock_divider: u32,
    reader: Dma,
    writer: Dma,
    argument: [u8; 4],
    response: [u8; 16],
    command: u8,
    flags: u8,
    block_size: [u8; 2],
    block_count: [u8; 4],
    command_event: u32,
    data_event: u32,
}

impl Controller {
    pub const PHY_SIZE: u32 = 0x200;

    pub fn new(name: String, registrations: Vec<Registration>, wide_csr: bool) -> Self {
        Self {
            name,
            registrations,
            card: None,
            wide_csr,
            clock_divider: 0,
            reader: Dma::default(),
            writer: Dma::default(),
            argument: [0; 4],
            response: [0; 16],
            command: 0,
            flags: 0,
            block_size: [0; 2],
            block_count: [0; 4],
            command_event: 1,
            data_event: 1,
        }
    }

    pub fn locate(&self, address: u32) -> Option<(Region, u32)> {
        self.registrations.iter().find_map(|registration| {
            let offset = address.checked_sub(registration.base)?;
            (registration.size.checked_sub(offset)? >= 4).then_some((registration.region, offset))
        })
    }

    pub fn read(&self, region: Region, offset: u32) -> u32 {
        match region {
            Region::Phy if self.wide_csr && offset == 4 => self.clock_divider,
            Region::Phy => u32::from(offset == 0 && self.card.is_none()),
            Region::Reader if self.wide_csr => self.reader.read_wide(offset),
            Region::Writer if self.wide_csr => self.writer.read_wide(offset),
            Region::Core if self.wide_csr => self.read_wide(offset),
            Region::Reader => self.reader.read(offset),
            Region::Writer => self.writer.read(offset),
            Region::Core => match offset {
                0x00..=0x0c => u32::from(self.argument[(offset / 4) as usize]),
                0x18 => u32::from(self.command),
                0x1c => u32::from(self.flags),
                0x24..=0x60 => u32::from(self.response[((offset - 0x24) / 4) as usize]),
                0x64 => self.command_event,
                0x68 => self.data_event,
                0x6c..=0x70 => u32::from(self.block_size[((offset - 0x6c) / 4) as usize]),
                0x74..=0x80 => u32::from(self.block_count[((offset - 0x74) / 4) as usize]),
                _ => 0,
            },
        }
    }

    fn read_wide(&self, offset: u32) -> u32 {
        match offset {
            0 => u32::from_be_bytes(self.argument),
            4 => (u32::from(self.command) << 8) | u32::from(self.flags),
            0xc..=0x18 => {
                let index = (offset - 0xc) as usize;
                u32::from_be_bytes(self.response[index..index + 4].try_into().unwrap())
            }
            0x1c => self.command_event,
            0x20 => self.data_event,
            0x24 => u32::from(u16::from_be_bytes(self.block_size)),
            0x28 => u32::from_be_bytes(self.block_count),
            _ => 0,
        }
    }

    fn write_wide(&mut self, offset: u32, value: u32, bus: &mut Bus) {
        match offset {
            0 => self.argument = value.to_be_bytes(),
            4 => {
                self.command = (value >> 8) as u8 & 0x3f;
                self.flags = value as u8 & 0x63;
            }
            8 if value & 1 != 0 => self.issue(bus),
            0x24 => self.block_size = (value as u16 & 0x3ff).to_be_bytes(),
            0x28 => self.block_count = value.to_be_bytes(),
            _ => {}
        }
    }

    pub fn write(&mut self, region: Region, offset: u32, value: u32, bus: &mut Bus) {
        match region {
            Region::Phy if self.wide_csr && offset == 4 => self.clock_divider = value,
            Region::Phy => {}
            Region::Reader if self.wide_csr => self.reader.write_wide(offset, value),
            Region::Writer if self.wide_csr => self.writer.write_wide(offset, value),
            Region::Core if self.wide_csr => self.write_wide(offset, value, bus),
            Region::Reader => self.reader.write(offset, value),
            Region::Writer => self.writer.write(offset, value),
            Region::Core => match offset {
                0x00..=0x0c => self.argument[(offset / 4) as usize] = value as u8,
                0x18 => self.command = value as u8 & 0x7f,
                0x1c => self.flags = value as u8,
                0x20 if value & 1 != 0 => self.issue(bus),
                0x6c..=0x70 => self.block_size[((offset - 0x6c) / 4) as usize] = value as u8,
                0x74..=0x80 => self.block_count[((offset - 0x74) / 4) as usize] = value as u8,
                _ => {}
            },
        }
    }

    fn issue(&mut self, bus: &mut Bus) {
        self.command_event = 5;
        self.data_event = 1;
        let Some(card) = self.card.as_mut() else {
            return;
        };
        let Some((kind, response)) = card.command(self.command, u32::from_be_bytes(self.argument))
        else {
            return;
        };
        let expected = self.flags & 7;
        if kind != expected && !(self.wide_csr && kind == 1 && expected == 3) {
            return;
        }
        self.command_event = 1;
        self.response = response.to_be_bytes();
        let result = match (self.flags >> 5) & 7 {
            1 if self.reader.enabled => self
                .reader
                .buffer(bus)
                .and_then(|buffer| card.read(buffer).map_err(|_| ())),
            2 if self.writer.enabled => self
                .writer
                .buffer(bus)
                .and_then(|buffer| card.write(buffer).map_err(|_| ())),
            _ => Ok(()),
        };
        if result.is_err() {
            self.data_event = 3;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spi_card_initializes_and_reads_one_sector_without_modifying_the_image() {
        fn command(card: &mut SpiCard, command: u8, argument: u32) -> u8 {
            for byte in [0x40 | command]
                .into_iter()
                .chain(argument.to_be_bytes())
                .chain([0xff])
            {
                assert_eq!(card.transfer(byte), 0xff);
            }
            card.transfer(0xff)
        }

        let path = std::env::temp_dir().join(format!("penv-spi-sd-{}.img", std::process::id()));
        let mut sector = [0; SECTOR as usize];
        sector[0] = 0x5a;
        std::fs::write(&path, sector).unwrap();
        let mut card = SpiCard::new(Card::new(File::open(&path).unwrap(), 2048).unwrap());
        assert_eq!(command(&mut card, 0, 0), 1);
        assert_eq!(command(&mut card, 55, 0), 1);
        card.deselect();
        assert_eq!(command(&mut card, 41, 0), 0);
        assert_eq!(command(&mut card, 24, 0), 4); // Write is outside the read-only subset.
        assert_eq!(command(&mut card, 17, 0), 0);
        assert_eq!(card.transfer(0xff), 0xff);
        assert_eq!(card.transfer(0xff), 0xfe);
        for byte in sector {
            assert_eq!(card.transfer(0xff), byte);
        }
        assert_eq!([card.transfer(0xff), card.transfer(0xff)], [0x97, 0xcd]);
        drop(card);
        assert_eq!(std::fs::read(&path).unwrap(), sector);
        std::fs::remove_file(path).unwrap();
    }
}
