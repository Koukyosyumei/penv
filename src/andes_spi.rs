use std::collections::VecDeque;

pub(crate) struct AndesSpi {
    pub base: u32,
    pub irq: Option<(usize, u32)>,
    name: String,
    registers: [u32; 32],
    receive: VecDeque<u8>,
    flash: Option<PuyaFlash>,
    address_assigned: bool,
    tx_count: u64,
    rx_count: u64,
}

impl AndesSpi {
    pub const SIZE: u32 = 0x100;
    const FORMAT: usize = 0x10 / 4;
    const TX_COUNT: usize = 0x18 / 4;
    const RX_COUNT: usize = 0x1c / 4;
    const TRANSFER: usize = 0x20 / 4;
    const ADDRESS: usize = 0x28 / 4;
    const CONTROL: usize = 0x30 / 4;
    const INT_ENABLE: usize = 0x38 / 4;
    const INT_STATUS: usize = 0x3c / 4;
    const RX_THRESHOLD: u32 = 4;
    const TX_THRESHOLD: u32 = 8;
    const END: u32 = 16;

    pub fn new(
        base: u32,
        name: String,
        irq: Option<(usize, u32)>,
        flash_size: Option<usize>,
    ) -> Self {
        Self {
            base,
            name,
            irq,
            registers: [0; 32],
            receive: VecDeque::new(),
            flash: flash_size.map(PuyaFlash::new),
            address_assigned: false,
            tx_count: 0,
            rx_count: 0,
        }
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    pub fn interrupt(&self) -> bool {
        self.registers[Self::INT_ENABLE] & self.registers[Self::INT_STATUS] & 0x1c != 0
    }

    fn mode(&self) -> u32 {
        (self.registers[Self::TRANSFER] >> 24) & 15
    }

    fn flag(&mut self, flag: u32, enabled: bool) {
        self.registers[Self::INT_STATUS] &= !flag;
        if enabled {
            self.registers[Self::INT_STATUS] |= flag;
        }
    }

    fn warn(&self, message: &str) {
        crate::monitor_log::record(2, &format!("{}: {message}", self.name));
    }

    fn transfer(&mut self, byte: u8) {
        let Some(flash) = &mut self.flash else { return };
        let received = flash.transfer(byte);
        let mode = self.mode();
        if matches!(mode, 0 | 2) {
            self.receive.push_back(received);
        }
        self.flag(Self::TX_THRESHOLD, true);
        self.flag(
            Self::RX_THRESHOLD,
            mode == 0
                && self.receive.len() >= ((self.registers[Self::CONTROL] >> 8) & 0xff) as usize,
        );
        self.flag(
            Self::END,
            mode == 1 && self.tx_count == u64::from(self.registers[Self::TX_COUNT]) + 1,
        );
        self.tx_count += 1;
    }

    fn command(&mut self, byte: u8) {
        let transfer = self.registers[Self::TRANSFER];
        if self.flash.is_none() {
            self.warn(
                "Trying to issue a transaction to a slave peripheral, but nothing is connected",
            );
        } else if self.registers[Self::FORMAT] & 4 != 0 {
            self.warn("Command mode is available only for Master mode.");
        } else if self.address_assigned && transfer & (1 << 29) == 0 {
            self.warn("Address mode is not enabled, ignoring command.");
        } else if !matches!(self.mode(), 0 | 1 | 2 | 7)
            || (self.mode() == 0
                && self.registers[Self::TX_COUNT] != self.registers[Self::RX_COUNT])
        {
            self.warn("Invalid SPI mode or transfer counts");
        } else if self.registers[Self::CONTROL] & 0x18 != 0 {
            self.warn("SPI DMA is unsupported");
        } else if transfer & (1 << 30) != 0 {
            self.transfer(byte);
            if transfer & (1 << 29) != 0 {
                let bytes = self.registers[Self::ADDRESS].to_le_bytes();
                for byte in &bytes[..3] {
                    self.transfer(*byte);
                }
                self.address_assigned = false;
            }
        } else if let Some(flash) = &mut self.flash {
            flash.transfer(byte);
            self.flag(Self::TX_THRESHOLD, true);
        }
    }

    pub fn read(&mut self, address: u32) -> u32 {
        let offset = address - self.base;
        match offset {
            0 => 0x0200_2000,
            0x2c => {
                let Some(byte) = self.receive.pop_front() else {
                    return 0;
                };
                let threshold = ((self.registers[Self::CONTROL] >> 8) & 0xff) as usize;
                self.flag(Self::RX_THRESHOLD, self.receive.len() >= threshold);
                self.flag(
                    Self::END,
                    self.receive.is_empty()
                        || (self.mode() == 2
                            && self.rx_count == u64::from(self.registers[Self::RX_COUNT]) + 1),
                );
                self.rx_count += 1;
                u32::from(byte)
            }
            0x34 => {
                (self.receive.len().min(31) as u32) << 8
                    | (u32::from(self.receive.is_empty()) << 14)
                    | (u32::from(self.receive.len() == 32) << 15)
                    | (1 << 22)
            }
            0x7c => 0x44,
            offset if offset % 4 == 0 => self
                .registers
                .get((offset / 4) as usize)
                .copied()
                .unwrap_or(0),
            _ => 0,
        }
    }

    pub fn write(&mut self, address: u32, value: u32) {
        match address - self.base {
            0x10 => {
                self.registers[Self::FORMAT] = value & 0x31f9f;
                if let Some(flash) = &mut self.flash {
                    flash.finish();
                }
            }
            0x18 => {
                self.registers[Self::TX_COUNT] = value;
                self.tx_count = 0;
            }
            0x1c => {
                self.registers[Self::RX_COUNT] = value;
                self.rx_count = 0;
            }
            0x20 => self.registers[Self::TRANSFER] = value & !(1 << 8),
            0x24 => {
                self.registers[0x24 / 4] = value & 0xff;
                self.command(value as u8);
            }
            0x28 => {
                self.registers[Self::ADDRESS] = value;
                self.address_assigned = self.registers[Self::FORMAT] & 4 == 0;
            }
            0x2c => self.transfer(value as u8),
            0x30 => {
                if value & 1 != 0 {
                    self.registers.fill(0);
                    self.tx_count = 0;
                    self.rx_count = 0;
                }
                if value & 2 != 0 {
                    self.receive.clear();
                }
                self.registers[Self::CONTROL] = value & 0x00ff_ff18;
            }
            0x38 => self.registers[Self::INT_ENABLE] = value & 0x1c,
            0x3c => self.registers[Self::INT_STATUS] &= !value,
            _ => {}
        }
    }
}

struct PuyaFlash {
    storage: Vec<u8>,
    command: Option<u8>,
    address_count: u8,
    address: usize,
    status: [u8; 3],
    reset_enabled: bool,
    clear_wel: bool,
}

impl PuyaFlash {
    fn new(size: usize) -> Self {
        Self {
            storage: vec![0; size],
            command: None,
            address_count: 0,
            address: 0,
            status: [0; 3],
            reset_enabled: false,
            clear_wel: false,
        }
    }

    fn finish(&mut self) {
        self.command = None;
        self.address_count = 0;
        self.address = 0;
        if self.clear_wel {
            self.status[0] &= !2;
        }
        self.clear_wel = false;
    }

    fn protected(&self) -> bool {
        let protection = (self.status[0] >> 2) & 31;
        let maximum = 0x20_0000;
        if protection & 7 == 0 {
            return false;
        }
        if protection & 6 == 6 {
            return self.address < maximum;
        }
        let (lower, size) = match protection {
            1..=5 => (false, 64 * 1024 << (protection - 1)),
            9..=13 => (true, 64 * 1024 << (protection - 9)),
            17..=20 => (false, 4 * 1024 << (protection - 17)),
            21 => (false, 32 * 1024),
            25..=28 => (true, 4 * 1024 << (protection - 25)),
            _ => return false,
        };
        if lower {
            self.address < size
        } else {
            (maximum - size..maximum).contains(&self.address)
        }
    }

    fn transfer(&mut self, byte: u8) -> u8 {
        let Some(command) = self.command else {
            self.command = Some(byte);
            match byte {
                0x06 => self.status[0] |= 2,
                0x04 => self.status[0] &= !2,
                0x66 => self.reset_enabled = true,
                0x99 if self.reset_enabled => {
                    self.reset_enabled = false;
                    self.status = [0; 3];
                    self.finish();
                }
                _ => {}
            }
            return 0;
        };
        match command {
            0x05 => self.status[0],
            0x35 => self.status[1],
            0x15 => self.status[2],
            0x01 | 0x31 | 0x11 if self.status[0] & 2 != 0 && self.status[1] & 1 == 0 => {
                match command {
                    0x01 => self.status[0] = (byte & 0xfc) | 2,
                    0x31 => self.status[1] = byte & 3,
                    _ => self.status[2] = byte & 0x18,
                }
                self.clear_wel = true;
                0
            }
            0x02 | 0x32 | 0x20 | 0x52 | 0xd8 if self.status[0] & 2 != 0 => {
                if self.address_count < 3 {
                    self.address = (self.address << 8) | usize::from(byte);
                    self.address_count += 1;
                } else if matches!(command, 0x02 | 0x32) && !self.protected() {
                    if let Some(value) = self.storage.get_mut(self.address) {
                        *value = byte;
                        let size = 256 << ((self.status[2] >> 3) & 3).min(2);
                        let mask = size - 1;
                        self.address = (self.address & !mask) | ((self.address + 1) & mask);
                    }
                    self.clear_wel = true;
                }
                if self.address_count == 3 && !self.protected() {
                    let size = match command {
                        0x20 => 4096,
                        0x52 => 32768,
                        0xd8 => 65536,
                        _ => 0,
                    };
                    if size != 0 {
                        let end = (self.address + size).min(self.storage.len());
                        if self.address < end {
                            self.storage[self.address..end].fill(0xff);
                        }
                        self.status[0] &= !2;
                    }
                }
                0
            }
            _ => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::AndesSpi;

    #[test]
    fn spi_command_and_fifo_drive_flash_program_status_and_erase() {
        let mut spi = AndesSpi::new(0, "spi".into(), None, Some(0x2000));
        spi.write(0x20, (1 << 30) | (1 << 24));
        spi.write(0x24, 0x06);
        spi.write(0x10, 0);
        spi.write(0x24, 0x02);
        for byte in [0, 0, 0x42, 0xab] {
            spi.write(0x2c, byte);
        }
        spi.write(0x10, 0);
        assert_eq!(spi.flash.as_ref().unwrap().storage[0x42], 0xab);

        spi.write(0x20, (1 << 30) | (2 << 24));
        spi.write(0x24, 0x05);
        spi.write(0x2c, 0);
        assert_eq!((spi.read(0x34) >> 8) & 31, 2);
        assert_eq!(spi.read(0x2c), 0);
        assert_eq!(spi.read(0x2c) & 2, 0);
        spi.write(0x38, 1 << 4);
        assert!(spi.interrupt());
        spi.write(0x3c, 1 << 4);
        assert!(!spi.interrupt());

        spi.write(0x10, 0);
        spi.write(0x20, (1 << 30) | (1 << 24));
        spi.write(0x24, 0x06);
        spi.write(0x10, 0);
        spi.write(0x24, 0x20);
        for byte in [0, 0, 0] {
            spi.write(0x2c, byte);
        }
        assert_eq!(spi.flash.as_ref().unwrap().storage[0x42], 0xff);
    }
}
