use std::io::{BufWriter, Write};
use std::path::Path;

const FIFO_UNDERRUN: u32 = 1 << 1;
const RELOAD: u32 = 1 << 3;
const LAYER1: u32 = 0x84;

#[derive(Clone, Copy)]
struct Layer([u32; 13]);

impl Default for Layer {
    fn default() -> Self {
        let mut registers = [0; 13];
        registers[5] = 0xff; // constant alpha
        registers[7] = 0x607; // blending factors
        Self(registers)
    }
}

pub struct Ltdc {
    pub base: u32,
    pub rcc_bound: bool,
    pub irq: Option<u8>,
    registers: [u32; 17],
    layers: [Layer; 2],
    active: Layer,
    frame_period: u64,
}

impl Ltdc {
    pub const MODEL: &str = "Video.STM32LTDC";
    pub const SIZE: u32 = 0xc00;

    pub fn new(base: u32, rcc_bound: bool, irq: Option<u8>) -> Self {
        let mut registers = [0; 17];
        registers[0x18 / 4] = 0x2220;
        Self {
            base,
            rcc_bound,
            irq,
            registers,
            layers: [Layer::default(); 2],
            active: Layer::default(),
            frame_period: u64::from(crate::machine::VIRTUAL_STEPS_PER_SECOND) / 60,
        }
    }

    pub fn reset(&mut self) {
        let period = self.frame_period;
        *self = Self::new(self.base, self.rcc_bound, self.irq);
        self.frame_period = period;
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    pub fn interrupt(&self) -> Option<u8> {
        self.irq
            .filter(|_| self.registers[0x34 / 4] & self.registers[0x38 / 4] != 0)
    }

    fn layer_offset(offset: u32) -> Option<(usize, usize)> {
        let layer = offset.checked_sub(LAYER1)?;
        let index = (layer % 0x80 / 4) as usize;
        (layer < 0x100 && index < 13 && index != 8).then_some(((layer / 0x80) as usize, index))
    }

    pub fn read(&self, address: u32) -> u32 {
        let offset = address - self.base;
        if let Some((layer, index)) = Self::layer_offset(offset) {
            return self.layers[layer].0[index];
        }
        self.registers
            .get((offset / 4) as usize)
            .copied()
            .unwrap_or(0)
    }

    pub fn write(&mut self, address: u32, value: u32) {
        let offset = address - self.base;
        if let Some((layer, index)) = Self::layer_offset(offset) {
            let mask = match index {
                0 => 0x13,
                1 | 2 => 0x0fff_0fff,
                3 => 0x00ff_ffff,
                4 => 7,
                5 => 0xff,
                7 => 0x707,
                11 => 0x1fff_1fff,
                12 => 0x7ff,
                _ => u32::MAX,
            };
            self.layers[layer].0[index] = value & mask;
            return;
        }
        match offset {
            0x08 | 0x0c | 0x10 | 0x14 => {
                self.registers[(offset / 4) as usize] = value & 0x0fff_07ff
            }
            0x18 => {
                self.registers[0x18 / 4] = (value & 0xf001_0001) | 0x2220;
            }
            0x24 if value & 1 != 0 => {
                self.active = self.layers[0];
                self.registers[0x38 / 4] |= RELOAD;
            }
            0x2c => self.registers[0x2c / 4] = value & 0x00ff_ffff,
            0x34 => self.registers[0x34 / 4] = value & 15,
            0x3c => self.registers[0x38 / 4] &= !(value & 15),
            0x40 => self.registers[0x40 / 4] = value & 0x7ff,
            _ => {}
        }
    }

    pub fn set_frame_rate(&mut self, fps: u32) -> Result<(), String> {
        if !(1..=crate::machine::VIRTUAL_STEPS_PER_SECOND).contains(&fps) {
            return Err("invalid LTDC frame rate".into());
        }
        self.frame_period = u64::from(crate::machine::VIRTUAL_STEPS_PER_SECOND / fps);
        Ok(())
    }

    pub fn tick(&mut self, step: u64) {
        if step.is_multiple_of(self.frame_period) {
            self.check_scanout();
        }
    }

    fn check_scanout(&mut self) {
        if self.registers[0x18 / 4] & self.active.0[0] & 1 != 0 && self.active.0[12] == 0 {
            self.registers[0x38 / 4] |= FIFO_UNDERRUN;
        }
    }

    pub fn capture<'a>(
        &self,
        path: &Path,
        read_ram: impl Fn(u32, usize) -> Option<&'a [u8]>,
    ) -> Result<(), String> {
        let layer = &self.active.0;
        if self.registers[0x18 / 4] & 1 == 0 || layer[0] & 1 == 0 {
            return Err("LTDC or its reloaded layer 1 is disabled".into());
        }
        let bytes_per_pixel = match layer[4] {
            0 => 4, // ARGB8888
            1 => 3, // RGB888
            2 => 2, // RGB565
            format => return Err(format!("unsupported LTDC pixel format {format}")),
        };
        let pitch = (layer[11] >> 16) as usize;
        let row_bytes = ((layer[11] & 0x1fff) as usize).saturating_sub(7);
        let height = layer[12] as usize;
        if row_bytes == 0 || row_bytes % bytes_per_pixel != 0 || row_bytes > pitch || height == 0 {
            return Err(format!(
                "invalid LTDC framebuffer geometry: {row_bytes} bytes per row, pitch {pitch}, {height} rows, {bytes_per_pixel} bytes per pixel"
            ));
        }
        let rows = (0..height)
            .map(|line| {
                let address = layer[10]
                    .checked_add((line * pitch) as u32)
                    .ok_or("LTDC framebuffer address overflow")?;
                read_ram(address, row_bytes)
                    .ok_or_else(|| format!("unmapped LTDC framebuffer row at {address:#010x}"))
            })
            .collect::<Result<Vec<_>, String>>()?;

        let width = row_bytes / bytes_per_pixel;
        let save = || -> std::io::Result<()> {
            let mut output = BufWriter::new(std::fs::File::create(path)?);
            write!(output, "P6\n{width} {height}\n255\n")?;
            for row in rows {
                for pixel in row.chunks_exact(bytes_per_pixel) {
                    let rgb = if bytes_per_pixel == 2 {
                        let value = u16::from_le_bytes([pixel[0], pixel[1]]);
                        let red = ((value >> 11) & 31) as u8;
                        let green = ((value >> 5) & 63) as u8;
                        let blue = (value & 31) as u8;
                        [
                            (red << 3) | (red >> 2),
                            (green << 2) | (green >> 4),
                            (blue << 3) | (blue >> 2),
                        ]
                    } else {
                        [pixel[2], pixel[1], pixel[0]]
                    };
                    output.write_all(&rgb)?;
                }
            }
            output.flush()
        };
        save().map_err(|error| format!("{}: {error}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{bus::Bus, nvic::ActiveExceptions, stm32h7_net_control::NetworkControl};

    #[test]
    fn captures_reloaded_rgb565_rows_and_respects_pitch_clock_and_reset() {
        const LTDC: u32 = 0x5000_1000;
        const RCC: u32 = 0x5802_4400;
        const RAM: u32 = 0x2000_0000;
        let mut bus = Bus::default();
        bus.map_ram(RAM, 0x100).unwrap();
        bus.map_nvic(0xf0, None).unwrap();
        bus.map_h7_ltdc(Ltdc::new(LTDC, true, Some(88))).unwrap();
        bus.map_h7_network_control(NetworkControl::new(RCC, None, None))
            .unwrap();
        let write = |bus: &mut Bus, address, value| {
            bus.write(address, 4, value, &mut std::io::sink()).unwrap();
        };

        let path = std::env::temp_dir().join(format!("penv-ltdc-{}.ppm", std::process::id()));
        write(&mut bus, LTDC + 0x18, 1);
        assert_eq!(bus.read::<4>(LTDC + 0x18).unwrap(), 0);
        assert!(bus.capture_ltdc(&path).is_err());
        write(&mut bus, RCC + 0xe4, 8);
        assert_eq!(bus.read::<4>(LTDC + 0x18).unwrap(), 0x2220);
        bus.memory(RAM, 8)
            .unwrap()
            .copy_from_slice(&[0x00, 0xf8, 0xaa, 0xaa, 0x1f, 0x00, 0xbb, 0xbb]);
        for (offset, value) in [
            (0xac, RAM),
            (0xb0, (4 << 16) | 9),
            (0xb4, 2),
            (0x94, 2),
            (0x84, 1),
            (0x18, 1),
            (0x34, RELOAD),
        ] {
            write(&mut bus, LTDC + offset, value);
        }
        assert!(bus.capture_ltdc(&path).is_err());
        write(&mut bus, LTDC + 0x24, 1);
        write(&mut bus, LTDC + 0xac, RAM + 0x100); // Staged address is still unmapped.
        bus.capture_ltdc(&path).unwrap();
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"P6\n1 2\n255\n\xff\x00\x00\x00\x00\xff"
        );
        bus.sample_nvic(&ActiveExceptions::default());
        assert_eq!(bus.read::<4>(0xe000_e208).unwrap(), 1 << (88 - 64));
        write(&mut bus, LTDC + 0x3c, RELOAD);
        assert_eq!(bus.read::<4>(LTDC + 0x38).unwrap(), 0);
        write(&mut bus, LTDC + 0x24, 1);
        assert!(
            bus.capture_ltdc(&path)
                .unwrap_err()
                .contains("unmapped LTDC framebuffer row")
        );
        write(&mut bus, RCC + 0x8c, 8);
        write(&mut bus, RCC + 0x8c, 0);
        assert_eq!(bus.read::<4>(LTDC + 0x18).unwrap(), 0x2220);
        assert!(bus.capture_ltdc(&path).is_err());
        std::fs::remove_file(path).unwrap();
    }
}
