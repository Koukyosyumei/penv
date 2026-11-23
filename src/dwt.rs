use crate::bus::CORTEX_M_STEP_HZ;

pub struct Dwt {
    pub base: u32,
    frequency: u32,
    clocked: bool,
    enabled: bool,
    cycles: u32,
    phase: u64,
}

impl Dwt {
    pub const SIZE: u32 = 0x1000;

    pub fn new(base: u32, frequency: u32) -> Self {
        Self {
            base,
            frequency,
            clocked: true,
            enabled: false,
            cycles: 0,
            phase: 0,
        }
    }

    pub fn reset(&mut self) {
        self.enabled = false;
        self.cycles = 0;
        self.phase = 0;
    }

    pub fn set_clocked(&mut self, clocked: bool) {
        self.clocked = clocked;
    }

    pub fn tick(&mut self) {
        if self.clocked && self.enabled {
            self.phase += u64::from(self.frequency);
            self.cycles = self
                .cycles
                .wrapping_add((self.phase / CORTEX_M_STEP_HZ) as u32);
            self.phase %= CORTEX_M_STEP_HZ;
        }
    }

    pub fn read(&self, address: u32) -> Option<u32> {
        match address.checked_sub(self.base)? {
            0 => Some(u32::from(self.enabled)),
            4 => Some(self.cycles),
            _ => None,
        }
    }

    pub fn write(&mut self, address: u32, value: u32) -> bool {
        match address.checked_sub(self.base) {
            Some(0) => self.enabled = value & 1 != 0,
            Some(4) => self.cycles = value,
            _ => return false,
        }
        true
    }
}
