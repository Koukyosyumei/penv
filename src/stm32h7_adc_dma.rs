use crate::bus::EthernetDma;

const ENABLE: u32 = 1;
const CIRCULAR: u32 = 1 << 8;
const INCREMENT_PERIPHERAL: u32 = 1 << 9;
const INCREMENT_MEMORY: u32 = 1 << 10;
const DOUBLE_BUFFER: u32 = 1 << 18;
const COMPLETE: u32 = 1 << 5;
const HALF: u32 = 1 << 4;
const TRANSFER_ERROR: u32 = 1 << 3;

#[derive(Clone, Copy)]
pub struct DmaRequest {
    pub endpoint: u32,
    pub id: u8,
}

#[derive(Clone, Copy)]
pub struct StreamTarget {
    pub base: u32,
    pub stream: usize,
}

pub struct DmaMux {
    pub base: u32,
    configs: Vec<u32>,
    generators: Vec<u32>,
    targets: Vec<Option<StreamTarget>>,
}

impl DmaMux {
    pub const SIZE: u32 = 0x400;

    pub fn new(base: u32, targets: Vec<Option<StreamTarget>>, generators: usize) -> Self {
        Self {
            base,
            configs: vec![0; targets.len()],
            generators: vec![0; generators],
            targets,
        }
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    pub fn reset(&mut self) {
        self.configs.fill(0);
        self.generators.fill(0);
    }

    pub fn targets(&self, request: u8) -> impl Iterator<Item = StreamTarget> + '_ {
        self.configs
            .iter()
            .zip(&self.targets)
            .filter_map(move |(&value, target)| {
                target.filter(|_| value & 0x7f == u32::from(request))
            })
    }

    pub fn read(&self, address: u32) -> u32 {
        let offset = (address - self.base) as usize;
        match offset {
            0..=0x7c => self.configs.get(offset / 4).copied().unwrap_or(0),
            0x100..=0x13c => self
                .generators
                .get((offset - 0x100) / 4)
                .copied()
                .unwrap_or(0),
            _ => 0,
        }
    }

    pub fn write(&mut self, address: u32, value: u32) {
        let offset = (address - self.base) as usize;
        match offset {
            0..=0x7c => {
                if let Some(register) = self.configs.get_mut(offset / 4) {
                    *register = value & 0x03ff_037f;
                }
            }
            0x100..=0x13c => {
                if let Some(register) = self.generators.get_mut((offset - 0x100) / 4) {
                    *register = value & 0x00ff_0107;
                }
            }
            _ => {}
        }
    }
}

struct Stream {
    control: u32,
    remaining: u32,
    initial: u32,
    peripheral: u32,
    memory: [u32; 2],
    fifo: u32,
    status: u32,
}

impl Default for Stream {
    fn default() -> Self {
        Self {
            control: 0,
            remaining: 0,
            initial: 0,
            peripheral: 0,
            memory: [0; 2],
            fifo: 0x21,
            status: 0,
        }
    }
}

impl Stream {
    fn transfer(&mut self, source: u32, value: u32, dma: &mut EthernetDma<'_>) -> bool {
        if self.control & ENABLE == 0 || self.remaining == 0 || self.control & (3 << 6) != 0 {
            return false;
        }
        let peripheral_size = (self.control >> 11) & 3;
        let memory_size = (self.control >> 13) & 3;
        let peripheral_width = 1u32 << peripheral_size;
        let memory_width = 1usize << memory_size;
        let index = self.initial - self.remaining;
        let from = self
            .peripheral
            .wrapping_add(if self.control & INCREMENT_PERIPHERAL != 0 {
                index * peripheral_width
            } else {
                0
            });
        let bank = usize::from(self.control & DOUBLE_BUFFER != 0 && self.control & (1 << 19) != 0);
        let to = self.memory[bank].wrapping_add(if self.control & INCREMENT_MEMORY != 0 {
            index * memory_width as u32
        } else {
            0
        });
        if from != source {
            return false;
        }
        if peripheral_size == 3 || memory_size == 3 || to as usize & (memory_width - 1) != 0 {
            self.status |= TRANSFER_ERROR;
            self.control &= !ENABLE;
            return false;
        }
        let source_mask = u32::MAX >> (32 - 8 * peripheral_width);
        let bytes = (value & source_mask).to_le_bytes();
        if !dma.write(to, &bytes[..memory_width]) {
            self.status |= TRANSFER_ERROR;
            self.control &= !ENABLE;
            return false;
        }
        self.remaining -= 1;
        if self.initial > 1 && self.remaining == self.initial / 2 {
            self.status |= HALF;
        }
        if self.remaining == 0 {
            self.status |= COMPLETE;
            if self.control & (CIRCULAR | DOUBLE_BUFFER) != 0 {
                if self.control & DOUBLE_BUFFER != 0 {
                    self.control ^= 1 << 19;
                }
                self.remaining = self.initial;
            } else {
                self.control &= !ENABLE;
            }
        }
        true
    }
}

pub struct AdcDma {
    pub base: u32,
    pub clock_mask: u32,
    streams: [Stream; Self::STREAMS],
    irq: [Option<u8>; Self::STREAMS],
}

impl AdcDma {
    pub const SIZE: u32 = 0x400;
    pub const STREAMS: usize = 8;

    pub fn new(base: u32, clock_mask: u32, irq: [Option<u8>; Self::STREAMS]) -> Self {
        Self {
            base,
            clock_mask,
            streams: std::array::from_fn(|_| Stream::default()),
            irq,
        }
    }

    pub fn reset(&mut self) {
        self.streams = std::array::from_fn(|_| Stream::default());
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    pub fn interrupts(&self) -> impl Iterator<Item = u8> + '_ {
        self.streams
            .iter()
            .zip(self.irq)
            .filter_map(|(stream, irq)| {
                irq.filter(|_| {
                    stream.status & (stream.control << 1) & (HALF | COMPLETE | TRANSFER_ERROR) != 0
                })
            })
    }

    pub fn request(
        &mut self,
        stream: usize,
        source: u32,
        value: u32,
        dma: &mut EthernetDma<'_>,
    ) -> bool {
        self.streams[stream].transfer(source, value, dma)
    }

    fn stream(offset: u32) -> Option<(usize, u32)> {
        let offset = offset.checked_sub(0x10)?;
        let index = (offset / 0x18) as usize;
        (index < Self::STREAMS).then_some((index, offset % 0x18))
    }

    pub fn read(&self, address: u32) -> u32 {
        let offset = address - self.base;
        if matches!(offset, 0 | 4) {
            let index = offset as usize;
            return self.streams[index..index + 4]
                .iter()
                .zip([0, 6, 16, 22])
                .fold(0, |word, (stream, shift)| word | (stream.status << shift));
        }
        let Some((index, register)) = Self::stream(offset) else {
            return 0;
        };
        let stream = &self.streams[index];
        match register {
            0 => stream.control,
            4 => stream.remaining,
            8 => stream.peripheral,
            12 => stream.memory[0],
            16 => stream.memory[1],
            20 => stream.fifo,
            _ => 0,
        }
    }

    pub fn write(&mut self, address: u32, value: u32) {
        let offset = address - self.base;
        if matches!(offset, 8 | 12) {
            let index = (offset - 8) as usize;
            for (stream, shift) in self.streams[index..index + 4]
                .iter_mut()
                .zip([0, 6, 16, 22])
            {
                stream.status &= !((value >> shift) & 0x3f);
            }
            return;
        }
        let Some((index, register)) = Self::stream(offset) else {
            return;
        };
        let stream = &mut self.streams[index];
        match register {
            0 => {
                if stream.control & ENABLE == 0 && value & ENABLE != 0 {
                    stream.initial = stream.remaining;
                }
                stream.control = value & 0x0fff_ffff;
            }
            4 if stream.control & ENABLE == 0 => stream.remaining = value & 0xffff,
            8 if stream.control & ENABLE == 0 => stream.peripheral = value,
            12 if stream.control & ENABLE == 0 => stream.memory[0] = value,
            16 if stream.control & ENABLE == 0 => stream.memory[1] = value,
            20 => stream.fifo = (stream.fifo & 0x38) | (value & 0x87),
            _ => {}
        }
    }
}
