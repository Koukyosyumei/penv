use std::collections::VecDeque;

const COMMAND_SENT: u32 = 1 << 7;
const WAIT_RESPONSE: u32 = 3 << 8;
const COMMAND_TIMEOUT: u32 = 1 << 2;
const RESPONSE_END: u32 = 1 << 6;
const DATA_END: u32 = 1 << 8;
const BLOCK_END: u32 = 1 << 10;
const RX_HALF_FULL: u32 = 1 << 15;
const RX_EMPTY: u32 = 1 << 19;
const BLOCK_SIZE: usize = 512;
const CAPACITY_UNIT: usize = 1024 * BLOCK_SIZE;
const CARD_ADDRESS: u32 = 1 << 16;
const START_COMMAND: u32 = 1 << 12;

pub struct Sdmmc {
    pub base: u32,
    pub irq: Option<u8>,
    registers: [u32; 24],
    status: u32,
    image: Option<Vec<u8>>,
    app_command: bool,
    card_state: u32,
    fifo: VecDeque<u32>,
}

impl Sdmmc {
    pub const MODEL: &str = "SD.STM32HSDMMC";
    pub const SIZE: u32 = 0x2000;

    pub fn new(base: u32, irq: Option<u8>) -> Self {
        Self {
            base,
            irq,
            registers: [0; 24],
            status: 0,
            image: None,
            app_command: false,
            card_state: 0,
            fifo: VecDeque::new(),
        }
    }

    pub fn reset(&mut self) {
        let image = self.image.take();
        *self = Self::new(self.base, self.irq);
        self.image = image;
    }

    pub fn attach(&mut self, image: Vec<u8>) -> Result<(), String> {
        if image.is_empty() || image.len() % CAPACITY_UNIT != 0 {
            return Err("SDHC image size must be a nonzero multiple of 512 KiB".into());
        }
        self.image = Some(image);
        Ok(())
    }

    pub(crate) fn attach_file(
        &mut self,
        path: &std::path::Path,
        size: usize,
    ) -> Result<(), String> {
        use std::io::Read;

        if path.as_os_str().is_empty() {
            return Err("No image file provided.".into());
        }
        let file = std::fs::File::open(path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                format!("Could not find image file '{}'", path.display())
            } else {
                format!("{}: {error}", path.display())
            }
        })?;
        if size == 0 {
            return Err("SD image size must be nonzero".into());
        }
        let capacity = size
            .div_ceil(CAPACITY_UNIT)
            .checked_mul(CAPACITY_UNIT)
            .ok_or("SD image capacity exceeds host address space")?;
        let mut image = Vec::new();
        image
            .try_reserve_exact(capacity)
            .map_err(|_| "cannot allocate SD image")?;
        file.take(size as u64)
            .read_to_end(&mut image)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        image.resize(capacity, 0);
        self.attach(image)?;
        self.reset();
        Ok(())
    }

    fn flags(&self) -> u32 {
        self.status
            | if self.fifo.is_empty() {
                RX_EMPTY
            } else if self.fifo.len() >= 8 {
                RX_HALF_FULL
            } else {
                0
            }
    }

    fn command(&mut self, value: u32) {
        let command = value & 63;
        let arg = self.registers[2];
        let app = std::mem::replace(&mut self.app_command, false);
        if value & WAIT_RESPONSE == 0 {
            if command == 0 {
                self.card_state = 0;
                self.fifo.clear();
            }
            self.status |= COMMAND_SENT;
            return;
        }
        let Some(image) = self.image.as_ref() else {
            self.status |= COMMAND_TIMEOUT;
            return;
        };
        let r1 = |state: u32| (state << 9) | (1u32 << 8);
        let mut response = [0; 4];
        let mut data = None;
        match (app, command) {
            (true, 41) => {
                self.card_state = 1;
                response[0] = 0xc0ff8000;
            }
            (true, 51) => data = Some(&[0x02, 0x05, 0, 0, 0, 0, 0, 0][..]),
            (true, 13) => data = Some(&[0; 64][..]),
            (true, 6) => response[0] = r1(self.card_state),
            (_, 2 | 10) => {
                self.card_state = 2;
                response = [0x0350454e, 0x56534430, 0x10000000, 0x010191ff];
            }
            (_, 3) => {
                self.card_state = 3;
                response[0] = CARD_ADDRESS | r1(self.card_state);
            }
            (_, 7) => {
                self.card_state = if arg == CARD_ADDRESS { 4 } else { 3 };
                response[0] = r1(self.card_state);
            }
            (_, 8) => response[0] = arg & 0xfff,
            (_, 9) => {
                let size = (image.len() / CAPACITY_UNIT) as u32 - 1;
                response = [
                    0x400e0032,
                    0x5b590000 | (size >> 16),
                    (size << 16) | 0x7f80,
                    0x0a4010ff,
                ];
            }
            (_, 12 | 13 | 16) => response[0] = r1(self.card_state),
            (_, 17 | 18) => {
                let start = u64::from(arg) * BLOCK_SIZE as u64;
                let length = if command == 17 {
                    BLOCK_SIZE
                } else {
                    self.registers[0x28 / 4] as usize
                };
                let bytes = usize::try_from(start).ok().and_then(|start| {
                    start
                        .checked_add(length)
                        .and_then(|end| image.get(start..end))
                });
                if let Some(bytes) =
                    bytes.filter(|bytes| !bytes.is_empty() && bytes.len() % BLOCK_SIZE == 0)
                {
                    response[0] = r1(self.card_state);
                    data = Some(bytes);
                } else {
                    response[0] = r1(self.card_state) | (1 << 31);
                }
            }
            (_, 24 | 25) => response[0] = r1(self.card_state) | (1 << 26),
            (_, 55) => {
                self.app_command = true;
                response[0] = r1(self.card_state) | (1 << 5);
            }
            _ => response[0] = r1(self.card_state) | (1 << 22),
        }
        self.registers[0x10 / 4] = command;
        self.registers[0x14 / 4..=0x20 / 4].copy_from_slice(&response);
        self.status |= RESPONSE_END;
        if let Some(bytes) = data {
            self.status &= !(DATA_END | BLOCK_END);
            self.fifo.clear();
            self.fifo.extend(
                bytes
                    .chunks_exact(4)
                    .map(|word| u32::from_le_bytes(word.try_into().unwrap())),
            );
        }
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    pub fn interrupt(&self) -> Option<u8> {
        self.irq
            .filter(|_| self.flags() & self.registers[0x3c / 4] != 0)
    }

    pub fn read(&mut self, address: u32) -> u32 {
        let offset = address - self.base;
        match offset {
            0x30 => self.fifo.len() as u32 * 4,
            0x34 => self.flags(),
            0x80 => match self.fifo.pop_front() {
                Some(word) => {
                    if self.fifo.is_empty() {
                        self.status |= DATA_END | BLOCK_END;
                    }
                    word
                }
                None => 0,
            },
            _ => self
                .registers
                .get((offset / 4) as usize)
                .copied()
                .unwrap_or(0),
        }
    }

    pub fn write(&mut self, address: u32, value: u32) {
        let offset = address - self.base;
        let register = (offset / 4) as usize;
        match offset {
            0x00 => {
                self.registers[register] = value & 3;
                if value & 3 == 0 {
                    self.status = 0;
                }
            }
            0x0c => {
                self.registers[register] = value & (0x1ffff & !START_COMMAND);
                if value & START_COMMAND != 0 {
                    self.command(value);
                }
            }
            0x38 => self.status &= !value,
            0x04 | 0x08 | 0x24 | 0x28 | 0x2c | 0x3c | 0x50..=0x5c => {
                self.registers[register] = value;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_slot_times_out_commands_and_clears_the_masked_irq() {
        let mut controller = Sdmmc::new(0, Some(49));
        controller.write(0, 3);
        controller.write(0x0c, START_COMMAND);
        assert_eq!(controller.read(0x34) & !RX_EMPTY, COMMAND_SENT);
        controller.write(0x38, COMMAND_SENT);
        controller.write(0x0c, START_COMMAND | (1 << 8) | 8);
        assert_eq!(controller.read(0x34) & !RX_EMPTY, COMMAND_TIMEOUT);
        assert_eq!(controller.read(0x0c) & START_COMMAND, 0);
        assert_eq!(controller.interrupt(), None);
        controller.write(0x3c, COMMAND_TIMEOUT);
        assert_eq!(controller.interrupt(), Some(49));
        controller.write(0x38, COMMAND_TIMEOUT);
        assert_eq!(controller.read(0x34) & !RX_EMPTY, 0);
        assert_eq!(controller.interrupt(), None);
    }

    #[test]
    fn card_reads_sector_bytes_and_keeps_image_across_reset() {
        let mut controller = Sdmmc::new(0, Some(49));
        let mut image = vec![0; CAPACITY_UNIT];
        image[512..516].copy_from_slice(&[1, 2, 3, 4]);
        controller.attach(image).unwrap();
        controller.reset();
        controller.write(0x08, 1);
        controller.write(0x3c, RX_HALF_FULL | DATA_END);
        controller.write(0x0c, START_COMMAND | (1 << 8) | 17);
        assert_eq!(controller.interrupt(), Some(49));
        assert_eq!(controller.read(0x80), 0x04030201);
        for _ in 1..128 {
            assert_eq!(controller.read(0x80), 0);
        }
        assert_ne!(controller.read(0x34) & DATA_END, 0);
        controller.write(0x38, DATA_END);
        assert_eq!(controller.interrupt(), None);
        controller.write(0x08, 1024);
        controller.write(0x0c, START_COMMAND | (1 << 8) | 17);
        assert_ne!(controller.read(0x14) & (1 << 31), 0);
        assert_ne!(controller.read(0x34) & RX_EMPTY, 0);
    }
}
