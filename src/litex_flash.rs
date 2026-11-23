use crate::bus::Region;

#[derive(Default)]
pub struct Flash {
    enabled: bool,
    selected: bool,
    clock: bool,
    incoming: u8,
    bits: u8,
    outgoing: u8,
    output_position: u8,
    command: Option<u8>,
    address: usize,
    address_bytes: u8,
    dummy: bool,
    index: usize,
    four_bytes: bool,
    extended_address: u8,
    write_enabled: bool,
    status: u8,
}

impl Flash {
    pub const SIZE: u32 = 0x10;

    pub fn read(&self, offset: u32) -> u32 {
        match offset {
            4 if self.output_position < 8 => u32::from((self.outgoing >> self.output_position) & 1),
            8 => u32::from(self.enabled),
            _ => 0,
        }
    }

    pub fn write(&mut self, offset: u32, value: u32, storage: &mut Region) {
        match offset {
            0 if self.enabled => self.signals(value, storage),
            8 => self.enabled = value & 1 != 0,
            _ => {}
        }
    }

    fn signals(&mut self, value: u32, storage: &mut Region) {
        let selected = value & 4 == 0;
        if self.selected && !selected {
            self.finish();
        }
        self.selected = selected;
        let clock = value & 2 != 0;
        let rising = clock && !self.clock;
        self.clock = clock;
        if !rising {
            return;
        }
        self.output_position = self.output_position.checked_sub(1).unwrap_or(8);
        if selected && value & 8 == 0 {
            self.incoming = (self.incoming << 1) | (value & 1) as u8;
            self.bits += 1;
            if self.bits == 8 {
                self.bits = 0;
                let byte = std::mem::take(&mut self.incoming);
                self.outgoing = self.transfer(byte, storage);
                self.output_position = 8;
            }
        }
    }

    fn finish(&mut self) {
        if matches!(
            self.command,
            Some(0x01 | 0x02 | 0x12 | 0x20 | 0x21 | 0x52 | 0x5c | 0x60 | 0xc5 | 0xc7 | 0xd8 | 0xdc)
        ) {
            self.write_enabled = false;
        }
        self.command = None;
        self.bits = 0;
        self.incoming = 0;
        self.index = 0;
    }

    fn transfer(&mut self, byte: u8, storage: &mut Region) -> u8 {
        let Some(command) = self.command else {
            self.command = Some(byte);
            self.address = 0;
            self.dummy = matches!(byte, 0x0b | 0x0c);
            self.address_bytes = match byte {
                0x0c | 0x12 | 0x13 | 0x21 | 0x5c | 0xdc => 4,
                0x02 | 0x03 | 0x0b | 0x20 | 0x52 | 0xd8 => 3 + u8::from(self.four_bytes),
                _ => 0,
            };
            match byte {
                0x04 => self.write_enabled = false,
                0x06 => self.write_enabled = true,
                0x60 | 0xc7 if self.write_enabled => storage.flash_erase(0, storage.flash_size()),
                0xb7 => self.four_bytes = true,
                0xe9 => self.four_bytes = false,
                _ => {}
            }
            return 0;
        };
        if self.address_bytes != 0 {
            self.address = (self.address << 8) | usize::from(byte);
            self.address_bytes -= 1;
            if self.address_bytes == 0 {
                if !self.four_bytes && matches!(command, 0x02 | 0x03 | 0x0b | 0x20 | 0x52 | 0xd8) {
                    self.address |= usize::from(self.extended_address) << 24;
                }
                let size = match command {
                    0x20 | 0x21 => 4096,
                    0x52 | 0x5c => 32768,
                    0xd8 | 0xdc => 65536,
                    _ => 0,
                };
                if size != 0 && self.write_enabled {
                    storage.flash_erase(self.address & !(size - 1), size);
                }
            }
            return 0;
        }
        if std::mem::take(&mut self.dummy) {
            return 0;
        }
        let index = self.index;
        self.index += 1;
        match command {
            0x03 | 0x0b | 0x0c | 0x13 => {
                storage.flash_read(self.address.wrapping_add(index) & (storage.flash_size() - 1))
            }
            0x02 | 0x12 if self.write_enabled => {
                let address = (self.address & !0xff) | ((self.address + index) & 0xff);
                storage.flash_program(address, byte);
                0
            }
            0x05 => self.status | (u8::from(self.write_enabled) << 1),
            0x70 => 0x80 | u8::from(!self.four_bytes),
            0x9f | 0xaf => {
                let power = storage.flash_size().ilog2() as u8;
                let capacity = if power > 25 { 0x20 + power - 26 } else { power };
                [0x20, 0xbb, capacity, 0, 0]
                    .get(index)
                    .copied()
                    .unwrap_or(0)
            }
            0xc8 => self.extended_address,
            0x01 if self.write_enabled => {
                self.status = byte & 0xfc;
                0
            }
            0xc5 if self.write_enabled => {
                self.extended_address = byte;
                0
            }
            _ => 0,
        }
    }
}
