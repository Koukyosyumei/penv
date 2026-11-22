const ENABLE: u32 = 1;
const DIRECTION: u32 = 1 << 4;
const CIRCULAR: u32 = 1 << 5;
const INCREMENT_PERIPHERAL: u32 = 1 << 6;
const INCREMENT_MEMORY: u32 = 1 << 7;
const MEMORY_TO_MEMORY: u32 = 1 << 14;

pub struct Stm32Dma {
    pub base: u32,
    channels: Vec<Channel>,
    selection: u32,
}

struct Channel {
    irq: Option<u8>,
    config: u32,
    count: u16,
    original: u16,
    peripheral: u32,
    memory: u32,
    current_peripheral: u32,
    current_memory: u32,
    flags: u32,
}

pub struct Transfer {
    pub channel: usize,
    pub from: u32,
    pub to: u32,
    pub read_width: usize,
    pub write_width: usize,
}

impl Stm32Dma {
    pub const SIZE: u32 = 0x100;

    pub fn new(base: u32, irqs: Vec<Option<u8>>) -> Self {
        Self {
            base,
            selection: 0,
            channels: irqs
                .into_iter()
                .map(|irq| Channel {
                    irq,
                    config: 0,
                    count: 0,
                    original: 0,
                    peripheral: 0,
                    memory: 0,
                    current_peripheral: 0,
                    current_memory: 0,
                    flags: 0,
                })
                .collect(),
        }
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    pub fn read(&self, address: u32) -> u32 {
        let offset = address - self.base;
        match offset {
            0 => self
                .channels
                .iter()
                .enumerate()
                .fold(0, |isr, (i, channel)| {
                    isr | ((channel.flags | u32::from(channel.flags != 0)) << (i * 4))
                }),
            4 => 0,
            0xa8 => self.selection,
            _ => {
                let (index, register) = Self::decode(offset);
                let Some(channel) = self.channels.get(index) else {
                    return 0;
                };
                match register {
                    0 => channel.config,
                    4 => u32::from(channel.count),
                    8 => channel.peripheral,
                    12 => channel.memory,
                    _ => 0,
                }
            }
        }
    }

    pub fn write(&mut self, address: u32, value: u32) {
        let offset = address - self.base;
        match offset {
            0 => {}
            4 => {
                for (index, channel) in self.channels.iter_mut().enumerate() {
                    let mask = value >> (index * 4);
                    if mask & 1 != 0 {
                        channel.flags = 0;
                    } else {
                        channel.flags &= !(mask & 0xe);
                    }
                }
            }
            0xa8 => self.selection = value & 0x0fff_ffff,
            _ => {
                let (index, register) = Self::decode(offset);
                let Some(channel) = self.channels.get_mut(index) else {
                    return;
                };
                // Addresses and the reload count are writable while the channel is disabled.
                if channel.config & ENABLE == 0 {
                    match register {
                        0 => channel.config = value & 0x7fff,
                        4 => {
                            channel.count = value as u16;
                            channel.original = channel.count;
                        }
                        8 => {
                            channel.peripheral = value;
                            channel.current_peripheral = value;
                        }
                        12 => {
                            channel.memory = value;
                            channel.current_memory = value;
                        }
                        _ => {}
                    }
                } else if register == 0 {
                    channel.config = (channel.config & !0xf) | (value & 0xf);
                }
            }
        }
    }

    fn decode(offset: u32) -> (usize, u32) {
        let offset = offset.wrapping_sub(8);
        ((offset / 0x14) as usize, offset % 0x14)
    }

    pub fn next_transfer(&self, requests: u8) -> Option<Transfer> {
        let (index, channel) = self
            .channels
            .iter()
            .enumerate()
            .filter(|(index, channel)| {
                channel.config & ENABLE != 0
                    && channel.count != 0
                    && (channel.config & MEMORY_TO_MEMORY != 0 || requests & (1 << index) != 0)
            })
            .max_by_key(|(index, channel)| {
                ((channel.config >> 12) & 3, std::cmp::Reverse(*index))
            })?;
        let peripheral_width = Self::width(channel.config >> 8);
        let memory_width = Self::width(channel.config >> 10);
        let (from, to, read_width, write_width) = if channel.config & DIRECTION != 0 {
            (
                channel.current_memory,
                channel.current_peripheral,
                memory_width,
                peripheral_width,
            )
        } else {
            (
                channel.current_peripheral,
                channel.current_memory,
                peripheral_width,
                memory_width,
            )
        };
        Some(Transfer {
            channel: index,
            from,
            to,
            read_width,
            write_width,
        })
    }

    fn width(bits: u32) -> usize {
        [1, 2, 4, 0][(bits & 3) as usize]
    }

    pub fn complete(&mut self, transfer: Transfer, success: bool) {
        let channel = &mut self.channels[transfer.channel];
        if !success {
            channel.flags |= 8;
            channel.config &= !ENABLE;
            return;
        }
        let (peripheral_width, memory_width) = if channel.config & DIRECTION != 0 {
            (transfer.write_width, transfer.read_width)
        } else {
            (transfer.read_width, transfer.write_width)
        };
        if channel.config & INCREMENT_PERIPHERAL != 0 {
            channel.current_peripheral = channel
                .current_peripheral
                .wrapping_add(peripheral_width as u32);
        }
        if channel.config & INCREMENT_MEMORY != 0 {
            channel.current_memory = channel.current_memory.wrapping_add(memory_width as u32);
        }
        channel.count -= 1;
        if channel.count == channel.original / 2 {
            channel.flags |= 4;
        }
        if channel.count == 0 {
            channel.flags |= 2;
            if channel.config & (CIRCULAR | MEMORY_TO_MEMORY) == CIRCULAR {
                channel.count = channel.original;
                channel.current_memory = channel.memory;
                channel.current_peripheral = channel.peripheral;
            }
        }
    }

    pub fn interrupts(&self) -> u32 {
        self.channels.iter().fold(0, |pending, channel| {
            if channel.flags & channel.config != 0
                && let Some(line) = channel.irq
            {
                pending | (1 << line)
            } else {
                pending
            }
        })
    }
}
