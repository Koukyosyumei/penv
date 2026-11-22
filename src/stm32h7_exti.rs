const CONFIGURABLE: [u32; 3] = [0x003f_ffff, 0x000a_0000, 0x0074_0000];

struct Bank {
    mask: u32,
    rising: u32,
    falling: u32,
    pending: u32,
    input: u32,
}

pub struct H7Exti {
    pub base: u32,
    banks: [Bank; 3],
    outputs: [Option<u8>; Self::LINES],
}

impl H7Exti {
    pub const LINES: usize = 96;
    pub const SIZE: u32 = 0x400;

    pub fn new(base: u32, outputs: [Option<u8>; Self::LINES]) -> Self {
        let banks = CONFIGURABLE.map(|configurable| Bank {
            mask: !configurable,
            rising: 0,
            falling: 0,
            pending: 0,
            input: 0,
        });
        Self {
            base,
            banks,
            outputs,
        }
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    pub fn sample(&mut self, inputs: [u32; 3]) -> impl Iterator<Item = u8> + '_ {
        let active = std::array::from_fn::<_, 3, _>(|index| {
            let bank = &mut self.banks[index];
            let input = inputs[index];
            let edges = (input & !bank.input & bank.rising) | (bank.input & !input & bank.falling);
            bank.input = input;
            bank.pending |= edges & CONFIGURABLE[index] & bank.mask;
            bank.pending | (input & !CONFIGURABLE[index] & bank.mask)
        });
        self.outputs
            .iter()
            .enumerate()
            .filter_map(move |(line, output)| {
                output.filter(|_| active[line / 32] & (1 << (line % 32)) != 0)
            })
    }

    pub fn read(&self, address: u32) -> u32 {
        match address - self.base {
            offset @ (0x00 | 0x20 | 0x40) => self.banks[(offset / 0x20) as usize].rising,
            offset @ (0x04 | 0x24 | 0x44) => self.banks[(offset / 0x20) as usize].falling,
            offset @ (0x80 | 0x90 | 0xa0) => self.banks[((offset - 0x80) / 0x10) as usize].mask,
            offset @ (0x88 | 0x98 | 0xa8) => self.banks[((offset - 0x80) / 0x10) as usize].pending,
            _ => 0,
        }
    }

    pub fn write(&mut self, address: u32, value: u32) {
        match address - self.base {
            offset @ (0x00 | 0x20 | 0x40) => self.banks[(offset / 0x20) as usize].rising = value,
            offset @ (0x04 | 0x24 | 0x44) => self.banks[(offset / 0x20) as usize].falling = value,
            offset @ (0x08 | 0x28 | 0x48) => {
                let bank = &mut self.banks[(offset / 0x20) as usize];
                bank.pending |= value & bank.mask;
            }
            offset @ (0x80 | 0x90 | 0xa0) => {
                self.banks[((offset - 0x80) / 0x10) as usize].mask = value
            }
            offset @ (0x88 | 0x98 | 0xa8) => {
                self.banks[((offset - 0x80) / 0x10) as usize].pending &= !value
            }
            _ => {}
        }
    }
}
