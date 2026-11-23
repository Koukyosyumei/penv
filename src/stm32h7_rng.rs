/// Deterministic STM32H7 RNG for simulation. Its output is reproducible.
pub struct Rng {
    pub base: u32,
    pub rcc_bound: bool,
    pub irq: Option<u8>,
    control: u32,
    state: u32,
}

impl Rng {
    pub const MODEL: &str = "Miscellaneous.STM32_RNG";
    pub const SIZE: u32 = 0x400;
    const ENABLE: u32 = 1 << 2;
    const IRQ_ENABLE: u32 = 1 << 3;

    pub fn new(base: u32, rcc_bound: bool, irq: Option<u8>) -> Self {
        Self {
            base,
            rcc_bound,
            irq,
            control: 0,
            state: 0x2c91_4f6d,
        }
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    pub fn reset(&mut self) {
        self.control = 0;
    }

    pub fn interrupt(&self) -> Option<u8> {
        self.irq.filter(|_| {
            self.control & (Self::ENABLE | Self::IRQ_ENABLE) == Self::ENABLE | Self::IRQ_ENABLE
        })
    }

    pub fn read(&mut self, address: u32) -> u32 {
        let enabled = self.control & Self::ENABLE != 0;
        match address - self.base {
            0 => self.control,
            4 => u32::from(enabled),
            8 if enabled => {
                self.state ^= self.state << 13;
                self.state ^= self.state >> 17;
                self.state ^= self.state << 5;
                self.state
            }
            _ => 0,
        }
    }

    pub fn write(&mut self, address: u32, value: u32) {
        if address == self.base {
            self.control = value & !(1 << 30);
        }
    }
}
