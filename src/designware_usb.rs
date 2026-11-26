use std::collections::BTreeMap;

/// Device-mode core with no USB host attached.
pub struct DesignWareUsb {
    pub base: u32,
    registers: BTreeMap<u32, u32>,
}

impl DesignWareUsb {
    pub const MODEL: &str = "USB.DesignWare_USB_OTG";
    pub const SIZE: u32 = 0x40000;

    pub fn new(base: u32) -> Self {
        Self {
            base,
            registers: BTreeMap::new(),
        }
    }

    pub fn reset(&mut self) {
        self.registers.clear();
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    pub fn read(&self, address: u32) -> u32 {
        let offset = address - self.base;
        self.registers
            .get(&offset)
            .copied()
            .unwrap_or(match offset {
                0x00 => 1 << 16, // B-device
                0x0c => 0x1400,
                0x10 => 1 << 31, // AHB idle
                0x14 => 0x04000020,
                0x3c => 0x2300,
                0x40 => 0x4f54310a,
                0x800 => 0x02200000,
                0x804 => 2, // Soft disconnect
                0x808 => 0x10,
                _ => 0,
            })
    }

    pub fn write(&mut self, address: u32, value: u32) {
        match address - self.base {
            // Reset/flush commands complete immediately with an idle core.
            0x10 if value & 1 != 0 => self.reset(),
            offset @ (0x08
            | 0x0c
            | 0x18
            | 0x24
            | 0x28
            | 0x38
            | 0x54
            | 0x104..=0x120
            | 0x800
            | 0x804
            | 0x810
            | 0x814
            | 0x81c
            | 0x828
            | 0x82c
            | 0x830
            | 0x834
            | 0xe00) => {
                self.registers.insert(offset, value);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detached_core_reports_idle_and_resets_configuration() {
        let base = 0x40040000;
        let mut usb = DesignWareUsb::new(base);
        assert_eq!(usb.read(base + 0x40), 0x4f54310a);
        assert_eq!(usb.read(base + 0x10), 1 << 31);
        usb.write(base + 0x18, 1 << 12);
        assert_eq!(usb.read(base + 0x18), 1 << 12);
        assert_eq!(usb.read(base + 0x14) & (1 << 12), 0);
        usb.write(base + 0x10, 1);
        assert_eq!(usb.read(base + 0x10), 1 << 31);
        assert_eq!(usb.read(base + 0x18), 0);
    }
}
