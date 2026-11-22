pub struct Exti {
    pub base: u32,
    outputs: Vec<Option<u8>>,
    direct: u32,
    input: u32,
    levels: u32,
    pending: u32,
    mask: u32,
    event_mask: u32,
    rising: u32,
    falling: u32,
    software: u32,
}

impl Exti {
    pub const SIZE: u32 = 0x400;

    pub fn new(base: u32, outputs: Vec<Option<u8>>, first_direct: u32) -> Self {
        let valid = u32::MAX >> (32 - outputs.len());
        let direct = if first_direct >= 32 {
            0
        } else {
            valid & (u32::MAX << first_direct)
        };
        Self {
            base,
            outputs,
            direct,
            input: 0,
            levels: 0,
            pending: 0,
            mask: 0,
            event_mask: 0,
            rising: 0,
            falling: 0,
            software: 0,
        }
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    pub fn sample(&mut self, inputs: u32) -> u32 {
        let rising = inputs & !self.input;
        let falling = self.input & !inputs;
        self.input = inputs;
        self.pending |=
            ((rising & self.rising) | (falling & self.falling)) & self.mask & !self.direct;
        self.levels = inputs & self.direct;
        let mut asserted = self.levels | self.pending | self.software;
        let mut active = 0;
        while asserted != 0 {
            let line = asserted.trailing_zeros() as usize;
            asserted &= asserted - 1;
            if let Some(Some(irq)) = self.outputs.get(line) {
                active |= 1 << irq;
            }
        }
        active
    }

    pub fn read(&self, address: u32) -> u32 {
        match address - self.base {
            0 => self.mask,
            4 => self.event_mask,
            8 => self.rising,
            12 => self.falling,
            16 => self.software,
            20 => self.pending | self.levels | self.software,
            _ => 0,
        }
    }

    pub fn write(&mut self, address: u32, value: u32) {
        match address - self.base {
            0 => self.mask = value,
            4 => self.event_mask = value,
            8 => self.rising = value,
            12 => self.falling = value,
            16 => self.software |= value & self.mask,
            20 => {
                self.pending &= !value;
                self.software &= !value;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_direct_and_latches_configured_edges_until_cleared() {
        let mut exti = Exti::new(0, vec![Some(4), Some(7)], 1);
        exti.write(0, 1);
        exti.write(8, 1);
        assert_eq!(exti.sample(3), (1 << 4) | (1 << 7));
        assert_eq!(exti.read(20), 3);
        assert_eq!(exti.sample(0), 1 << 4);
        exti.write(20, 1);
        assert_eq!(exti.sample(0), 0);
        exti.write(16, 3);
        assert_eq!(exti.sample(0), 1 << 4);
    }
}
