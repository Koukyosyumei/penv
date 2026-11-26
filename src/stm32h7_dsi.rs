/// STM32H747 DSI host initialization and transmit path with an optional NT35510 panel.
pub struct Dsi {
    pub base: u32,
    panel: bool,
    response: Option<u32>,
    registers: [u32; 0x434 / 4],
}

impl Dsi {
    pub const MODEL: &str = "Video.STM32H7_DSI";
    pub const SIZE: u32 = 0x800;

    pub fn new(base: u32, panel: bool) -> Self {
        Self {
            base,
            panel,
            response: None,
            registers: [0; 0x434 / 4],
        }
    }

    pub fn reset(&mut self) {
        self.registers.fill(0);
        self.response = None;
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    pub fn read(&mut self, address: u32) -> u32 {
        match address - self.base {
            0 => 0x3133302a,
            0x70 => self.response.take().unwrap_or(0),
            0x74 => 0x5 | (u32::from(self.response.is_none()) << 4),
            0x40c => {
                let regulator = self.registers[0x430 / 4];
                if regulator & (1 << 24) == 0 {
                    0
                } else {
                    (1 << 12) | ((regulator & 1) << 8)
                }
            }
            0xb0 if self.registers[0xa0 / 4] & 6 == 6
                && self.read(self.base + 0x40c) & (1 << 8) != 0 =>
            {
                0x94
            } // Clock and data lanes in stop state.
            offset => self
                .registers
                .get(offset as usize / 4)
                .copied()
                .unwrap_or(0),
        }
    }

    pub fn write(&mut self, address: u32, value: u32) {
        match address - self.base {
            // Virtual channel 0, DCS read (0x06), NT35510 Read ID2 (0xdb).
            0x6c if self.panel && value & 0xffff == 0xdb06 => self.response = Some(0x80),
            0x6c | 0x70 => {}
            offset @ (0x04..=0x68
            | 0x78..=0xac
            | 0xb4..=0xbc
            | 0xc4
            | 0xc8
            | 0x400
            | 0x404
            | 0x408
            | 0x418..=0x430) => {
                self.registers[offset as usize / 4] = value;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dsi_lock_requires_regulator_and_pll_enable() {
        let mut dsi = Dsi::new(0, false);
        dsi.write(0x430, 1);
        assert_eq!(dsi.read(0x40c), 0);
        dsi.write(0x430, 1 << 24);
        assert_eq!(dsi.read(0x40c), 1 << 12);
        dsi.write(0x430, (1 << 24) | 1);
        assert_eq!(dsi.read(0x40c), (1 << 12) | (1 << 8));
        assert_eq!(dsi.read(0xb0), 0);
        dsi.write(0xa0, 6);
        assert_eq!(dsi.read(0xb0), 0x94);
        dsi.write(0x6c, 0x05);
        assert_eq!(dsi.read(0x74), 0x15);
        dsi.reset();
        assert_eq!(dsi.read(0x40c), 0);
        assert_eq!(dsi.read(0xb0), 0);
    }
}
