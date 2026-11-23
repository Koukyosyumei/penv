use std::collections::BTreeMap;

const SECTOR_SIZE: u32 = 4096;

const TEF: u32 = 1;
const TCF: u32 = 1 << 1;
const FTF: u32 = 1 << 2;
const SMF: u32 = 1 << 3;

pub struct Qspi {
    pub name: String,
    pub base: u32,
    pub rcc_bound: bool,
    pub irq: Option<u8>,
    capacity_exponent: Option<u8>,
    macronix: bool,
    registers: [u32; 13],
    response: [u8; 88],
    flash: BTreeMap<u32, Box<[u8; SECTOR_SIZE as usize]>>,
    flash_read: bool,
    flash_address: u32,
    status_configuration: u8,
    remaining: u32,
    write_remaining: u32,
    index: usize,
    flags: u32,
    write_enabled: bool,
    reset_enabled: bool,
}

impl Qspi {
    pub const MODEL: &str = "SPI.STM32H7_QuadSPI";
    pub const FLASH: &str = "SPI.ISSI_IS25WP";
    pub const MACRONIX_FLASH: &str = "SPI.Macronix_MX25R";
    pub const SIZE: u32 = 0x400;
    pub const DATA: u32 = 0x20;

    pub fn new(name: String, base: u32, rcc_bound: bool, irq: Option<u8>) -> Self {
        Self {
            name,
            base,
            rcc_bound,
            irq,
            capacity_exponent: None,
            macronix: false,
            registers: [0; 13],
            response: [0; 88],
            flash: BTreeMap::new(),
            flash_read: false,
            flash_address: 0,
            status_configuration: 0,
            remaining: 0,
            write_remaining: 0,
            index: 0,
            flags: 0,
            write_enabled: false,
            reset_enabled: false,
        }
    }

    pub fn attach(&mut self, capacity: u32, macronix: bool) -> Result<(), &'static str> {
        if self.capacity_exponent.is_some() || !capacity.is_power_of_two() || capacity < 0x40000 {
            return Err("invalid or duplicate SPI flash capacity");
        }
        self.capacity_exponent = Some(capacity.ilog2() as u8);
        self.macronix = macronix;
        Ok(())
    }

    pub fn reset(&mut self) {
        self.registers = [0; 13];
        self.remaining = 0;
        self.write_remaining = 0;
        self.flags = 0;
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    pub fn interrupt(&self) -> Option<u8> {
        self.irq
            .filter(|_| (self.status() & (self.registers[0] >> 16) & 0xf) != 0)
    }

    fn status(&self) -> u32 {
        let threshold = ((self.registers[0] >> 8) & 31) + 1;
        let level = self.remaining.min(32);
        self.flags
            | (level << 8)
            | (u32::from(level >= threshold || self.write_remaining != 0) * FTF)
    }

    pub fn read(&mut self, address: u32, width: usize) -> u32 {
        let offset = address - self.base;
        if offset == 8 {
            return self.status();
        }
        if offset == Self::DATA {
            let mut word = [0; 4];
            for byte in &mut word[..width] {
                if self.remaining == 0 {
                    break;
                }
                *byte = if self.flash_read {
                    let address = self.flash_address.wrapping_add(self.index as u32);
                    self.flash_byte(address)
                } else {
                    self.response.get(self.index).copied().unwrap_or(0xff)
                };
                self.index = self.index.saturating_add(1);
                self.remaining -= 1;
            }
            return u32::from_le_bytes(word);
        }
        self.registers
            .get((offset / 4) as usize)
            .copied()
            .unwrap_or(0)
    }

    pub fn write(&mut self, address: u32, width: usize, value: u32) {
        let offset = address - self.base;
        match offset {
            0 => {
                self.registers[0] = value & !2;
                if value & 2 != 0 {
                    self.remaining = 0;
                    self.write_remaining = 0;
                }
            }
            8 => {}
            0xc => self.flags &= !(value & (TEF | TCF | SMF)),
            0x14 => {
                // HAL_QSPI_Transmit rewrites the same CCR after the address
                // phase to select indirect write. Keep the active transfer.
                if self.registers[5] == value && self.write_remaining != 0 {
                    return;
                }
                self.registers[5] = value;
                self.remaining = 0;
                self.write_remaining = 0;
                if value & (3 << 10) == 0 {
                    self.command();
                }
            }
            0x18 => {
                self.registers[6] = value;
                if self.registers[5] & (3 << 10) != 0 {
                    self.command();
                }
            }
            Self::DATA => {
                if self.write_remaining != 0 {
                    if self.registers[5] as u8 == 0x01 && self.write_enabled {
                        self.status_configuration = value as u8 & 0xfc;
                    }
                    if self.write_enabled
                        && matches!(self.registers[5] as u8, 0x02 | 0x12 | 0x32 | 0x34 | 0x38)
                    {
                        for byte in value
                            .to_le_bytes()
                            .into_iter()
                            .take(width.min(self.write_remaining as usize))
                        {
                            let address = (self.flash_address & !0xff)
                                | ((self.flash_address as usize + self.index) as u32 & 0xff);
                            self.program_byte(address, byte);
                            self.index += 1;
                        }
                    }
                    self.write_remaining = self.write_remaining.saturating_sub(width as u32);
                    if self.write_remaining == 0 {
                        self.write_enabled = false;
                        self.flags |= TCF;
                    }
                }
            }
            _ => {
                if let Some(register) = self.registers.get_mut((offset / 4) as usize) {
                    *register = value;
                }
            }
        }
    }

    fn flash_byte(&self, address: u32) -> u8 {
        self.flash
            .get(&(address / SECTOR_SIZE))
            .map_or(0xff, |sector| sector[(address % SECTOR_SIZE) as usize])
    }

    fn program_byte(&mut self, address: u32, byte: u8) {
        if self
            .capacity_exponent
            .is_some_and(|capacity| address < 1u32 << capacity)
        {
            let sector = self
                .flash
                .entry(address / SECTOR_SIZE)
                .or_insert_with(|| Box::new([0xff; SECTOR_SIZE as usize]));
            sector[(address % SECTOR_SIZE) as usize] &= byte;
        }
    }

    fn erase(&mut self, size: u32) {
        if self.write_enabled {
            let start = self.flash_address / size * size / SECTOR_SIZE;
            let end = start + size / SECTOR_SIZE;
            self.flash
                .retain(|&sector, _| !(start..end).contains(&sector));
            self.write_enabled = false;
        }
    }

    fn command(&mut self) {
        let ccr = self.registers[5];
        if self.registers[0] & 1 == 0 || ccr & (3 << 8) == 0 {
            return;
        }
        let Some(capacity) = self.capacity_exponent else {
            self.flags |= TEF;
            return;
        };
        let mode = (ccr >> 26) & 3;
        let instruction = ccr as u8;
        self.flash_read = false;
        self.flash_address = self.registers[6];
        self.index = 0;
        let status = (u8::from(self.write_enabled) << 1) | self.status_configuration;
        if mode == 0 && ccr & (3 << 24) != 0 {
            if matches!(instruction, 0x01 | 0x02 | 0x12 | 0x32 | 0x34 | 0x38) {
                self.write_remaining = self.registers[4].saturating_add(1);
            }
            return;
        }
        self.response.fill(0xff);
        self.index = 0;
        match (mode, instruction) {
            (0, 0x06) => self.write_enabled = true,
            (0, 0x04) => self.write_enabled = false,
            (0, 0x66) => self.reset_enabled = true,
            (0, 0x99) => {
                if self.reset_enabled {
                    self.write_enabled = false;
                    self.status_configuration = 0;
                }
                self.reset_enabled = false;
            }
            (0, 0xab | 0xb7 | 0xe9) => {}
            (0, 0x20 | 0x21) => self.erase(0x1000),
            (0, 0x52 | 0x5c) => self.erase(0x8000),
            (0, 0xd8 | 0xdc) => self.erase(0x10000),
            (
                1,
                0x03 | 0x0b | 0x13 | 0x0c | 0x3b | 0x3c | 0x6b | 0x6c | 0xbb | 0xbc | 0xeb | 0xec,
            ) => {
                self.flash_read = true;
            }
            (1, 0x9f) => {
                let id = if self.macronix {
                    let code = if capacity > 25 {
                        capacity - 26 + 0x20
                    } else {
                        capacity
                    };
                    [0xc2, 0x28, code]
                } else {
                    [0x9d, 0x70, capacity]
                };
                self.response[..3].copy_from_slice(&id);
            }
            (1 | 2, 0x05) => self.response.fill(status),
            (1, 0x5a) => {
                self.response = sfdp(capacity);
                self.index = self.registers[6] as usize;
            }
            _ => {
                self.flags |= TEF;
                return;
            }
        };
        if mode == 2 {
            let mask = self.registers[9];
            let target = self.registers[10];
            let equal = !(u32::from(status) ^ target);
            let matched = if self.registers[0] & (1 << 23) == 0 {
                equal & mask == mask
            } else {
                equal & mask != 0
            };
            if matched {
                self.flags |= SMF;
            }
        } else {
            if mode == 1 && ccr & (3 << 24) != 0 {
                self.remaining = self.registers[4].saturating_add(1);
            }
            self.flags |= TCF;
        }
    }
}

// JEDEC basic flash parameter table for the modeled IS25WP features.
fn sfdp(capacity: u8) -> [u8; 88] {
    let mut data = [0xff; 88];
    data[..16].copy_from_slice(&[
        b'S', b'F', b'D', b'P', 6, 1, 0, 0xff, 0, 6, 1, 16, 24, 0, 0, 0xff,
    ]);
    let density = (1u64 << (capacity + 3)) - 1;
    let density = if density <= 0x7fff_ffff {
        density as u32
    } else {
        0x8000_0000 | u32::from(capacity + 3)
    };
    let parameters: [u32; 16] = [
        0x0003_20fd | (1 << 17) | (1 << 20) | (1 << 21) | (1 << 22),
        density,
        0x6b08_eb44,
        0xbb04_3b04,
        0,
        0,
        0,
        0xd812_200c,
        0,
        0,
        8 << 4,
        0,
        0,
        0,
        2 << 20,
        1 << 24,
    ];
    for (word, value) in data[24..].as_chunks_mut::<4>().0.iter_mut().zip(parameters) {
        *word = value.to_le_bytes();
    }
    data
}

#[cfg(test)]
mod tests {
    use super::*;

    fn indirect(qspi: &mut Qspi, operation: u32, opcode: u8, address: u32, size: u32) {
        qspi.write(0xc, 4, TCF | TEF);
        qspi.write(0x10, 4, size.saturating_sub(1));
        qspi.write(0x14, 4, operation | 0x1900 | u32::from(opcode));
        qspi.write(0x18, 4, address);
    }

    #[test]
    fn issi_indirect_read_program_and_erase() {
        let mut qspi = Qspi::new("qspi".into(), 0, false, Some(92));
        qspi.attach(0x0800_0000, false).unwrap();
        qspi.write(0, 4, 1);
        let address = 0x0100_1000;
        indirect(&mut qspi, 0x0500_0000, 0x13, address, 4);
        assert_eq!(qspi.read(Qspi::DATA, 4), u32::MAX);
        qspi.write(0x14, 4, 0x0106);
        indirect(&mut qspi, 0x0100_0000, 0x12, address, 4);
        qspi.write(0x14, 4, 0x0100_1912);
        assert_ne!(qspi.read(8, 4) & FTF, 0);
        qspi.write(Qspi::DATA, 4, 0x12345678);
        indirect(&mut qspi, 0x0500_0000, 0x13, address, 4);
        assert_eq!(qspi.read(Qspi::DATA, 4), 0x12345678);
        qspi.write(0x14, 4, 0x0106);
        indirect(&mut qspi, 0, 0x21, address, 0);
        indirect(&mut qspi, 0x0500_0000, 0x13, address, 4);
        assert_eq!(qspi.read(Qspi::DATA, 4), u32::MAX);
    }

    #[test]
    fn issi_program_requires_write_enable_and_only_clears_bits() {
        let mut qspi = Qspi::new("qspi".into(), 0, false, None);
        qspi.attach(0x0800_0000, false).unwrap();
        qspi.write(0, 4, 1);
        for (enabled, value) in [(false, 0), (true, 0xf0), (true, 0xaa)] {
            if enabled {
                qspi.write(0x14, 4, 0x0106);
            }
            indirect(&mut qspi, 0x0100_0000, 0x02, 0x100, 1);
            qspi.write(Qspi::DATA, 1, value);
        }
        indirect(&mut qspi, 0x0500_0000, 0x03, 0x100, 1);
        assert_eq!(qspi.read(Qspi::DATA, 1), 0xa0);
    }

    #[test]
    fn macronix_jedec_id_and_quad_program() {
        let mut qspi = Qspi::new("qspi".into(), 0, false, Some(92));
        qspi.attach(0x1000_0000, true).unwrap();
        qspi.write(0, 4, 1 | (TCF << 16));
        qspi.write(0x10, 4, 2);
        qspi.write(0x14, 4, 0x0500_019f);
        assert_eq!(qspi.read(Qspi::DATA, 4), 0x0022_28c2);
        qspi.write(0x14, 4, 0x0106);
        indirect(&mut qspi, 0x0300_0000, 0x38, 0x100, 4);
        qspi.write(Qspi::DATA, 4, 0x1234_5678);
        assert_eq!(qspi.interrupt(), Some(92));
        indirect(&mut qspi, 0x0700_0000, 0xeb, 0x100, 4);
        assert_eq!(qspi.read(Qspi::DATA, 4), 0x1234_5678);
    }

    #[test]
    fn reads_sfdp_and_completes_status_register_write_via_irq() {
        let mut qspi = Qspi::new("qspi".into(), 0, false, Some(92));
        qspi.attach(0x0800_0000, false).unwrap();
        qspi.write(0, 4, 1 | (TCF << 16) | (FTF << 16));
        qspi.write(0x10, 4, 3);
        qspi.write(0x14, 4, 0x0500_055a);
        assert_eq!(qspi.interrupt(), None);
        qspi.write(0x18, 4, 0);
        assert_eq!(qspi.interrupt(), Some(92));
        assert_eq!(qspi.read(Qspi::DATA, 4), u32::from_le_bytes(*b"SFDP"));
        qspi.write(0xc, 4, TCF);
        assert_eq!(qspi.interrupt(), None);

        qspi.write(0x14, 4, 0x0106);
        qspi.write(0xc, 4, TCF);
        qspi.write(0x10, 4, 0);
        qspi.write(0x14, 4, 0x0100_0101);
        assert_eq!(qspi.read(8, 4), FTF);
        assert_eq!(qspi.interrupt(), Some(92));
        qspi.write(Qspi::DATA, 1, 0x40);
        assert_eq!(qspi.read(8, 4), TCF);
        qspi.write(0x14, 4, 0x0500_0105);
        assert_eq!(qspi.read(Qspi::DATA, 1), 0x40);
    }
}
