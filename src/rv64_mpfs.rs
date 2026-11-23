// The PolarFire MSS system register window exposed by Renode's MPFS_Sysreg.
pub struct Sysreg {
    pub base: u32,
    subblock_clock: u32,
    soft_reset: u32,
}

impl Sysreg {
    pub const SIZE: u64 = 0x1000;

    const SUBBLOCK_CLOCK: u32 = 0x84;
    const SOFT_RESET: u32 = 0x88;
    const PERIPHERAL_FLAGS: u32 = 0x10fd_ffff;

    pub fn new(base: u32) -> Self {
        Self {
            base,
            subblock_clock: 0,
            soft_reset: 0x7fff_fffe,
        }
    }

    fn offset(&self, address: u32, width: usize) -> Option<u32> {
        let offset = address.checked_sub(self.base)?;
        (width == 4 && offset % 4 == 0 && u64::from(offset) < Self::SIZE).then_some(offset)
    }

    pub fn read(&self, address: u32, width: usize) -> Option<u32> {
        Some(match self.offset(address, width)? {
            0x008 => 0x10,   // Clock configuration
            0x00c => 0x1064, // RTC clock configuration
            0x024 => 0x1f09, // Device status
            Self::SUBBLOCK_CLOCK => self.subblock_clock,
            Self::SOFT_RESET => self.soft_reset,
            0x0b8 => 0xff,  // eNVM configuration
            0x14c => 0x707, // PLL status
            _ => 0,
        })
    }

    pub fn write(&mut self, address: u32, width: usize, value: u32) -> Option<()> {
        // Renode's tagged or undefined registers ignore writes. Peripheral
        // clock and reset effects will need wiring as the MSS models are added.
        let register = match self.offset(address, width)? {
            Self::SUBBLOCK_CLOCK => &mut self.subblock_clock,
            Self::SOFT_RESET => &mut self.soft_reset,
            _ => return Some(()),
        };
        *register = (*register & !Self::PERIPHERAL_FLAGS) | (value & Self::PERIPHERAL_FLAGS);
        Some(())
    }
}

// Behavioral DDR training mock used by the upstream PolarFire platform.
pub struct DdrMock {
    pub base: u32,
    next_word: u32,
    byte_lane: u32,
}

impl DdrMock {
    pub const SIZE: u64 = 0x1000;

    pub fn new(base: u32) -> Self {
        Self {
            base,
            next_word: 0,
            byte_lane: 0,
        }
    }

    pub fn accepts(&self, address: u32, width: usize) -> bool {
        matches!(width, 1 | 4)
            && address
                .checked_sub(self.base)
                .is_some_and(|offset| u64::from(offset) + width as u64 <= Self::SIZE)
    }

    pub fn read(&mut self, address: u32, width: usize) -> u32 {
        let offset = address - self.base;
        if width == 1 {
            return match offset {
                0x814 => {
                    let value = 1 << self.byte_lane;
                    self.byte_lane = (self.byte_lane + 1) % 5;
                    value
                }
                0x834 => 8,
                0x84c..=0x84f => 0xff,
                _ => 0,
            };
        }
        match offset {
            0x008 | 0x010 => 0x200,
            0x02c | 0x824 => u32::MAX,
            0x0b4 => 0x5245_4e44,
            0x81c | 0x820 => 0,
            0x834 => 8,
            0x84c | 0x850 | 0xc08 => 0xff,
            0x864 => 0x4433_2211,
            0x868 => 0xddcc_bbaa,
            _ => {
                let value = self.next_word;
                self.next_word = !self.next_word;
                value
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::DdrMock;

    #[test]
    fn ddr_training_state_is_per_controller_and_per_access_width() {
        let mut phy = DdrMock::new(0x2000_7000);
        let mut controller = DdrMock::new(0x3e00_1000);
        assert_eq!(phy.read(phy.base + 8, 4), 0x200);
        assert_eq!(phy.read(phy.base, 4), 0);
        assert_eq!(phy.read(phy.base + 0xb4, 4), 0x5245_4e44);
        assert_eq!(phy.read(phy.base + 0x814, 1), 1);
        assert_eq!(phy.read(phy.base, 4), u32::MAX);
        assert_eq!(controller.read(controller.base, 4), 0);
        for expected in [2, 4, 8, 16, 1] {
            assert_eq!(phy.read(phy.base + 0x814, 1), expected);
        }
        assert!(!phy.accepts(phy.base + 0x1000, 1));
        assert!(!phy.accepts(phy.base, 8));
    }
}

// MPFS CoreSPI register interface for controllers with no SPI slave attached.
pub struct Spi {
    pub base: u32,
    control: u32,
    frame_size: u32,
    clock_rate: u32,
    slave_select: u32,
    enhanced_control: u32,
    command_size: u32,
}

impl Spi {
    pub const SIZE: u64 = 0x1000;
    const RESET: u32 = 1 << 31;
    const FRAME_COUNT: u32 = 0xffff << 8;
    const CONTROL_WRITABLE: u32 = (1 << 29) | Self::FRAME_COUNT | 0xf3;

    pub fn new(base: u32) -> Self {
        Self {
            base,
            control: Self::RESET | (1 << 8) | 2,
            frame_size: 4,
            clock_rate: 0,
            slave_select: 0,
            enhanced_control: 0,
            command_size: 0,
        }
    }

    fn offset(&self, address: u32, width: usize) -> Option<u32> {
        let offset = address.checked_sub(self.base)?;
        (width == 4 && offset.is_multiple_of(4) && u64::from(offset) < Self::SIZE).then_some(offset)
    }

    pub fn read(&self, address: u32, width: usize) -> Option<u32> {
        Some(match self.offset(address, width)? {
            0x00 => self.control,
            0x04 => self.frame_size,
            0x08 => 0x2440, // RX and TX FIFOs empty, SSEL at its reset value
            0x18 => self.clock_rate,
            0x1c => self.slave_select,
            0x28 => self.enhanced_control,
            0x34 => self.command_size,
            0x38 => 2, // Slave hardware status: TX FIFO empty
            0x3c => 4 | (u32::from(self.slave_select != 0) << 6),
            offset @ 0x40..=0x4c => (self.control >> ((offset - 0x40) * 2)) & 0xff,
            _ => 0,
        })
    }

    pub fn write(&mut self, address: u32, width: usize, value: u32) -> Option<()> {
        match self.offset(address, width)? {
            0x00 => self.write_control(value),
            0x04 => self.frame_size = value & 0x3f,
            0x18 => self.clock_rate = value & 0xff,
            0x1c => self.slave_select = value & 0xff,
            0x28 => self.enhanced_control = value & 0x34,
            0x2c if value & (1 << 4) != 0 => self.control &= !Self::FRAME_COUNT,
            0x34 => self.command_size = value & 0x3f,
            offset @ 0x40..=0x4c => {
                let shift = (offset - 0x40) * 2;
                self.write_control((self.control & !(0xff << shift)) | ((value & 0xff) << shift));
            }
            _ => {}
        }
        Some(())
    }

    fn write_control(&mut self, value: u32) {
        if value & Self::RESET != 0 {
            *self = Self::new(self.base);
            return;
        }
        let was_enabled = self.control & 1 != 0;
        self.control = value & Self::CONTROL_WRITABLE;
        if was_enabled && self.control & 1 == 0 {
            self.control &= !Self::FRAME_COUNT;
        }
    }
}

#[cfg(test)]
mod spi_tests {
    use super::Spi;

    #[test]
    fn core_spi_probe_registers_and_reset() {
        let base = 0x2010_8000;
        let mut spi = Spi::new(base);
        let other = Spi::new(0x2010_9000);
        assert_eq!(spi.read(base, 4), Some(0x8000_0102));
        assert_eq!(spi.read(base + 4, 4), Some(4));
        assert_eq!(spi.read(base + 8, 4), Some(0x2440));
        assert_eq!(spi.read(base + 0x20, 4), Some(0));

        assert_eq!(spi.write(base, 4, 0x2400_0133), Some(()));
        assert_eq!(spi.read(base, 4), Some(0x2000_0133));
        assert_eq!(spi.write(base + 4, 4, 8), Some(()));
        assert_eq!(spi.write(base + 0x1c, 4, 0x301), Some(()));
        assert_eq!(spi.read(base + 0x1c, 4), Some(1));
        assert_eq!(spi.read(base + 0x3c, 4), Some(0x44));
        assert_eq!(other.read(other.base + 4, 4), Some(4));

        assert_eq!(spi.write(base + 0x44, 4, 0x42), Some(()));
        assert_eq!(spi.read(base, 4), Some(0x2000_4233));
        assert_eq!(spi.write(base + 0x2c, 4, 0x10), Some(()));
        assert_eq!(spi.read(base + 0x44, 4), Some(0));
        assert_eq!(spi.write(base, 4, 0x8000_0000), Some(()));
        assert_eq!(spi.read(base, 4), Some(0x8000_0102));
        assert_eq!(spi.read(base + 4, 4), Some(4));
        assert_eq!(spi.read(base + 0x1c, 4), Some(0));
        assert_eq!(spi.read(base + 1, 4), None);
        assert_eq!(spi.read(base, 2), None);
        assert_eq!(spi.read(other.base, 4), None);
    }
}
