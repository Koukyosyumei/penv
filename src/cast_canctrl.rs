use std::collections::VecDeque;

use crate::{bus_translation::NativePeripheral, dma_engine::Width};

const BUFFER: usize = 0x50;
const TX_SIZE: usize = 72;
const STATUS: usize = 0xa0;
const ENABLE: usize = 0xa4;
const FLAGS: usize = 0xa5;
const FILTER_CONTROL: usize = 0xb4;
const FILTER_WORD: usize = 0xb8;
const CAPACITY: usize = 16;
const FD_LENGTHS: [usize; 16] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 12, 16, 20, 24, 32, 48, 64];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub id: u32,
    pub data: Vec<u8>,
    pub extended: bool,
    pub fd: bool,
    pub bit_rate_switch: bool,
    pub remote: bool,
}

type FrameHandler = Box<dyn FnMut(Frame)>;

pub struct CastCan {
    regs: [u8; 0x100],
    primary: [u8; TX_SIZE],
    secondary: [u8; TX_SIZE],
    queue: VecDeque<[u8; TX_SIZE]>,
    masks: [u32; CAPACITY],
    codes: [u32; CAPACITY],
    counter: u32,
    on_sent: Option<FrameHandler>,
}

impl Default for CastCan {
    fn default() -> Self {
        let mut can = Self {
            regs: [0; 0x100],
            primary: [0; TX_SIZE],
            secondary: [0; TX_SIZE],
            queue: VecDeque::new(),
            masks: [0; CAPACITY],
            codes: [0; CAPACITY],
            counter: 0,
            on_sent: None,
        };
        can.reset();
        can
    }
}

impl CastCan {
    pub fn reset(&mut self) {
        self.regs.fill(0);
        for (offset, value) in [
            (0xa0, 0x80),
            (0xa2, 0x84),
            (0xa4, 0xfe),
            (0xa7, 0x1b),
            (0xa8, 3),
            (0xa9, 2),
            (0xaa, 2),
            (0xab, 1),
            (0xb5, 2),
            (0xb6, 1),
            (0xbc, 9),
            (0xbd, 7),
            (0xd0, 1),
        ] {
            self.regs[offset] = value;
        }
        self.primary.fill(0);
        self.secondary.fill(0);
        self.queue.clear();
        self.codes.fill(0);
        self.masks.fill(0);
        self.masks[0] = 0x1fff_ffff;
        self.counter = 0;
    }

    pub fn on_frame_sent(&mut self, handler: impl FnMut(Frame) + 'static) {
        self.on_sent = Some(Box::new(handler));
    }

    pub fn irq(&self) -> bool {
        self.regs[FLAGS] & (self.regs[ENABLE] | 1) != 0
    }

    fn filter_bank(&mut self) -> &mut [u32; CAPACITY] {
        if self.regs[FILTER_CONTROL] & 0x20 != 0 {
            &mut self.masks
        } else {
            &mut self.codes
        }
    }

    pub fn read_byte(&mut self, offset: usize) -> u8 {
        match offset {
            FILTER_WORD..=0xbb => {
                let filter = usize::from(self.regs[FILTER_CONTROL] & 0xf);
                self.filter_bank()[filter].to_le_bytes()[offset - FILTER_WORD]
            }
            0xa2 => {
                self.regs[offset]
                    | match self.queue.len() {
                        0 => 0,
                        1..=8 => 1,
                        9..=15 => 2,
                        _ => 3,
                    }
            }
            _ => self.regs.get(offset).copied().unwrap_or(0),
        }
    }

    pub fn write_byte(&mut self, offset: usize, value: u8) {
        self.write_transaction(offset, &[value]);
    }

    fn write_transaction(&mut self, offset: usize, bytes: &[u8]) {
        let mut command = None;
        let mut queue_next = false;
        for (index, &value) in bytes.iter().enumerate() {
            match offset + index {
                at if (BUFFER..BUFFER + TX_SIZE).contains(&at) => {
                    let buffer = if self.regs[0xa1] & 0x80 != 0 {
                        &mut self.secondary
                    } else {
                        &mut self.primary
                    };
                    buffer[at - BUFFER] = value;
                }
                STATUS => {
                    let entering_reset = value & 0x80 != 0 && self.regs[STATUS] & 0x80 == 0;
                    if entering_reset {
                        self.queue.clear();
                        self.regs[FLAGS] = 0;
                    }
                    self.regs[STATUS] = value;
                }
                0xa1 => {
                    self.regs[0xa1] = value & 0x80;
                    command = Some(value);
                }
                0xa2 => {
                    self.regs[0xa2] = self.regs[0xa2] & 0x84 | value & 0x20;
                    queue_next = value & 0x40 != 0;
                }
                FLAGS => self.regs[FLAGS] &= !value,
                ENABLE => self.regs[ENABLE] = value & 0xfe,
                FILTER_WORD..=0xbb if self.regs[STATUS] & 0x80 != 0 => {
                    let filter = usize::from(self.regs[FILTER_CONTROL] & 0xf);
                    let mut word = self.filter_bank()[filter].to_le_bytes();
                    word[offset + index - FILTER_WORD] = value;
                    self.filter_bank()[filter] = u32::from_le_bytes(word);
                }
                FILTER_WORD..=0xbb | 0xbc | 0xbd | 0..=0x4f => {}
                at if at < self.regs.len() => self.regs[at] = value,
                _ => {}
            }
        }
        if self.regs[STATUS] & 0x80 != 0 {
            return;
        }
        if queue_next && self.queue.len() < CAPACITY {
            self.queue.push_back(self.secondary);
        }
        if let Some(value) = command {
            if value & 1 != 0 {
                self.queue.clear();
                self.regs[FLAGS] |= 1;
            } else if value & 6 != 0 {
                while let Some(slot) = self.queue.pop_front() {
                    self.send(slot, 4);
                    if value & 2 == 0 {
                        break;
                    }
                }
            }
            if value & 0x10 != 0 {
                self.send(self.primary, 8);
            }
        }
    }

    fn send(&mut self, slot: [u8; TX_SIZE], interrupt: u8) {
        let control = slot[4];
        let fd = control & 0x20 != 0;
        let extended = control & 0x80 != 0;
        let dlc = usize::from(control & 0xf);
        let len = if fd { FD_LENGTHS[dlc] } else { dlc.min(8) };
        let remote = !fd && control & 0x40 != 0;
        let frame = Frame {
            id: u32::from_le_bytes(slot[..4].try_into().unwrap())
                & if extended { 0x1fff_ffff } else { 0x7ff },
            extended,
            fd,
            remote,
            bit_rate_switch: fd && control & 0x10 != 0,
            data: if remote {
                vec![0; len]
            } else {
                slot[8..8 + len].to_vec()
            },
        };
        self.regs[FLAGS] |= self.regs[ENABLE] & interrupt;
        if let Some(handler) = &mut self.on_sent {
            handler(frame);
        }
    }
}

impl NativePeripheral for CastCan {
    fn read(&mut self, region: Option<&str>, offset: u64) -> u64 {
        self.read_width(region, offset, Width::Byte)
    }

    fn write(&mut self, region: Option<&str>, offset: u64, value: u64) {
        self.write_width(region, offset, Width::Byte, value);
    }

    fn read_width(&mut self, region: Option<&str>, offset: u64, width: Width) -> u64 {
        let mut bytes = [0; 8];
        for (i, byte) in bytes[..width as usize].iter_mut().enumerate() {
            let at = offset as usize + i;
            *byte = if region == Some("counter") {
                if (4..8).contains(&at) {
                    self.counter.to_le_bytes()[at - 4]
                } else {
                    0
                }
            } else {
                self.read_byte(at)
            };
        }
        u64::from_le_bytes(bytes)
    }

    fn write_width(&mut self, region: Option<&str>, offset: u64, width: Width, value: u64) {
        let bytes = value.to_le_bytes();
        if region != Some("counter") {
            self.write_transaction(offset as usize, &bytes[..width as usize]);
            return;
        }
        let mut counter = self.counter.to_le_bytes();
        for (i, &byte) in bytes[..width as usize].iter().enumerate() {
            match offset as usize + i {
                0 if byte & 4 != 0 => counter = [0; 4],
                at @ 4..=7 => counter[at - 4] = byte,
                _ => {}
            }
        }
        self.counter = u32::from_le_bytes(counter);
    }
}
