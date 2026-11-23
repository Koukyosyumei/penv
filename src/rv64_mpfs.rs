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
