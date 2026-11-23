use std::collections::VecDeque;

const INRQ: u32 = 1;
const SLEEP: u32 = 1 << 1;
const RFLM: u32 = 1 << 3;
const RESET: u32 = 1 << 15;
const SLAKI: u32 = 1 << 4;
const EXTENDED_ID: u32 = 1 << 2;
const STANDARD_FRAME_MASK: u32 = 0xffe0_0002;
const LOOPBACK: u32 = 1 << 30;
const FULL: u32 = 1 << 3;
const OVERRUN: u32 = 1 << 4;
const RELEASE: u32 = 1 << 5;
const BANKS: usize = 28;
const BANK_MASK: u32 = (1 << BANKS) - 1;

#[derive(Default)]
struct Fifo {
    messages: VecDeque<[u32; 4]>,
    flags: u32,
}

impl Fifo {
    fn enqueue(&mut self, frame: [u32; 4], locked: bool) {
        if self.messages.len() == 3 {
            self.flags |= OVERRUN;
            if locked {
                return;
            }
            *self.messages.back_mut().unwrap() = frame;
            return;
        }
        self.messages.push_back(frame);
        if self.messages.len() == 3 {
            self.flags |= FULL;
        }
    }

    fn status(&self) -> u32 {
        self.flags | self.messages.len() as u32
    }

    fn write(&mut self, value: u32) {
        self.flags &= !(value & (FULL | OVERRUN));
        if value & RELEASE != 0 {
            self.messages.pop_front();
        }
    }
}

pub struct Filters {
    control: u32,
    list: u32,
    wide: u32,
    fifo: u32,
    active: u32,
    banks: [[u32; 2]; BANKS],
}

impl Default for Filters {
    fn default() -> Self {
        Self {
            control: 0x2a1c_0e01,
            list: 0,
            wide: 0,
            fifo: 0,
            active: 0,
            banks: [[0; 2]; BANKS],
        }
    }
}

impl Filters {
    pub fn read(&self, offset: u32) -> u32 {
        match offset {
            0x200 => self.control,
            0x204 => self.list,
            0x20c => self.wide,
            0x214 => self.fifo,
            0x21c => self.active,
            0x240..=0x31c => self.banks[((offset - 0x240) / 8) as usize][(offset / 4 & 1) as usize],
            _ => 0,
        }
    }

    pub fn write(&mut self, offset: u32, value: u32) {
        let init = self.control & 1 != 0;
        match offset {
            0x200 => self.control = (self.control & !0x3f01) | (value & 0x3f01),
            // Renode preserves the full filter configuration words. Only the
            // banks that exist participate in filter matching.
            0x204 if init => self.list = value,
            0x20c if init => self.wide = value,
            0x214 if init => self.fifo = value,
            0x21c => self.active = value & BANK_MASK,
            0x240..=0x31c => {
                let bank = ((offset - 0x240) / 8) as usize;
                if init || self.active & (1 << bank) == 0 {
                    self.banks[bank][(offset / 4 & 1) as usize] = value;
                }
            }
            _ => {}
        }
    }

    pub fn accept(&self, slave: bool, id: u32) -> Option<(usize, u32)> {
        if self.control & 1 != 0 {
            return None;
        }
        let split = ((self.control >> 8) & 0x3f) as usize;
        let mut next_index = [0, 0];
        let mut best = None;
        for (bank, &[first, second]) in self.banks.iter().enumerate() {
            if (bank >= split) != slave {
                continue;
            }
            let bit = 1 << bank;
            let wide = self.wide & bit != 0;
            let list = self.list & bit != 0;
            let fifo = usize::from(self.fifo & bit != 0);
            let base_index = next_index[fifo];
            let count = (if wide { 1 } else { 2 }) * (if list { 2 } else { 1 });
            next_index[fifo] += count;
            if self.active & bit == 0 {
                continue;
            }
            let index = if wide {
                if list {
                    [first, second].iter().position(|&filter| id == filter)
                } else {
                    (id & second == first & second).then_some(0)
                }
            } else {
                // STID, RTR, IDE and the three most significant EXID bits.
                let short = (id >> 16 & 0xffe0) | (id << 2 & 0x18) | (id >> 18 & 7);
                if list {
                    [first & 0xffff, first >> 16, second & 0xffff, second >> 16]
                        .iter()
                        .position(|&filter| short == filter)
                } else {
                    [first, second].iter().position(|&pair| {
                        let mask = pair >> 16;
                        short & mask == pair & mask
                    })
                }
            };
            if let Some(index) = index {
                let priority = (!wide, !list, bank);
                if best.is_none_or(|(current, _, _)| priority < current) {
                    best = Some((priority, fifo, base_index + index as u32));
                }
            }
        }
        best.map(|(_, fifo, index)| (fifo, index))
    }
}

pub struct StmCan {
    pub base: u32,
    pub master: Option<u32>,
    pub filters: Option<Filters>,
    irq: [Option<u8>; 4],
    control: u32,
    status: u32,
    tx_status: u32,
    interrupt_enable: u32,
    error: u32,
    timing: u32,
    tx: [[u32; 4]; 3],
    rx: [Fifo; 2],
}

impl StmCan {
    pub const MODEL: &str = "CAN.STMCAN";
    pub const SIZE: u32 = 0x400;

    pub fn new(base: u32, master: Option<u32>, irq: [Option<u8>; 4]) -> Self {
        Self {
            base,
            master,
            filters: master.is_none().then(Filters::default),
            irq,
            control: 0x0001_0002,
            status: 0x0000_0c02,
            tx_status: 0,
            interrupt_enable: 0,
            error: 0,
            timing: 0x0123_0000,
            tx: [[0; 4]; 3],
            rx: Default::default(),
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new(self.base, self.master, self.irq);
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    pub fn interrupts(&self) -> impl Iterator<Item = u8> {
        let tx = self.interrupt_enable & 1 != 0 && self.tx_status & 0x0001_0101 != 0;
        let rx = |fifo: usize| {
            let enables = self.interrupt_enable >> (1 + 3 * fifo);
            (enables & 1 != 0 && !self.rx[fifo].messages.is_empty())
                || (enables & 2 != 0 && self.rx[fifo].flags & FULL != 0)
                || (enables & 4 != 0 && self.rx[fifo].flags & OVERRUN != 0)
        };
        let sce = self.interrupt_enable & (1 << 17) != 0 && self.status & SLAKI != 0;
        self.irq
            .into_iter()
            .zip([tx, rx(0), rx(1), sce])
            .filter_map(|(line, high)| if high { line } else { None })
    }

    pub fn read(&self, offset: u32) -> u32 {
        match offset {
            0 => self.control,
            4 => self.status,
            8 => self
                .tx
                .iter()
                .enumerate()
                .fold(self.tx_status, |status, (slot, frame)| {
                    status | (u32::from(frame[0] & 1 == 0) << (26 + slot))
                }),
            0x0c | 0x10 => self.rx[(offset / 4 - 3) as usize].status(),
            0x14 => self.interrupt_enable,
            0x18 => self.error,
            0x1c if self.status & INRQ != 0 => self.timing,
            0x180..=0x1ac => self.tx[((offset - 0x180) / 16) as usize][(offset / 4 & 3) as usize],
            0x1b0..=0x1cc => self.rx[((offset - 0x1b0) / 16) as usize]
                .messages
                .front()
                .map_or(0, |frame| frame[(offset / 4 & 3) as usize]),
            _ => 0,
        }
    }

    pub fn write(&mut self, offset: u32, value: u32) {
        match offset {
            0 if value & RESET != 0 => self.reset(),
            0 => {
                let previous = self.status;
                self.control = value & 0x0001_00ff;
                let mode = if value & INRQ != 0 {
                    INRQ
                } else {
                    value & SLEEP
                };
                self.status = (self.status & !3) | mode;
                if previous & SLEEP == 0 && mode == SLEEP {
                    self.status |= SLAKI;
                }
            }
            4 => self.status &= !(value & SLAKI),
            8 => {
                for (slot, frame) in self.tx.iter_mut().enumerate() {
                    let shift = slot * 8;
                    if value & (1 << shift) != 0 {
                        self.tx_status &= !(0xf << shift);
                    }
                    if value & (0x80 << shift) != 0 && frame[0] & 1 != 0 {
                        frame[0] &= !1;
                        self.tx_status = (self.tx_status & !(0xf << shift)) | (1 << shift);
                    }
                }
            }
            0x0c | 0x10 => self.rx[(offset / 4 - 3) as usize].write(value),
            0x14 => self.interrupt_enable = value & 0x0003_8f7f,
            0x18 => self.error = value & 0x70,
            0x1c if self.status & INRQ != 0 => self.timing = value & 0xc37f_03ff,
            0x180..=0x1ac => {
                let frame = &mut self.tx[((offset - 0x180) / 16) as usize];
                if frame[0] & 1 == 0 {
                    frame[(offset / 4 & 3) as usize] = value;
                }
            }
            _ => {}
        }
    }

    pub fn transmit(&mut self) -> [Option<[u32; 4]>; 3] {
        let loopback = self.timing & LOOPBACK != 0 && self.status & 3 == 0;
        std::array::from_fn(|slot| {
            let frame = &mut self.tx[slot];
            if !loopback || frame[0] & 1 == 0 {
                return None;
            }
            frame[0] &= !1;
            let shift = slot * 8;
            self.tx_status = (self.tx_status & !(0xf << shift)) | (3 << shift);
            let mut received = *frame;
            if received[0] & EXTENDED_ID == 0 {
                received[0] &= STANDARD_FRAME_MASK;
            }
            Some(received)
        })
    }

    pub fn receive(&mut self, fifo: usize, filter: u32, mut frame: [u32; 4]) {
        frame[1] = (frame[1] & 0xf) | (filter << 8);
        self.rx[fifo].enqueue(frame, self.control & RFLM != 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Renode Infrastructure STMCANTest.InitTest.
    #[test]
    fn renode_stmcan_init_test() {
        let mut can = StmCan::new(0, None, [None; 4]);
        can.reset();
        for (offset, expected) in [
            (0x00, 0x0001_0002),
            (0x04, 0x0000_0c02),
            (0x08, 0x1c00_0000),
            (0x0c, 0),
            (0x10, 0),
            (0x14, 0),
            (0x18, 0),
            (0x1c, 0),
        ] {
            assert_eq!(can.read(offset), expected, "offset {offset:#x}");
        }
        can.write(0x00, 0x0001_0001);
        assert_eq!(can.read(0x04), 0x0000_0c01);
        assert_eq!(can.read(0x1c), 0x0123_0000);

        can.reset();
        for mailbox in [0x180, 0x190, 0x1a0] {
            assert_eq!(can.read(mailbox) & 1, 0);
            for field in [4, 8, 12] {
                assert_eq!(can.read(mailbox + field), 0);
            }
        }
        for (field, mask) in [(0, 1), (4, 0x10), (8, u32::MAX), (12, u32::MAX)] {
            for fifo in [0x1b0, 0x1c0] {
                can.write(fifo + field, 0xff00_00ff);
            }
            for fifo in [0x1b0, 0x1c0] {
                assert_eq!(can.read(fifo + field) & mask, 0);
            }
        }

        can.reset();
        let filters = can.filters.as_mut().unwrap();
        assert_eq!(filters.read(0x200), 0x2a1c_0e01);
        for offset in [0x204, 0x20c, 0x214] {
            assert_eq!(filters.read(offset), 0);
            filters.write(offset, 0xff00_00ff);
            assert_eq!(filters.read(offset), 0xff00_00ff);
            filters.write(0x200, 0x2a1c_0e00);
            assert_eq!(filters.read(0x200), 0x2a1c_0e00);
            filters.write(offset, 0x00ff_ff00);
            assert_eq!(filters.read(offset), 0xff00_00ff);
            filters.write(0x200, 0x2a1c_0e01);
        }
        assert_eq!(filters.read(0x21c), 0);
        filters.write(0x21c, 0xf000_0000);
        assert_eq!(filters.read(0x21c), 0);
        for index in 0..28 {
            let offset = 0x240 + index * 8;
            assert_eq!(filters.read(offset), 0);
            filters.write(offset, 0xff00_00ff);
            assert_eq!(filters.read(offset), 0xff00_00ff);
            filters.write(0x200, 0x2a1c_0e00);
            assert_eq!(filters.read(0x200), 0x2a1c_0e00);
            filters.write(offset, 0x00ff_ff00);
            assert_eq!(filters.read(offset), 0x00ff_ff00);
        }
    }

    #[test]
    fn extended_frame_matches_16_bit_list_and_prefers_32_bit_filter() {
        let mut filters = Filters::default();
        let id = (0x123 << 21) | (0b101 << 18) | 4;
        let short = (0x123 << 5) | 0x10 | 0b101;
        filters.write(0x204, 1);
        filters.write(0x20c, 1 << 1);
        filters.write(0x214, 1 << 1);
        filters.write(0x240, short);
        filters.write(0x248, id);
        filters.write(0x24c, u32::MAX);
        filters.write(0x21c, 3);
        assert_eq!(filters.accept(false, id), None);

        filters.write(0x200, 14 << 8);
        assert_eq!(filters.accept(false, id), Some((1, 0)));
        filters.write(0x21c, 1);
        assert_eq!(filters.accept(false, id), Some((0, 0)));
        assert_eq!(filters.accept(false, id | 2), None);
        assert_eq!(filters.accept(true, id), None);
    }

    #[test]
    fn fifo_overrun_preserves_pending_messages_and_can_be_acknowledged() {
        for locked in [false, true] {
            let mut fifo = Fifo::default();
            for index in 1..=3 {
                fifo.enqueue([index, 0, 0, 0], locked);
            }
            assert_eq!(fifo.status(), FULL | 3);
            fifo.write(FULL);
            fifo.enqueue([4, 0, 0, 0], locked);
            assert_eq!(fifo.status(), OVERRUN | 3);
            let indices: Vec<_> = fifo.messages.iter().map(|frame| frame[0]).collect();
            assert_eq!(indices, [1, 2, if locked { 3 } else { 4 }]);
            fifo.write(OVERRUN | RELEASE);
            assert_eq!(fifo.status(), 2);
            assert_eq!(fifo.messages.front().unwrap()[0], 2);
        }
    }
}
