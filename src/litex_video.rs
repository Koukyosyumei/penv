use std::io::Write;

use crate::bus::Bus;

pub struct ReferenceFrame {
    data: Vec<u8>,
    pixels_start: usize,
    width: u16,
    height: u16,
}

impl ReferenceFrame {
    pub fn parse(data: Vec<u8>) -> Result<Self, String> {
        let mut lines = data.splitn(4, |byte| *byte == b'\n');
        let format = "expected P6 RGB PPM with separate magic, dimensions and maxval lines";
        if lines.next() != Some(b"P6") {
            return Err(format.into());
        }
        let dimensions = lines
            .next()
            .and_then(|line| std::str::from_utf8(line).ok())
            .ok_or(format)?;
        let mut dimensions = dimensions.split_ascii_whitespace();
        let width = dimensions
            .next()
            .and_then(|value| value.parse::<u16>().ok())
            .ok_or(format)?;
        let height = dimensions
            .next()
            .and_then(|value| value.parse::<u16>().ok())
            .ok_or(format)?;
        if dimensions.next().is_some() || lines.next() != Some(b"255") || width == 0 || height == 0
        {
            return Err(format.into());
        }
        let pixels = lines.next().ok_or(format)?;
        if pixels.len() as u64 != u64::from(width) * u64::from(height) * 3 {
            return Err("reference PPM pixel data does not match its dimensions".into());
        }
        let pixels_start = data.len() - pixels.len();
        Ok(Self {
            data,
            pixels_start,
            width,
            height,
        })
    }
}

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

    fn layout(&self) -> Result<(u32, u16, u16), String> {
        let (offset, width, height) = self.active.ok_or("LiteX framebuffer DMA is disabled")?;
        let bytes = u64::from(width) * u64::from(height) * 4;
        if width == 0
            || height == 0
            || offset & 3 != 0
            || u64::from(offset) + bytes > u64::from(self.memory_size)
        {
            return Err("LiteX framebuffer exceeds its mapped memory".into());
        }
        Ok((self.memory + offset, width, height))
    }

    fn rgb(&self, bus: &Bus, address: u32) -> Result<[u8; 3], String> {
        let [r, g, b, _] = bus
            .read_ram::<4>(address)
            .map_err(|error| error.to_string())?
            .to_le_bytes();
        Ok(if self.swap_red_blue {
            [b, g, r]
        } else {
            [r, g, b]
        })
    }

    pub fn matches(&self, bus: &Bus, reference: &ReferenceFrame) -> Result<bool, String> {
        if self.active.is_none() {
            return Ok(false);
        }
        let (mut address, width, height) = self.layout()?;
        if (width, height) != (reference.width, reference.height) {
            return Ok(false);
        }
        for expected in reference.data[reference.pixels_start..].as_chunks::<3>().0 {
            if self.rgb(bus, address)? != *expected {
                return Ok(false);
            }
            address = address.wrapping_add(4);
        }
        Ok(true)
    }

    pub fn render(&self, bus: &Bus, mut output: impl Write) -> Result<(), String> {
        let (mut address, width, height) = self.layout()?;
        writeln!(output, "P6\n{width} {height}\n255").map_err(|error| error.to_string())?;
        let mut row = vec![0; usize::from(width) * 3];
        for _ in 0..height {
            for rgb in row.as_chunks_mut::<3>().0 {
                *rgb = self.rgb(bus, address)?;
                address = address.wrapping_add(4);
            }
            output.write_all(&row).map_err(|error| error.to_string())?;
        }
        output.flush().map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_active_frame_dimensions_and_rgb_pixels() {
        let mut bus = Bus::default();
        bus.map_ram(0x1000, 8).unwrap();
        bus.memory(0x1004, 4)
            .unwrap()
            .copy_from_slice(&[1, 2, 3, 255]);
        let mut video = Framebuffer {
            name: "video".into(),
            dma: 0x2000,
            vtg: 0x3000,
            memory: 0x1000,
            memory_size: 8,
            offset: 4,
            width: 1,
            height: 1,
            swap_red_blue: true,
            active: None,
        };
        let reference = ReferenceFrame::parse(b"P6\n1 1\n255\n\x03\x02\x01".to_vec()).unwrap();
        assert!(ReferenceFrame::parse(b"P6\n1 1\n255\n\x03\x02".to_vec()).is_err());
        assert!(!video.matches(&bus, &reference).unwrap());
        video.write(0x2008, 1);
        assert!(video.matches(&bus, &reference).unwrap());
        bus.memory(0x1004, 1).unwrap()[0] = 4;
        assert!(!video.matches(&bus, &reference).unwrap());
        video.active = Some((0, 2, 1));
        assert!(!video.matches(&bus, &reference).unwrap());
    }
}
