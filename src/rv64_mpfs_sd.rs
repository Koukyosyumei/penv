use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    path::Path,
};

pub const FIRE_MEDIA_SELECT: u32 = 0x4fff_ff00;

const SECTOR_SIZE: usize = 512;
const CSD_CAPACITY_UNIT: u64 = 512 * 1024;
const CARD_ADDRESS: u32 = 1 << 16;
const OCR_HIGH_CAPACITY: u32 = 1 << 30;
const OCR_READY: u32 = 1 << 31;
const COMMAND_COMPLETE: u32 = 1;
const TRANSFER_COMPLETE: u32 = 1 << 1;
const BUFFER_READ_READY: u32 = 1 << 5;
const DATA_TIMEOUT: u32 = 1 << 20;
const COMMAND_TIMEOUT: u32 = 1 << 16;

pub struct Controller {
    pub name: String,
    pub base: u32,
    irq: usize,
    card: Option<Card>,
    sd_selected: bool,
    phy_settings: u32,
    host_control: u32,
    clock_control: u32,
    host_control2: u32,
    block_size_count: u32,
    argument: u32,
    command_mode: u32,
    response: [u32; 4],
    status: u32,
    status_enable: u32,
    signal_enable: u32,
    sector: [u8; SECTOR_SIZE],
    cursor: usize,
    transfer_size: usize,
    dma_address: u64,
    dma_read: Option<DmaRead>,
    next_pio_block: Option<(u32, usize)>,
}

enum DmaRead {
    Blocks { sector: u32, size: usize },
    Buffer { size: usize },
}

struct Card {
    file: File,
    length: u64,
    capacity: u64,
    app_command: bool,
    mmc_protocol: bool,
    mmc_bus_width: u8,
    mmc_timing: u8,
    state: CardState,
}

#[derive(Clone, Copy)]
enum CardState {
    Idle = 0,
    Ready = 1,
    Identification = 2,
    Standby = 3,
    Transfer = 4,
}

impl Controller {
    pub const SIZE: u64 = 0x2000;

    pub fn new(name: String, base: u32, irq: usize) -> Self {
        Self {
            name,
            base,
            irq,
            card: None,
            sd_selected: true,
            phy_settings: 0,
            host_control: 0,
            clock_control: 2,
            host_control2: 0,
            block_size_count: 0,
            argument: 0,
            command_mode: 0,
            response: [0; 4],
            status: 0,
            status_enable: 0,
            signal_enable: 0,
            sector: [0; SECTOR_SIZE],
            cursor: 0,
            transfer_size: 0,
            dma_address: 0,
            dma_read: None,
            next_pio_block: None,
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
            || !(2 * 1024 * 1024 * 1024..=2 * 1024 * 1024 * 1024 * 1024).contains(&capacity)
            || !capacity.is_multiple_of(CSD_CAPACITY_UNIT)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid SD image or unsupported high-capacity SD size",
            ));
        }
        self.card = Some(Card {
            file,
            length,
            capacity,
            app_command: false,
            mmc_protocol: false,
            mmc_bus_width: 0,
            mmc_timing: 0,
            state: CardState::Idle,
        });
        Ok(())
    }

    pub fn select_sd(&mut self, selected: bool) {
        if self.sd_selected != selected {
            self.sd_selected = selected;
            self.transfer_size = 0;
            self.dma_read = None;
            self.next_pio_block = None;
            self.status = 0;
            if let Some(card) = &mut self.card {
                card.reset();
            }
        }
    }

    pub fn sd_selected(&self) -> bool {
        self.sd_selected
    }

    pub fn interrupt(&self) -> (usize, bool) {
        (self.irq, self.status & self.signal_enable != 0)
    }

    fn access(&self, address: u32, width: usize) -> Option<(u32, u32, u32)> {
        let offset = address.checked_sub(self.base)?;
        if !matches!(width, 1 | 2 | 4)
            || !(offset as usize).is_multiple_of(width)
            || u64::from(offset) >= Self::SIZE
        {
            return None;
        }
        let shift = (offset & 3) * 8;
        let mask = (u32::MAX >> (32 - width * 8)) << shift;
        Some((offset, shift, mask))
    }

    pub fn read(&mut self, address: u32, width: usize) -> Option<u32> {
        let (offset, shift, mask) = self.access(address, width)?;
        let word = match offset & !3 {
            0x010 => self.phy_settings,
            0x240 => 0x1740_0184, // Clock, voltage and 64-bit SDMA capabilities
            0x200 | 0x258 => self.dma_address as u32,
            0x25c => (self.dma_address >> 32) as u32,
            0x204 => self.block_size_count,
            0x208 => self.argument,
            0x20c => self.command_mode,
            0x210 | 0x214 | 0x218 | 0x21c => self.response[((offset - 0x210) / 4) as usize],
            0x220 => self.read_data(),
            0x224 => {
                0x00f2_0400
                    | if self.card.is_some() { 0x0005_0000 } else { 0 }
                    | if self.cursor < self.transfer_size {
                        1 << 11
                    } else {
                        0
                    }
            }
            0x228 => self.host_control,
            0x22c => self.clock_control,
            0x230 => self.status | (u32::from(self.status >> 16 != 0) << 15),
            0x234 => self.status_enable,
            0x238 => self.signal_enable,
            0x23c => self.host_control2,
            // Renode returns zero for unhandled register offsets.
            _ => 0,
        };
        Some((word & mask) >> shift)
    }

    pub fn write(&mut self, address: u32, width: usize, value: u32) -> Option<()> {
        let (offset, shift, mask) = self.access(address, width)?;
        let value = (value << shift) & mask;
        let update = |word: u32| (word & !mask) | value;
        match offset & !3 {
            0x010 => {
                let value = update(self.phy_settings);
                let acknowledge = u32::from(value & (3 << 24) != 0) << 26;
                self.phy_settings = (value & 0x0000_ff3f) | acknowledge;
            }
            0x200 | 0x258 => {
                self.dma_address = self.dma_address & 0xffff_ffff_0000_0000
                    | u64::from(update(self.dma_address as u32));
            }
            0x25c => {
                let address_high = update((self.dma_address >> 32) as u32);
                self.dma_address =
                    u64::from(address_high) << 32 | u64::from(self.dma_address as u32);
            }
            0x204 => self.block_size_count = update(self.block_size_count),
            0x208 => self.argument = update(self.argument),
            0x20c => {
                self.command_mode = update(self.command_mode);
                if mask & 0xff00_0000 != 0 {
                    self.command();
                }
            }
            0x228 => self.host_control = update(self.host_control),
            0x22c => self.write_clock_control(update(self.clock_control)),
            0x230 => self.status &= !value,
            0x234 => self.status_enable = update(self.status_enable),
            0x238 => self.signal_enable = update(self.signal_enable),
            0x23c => self.host_control2 = update(self.host_control2),
            _ => {}
        }
        Some(())
    }

    fn write_clock_control(&mut self, value: u32) {
        // The internal clock is stable and all requested reset lines complete
        // synchronously. Resetting the host leaves the card attached.
        self.clock_control = (value & 0x00ff_ffff) | 2;
        if value & (1 << 24) != 0 {
            self.clock_control = 2;
            self.host_control = 0;
            self.host_control2 = 0;
            self.block_size_count = 0;
            self.argument = 0;
            self.command_mode = 0;
            self.response.fill(0);
            self.status = 0;
            self.status_enable = 0;
            self.signal_enable = 0;
            self.dma_address = 0;
        }
        if value & (5 << 24) != 0 {
            self.transfer_size = 0;
            self.dma_read = None;
            self.next_pio_block = None;
        }
    }

    fn command(&mut self) {
        self.response.fill(0);
        self.transfer_size = 0;
        self.dma_read = None;
        self.next_pio_block = None;
        let index = (self.command_mode >> 24) & 0x3f;
        let card = self.card.as_mut();
        let flags = match card {
            Some(card)
                if matches!(index, 17 | 18)
                    && self.command_mode & 0x0020_0011 == 0x0020_0011
                    && matches!(card.state, CardState::Transfer)
                    && (self.block_size_count & 0xfff) == SECTOR_SIZE as u32 =>
            {
                let blocks = if index == 17 {
                    1
                } else {
                    (self.block_size_count >> 16) as usize
                };
                if blocks == 0 {
                    DATA_TIMEOUT
                } else {
                    self.dma_read = Some(DmaRead::Blocks {
                        sector: self.argument,
                        size: blocks * SECTOR_SIZE,
                    });
                    self.response[0] = card.status();
                    COMMAND_COMPLETE
                }
            }
            Some(card)
                if matches!(index, 17 | 18)
                    && matches!(card.state, CardState::Transfer)
                    && (self.block_size_count & 0xfff) == SECTOR_SIZE as u32
                    && self.command_mode & 0x0020_0011 == 0x0020_0010 =>
            {
                let blocks = if index == 17 {
                    1
                } else {
                    (self.block_size_count >> 16) as usize
                };
                if blocks != 0
                    && card.contains(self.argument, blocks * SECTOR_SIZE)
                    && card.read_blocks(self.argument, &mut self.sector).is_ok()
                {
                    self.next_pio_block = (blocks > 1).then(|| (self.argument + 1, blocks - 1));
                    card.app_command = false;
                    self.response[0] = card.status();
                    self.cursor = 0;
                    self.transfer_size = SECTOR_SIZE;
                    COMMAND_COMPLETE | BUFFER_READ_READY
                } else {
                    COMMAND_TIMEOUT
                }
            }
            // Renode attaches a single card directly to the controller. The
            // Fire HSS can use it through its legacy MMC command path.
            Some(card)
                if card.mmc_protocol
                    && index == 8
                    && self.command_mode & 0x0020_0011 == 0x0020_0010
                    && matches!(card.state, CardState::Transfer) =>
            {
                self.response[0] = card.status();
                self.sector.fill(0);
                self.sector[183] = card.mmc_bus_width;
                self.sector[185] = card.mmc_timing;
                self.sector[192] = 8; // EXT_CSD revision
                self.sector[196] = 0x12; // Legacy Renode card: 52 MHz and HS200 at 1.8 V
                let sectors = (card.capacity / SECTOR_SIZE as u64).min(u64::from(u32::MAX));
                self.sector[212..216].copy_from_slice(&(sectors as u32).to_le_bytes());
                self.cursor = 0;
                self.transfer_size = SECTOR_SIZE;
                COMMAND_COMPLETE | BUFFER_READ_READY
            }
            Some(card)
                if card.mmc_protocol
                    && index == 21
                    && self.command_mode & 0x0020_0011 == 0x0020_0010
                    && matches!(card.state, CardState::Transfer) =>
            {
                let size = (self.block_size_count & 0xfff) as usize;
                if let Some(pattern) = tuning_pattern(size) {
                    self.response[0] = card.status();
                    for (chunk, word) in self.sector.chunks_exact_mut(4).zip(pattern) {
                        chunk.copy_from_slice(&word.to_le_bytes());
                    }
                    self.cursor = 0;
                    self.transfer_size = size;
                    COMMAND_COMPLETE | BUFFER_READ_READY
                } else {
                    COMMAND_TIMEOUT
                }
            }
            Some(card)
                if !card.mmc_protocol
                    && matches!(card.state, CardState::Transfer)
                    && self.command_mode & 0x0020_0010 == 0x0020_0010
                    && ((matches!(index, 13 | 51) && card.app_command)
                        || (index == 6 && !card.app_command)) =>
            {
                card.app_command = false;
                self.response[0] = card.status();
                self.sector.fill(0);
                let size = if index == 51 {
                    self.sector[0] = 2; // SCR: SD specification version 2.0
                    self.sector[1] = 5; // 1-bit and 4-bit data bus
                    8
                } else {
                    if index == 6 {
                        self.sector[13] = 1; // Switch status: default speed only
                    }
                    64
                };
                if self.command_mode & 1 != 0 {
                    self.dma_read = Some(DmaRead::Buffer { size });
                    COMMAND_COMPLETE
                } else {
                    self.cursor = 0;
                    self.transfer_size = size;
                    COMMAND_COMPLETE | BUFFER_READ_READY
                }
            }
            Some(card) if !matches!(index, 17 | 18) => {
                match card.identify(index, self.argument, !self.sd_selected) {
                    Some(response) => {
                        self.response = response;
                        COMMAND_COMPLETE
                            | if (self.command_mode >> 16) & 3 == 3 {
                                TRANSFER_COMPLETE
                            } else {
                                0
                            }
                    }
                    None => COMMAND_TIMEOUT,
                }
            }
            _ => COMMAND_TIMEOUT,
        };
        self.status |= flags & self.status_enable;
    }

    fn complete_dma_read(&mut self, bus: &mut crate::bus::Bus) {
        let Some(request) = self.dma_read.take() else {
            return;
        };
        let success = match request {
            DmaRead::Blocks { sector, size } => {
                let card = self.card.as_mut().unwrap();
                card.contains(sector, size)
                    && bus
                        .memory64(self.dma_address, size)
                        .is_some_and(|buffer| card.read_blocks(sector, buffer).is_ok())
            }
            DmaRead::Buffer { size } => {
                bus.memory64(self.dma_address, size).is_some_and(|buffer| {
                    buffer.copy_from_slice(&self.sector[..size]);
                    true
                })
            }
        };
        let flags = if success {
            TRANSFER_COMPLETE
        } else {
            DATA_TIMEOUT
        };
        self.status |= flags & self.status_enable;
    }

    fn read_data(&mut self) -> u32 {
        let end = self.cursor + 4;
        let Some(bytes) = self.sector[..self.transfer_size].get(self.cursor..end) else {
            return 0;
        };
        let value = u32::from_le_bytes(bytes.try_into().unwrap());
        self.cursor = end;
        if end == self.transfer_size {
            if let Some((sector, remaining)) = self.next_pio_block.take() {
                let card = self.card.as_mut().unwrap();
                if card.read_blocks(sector, &mut self.sector).is_ok() {
                    self.cursor = 0;
                    self.next_pio_block = (remaining > 1).then(|| (sector + 1, remaining - 1));
                    self.status |= BUFFER_READ_READY & self.status_enable;
                } else {
                    self.status |= DATA_TIMEOUT & self.status_enable;
                }
            } else {
                self.status |= TRANSFER_COMPLETE & self.status_enable;
            }
        }
        value
    }
}

// eMMC tuning blocks used by the Microchip PolarFire MSS MMC driver
// (mss_mmc.c, calc_write_pattern, MIT).
fn tuning_pattern(size: usize) -> Option<&'static [u32]> {
    const BUS_4: &[u32] = &[
        0x00ff0fff, 0xccc3ccff, 0xffcc3cc3, 0xeffefffe, 0xddffdfff, 0xfbfffbff, 0xff7fffbf,
        0xefbdf777, 0xf0fff0ff, 0x3cccfc0f, 0xcfcc33cc, 0xeeffefff, 0xfdfffdff, 0xffbfffdf,
        0xfff7ffbb, 0xde7b7ff7,
    ];
    const BUS_8: &[u32] = &[
        0xff00ffff, 0x0000ffff, 0xccccffff, 0xcccc33cc, 0xcc3333cc, 0xffffcccc, 0xffffeeff,
        0xffeeeeff, 0xffddffff, 0xddddffff, 0xbbffffff, 0xbbffffff, 0xffffffbb, 0xffffff77,
        0x77ff7777, 0xffeeddbb, 0x00ffffff, 0x00ffffff, 0xccffff00, 0xcc33cccc, 0x3333cccc,
        0xffcccccc, 0xffeeffff, 0xeeeeffff, 0xddffffff, 0xddffffff, 0xffffffdd, 0xffffffbb,
        0xffffbbbb, 0xffff77ff, 0xff7777ff, 0xeeddbb77,
    ];
    match size {
        64 => Some(BUS_4),
        128 => Some(BUS_8),
        _ => None,
    }
}

impl Card {
    fn reset(&mut self) {
        self.app_command = false;
        self.mmc_protocol = false;
        self.mmc_bus_width = 0;
        self.mmc_timing = 0;
        self.state = CardState::Idle;
    }

    fn status(&self) -> u32 {
        (self.state as u32) << 9 | (1 << 8)
    }

    fn identify(&mut self, index: u32, argument: u32, legacy_mmc: bool) -> Option<[u32; 4]> {
        let app_command = std::mem::take(&mut self.app_command);
        let short = match (app_command, index) {
            (_, 0) => {
                self.reset();
                0
            }
            (_, 8) => argument & 0xfff,
            (_, 1) if legacy_mmc => {
                self.mmc_protocol = true;
                self.state = CardState::Ready;
                OCR_READY | OCR_HIGH_CAPACITY | 0x00ff_8080
            }
            (_, 6) if self.mmc_protocol && matches!(self.state, CardState::Transfer) => {
                if argument >> 24 & 3 == 3 {
                    match (argument >> 16) & 0xff {
                        183 => self.mmc_bus_width = (argument >> 8) as u8,
                        185 => self.mmc_timing = (argument >> 8) as u8,
                        _ => {}
                    }
                }
                self.status()
            }
            (true, 6) if !self.mmc_protocol => self.status(),
            (_, 55) if !self.mmc_protocol => {
                self.app_command = true;
                self.status() | 1 << 5
            }
            (true, 41) => {
                let ready = argument & OCR_HIGH_CAPACITY != 0;
                if ready {
                    self.state = CardState::Ready;
                }
                0x00ff_8000 | OCR_HIGH_CAPACITY | if ready { OCR_READY } else { 0 }
            }
            (_, 2) if matches!(self.state, CardState::Ready) => {
                self.state = CardState::Identification;
                return Some(Self::long_response(
                    0xab00_0052_454e_4f44_0000_0000_0001_2800,
                ));
            }
            (_, 3) if matches!(self.state, CardState::Identification) => {
                self.state = CardState::Standby;
                if self.mmc_protocol {
                    self.status()
                } else {
                    CARD_ADDRESS | self.status()
                }
            }
            (_, 7)
                if (argument == 0 || argument == CARD_ADDRESS)
                    && matches!(self.state, CardState::Standby | CardState::Transfer) =>
            {
                self.state = if argument == 0 {
                    CardState::Standby
                } else {
                    CardState::Transfer
                };
                self.status()
            }
            (_, 9) if argument == CARD_ADDRESS => {
                let size = u128::from(self.capacity / CSD_CAPACITY_UNIT - 1);
                let csd =
                    (1 << 126) | (0x32 << 96) | (0x15 << 84) | (9 << 80) | (size << 48) | (9 << 22);
                return Some(Self::long_response(csd));
            }
            (_, 12) if matches!(self.state, CardState::Transfer) => self.status(),
            (_, 13) if argument == CARD_ADDRESS => self.status(),
            (_, 16) if argument == SECTOR_SIZE as u32 => self.status(),
            _ => return None,
        };
        Some([short, 0, 0, 0])
    }

    fn long_response(register: u128) -> [u32; 4] {
        // SDHCI strips the low CRC/end byte and exposes the result low word first.
        let response = register >> 8;
        std::array::from_fn(|index| (response >> (index * 32)) as u32)
    }

    fn contains(&self, sector: u32, size: usize) -> bool {
        u64::from(sector) * SECTOR_SIZE as u64 + size as u64 <= self.capacity
    }

    fn read_blocks(&mut self, sector: u32, buffer: &mut [u8]) -> io::Result<()> {
        let offset = u64::from(sector) * SECTOR_SIZE as u64;
        if !self.contains(sector, buffer.len()) {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "SD sector outside capacity",
            ));
        }
        buffer.fill(0);
        let stored = self.length.saturating_sub(offset).min(buffer.len() as u64) as usize;
        if stored != 0 {
            self.file.seek(SeekFrom::Start(offset))?;
            self.file.read_exact(&mut buffer[..stored])?;
        }
        Ok(())
    }
}

impl crate::bus::Bus {
    pub fn write_rv64_sd(&mut self, address: u32, width: usize, value: u32) -> bool {
        let Some(index) = self
            .rv64_devices
            .mpfs_sd
            .iter_mut()
            .position(|sd| sd.write(address, width, value).is_some())
        else {
            return false;
        };
        if self.rv64_devices.mpfs_sd[index].dma_read.is_some() {
            let mut controllers = std::mem::take(&mut self.rv64_devices.mpfs_sd);
            controllers[index].complete_dma_read(self);
            self.rv64_devices.mpfs_sd = controllers;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::Controller;

    #[test]
    fn host_reset_and_unattached_phy_complete_without_waiting() {
        let mut sd = Controller::new("mmc".into(), 0x2000_8000, 88);
        assert_eq!(sd.write(sd.base, 4, 1), Some(()));
        assert_eq!(sd.read(sd.base, 4), Some(0));
        assert_eq!(sd.write(sd.base + 0x10, 4, 0x0100_1234), Some(()));
        assert_eq!(sd.read(sd.base + 0x10, 4), Some(0x0400_1234));
        assert_eq!(sd.write(sd.base + 0x10, 4, 0x0000_1234), Some(()));
        assert_eq!(sd.read(sd.base + 0x10, 4), Some(0x0000_1234));
        assert_eq!(sd.read(sd.base + 0x240, 4), Some(0x1740_0184));
    }

    #[test]
    fn sdhci_byte_and_halfword_accesses_preserve_adjacent_fields() {
        let mut sd = Controller::new("mmc".into(), 0x2000_8000, 88);
        let base = sd.base;
        assert_eq!(sd.read(base + 0x2fe, 2), Some(0));
        assert_eq!(sd.read(base + 0x22d, 2), None);
        sd.write(base + 0x228, 1, 0x21).unwrap();
        sd.write(base + 0x229, 1, 0x0f).unwrap();
        assert_eq!(sd.read(base + 0x228, 2), Some(0x0f21));
        sd.write(base + 0x22c, 2, 5).unwrap();
        sd.write(base + 0x22e, 1, 0xe).unwrap();
        assert_eq!(sd.read(base + 0x22c, 4), Some(0x000e_0007));
        sd.write(base + 0x22f, 1, 1).unwrap();
        assert_eq!(sd.read(base + 0x22c, 4), Some(2));
        assert_eq!(sd.read(base + 0x228, 2), Some(0));
    }

    #[test]
    fn fire_legacy_mmc_path_reports_the_attached_sd_capacity() {
        let path =
            std::env::temp_dir().join(format!("penv-fire-legacy-mmc-{}", std::process::id()));
        std::fs::write(&path, [0u8; 512]).unwrap();
        let mut sd = Controller::new("mmc".into(), 0x2000_8000, 88);
        sd.select_sd(false);
        let result = sd.attach(&path, 0x1_0000_0000);
        std::fs::remove_file(path).unwrap();
        result.unwrap();
        let base = sd.base;
        sd.write(base + 0x234, 4, u32::MAX).unwrap();
        sd.write(base + 0x20c, 4, 0).unwrap(); // CMD0
        sd.write(base + 0x20c, 4, 0x0102_0000).unwrap(); // CMD1
        assert_eq!(sd.read(base + 0x210, 4), Some(0xc0ff_8080));
        sd.write(base + 0x20c, 4, 0x0201_0000).unwrap(); // CMD2
        sd.write(base + 0x208, 4, 0x10000).unwrap();
        sd.write(base + 0x20c, 4, 0x0302_0000).unwrap(); // CMD3
        sd.write(base + 0x20c, 4, 0x0702_0000).unwrap(); // CMD7
        sd.write(base + 0x208, 4, 0x03b9_0200).unwrap();
        sd.write(base + 0x20c, 4, 0x0603_0000).unwrap(); // CMD6: HS200
        sd.write(base + 0x230, 4, u32::MAX).unwrap();
        sd.write(base + 0x20c, 4, 0x0822_0010).unwrap(); // CMD8 with data
        assert_eq!(sd.read(base + 0x230, 4), Some(0x21));
        let ext_csd: Vec<_> = (0..128)
            .flat_map(|_| sd.read(base + 0x220, 4).unwrap().to_le_bytes())
            .collect();
        assert_eq!(ext_csd[185], 2);
        assert_eq!(ext_csd[212..216], (0x0080_0000u32).to_le_bytes());
        assert_eq!(sd.read(base + 0x230, 4), Some(0x23));
        sd.write(base + 0x230, 4, u32::MAX).unwrap();
        sd.write(base + 0x204, 4, 128).unwrap();
        sd.write(base + 0x20c, 4, 0x1522_0010).unwrap(); // CMD21 with data
        assert_eq!(sd.read(base + 0x220, 4), Some(0xff00_ffff));
        for _ in 1..32 {
            sd.read(base + 0x220, 4).unwrap();
        }
        assert_eq!(sd.read(base + 0x230, 4), Some(0x23));
        assert_eq!(sd.read(base + 0x220, 4), Some(0));
    }

    #[test]
    fn sdma_copies_multiple_blocks_and_reports_unmapped_ram() {
        let path = std::env::temp_dir().join(format!("penv-fire-sdma-{}", std::process::id()));
        let image = [[0x22u8; 512], [0x55; 512]].concat();
        std::fs::write(&path, &image).unwrap();
        let mut sd = Controller::new("mmc".into(), 0x2000_8000, 88);
        let result = sd.attach(&path, 0x1_0000_0000);
        std::fs::remove_file(path).unwrap();
        result.unwrap();
        sd.card.as_mut().unwrap().state = super::CardState::Transfer;
        let base = sd.base;
        let address = 0x1_0000_0100;
        let mut bus = crate::bus::Bus::new(0x1000).unwrap();
        bus.map_ram64(0x1_0000_0000, 0x1000).unwrap();
        bus.rv64_devices.mpfs_sd.push(sd);
        for (offset, value) in [
            (0x234, u32::MAX),
            (0x238, 2),
            (0x258, address as u32),
            (0x25c, (address >> 32) as u32),
            (0x204, 0x0002_0200),
            (0x20c, 0x1222_0011),
        ] {
            assert!(bus.write_rv64_sd(base + offset, 4, value));
        }
        assert_eq!(bus.ram64(address, image.len()).unwrap(), image);
        assert_eq!(bus.rv64_devices.mpfs_sd[0].interrupt(), (88, true));
        assert_eq!(bus.rv64_devices.read(base + 0x230, 4), Some(3));

        assert!(bus.write_rv64_sd(base + 0x230, 4, 3));
        assert!(bus.write_rv64_sd(base + 0x25c, 4, 2));
        assert!(bus.write_rv64_sd(base + 0x20c, 4, 0x1222_0011));
        assert_eq!(bus.rv64_devices.read(base + 0x230, 4), Some(0x108001));
        assert_eq!(bus.rv64_devices.mpfs_sd[0].interrupt(), (88, false));

        assert!(bus.write_rv64_sd(base + 0x230, 4, u32::MAX));
        assert!(bus.write_rv64_sd(base + 0x20c, 4, 0x1222_0010));
        for expected in [0x2222_2222, 0x5555_5555] {
            for _ in 0..128 {
                assert_eq!(bus.rv64_devices.read(base + 0x220, 4), Some(expected));
            }
        }
        assert_eq!(bus.rv64_devices.read(base + 0x230, 4), Some(0x23));

        assert!(bus.write_rv64_sd(base + 0x25c, 4, 1));
        assert!(bus.write_rv64_sd(base + 0x234, 4, u32::MAX));
        assert!(bus.write_rv64_sd(base + 0x230, 4, u32::MAX));
        assert!(bus.write_rv64_sd(base + 0x20c, 4, 0x3702_0000));
        assert!(bus.write_rv64_sd(base + 0x204, 4, 8));
        assert!(bus.write_rv64_sd(base + 0x20c, 4, 0x3322_0011));
        assert_eq!(bus.ram64(address, 8).unwrap(), [2, 5, 0, 0, 0, 0, 0, 0]);
        assert_eq!(bus.rv64_devices.read(base + 0x230, 4), Some(3));

        assert!(bus.write_rv64_sd(base + 0x230, 4, u32::MAX));
        assert!(bus.write_rv64_sd(base + 0x20c, 4, 0x3702_0000));
        assert!(bus.write_rv64_sd(base + 0x204, 4, 64));
        assert!(bus.write_rv64_sd(base + 0x20c, 4, 0x0d22_0011));
        assert_eq!(bus.ram64(address, 64).unwrap(), [0; 64]);
        assert_eq!(bus.rv64_devices.read(base + 0x230, 4), Some(3));
    }
}
