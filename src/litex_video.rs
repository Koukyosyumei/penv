use std::io::{Cursor, Write};

use crate::bus::Bus;

#[derive(Clone, Copy)]
pub struct FrameRegion {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

impl FrameRegion {
    fn fits(self, width: u16, height: u16) -> bool {
        self.width != 0
            && self.height != 0
            && u32::from(self.x) + u32::from(self.width) <= u32::from(width)
            && u32::from(self.y) + u32::from(self.height) <= u32::from(height)
    }
}

pub struct ReferenceFrame {
    data: Vec<u8>,
    pixels_start: usize,
    region: Option<FrameRegion>,
    width: u16,
    height: u16,
}

impl ReferenceFrame {
    pub fn parse(data: Vec<u8>) -> Result<Self, String> {
        if data.starts_with(b"\x89PNG\r\n\x1a\n") {
            return Self::parse_png(&data);
        }
        Self::parse_ppm(data)
    }

    fn parse_ppm(data: Vec<u8>) -> Result<Self, String> {
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
            region: None,
            width,
            height,
        })
    }

    fn parse_png(data: &[u8]) -> Result<Self, String> {
        let png_error = |error: png::DecodingError| format!("invalid reference PNG: {error}");
        let mut decoder = png::Decoder::new(Cursor::new(data));
        decoder.set_transformations(png::Transformations::normalize_to_color8());
        let mut reader = decoder.read_info().map_err(png_error)?;
        let info = reader.info();
        if info.animation_control.is_some() {
            return Err("reference PNG must be a static image".into());
        }
        let dimensions_error = "reference PNG dimensions exceed 65535 pixels per axis";
        let width = u16::try_from(info.width).map_err(|_| dimensions_error)?;
        let height = u16::try_from(info.height).map_err(|_| dimensions_error)?;

        const MAX_DECODED_BYTES: usize = 64 * 1024 * 1024;
        let size = reader
            .output_buffer_size()
            .filter(|&size| size <= MAX_DECODED_BYTES)
            .ok_or("reference PNG exceeds the 64 MiB decoded image limit")?;
        let pixel_count = usize::from(width) * usize::from(height);
        let rgb_size = pixel_count
            .checked_mul(3)
            .filter(|&size| size <= MAX_DECODED_BYTES)
            .ok_or("reference PNG exceeds the 64 MiB decoded image limit")?;
        let mut decoded = vec![0; size];
        let frame = reader.next_frame(&mut decoded).map_err(png_error)?;
        reader.finish().map_err(png_error)?;
        decoded.truncate(frame.buffer_size());

        let data = if frame.color_type == png::ColorType::Rgb {
            decoded
        } else {
            let mut rgb = Vec::with_capacity(rgb_size);
            match frame.color_type {
                png::ColorType::Rgba => {
                    for pixel in decoded.chunks_exact(4) {
                        rgb.extend_from_slice(&pixel[..3]);
                    }
                }
                png::ColorType::Grayscale | png::ColorType::GrayscaleAlpha => {
                    for pixel in decoded.chunks_exact(frame.color_type.samples()) {
                        rgb.extend_from_slice(&[pixel[0]; 3]);
                    }
                }
                _ => return Err("unsupported reference PNG color type".into()),
            }
            rgb
        };
        Ok(Self {
            data,
            pixels_start: 0,
            region: None,
            width,
            height,
        })
    }

    pub fn with_region(mut self, region: FrameRegion) -> Result<Self, String> {
        if !region.fits(self.width, self.height) {
            return Err("frame region must be nonempty and fit inside the reference image".into());
        }
        self.region = Some(region);
        Ok(self)
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
        let (base, width, height) = self.layout()?;
        let region = match reference.region {
            Some(region) if region.fits(width, height) => region,
            None if (width, height) == (reference.width, reference.height) => FrameRegion {
                x: 0,
                y: 0,
                width,
                height,
            },
            _ => return Ok(false),
        };
        let reference_stride = usize::from(reference.width) * 3;
        let row_bytes = usize::from(region.width) * 3;
        for row in u32::from(region.y)..u32::from(region.y) + u32::from(region.height) {
            let mut address = base + (row * u32::from(width) + u32::from(region.x)) * 4;
            let start = reference.pixels_start
                + row as usize * reference_stride
                + usize::from(region.x) * 3;
            for expected in reference.data[start..start + row_bytes].as_chunks::<3>().0 {
                if self.rgb(bus, address)? != *expected {
                    return Ok(false);
                }
                address = address.wrapping_add(4);
            }
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
    fn normalizes_png_color_and_rejects_a_truncated_image() {
        for (color, samples, expected) in [
            (png::ColorType::Rgb, &[3, 2, 1][..], [3, 2, 1]),
            (png::ColorType::Rgba, &[3, 2, 1, 0][..], [3, 2, 1]),
            (png::ColorType::GrayscaleAlpha, &[9, 0][..], [9; 3]),
        ] {
            let mut bytes = Vec::new();
            let mut encoder = png::Encoder::new(&mut bytes, 1, 1);
            encoder.set_color(color);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(samples).unwrap();
            writer.finish().unwrap();
            let frame = ReferenceFrame::parse(bytes.clone()).unwrap();
            assert_eq!((frame.width, frame.height), (1, 1));
            assert_eq!(frame.data, expected);
            bytes.truncate(bytes.len() - 12); // Remove the required IEND chunk.
            assert!(ReferenceFrame::parse(bytes).is_err());
        }
    }

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

    #[test]
    fn compares_only_the_region_at_the_same_coordinates_in_both_images() {
        let mut bus = Bus::default();
        bus.map_ram(0x1000, 48).unwrap();
        let mut video = Framebuffer {
            name: "video".into(),
            dma: 0x2000,
            vtg: 0x3000,
            memory: 0x1000,
            memory_size: 48,
            offset: 0,
            width: 4,
            height: 3,
            swap_red_blue: false,
            active: Some((0, 4, 3)),
        };
        let region = FrameRegion {
            x: 1,
            y: 1,
            width: 2,
            height: 2,
        };
        let mut ppm = b"P6\n3 3\n255\n".to_vec();
        ppm.extend_from_slice(&[9; 27]);
        let reference = ReferenceFrame::parse(ppm).unwrap();
        assert!(!video.matches(&bus, &reference).unwrap());
        let reference = reference.with_region(region).unwrap();
        for address in [0x1014, 0x1018, 0x1024, 0x1028] {
            bus.memory(address, 4)
                .unwrap()
                .copy_from_slice(&[9, 9, 9, 0]);
        }
        assert!(video.matches(&bus, &reference).unwrap());
        bus.memory(0x1028, 1).unwrap()[0] = 8;
        assert!(!video.matches(&bus, &reference).unwrap());
        video.active = Some((0, 2, 3));
        assert!(!video.matches(&bus, &reference).unwrap());
        assert!(
            reference
                .with_region(FrameRegion {
                    x: u16::MAX,
                    ..region
                })
                .is_err()
        );
    }
}
