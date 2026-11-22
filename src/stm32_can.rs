use std::collections::VecDeque;

const INIT: u32 = 1;
const SLEEP: u32 = 2;
const FIFO_LOCK: u32 = 1 << 3;
const LOOPBACK: u32 = 1 << 30;
const SILENT: u32 = 1 << 31;
const REQUEST_COMPLETE: u32 = 0x0001_0101;
const MAILBOX_EMPTY: u32 = 7 << 26;

pub struct Stm32Can {
    pub base: u32,
    pub size: u32,
    pub routes: [u32; 4],
    control: u32,
    status: u32,
    tx_status: u32,
    irq_enable: u32,
    error_status: u32,
    timing: u32,
    tx: [[u32; 4]; 3],
    rx: [VecDeque<[u32; 4]>; 2],
    rx_flags: [u32; 2],
    filter_control: u32,
    filter_list: u32,
    filter_scale: u32,
    filter_fifo: u32,
    filter_enable: u32,
    filters: [[u32; 2]; 28],
}

impl Stm32Can {
    pub const SIZE: u32 = 0x400;

    pub fn new(base: u32, size: u32, routes: [u32; 4]) -> Self {
        Self {
            base,
            size,
            routes,
            control: 0x0001_0002,
            status: 0x0000_0c02,
            tx_status: 0,
            irq_enable: 0,
            error_status: 0,
            timing: 0x0123_0000,
            tx: [[0; 4]; 3],
            rx: [VecDeque::new(), VecDeque::new()],
            rx_flags: [0; 2],
            filter_control: 0x2a1c_0e01,
            filter_list: 0,
            filter_scale: 0,
            filter_fifo: 0,
            filter_enable: 0,
            filters: [[0; 2]; 28],
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new(self.base, self.size, self.routes);
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < self.size
    }

    pub fn read(&self, address: u32) -> u32 {
        match address - self.base {
            0 => self.control,
            4 => self.status,
            8 => self.tx_status | MAILBOX_EMPTY,
            0x0c => self.fifo_status(0),
            0x10 => self.fifo_status(1),
            0x14 => self.irq_enable,
            0x18 => self.error_status,
            0x1c if self.status & INIT != 0 => self.timing,
            0x200 => self.filter_control,
            0x204 => self.filter_list,
            0x20c => self.filter_scale,
            0x214 => self.filter_fifo,
            0x21c => self.filter_enable,
            offset @ 0x180..=0x1ac => {
                let offset = (offset - 0x180) as usize / 4;
                self.tx[offset / 4][offset % 4]
            }
            offset @ 0x1b0..=0x1cc => {
                let offset = (offset - 0x1b0) as usize / 4;
                self.rx[offset / 4]
                    .front()
                    .map_or(0, |frame| frame[offset % 4])
            }
            offset @ 0x240..=0x31c => {
                let offset = (offset - 0x240) as usize / 4;
                self.filters[offset / 2][offset % 2]
            }
            _ => 0,
        }
    }

    pub fn write(&mut self, address: u32, value: u32) {
        match address - self.base {
            0 if value & (1 << 15) != 0 => *self = Self::new(self.base, self.size, self.routes),
            0 => {
                self.control = value & 0x0001_00ff;
                self.status = (self.status & !(INIT | SLEEP)) | (self.control & (INIT | SLEEP));
            }
            4 => self.status &= !(value & 0x1c),
            8 => {
                let complete = value & REQUEST_COMPLETE;
                self.tx_status &= !(complete | (complete << 1) | (complete << 2) | (complete << 3));
            }
            0x0c => self.release_fifo(0, value),
            0x10 => self.release_fifo(1, value),
            0x14 => self.irq_enable = value & 0x0003_8f7f,
            0x18 => self.error_status = (self.error_status & !(7 << 4)) | (value & (7 << 4)),
            0x1c if self.status & INIT != 0 => self.timing = value & 0xc37f_03ff,
            0x180..=0x1ac => {
                let offset = ((address - self.base - 0x180) / 4) as usize;
                let mailbox = offset / 4;
                let register = offset % 4;
                self.tx[mailbox][register] = value;
                if register == 0 && value & 1 != 0 && self.status & (INIT | SLEEP) == 0 {
                    self.transmit(mailbox);
                }
            }
            0x200 => self.filter_control = (self.filter_control & !0x3f01) | (value & 0x3f01),
            offset @ (0x204 | 0x20c | 0x214 | 0x21c) if self.filter_control & 1 != 0 => {
                let register = match offset {
                    0x204 => &mut self.filter_list,
                    0x20c => &mut self.filter_scale,
                    0x214 => &mut self.filter_fifo,
                    _ => &mut self.filter_enable,
                };
                *register = value & 0x0fff_ffff;
            }
            offset @ 0x240..=0x31c if self.filter_control & 1 != 0 => {
                let offset = (offset - 0x240) as usize / 4;
                self.filters[offset / 2][offset % 2] = value;
            }
            _ => {}
        }
    }

    fn transmit(&mut self, mailbox: usize) {
        self.tx[mailbox][0] &= !1;
        let shift = mailbox * 8;
        self.tx_status = (self.tx_status & !(0xf << shift)) | (3 << shift);
        if self.timing & LOOPBACK != 0 {
            self.receive(self.tx[mailbox]);
        } else if self.timing & SILENT == 0 {
            self.error_status = (self.error_status & !(7 << 4)) | (5 << 4);
            self.status |= 1 << 2;
        }
    }

    fn receive(&mut self, mut frame: [u32; 4]) {
        let Some(bank) = self.accepting_filter(frame[0]) else {
            return;
        };
        let fifo = ((self.filter_fifo >> bank) & 1) as usize;
        frame[1] = (frame[1] & 0xf) | ((bank as u32) << 8);
        if self.rx[fifo].len() == 3 {
            self.rx_flags[fifo] |= 1 << 4;
            if self.control & FIFO_LOCK != 0 {
                return;
            }
            self.rx[fifo].pop_back();
        }
        self.rx[fifo].push_back(frame);
        if self.rx[fifo].len() == 3 {
            self.rx_flags[fifo] |= 1 << 3;
        }
    }

    fn accepting_filter(&self, id: u32) -> Option<usize> {
        if self.filter_control & 1 != 0 {
            return None;
        }
        let short = ((id >> 16) & 0xffe0) | ((id & 2) << 3) | ((id & 4) << 1) | ((id >> 18) & 7);
        (0..28)
            .filter(|bank| self.filter_enable & (1 << bank) != 0)
            .filter_map(|bank| {
                let bit = 1 << bank;
                let list = self.filter_list & bit != 0;
                let wide = self.filter_scale & bit != 0;
                let [first, second] = self.filters[bank];
                let matches = match (wide, list) {
                    (true, true) => [first, second].contains(&id),
                    (true, false) => (id ^ first) & second == 0,
                    (false, true) => [first >> 16, first & 0xffff, second >> 16, second & 0xffff]
                        .contains(&short),
                    (false, false) => {
                        (short ^ (first & 0xffff)) & (first >> 16) == 0
                            || (short ^ (second & 0xffff)) & (second >> 16) == 0
                    }
                };
                matches.then_some((!wide, !list, bank))
            })
            .min()
            .map(|(_, _, bank)| bank)
    }

    fn fifo_status(&self, fifo: usize) -> u32 {
        self.rx_flags[fifo] | self.rx[fifo].len() as u32
    }

    fn release_fifo(&mut self, fifo: usize, value: u32) {
        self.rx_flags[fifo] &= !(value & 0x18);
        if value & (1 << 5) != 0 {
            self.rx[fifo].pop_front();
        }
    }

    pub fn interrupts(&self) -> u32 {
        let mut irqs = 0;
        if self.irq_enable & 1 != 0 && self.tx_status & REQUEST_COMPLETE != 0 {
            irqs |= self.routes[0];
        }
        for fifo in 0..2 {
            let sources = (u32::from(!self.rx[fifo].is_empty()) | (self.rx_flags[fifo] >> 2))
                << (1 + fifo * 3);
            if self.irq_enable & sources != 0 {
                irqs |= self.routes[fifo + 1];
            }
        }
        if self.irq_enable & (1 << 15) != 0 && self.status & (1 << 2) != 0 {
            irqs |= self.routes[3];
        }
        irqs
    }
}
