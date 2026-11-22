use std::io::Write;

use crate::bus::Bus;

pub struct Framebuffer {
    pub name: String,
    pub dma: u32,
    pub vtg: u32,
    pub memory: u32,
    pub memory_size: u32,
    pub offset: u32,
    pub width: u16,
    pub height: u16,
    pub swap_red_blue: bool,
    pub active: Option<(u32, u16, u16)>,
}

impl Framebuffer {
    pub const DMA_SIZE: u32 = 0xc;
    pub const VTG_SIZE: u32 = 0x24;

    pub fn read(&self, address: u32) -> Option<u32> {
        if address.wrapping_sub(self.dma) < Self::DMA_SIZE {
            return Some(match address - self.dma {
                0 => self.offset,
                8 => u32::from(self.active.is_some()),
                _ => 0,
            });
        }
        if address.wrapping_sub(self.vtg) < Self::VTG_SIZE {
            return Some(match address - self.vtg {
                4 => u32::from(self.width),
                0x14 => u32::from(self.height),
                _ => 0,
            });
        }
        None
    }

    pub fn write(&mut self, address: u32, value: u32) -> bool {
        if address.wrapping_sub(self.dma) < Self::DMA_SIZE {
            match address - self.dma {
                0 => self.offset = value,
                8 => {
                    self.active = (value & 1 != 0).then_some((self.offset, self.width, self.height))
                }
                _ => {}
            }
            return true;
        }
        if address.wrapping_sub(self.vtg) < Self::VTG_SIZE {
            match address - self.vtg {
                4 => self.width = value as u16,
                0x14 => self.height = value as u16,
                _ => {}
            }
            return true;
        }
        false
    }

    pub fn render(&self, bus: &Bus, mut output: impl Write) -> Result<(), String> {
        let (offset, width, height) = self.active.ok_or("LiteX framebuffer DMA is disabled")?;
        let bytes = u64::from(width) * u64::from(height) * 4;
        if width == 0
            || height == 0
            || offset & 3 != 0
            || u64::from(offset) + bytes > u64::from(self.memory_size)
        {
            return Err("LiteX framebuffer exceeds its mapped memory".into());
        }
        writeln!(output, "P6\n{width} {height}\n255").map_err(|error| error.to_string())?;
        let mut row = vec![0; usize::from(width) * 3];
        let mut address = self.memory + offset;
        for _ in 0..height {
            for rgb in row.as_chunks_mut::<3>().0 {
                let [r, g, b, _] = bus
                    .read_ram::<4>(address)
                    .map_err(|error| error.to_string())?
                    .to_le_bytes();
                *rgb = if self.swap_red_blue {
                    [b, g, r]
                } else {
                    [r, g, b]
                };
                address = address.wrapping_add(4);
            }
            output.write_all(&row).map_err(|error| error.to_string())?;
        }
        output.flush().map_err(|error| error.to_string())
    }
}
