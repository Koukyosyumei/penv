use crate::bus::EthernetDma;

const START: u32 = 1;
const TRANSFER_ERROR: u32 = 1;
const COMPLETE: u32 = 1 << 1;
const CONFIG_ERROR: u32 = 1 << 5;
const FLAGS: u32 = TRANSFER_ERROR | COMPLETE | CONFIG_ERROR;

pub struct Dma2d {
    pub base: u32,
    pub rcc_bound: bool,
    pub irq: Option<u8>,
    registers: [u32; 18],
    status: u32,
}

impl Dma2d {
    pub const MODEL: &str = "DMA.STM32DMA2D";
    pub const SIZE: u32 = 0xc00;

    pub fn new(base: u32, rcc_bound: bool, irq: Option<u8>) -> Self {
        Self {
            base,
            rcc_bound,
            irq,
            registers: [0; 18],
            status: 0,
        }
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    pub fn reset(&mut self) {
        self.registers = [0; 18];
        self.status = 0;
    }

    pub fn interrupt(&self) -> Option<u8> {
        self.irq
            .filter(|_| self.status & (self.registers[0] >> 8) & FLAGS != 0)
    }

    pub fn read(&self, address: u32) -> u32 {
        match address - self.base {
            4 => self.status,
            offset => self
                .registers
                .get((offset / 4) as usize)
                .copied()
                .unwrap_or(0),
        }
    }

    pub fn write(&mut self, address: u32, value: u32, memory: &mut EthernetDma<'_>) {
        let offset = address - self.base;
        match offset {
            0 => {
                self.registers[0] = value & (0x3_0000 | (FLAGS << 8));
                if value & START != 0 {
                    self.status |= self.transfer(memory);
                }
            }
            4 => {}
            8 => self.status &= !(value & FLAGS),
            0x10 | 0x18 | 0x40 => self.registers[(offset / 4) as usize] = value & 0x3fff,
            0x34 => self.registers[(offset / 4) as usize] = value & 7,
            0x44 => self.registers[(offset / 4) as usize] = value & 0x3fff_ffff,
            _ => {
                if let Some(register) = self.registers.get_mut((offset / 4) as usize) {
                    *register = value;
                }
            }
        }
    }

    fn transfer(&self, memory: &mut EthernetDma<'_>) -> u32 {
        let mode = (self.registers[0] >> 16) & 3;
        let format = match mode {
            0 => self.registers[0x1c / 4] & 15,
            3 => self.registers[0x34 / 4],
            _ => return CONFIG_ERROR,
        };
        let bytes_per_pixel = match format {
            0 => 4,     // ARGB8888
            1 => 3,     // RGB888
            2..=4 => 2, // RGB565, ARGB1555, ARGB4444
            _ => return CONFIG_ERROR,
        };
        let dimensions = self.registers[0x44 / 4];
        let pixels = (dimensions >> 16) as usize;
        let lines = dimensions & 0xffff;
        if pixels == 0 || lines == 0 {
            return CONFIG_ERROR;
        }
        let mut row = vec![0; pixels * bytes_per_pixel];
        if mode == 3 {
            let color = self.registers[0x38 / 4].to_le_bytes();
            for pixel in row.chunks_exact_mut(bytes_per_pixel) {
                pixel.copy_from_slice(&color[..bytes_per_pixel]);
            }
        }
        let mut output = self.registers[0x3c / 4];
        let mut input = self.registers[0xc / 4];
        let output_stride = ((pixels + self.registers[0x40 / 4] as usize) * bytes_per_pixel) as u32;
        let input_stride = ((pixels + self.registers[0x10 / 4] as usize) * bytes_per_pixel) as u32;
        for line in 0..lines {
            if mode == 0 {
                let Some(data) = memory.read(input, row.len()) else {
                    return TRANSFER_ERROR;
                };
                row.copy_from_slice(data);
            }
            if !memory.write(output, &row) {
                return TRANSFER_ERROR;
            }
            if line + 1 < lines {
                let Some(next) = output.checked_add(output_stride) else {
                    return TRANSFER_ERROR;
                };
                output = next;
                if mode == 0 {
                    let Some(next) = input.checked_add(input_stride) else {
                        return TRANSFER_ERROR;
                    };
                    input = next;
                }
            }
        }
        COMPLETE
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{bus::Bus, nvic::ActiveExceptions, stm32h7_net_control::NetworkControl};

    #[test]
    fn fills_and_copies_rows_with_offsets_and_signals_nvic() {
        const BASE: u32 = 0x5200_1000;
        const RCC: u32 = 0x5802_4400;
        const RAM: u32 = 0x2000_0000;
        let mut bus = Bus::default();
        bus.map_ram(RAM, 0x100).unwrap();
        bus.map_nvic(0xf0, None).unwrap();
        bus.map_h7_dma2d(Dma2d::new(BASE, true, Some(90))).unwrap();
        bus.map_h7_network_control(NetworkControl::new(RCC, None, None))
            .unwrap();

        let write = |bus: &mut Bus, address, value| {
            bus.write(address, 4, value, &mut std::io::sink()).unwrap();
        };
        write(&mut bus, BASE + 0x38, 0xffff_ffff);
        write(&mut bus, RCC + 0xd4, 1 << 4);
        assert_eq!(bus.read::<4>(BASE + 0x38).unwrap(), 0);

        for (offset, value) in [
            (0x34, 2),
            (0x38, 0xf81f),
            (0x3c, RAM),
            (0x40, 1),
            (0x44, 0x20002),
            (0, 0x30000 | (COMPLETE << 8) | START),
        ] {
            write(&mut bus, BASE + offset, value);
        }
        assert_eq!(
            bus.memory(RAM, 12).unwrap(),
            &[0x1f, 0xf8, 0x1f, 0xf8, 0, 0, 0x1f, 0xf8, 0x1f, 0xf8, 0, 0]
        );
        assert_eq!(bus.read::<4>(BASE).unwrap() & START, 0);
        assert_eq!(bus.read::<4>(BASE + 4).unwrap(), COMPLETE);
        bus.sample_nvic(&ActiveExceptions::default());
        assert_eq!(bus.read::<4>(0xe000_e208).unwrap(), 1 << (90 - 64));
        write(&mut bus, BASE + 8, COMPLETE);
        write(&mut bus, 0xe000_e288, 1 << (90 - 64));
        bus.sample_nvic(&ActiveExceptions::default());
        assert_eq!(bus.read::<4>(0xe000_e208).unwrap(), 0);

        for (offset, value) in [
            (0xc, RAM),
            (0x10, 1),
            (0x1c, 2),
            (0x3c, RAM + 0x20),
            (0x40, 0),
            (0, START),
        ] {
            write(&mut bus, BASE + offset, value);
        }
        assert_eq!(
            bus.memory(RAM + 0x20, 8).unwrap(),
            &[0x1f, 0xf8, 0x1f, 0xf8, 0x1f, 0xf8, 0x1f, 0xf8]
        );
        write(&mut bus, BASE + 8, COMPLETE);
        write(&mut bus, BASE, 0x10000 | START);
        assert_eq!(bus.read::<4>(BASE + 4).unwrap(), CONFIG_ERROR);
        write(&mut bus, BASE + 8, CONFIG_ERROR);
        write(&mut bus, BASE + 0x3c, RAM + 0x100);
        write(&mut bus, BASE, START);
        assert_eq!(bus.read::<4>(BASE + 4).unwrap(), TRANSFER_ERROR);

        write(&mut bus, RCC + 0x7c, 1 << 4);
        write(&mut bus, RCC + 0x7c, 0);
        assert_eq!(bus.read::<4>(BASE + 4).unwrap(), 0);
        assert_eq!(bus.read::<4>(BASE + 0x38).unwrap(), 0);
    }
}
