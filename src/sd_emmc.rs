//! In-memory eMMC card command and data interface.
use std::{collections::HashMap, io};

const SECTOR_SIZE: usize = 512;

struct Partition {
    sectors: u32,
    data: HashMap<u32, [u8; SECTOR_SIZE]>,
}

impl Partition {
    fn new(sectors: u32) -> Self {
        Self {
            sectors,
            data: HashMap::new(),
        }
    }

    fn check(&self, sector: u32, len: usize) -> io::Result<()> {
        if sector >= self.sectors || len > SECTOR_SIZE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "transfer outside eMMC sector",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum Transfer {
    Idle,
    Read(u32, usize),
    Write(u32, usize),
}

pub struct Card {
    storage: Partition,
    transfer: Transfer,
}

impl Card {
    pub fn new(capacity: u64, spi_mode: bool) -> io::Result<Self> {
        if spi_mode {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "eMMC does not support SPI mode",
            ));
        }
        let sectors = u32::try_from(capacity / SECTOR_SIZE as u64)
            .ok()
            .filter(|&sectors| sectors != 0 && capacity.is_multiple_of(SECTOR_SIZE as u64))
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid eMMC capacity"))?;
        Ok(Self {
            storage: Partition::new(sectors),
            transfer: Transfer::Idle,
        })
    }

    pub fn reset(&mut self) {
        self.transfer = Transfer::Idle;
    }

    pub fn handle_command(&mut self, command: u8, argument: u32) -> io::Result<u32> {
        match command {
            0 => self.reset(),
            17 | 24 => {
                self.storage.check(argument, 0)?;
                self.transfer = if command == 17 {
                    Transfer::Read(argument, 0)
                } else {
                    Transfer::Write(argument, 0)
                };
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "unsupported eMMC command",
                ));
            }
        }
        Ok(0)
    }

    pub fn read_data(&mut self, len: usize) -> io::Result<Vec<u8>> {
        let Transfer::Read(sector, cursor) = self.transfer else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "no active eMMC read",
            ));
        };
        let end = cursor.checked_add(len).ok_or(io::ErrorKind::InvalidInput)?;
        self.storage.check(sector, end)?;
        let data = self
            .storage
            .data
            .get(&sector)
            .map_or_else(|| vec![0; len], |data| data[cursor..end].to_vec());
        self.transfer = if end == SECTOR_SIZE {
            Transfer::Idle
        } else {
            Transfer::Read(sector, end)
        };
        Ok(data)
    }

    pub fn write_data(&mut self, data: &[u8]) -> io::Result<()> {
        let Transfer::Write(sector, cursor) = self.transfer else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "no active eMMC write",
            ));
        };
        let end = cursor
            .checked_add(data.len())
            .ok_or(io::ErrorKind::InvalidInput)?;
        self.storage.check(sector, end)?;
        self.storage.data.entry(sector).or_insert([0; SECTOR_SIZE])[cursor..end]
            .copy_from_slice(data);
        self.transfer = if end == SECTOR_SIZE {
            Transfer::Idle
        } else {
            Transfer::Write(sector, end)
        };
        Ok(())
    }
}
