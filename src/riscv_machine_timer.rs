pub(crate) struct RiscvMachineTimer {
    pub base: u32,
    pub hart: usize,
    pub time_provider: bool,
    time: u64,
    compare: u64,
    frequency: u32,
    fractional_ticks: u64,
}

impl RiscvMachineTimer {
    pub const SIZE: u32 = 0x10;

    pub(crate) fn reset(&mut self) {
        self.time = 0;
        self.compare = u64::MAX;
        self.fractional_ticks = 0;
    }

    pub fn new(base: u32, hart: usize, time_provider: bool, frequency: u32) -> Self {
        Self {
            base,
            hart,
            time_provider,
            time: 0,
            compare: u64::MAX,
            frequency,
            fractional_ticks: 0,
        }
    }

    pub(crate) fn set_frequency(&mut self, frequency: u32) {
        self.frequency = frequency;
    }

    pub fn tick(&mut self, rate: u32) -> bool {
        let ticks = self.fractional_ticks + u64::from(self.frequency);
        self.time = self.time.wrapping_add(ticks / u64::from(rate));
        self.fractional_ticks = ticks % u64::from(rate);
        self.interrupt_pending()
    }

    pub fn interrupt_pending(&self) -> bool {
        self.time >= self.compare
    }

    pub fn time(&self) -> u64 {
        self.time
    }

    fn register_offset(&self, address: u32, width: usize) -> Option<u32> {
        let offset = address.wrapping_sub(self.base);
        (matches!(width, 4 | 8) && offset < Self::SIZE && offset % width as u32 == 0)
            .then_some(offset)
    }

    pub fn read(&self, address: u32, width: usize) -> Option<u64> {
        let offset = self.register_offset(address, width)?;
        let value = if offset < 8 { self.time } else { self.compare };
        Some(if width == 8 {
            value
        } else {
            u64::from((value >> ((offset & 4) * 8)) as u32)
        })
    }

    pub fn write(&mut self, address: u32, width: usize, value: u64) -> Option<()> {
        let offset = self.register_offset(address, width)?;
        let register = if offset < 8 {
            &mut self.time
        } else {
            &mut self.compare
        };
        if width == 8 {
            *register = value;
        } else {
            let shift = (offset & 4) * 8;
            *register =
                (*register & !(u64::from(u32::MAX) << shift)) | (u64::from(value as u32) << shift);
        }
        Some(())
    }
}

#[cfg(test)]
mod tests {
    use super::RiscvMachineTimer;

    #[test]
    fn split_register_access_preserves_other_half_and_sets_interrupt_at_compare() {
        let mut timer = RiscvMachineTimer::new(0x8000, 0, true, 3);
        assert_eq!(timer.read(0x8008, 8), Some(u64::MAX));
        timer.write(0x800c, 4, 1).unwrap();
        timer.write(0x8008, 4, 0).unwrap();
        timer.write(0x8000, 8, u64::from(u32::MAX)).unwrap();
        assert!(!timer.tick(4));
        assert!(timer.tick(4));
        assert_eq!(timer.read(0x8000, 4), Some(0));
        assert_eq!(timer.read(0x8004, 4), Some(1));
        assert_eq!(timer.read(0x8008, 8), Some(1 << 32));
        assert_eq!(timer.read(0x8001, 4), None);
    }
}
