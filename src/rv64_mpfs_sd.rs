use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    path::Path,
};

const SECTOR_SIZE: usize = 512;
const COMMAND_COMPLETE: u32 = 1;
const TRANSFER_COMPLETE: u32 = 1 << 1;
const BUFFER_READ_READY: u32 = 1 << 5;
const COMMAND_TIMEOUT: u32 = (1 << 15) | (1 << 16);

pub struct Controller {
    pub name: String,
    pub base: u32,
    irq: usize,
    card: Option<Card>,
    block_size_count: u32,
    argument: u32,
    command_mode: u32,
    response: u32,
    status: u32,
    status_enable: u32,
    signal_enable: u32,
    sector: [u8; SECTOR_SIZE],
    cursor: usize,
}

struct Card {
    file: File,
    length: u64,
    capacity: u64,
}

impl Controller {
    pub const SIZE: u64 = 0x2000;

    pub fn new(name: String, base: u32, irq: usize) -> Self {
        Self {
            name,
            base,
            irq,
            card: None,
            block_size_count: 0,
            argument: 0,
            command_mode: 0,
            response: 0,
            status: 0,
            status_enable: 0,
            signal_enable: 0,
            sector: [0; SECTOR_SIZE],
            cursor: SECTOR_SIZE,
        }
    }

    pub fn attach(&mut self, path: &Path, capacity: u64) -> io::Result<()> {
        if self.card.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "SD card already attached",
            ));
        }
        let file = File::open(path)?;
        let metadata = file.metadata()?;
        let length = metadata.len();
        if !metadata.is_file()
            || length == 0
            || length > capacity
            || !capacity.is_multiple_of(SECTOR_SIZE as u64)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid SD image or capacity",
            ));
        }
        self.card = Some(Card {
            file,
            length,
            capacity,
        });
        Ok(())
    }

    pub fn interrupt(&self) -> (usize, bool) {
        (self.irq, self.status & self.signal_enable != 0)
    }

    pub fn read(&mut self, address: u32, width: usize) -> Option<u32> {
        let offset = address.checked_sub(self.base)?;
        if width != 4 {
            return None;
        }
        Some(match offset {
            0x204 => self.block_size_count,
            0x208 => self.argument,
            0x20c => self.command_mode,
            0x210 => self.response,
            0x220 => self.read_data(),
            0x224 => {
                0x00f2_0400
                    | if self.card.is_some() { 0x0005_0000 } else { 0 }
                    | if self.cursor < SECTOR_SIZE {
                        1 << 11
                    } else {
                        0
                    }
            }
            0x22c => 1 << 1, // Internal clock stable
            0x230 => self.status,
            0x234 => self.status_enable,
            0x238 => self.signal_enable,
            _ => return None,
        })
    }

    pub fn write(&mut self, address: u32, width: usize, value: u32) -> Option<()> {
        match (address.checked_sub(self.base)?, width) {
            (0x204, 4) => self.block_size_count = value,
            (0x208, 4) => self.argument = value,
            (0x20c, 4) => {
                self.command_mode = value;
                self.command();
            }
            (0x20c, 2) => self.command_mode = (self.command_mode & 0xffff_0000) | value,
            (0x20e, 2) => {
                self.command_mode = (self.command_mode & 0xffff) | (value << 16);
                self.command();
            }
            (0x230, 4) => self.status &= !value,
            (0x234, 4) => self.status_enable = value,
            (0x238, 4) => self.signal_enable = value,
            _ => return None,
        }
        Some(())
    }

    fn command(&mut self) {
        self.response = 0;
        self.cursor = SECTOR_SIZE;
        let index = (self.command_mode >> 24) & 0x3f;
        let flags = match (index, self.card.as_mut()) {
            (0, Some(_)) => COMMAND_COMPLETE,
            (8, Some(_)) => {
                self.response = self.argument & 0xfff;
                COMMAND_COMPLETE
            }
            (17, Some(card))
                if (self.block_size_count & 0xfff) == SECTOR_SIZE as u32
                    && self.command_mode & 0x11 == 0x10 =>
            {
                if card.read_sector(self.argument, &mut self.sector).is_ok() {
                    self.cursor = 0;
                    COMMAND_COMPLETE | BUFFER_READ_READY
                } else {
                    COMMAND_TIMEOUT
                }
            }
            _ => COMMAND_TIMEOUT,
        };
        self.status |= flags & self.status_enable;
    }

    fn read_data(&mut self) -> u32 {
        let end = self.cursor + 4;
        let Some(bytes) = self.sector.get(self.cursor..end) else {
            return 0;
        };
        self.cursor = end;
        if end == SECTOR_SIZE {
            self.status |= TRANSFER_COMPLETE & self.status_enable;
        }
        u32::from_le_bytes(bytes.try_into().unwrap())
    }
}

impl Card {
    fn read_sector(&mut self, sector: u32, buffer: &mut [u8; SECTOR_SIZE]) -> io::Result<()> {
        let offset = u64::from(sector) * SECTOR_SIZE as u64;
        if offset >= self.capacity {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "SD sector outside capacity",
            ));
        }
        buffer.fill(0);
        let stored = self.length.saturating_sub(offset).min(SECTOR_SIZE as u64) as usize;
        if stored != 0 {
            self.file.seek(SeekFrom::Start(offset))?;
            self.file.read_exact(&mut buffer[..stored])?;
        }
        Ok(())
    }
}
