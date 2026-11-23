use crate::rv64_ethernet::Phy;

// The register interface used by firmware to enumerate a Cadence GEM.
pub struct Gem {
    pub name: String,
    pub windows: Vec<u32>,
    phys: [Option<Phy>; 32],
    control: u32,
    configuration: u32,
    dma_configuration: u32,
    management_result: u16,
    interrupt_mask: u32,
    interrupt_status: u32,
    address_low: u32,
    address_high: u32,
    module_id: u32,
}

impl Gem {
    pub const SIZE: u64 = 0x1000;

    pub fn new(name: String, windows: Vec<u32>, module_id: u32, index: u8) -> Self {
        Self {
            name,
            windows,
            phys: [None; 32],
            control: 0,
            configuration: 0x80000,
            dma_configuration: 0x0002_0784,
            management_result: 0,
            interrupt_mask: u32::MAX,
            interrupt_status: 0,
            address_low: 2,
            address_high: u32::from(index) << 8,
            module_id,
        }
    }

    fn offset(&self, address: u32, width: usize) -> Option<u32> {
        if width != 4 || !address.is_multiple_of(4) {
            return None;
        }
        self.windows.iter().find_map(|&base| {
            let offset = address.checked_sub(base)?;
            (u64::from(offset) < Self::SIZE).then_some(offset)
        })
    }

    pub fn read(&self, address: u32, width: usize) -> Option<u32> {
        Some(match self.offset(address, width)? {
            0x000 => self.control,
            0x004 => self.configuration,
            0x008 => 1 << 2, // MDIO management idle
            0x010 => self.dma_configuration,
            0x024 => self.interrupt_status,
            0x030 => self.interrupt_mask,
            0x034 => u32::from(self.management_result),
            0x088 => self.address_low,
            0x08c => self.address_high,
            0x0fc => self.module_id,
            0x280 => 1 << 25, // 32-bit DMA bus
            0x284 => 3 << 20, // RX/TX packet buffers
            0x294 => 1 << 23, // 64-bit address capability
            _ => 0,
        })
    }

    pub fn write(&mut self, address: u32, width: usize, value: u32) -> Option<()> {
        match self.offset(address, width)? {
            0x000 => self.control = value & 0x1c,
            0x004 => self.configuration = value,
            0x010 => self.dma_configuration = value,
            0x024 => self.interrupt_status &= !value,
            0x028 => self.interrupt_mask &= !value,
            0x02c => self.interrupt_mask |= value,
            0x034 => self.mdio(value),
            0x088 => self.address_low = value,
            0x08c => self.address_high = value & 0xffff,
            _ => {}
        }
        Some(())
    }

    fn mdio(&mut self, command: u32) {
        let register = ((command >> 18) & 31) as usize;
        let address = ((command >> 23) & 31) as usize;
        let Some(phy) = &self.phys[address] else {
            self.management_result = u16::MAX;
            return;
        };
        match (command >> 28) & 3 {
            1 => {} // Generic Renode PHYs ignore register writes.
            2 => self.management_result = phy.registers[register],
            _ => return,
        }
        self.interrupt_status |= 1;
    }

    pub fn attach(&mut self, address: usize, phy: Phy) -> bool {
        let Some(slot) = self.phys.get_mut(address) else {
            return false;
        };
        if slot.is_some() {
            return false;
        }
        *slot = Some(phy);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gem_windows_share_management_and_station_address_state() {
        let a = 0x2011_0000;
        let b = 0x2811_0000;
        let mut gem = Gem::new("mac0".into(), vec![a, b], 0x20118, 0);
        let mut phy = Phy::default();
        phy.registers[2] = 7;
        assert!(gem.attach(3, phy));
        gem.write(a + 0x34, 4, (2 << 28) | (3 << 23) | (2 << 18))
            .unwrap();
        assert_eq!(gem.read(b + 0x34, 4), Some(7));
        gem.write(b + 0x88, 4, 0x1002).unwrap();
        assert_eq!(gem.read(a + 0x88, 4), Some(0x1002));
        assert_eq!(gem.read(b + 0x294, 4), Some(1 << 23));
        assert_eq!(gem.read(b + 0x1000, 4), None);
    }
}
