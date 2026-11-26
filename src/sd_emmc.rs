//! In-memory eMMC card command and data interface.
use std::{collections::HashMap, io};

const SECTOR_SIZE: usize = 512;
const PARTITION_CONFIG: usize = 179;
const ACCESS_MASK: u8 = 7;
const SWITCH_ERROR: u32 = 1 << 7;
const PARTITION_SIZE_UNIT: u32 = 128 * 1024 / SECTOR_SIZE as u32;
const BOOT_SIZE_MULTIPLIER: u8 = 64;
const RPMB_SIZE_MULTIPLIER: u8 = 1;

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
    ExtendedCsd(usize),
}

pub struct Card {
    storage: [Partition; 4],
    active_partition: usize,
    extended_csd: [u8; SECTOR_SIZE],
    switch_error: bool,
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
        let mut extended_csd = [0; SECTOR_SIZE];
        extended_csd[160] = 7;
        extended_csd[168] = RPMB_SIZE_MULTIPLIER;
        extended_csd[192] = 8;
        extended_csd[196] = 2;
        extended_csd[212..216].copy_from_slice(&sectors.to_le_bytes());
        extended_csd[221] = 1;
        extended_csd[224] = 1;
        extended_csd[226] = BOOT_SIZE_MULTIPLIER;
        let boot = u32::from(BOOT_SIZE_MULTIPLIER) * PARTITION_SIZE_UNIT;
        Ok(Self {
            storage: [
                sectors,
                boot,
                boot,
                u32::from(RPMB_SIZE_MULTIPLIER) * PARTITION_SIZE_UNIT,
            ]
            .map(Partition::new),
            active_partition: 0,
            extended_csd,
            switch_error: false,
            transfer: Transfer::Idle,
        })
    }

    pub fn reset(&mut self) {
        self.active_partition = 0;
        self.extended_csd[PARTITION_CONFIG] = 0;
        self.switch_error = false;
        self.transfer = Transfer::Idle;
    }

    pub fn handle_command(&mut self, command: u8, argument: u32) -> io::Result<u32> {
        match command {
            0 => self.reset(),
            6 => self.switch(argument),
            8 => self.transfer = Transfer::ExtendedCsd(0),
            13 => {}
            17 | 24 => {
                self.storage[self.active_partition].check(argument, 0)?;
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
        Ok(if self.switch_error { SWITCH_ERROR } else { 0 })
    }

    fn switch(&mut self, argument: u32) {
        self.switch_error = false;
        if ((argument >> 16) & 0xff) as usize != PARTITION_CONFIG {
            return;
        }
        let previous = self.extended_csd[PARTITION_CONFIG];
        let value = (argument >> 8) as u8;
        let next = match (argument >> 24) & 3 {
            1 => previous | value,
            2 => previous & !value,
            3 => value,
            _ => return,
        };
        let target = usize::from(next & ACCESS_MASK);
        if target >= self.storage.len() {
            self.switch_error = true;
            return;
        }
        self.extended_csd[PARTITION_CONFIG] = next;
        self.active_partition = target;
        self.transfer = Transfer::Idle;
    }

    pub fn read_data(&mut self, len: usize) -> io::Result<Vec<u8>> {
        if let Transfer::ExtendedCsd(cursor) = self.transfer {
            let available = len.min(SECTOR_SIZE - cursor);
            let end = cursor + available;
            let mut data = Vec::new();
            data.try_reserve_exact(len).map_err(io::Error::other)?;
            data.extend_from_slice(&self.extended_csd[cursor..end]);
            data.resize(len, 0);
            self.transfer = if end == SECTOR_SIZE {
                Transfer::Idle
            } else {
                Transfer::ExtendedCsd(end)
            };
            return Ok(data);
        }
        let Transfer::Read(sector, cursor) = self.transfer else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "no active eMMC read",
            ));
        };
        let end = cursor.checked_add(len).ok_or(io::ErrorKind::InvalidInput)?;
        self.storage[self.active_partition].check(sector, end)?;
        let data = self.storage[self.active_partition]
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
        self.storage[self.active_partition].check(sector, end)?;
        self.storage[self.active_partition]
            .data
            .entry(sector)
            .or_insert([0; SECTOR_SIZE])[cursor..end]
            .copy_from_slice(data);
        self.transfer = if end == SECTOR_SIZE {
            Transfer::Idle
        } else {
            Transfer::Write(sector, end)
        };
        Ok(())
    }
}
