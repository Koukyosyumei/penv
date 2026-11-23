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
const COMMAND_TIMEOUT: u32 = (1 << 15) | (1 << 16);

pub struct Controller {
    pub name: String,
    pub base: u32,
    irq: usize,
    card: Option<Card>,
    sd_selected: bool,
    phy_settings: u32,
    block_size_count: u32,
    argument: u32,
    command_mode: u32,
    response: [u32; 4],
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
    app_command: bool,
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
            block_size_count: 0,
            argument: 0,
            command_mode: 0,
            response: [0; 4],
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
            state: CardState::Idle,
        });
        Ok(())
    }

    pub fn select_sd(&mut self, selected: bool) {
        if self.sd_selected != selected {
            self.sd_selected = selected;
            self.cursor = SECTOR_SIZE;
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

    pub fn read(&mut self, address: u32, width: usize) -> Option<u32> {
        let offset = address.checked_sub(self.base)?;
        if width != 4 {
            return None;
        }
        Some(match offset {
            0x000 | 0x004 | 0x018 | 0x228 | 0x23c => 0,
            0x010 => self.phy_settings,
            0x240 => 0x0700_0184, // Voltage support and clock fields. DMA is unavailable.
            0x204 => self.block_size_count,
            0x208 => self.argument,
            0x20c => self.command_mode,
            0x210 | 0x214 | 0x218 | 0x21c => self.response[((offset - 0x210) / 4) as usize],
            0x220 => self.read_data(),
            0x224 => {
                0x00f2_0400
                    | if self.sd_selected && self.card.is_some() {
                        0x0005_0000
                    } else {
                        0
                    }
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
            // These offsets have no active fields in the upstream controller.
            (0x000 | 0x004 | 0x018 | 0x228 | 0x22c | 0x23c | 0x240, 4) => {}
            (0x010, 4) => {
                let acknowledge = u32::from(value & (3 << 24) != 0) << 26;
                self.phy_settings = (value & 0x0000_ff3f) | acknowledge;
            }
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
        self.response.fill(0);
        self.cursor = SECTOR_SIZE;
        let index = (self.command_mode >> 24) & 0x3f;
        let card = self.card.as_mut().filter(|_| self.sd_selected);
        let flags = match card {
            Some(card)
                if index == 17
                    && matches!(card.state, CardState::Transfer)
                    && (self.block_size_count & 0xfff) == SECTOR_SIZE as u32
                    && self.command_mode & 0x0020_0011 == 0x0020_0010 =>
            {
                if card.read_sector(self.argument, &mut self.sector).is_ok() {
                    card.app_command = false;
                    self.response[0] = card.status();
                    self.cursor = 0;
                    COMMAND_COMPLETE | BUFFER_READ_READY
                } else {
                    COMMAND_TIMEOUT
                }
            }
            Some(card) if index != 17 => match card.identify(index, self.argument) {
                Some(response) => {
                    self.response = response;
                    COMMAND_COMPLETE
                }
                None => COMMAND_TIMEOUT,
            },
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
    fn reset(&mut self) {
        self.app_command = false;
        self.state = CardState::Idle;
    }

    fn status(&self) -> u32 {
        (self.state as u32) << 9 | (1 << 8)
    }

    fn identify(&mut self, index: u32, argument: u32) -> Option<[u32; 4]> {
        let app_command = std::mem::take(&mut self.app_command);
        let short = match (app_command, index) {
            (_, 0) => {
                self.reset();
                0
            }
            (_, 8) => argument & 0xfff,
            (_, 55) => {
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
                CARD_ADDRESS | self.status()
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
        assert_eq!(sd.read(sd.base + 0x240, 4), Some(0x0700_0184));
    }
}
