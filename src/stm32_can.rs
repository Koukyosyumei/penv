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

mod f4 {
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
                0x240..=0x31c => {
                    self.banks[((offset - 0x240) / 8) as usize][(offset / 4 & 1) as usize]
                }
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
                0x180..=0x1ac => {
                    self.tx[((offset - 0x180) / 16) as usize][(offset / 4 & 3) as usize]
                }
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

        // Renode Infrastructure STMCANTest.AnotherTest: construction and reset.
        #[test]
        fn renode_stmcan_another_test() {
            let mut can = StmCan::new(0, None, [None; 4]);
            can.reset();
        }

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
}

pub use f4::{Filters, StmCan};
