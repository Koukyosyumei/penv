use crate::{bus::EthernetDma, stm32_uart::Stm32Irq};

const ENABLE: u32 = 1;
const TRANSFER_COMPLETE: u32 = 1 << 1;
const HALF_TRANSFER: u32 = 1 << 2;
const TRANSFER_ERROR: u32 = 1 << 3;
const MEMORY_SOURCE: u32 = 1 << 4;
const CIRCULAR: u32 = 1 << 5;
const PERIPHERAL_INCREMENT: u32 = 1 << 6;
const MEMORY_INCREMENT: u32 = 1 << 7;
const MEMORY_TO_MEMORY: u32 = 1 << 14;

#[derive(Default)]
struct Channel {
    control: u32,
    remaining: u32,
    initial: u32,
    peripheral: u32,
    memory: u32,
    flags: u32,
}

impl Channel {
    fn interrupt(&self) -> bool {
        self.flags & self.control & (TRANSFER_COMPLETE | HALF_TRANSFER | TRANSFER_ERROR) != 0
    }

    fn software_request(&self) -> bool {
        self.control & (ENABLE | MEMORY_TO_MEMORY) == ENABLE | MEMORY_TO_MEMORY
            && self.remaining != 0
    }

    fn transfer(&mut self, sample: Option<(u32, u32)>, dma: &mut EthernetDma<'_>) -> bool {
        if self.control & ENABLE == 0 || self.remaining == 0 {
            return false;
        }
        let peripheral_width = 1usize << ((self.control >> 8) & 3);
        let memory_width = 1usize << ((self.control >> 10) & 3);
        if peripheral_width > 4 || memory_width > 4 {
            return self.fail();
        }

        let index = self.initial - self.remaining;
        let peripheral = if self.control & PERIPHERAL_INCREMENT != 0 {
            self.peripheral
                .wrapping_add(index * peripheral_width as u32)
        } else {
            self.peripheral
        };
        let memory = if self.control & MEMORY_INCREMENT != 0 {
            self.memory.wrapping_add(index * memory_width as u32)
        } else {
            self.memory
        };
        let (source, source_width, destination, destination_width) =
            if self.control & MEMORY_SOURCE == 0 {
                (peripheral, peripheral_width, memory, memory_width)
            } else {
                (memory, memory_width, peripheral, peripheral_width)
            };
        if source as usize & (source_width - 1) != 0
            || destination as usize & (destination_width - 1) != 0
        {
            return self.fail();
        }

        let mut data = [0; 4];
        if let Some((sample_address, sample_value)) = sample {
            if self.control & (MEMORY_SOURCE | MEMORY_TO_MEMORY) != 0 || source != sample_address {
                return false;
            }
            data[..source_width].copy_from_slice(&sample_value.to_le_bytes()[..source_width]);
        } else if let Some(bytes) = dma.read(source, source_width) {
            data[..source_width].copy_from_slice(bytes);
        } else {
            return self.fail();
        }
        if !dma.write(destination, &data[..destination_width]) {
            return self.fail();
        }

        self.remaining -= 1;
        if self.remaining == 0 {
            self.flags |= TRANSFER_COMPLETE;
            if self.control & CIRCULAR != 0 && self.control & MEMORY_TO_MEMORY == 0 {
                self.remaining = self.initial;
            }
        } else if self.remaining == self.initial / 2 {
            self.flags |= HALF_TRANSFER;
        }
        true
    }

    fn fail(&mut self) -> bool {
        self.flags |= TRANSFER_ERROR;
        self.control &= !ENABLE;
        false
    }

    fn configure(&mut self, value: u32) {
        let widths = 0xf << 8;
        let value = value & 0x7fff;
        self.control = if self.control & ENABLE != 0 && value & ENABLE != 0 {
            (value & !widths) | (self.control & widths)
        } else {
            value
        };
    }
}

pub struct Bdma {
    pub base: u32,
    pub rcc_bound: bool,
    channels: [Channel; Self::CHANNELS],
    routes: [Option<Stm32Irq>; Self::CHANNELS],
}

impl Bdma {
    pub const CHANNELS: usize = 8;
    pub const SIZE: u32 = 0x400;

    pub fn new(base: u32, rcc_bound: bool, routes: [Option<Stm32Irq>; Self::CHANNELS]) -> Self {
        Self {
            base,
            rcc_bound,
            channels: std::array::from_fn(|_| Channel::default()),
            routes,
        }
    }

    pub fn reset(&mut self) {
        self.channels = std::array::from_fn(|_| Channel::default());
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    pub fn interrupts(&self) -> impl Iterator<Item = Stm32Irq> + '_ {
        self.channels
            .iter()
            .zip(self.routes)
            .filter_map(|(channel, route)| route.filter(|_| channel.interrupt()))
    }

    pub fn tick(&mut self, dma: &mut EthernetDma<'_>) {
        for channel in &mut self.channels {
            if channel.software_request() {
                channel.transfer(None, dma);
            }
        }
    }

    pub(crate) fn needs_tick(&self) -> bool {
        self.channels.iter().any(Channel::software_request)
    }

    pub fn request(
        &mut self,
        channel: usize,
        source: u32,
        value: u32,
        dma: &mut EthernetDma<'_>,
    ) -> bool {
        self.channels
            .get_mut(channel)
            .is_some_and(|channel| channel.transfer(Some((source, value)), dma))
    }

    pub fn read(&self, address: u32) -> u32 {
        let offset = address - self.base;
        if offset == 0 {
            return self
                .channels
                .iter()
                .enumerate()
                .fold(0, |word, (index, channel)| {
                    let flags = channel.flags | u32::from(channel.interrupt());
                    word | (flags << (index * 4))
                });
        }
        let Some((index, register)) = Self::register(offset) else {
            return 0;
        };
        let channel = &self.channels[index];
        match register {
            0 => channel.control,
            4 => channel.remaining,
            8 => channel.peripheral,
            12 => channel.memory,
            _ => 0,
        }
    }

    pub fn write(&mut self, address: u32, value: u32) {
        let offset = address - self.base;
        if offset == 4 {
            for (index, channel) in self.channels.iter_mut().enumerate() {
                let clear = (value >> (index * 4)) & 0xf;
                channel.flags &= if clear & 1 != 0 { 0 } else { !clear };
            }
            return;
        }
        let Some((index, register)) = Self::register(offset) else {
            return;
        };
        let channel = &mut self.channels[index];
        match register {
            0 => channel.configure(value),
            4 => {
                channel.remaining = value & 0xffff;
                channel.initial = channel.remaining;
            }
            8 => channel.peripheral = value,
            12 => channel.memory = value,
            _ => {}
        }
    }

    fn register(offset: u32) -> Option<(usize, u32)> {
        let offset = offset.checked_sub(8)?;
        let index = (offset / 0x14) as usize;
        (index < Self::CHANNELS).then_some((index, offset % 0x14))
    }
}
