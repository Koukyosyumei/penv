/// Bosch M_CAN configuration and interrupt registers.
pub struct Fdcan {
    pub base: u32,
    pub lines: [Option<u8>; 2],
    registers: [u32; 64],
    fifo0_count: u32,
    fifo0_get: u32,
    fifo0_put: u32,
    tx_fifo: std::collections::VecDeque<u32>,
    tx_put: u32,
    tx_event_count: u32,
    tx_event_get: u32,
    tx_event_put: u32,
    tx_event_lost: bool,
}

impl Fdcan {
    pub const MODEL: &str = "CAN.MCAN";
    pub const SIZE: u32 = 0x400;

    pub fn new(base: u32, lines: [Option<u8>; 2]) -> Self {
        let mut registers = [0; 64];
        registers[0] = 0x3301_1200;
        registers[0x18 / 4] = 1; // INIT at reset
        registers[0x44 / 4] = 0x707; // LEC and DLEC have not changed.
        Self {
            base,
            lines,
            registers,
            fifo0_count: 0,
            fifo0_get: 0,
            fifo0_put: 0,
            tx_fifo: Default::default(),
            tx_put: 0,
            tx_event_count: 0,
            tx_event_get: 0,
            tx_event_put: 0,
            tx_event_lost: false,
        }
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    pub fn interrupts(&self) -> impl Iterator<Item = u8> + '_ {
        let pending = self.registers[0x50 / 4] & self.registers[0x54 / 4];
        let select = self.registers[0x58 / 4];
        let enable = self.registers[0x5c / 4];
        self.lines
            .into_iter()
            .enumerate()
            .filter_map(move |(index, line)| {
                let mask = if index == 0 { !select } else { select };
                line.filter(|_| enable & (1 << index) != 0 && pending & mask != 0)
            })
    }

    pub fn read(&self, address: u32) -> u32 {
        if address - self.base == 0xf4 {
            let capacity = self.tx_event_capacity();
            let full = capacity != 0 && self.tx_event_count == capacity;
            return self.tx_event_count
                | (self.tx_event_get << 8)
                | (self.tx_event_put << 16)
                | (u32::from(full) << 24)
                | (u32::from(self.tx_event_lost) << 25);
        }
        if address - self.base == 0xc4 {
            return self.tx_status();
        }
        if address - self.base == 0xa4 {
            let capacity = self.fifo0_capacity();
            let full = capacity != 0 && self.fifo0_count == capacity;
            return self.fifo0_count
                | self.fifo0_get << 8
                | self.fifo0_put << 16
                | u32::from(full) << 24
                | ((self.registers[0x50 / 4] >> 3) & 1) << 25;
        }
        self.registers
            .get(((address - self.base) / 4) as usize)
            .copied()
            .unwrap_or(0)
    }

    fn tx_event_capacity(&self) -> u32 {
        ((self.registers[0xf0 / 4] >> 16) & 0x3f).min(32)
    }

    #[allow(dead_code)] // Called by the loopback service pending shared wiring.
    fn store_tx_event(&mut self, ram: &mut [u8], identifier: u32, header: u32) {
        if self.registers[0x18 / 4] & (1 << 11) != 0 {
            return;
        }
        let config = self.registers[0xf0 / 4];
        let capacity = self.tx_event_capacity();
        let offset = (config & 0xfffc) as usize + self.tx_event_put as usize * 8;
        let destination = ram
            .get_mut(offset..offset + 8)
            .filter(|_| self.tx_event_count < capacity);
        let Some(destination) = destination else {
            self.tx_event_lost = true;
            self.registers[0x50 / 4] |= 1 << 15;
            return;
        };
        destination[..4].copy_from_slice(&identifier.to_le_bytes());
        let event_header = (header & 0xff3f0000) | (2 << 22);
        destination[4..].copy_from_slice(&event_header.to_le_bytes());
        self.tx_event_put = (self.tx_event_put + 1) % capacity;
        self.tx_event_count += 1;
        let watermark = (config >> 24) & 0x3f;
        self.registers[0x50 / 4] |= (1 << 12)
            | (u32::from(watermark != 0 && self.tx_event_count >= watermark) << 13)
            | (u32::from(self.tx_event_count == capacity) << 14);
    }

    fn acknowledge_tx_event(&mut self, value: u32) {
        let capacity = self.tx_event_capacity();
        let index = value & 0x1f;
        if index >= capacity || self.tx_event_count == 0 {
            return;
        }
        let consumed = (index + capacity - self.tx_event_get) % capacity + 1;
        if consumed <= self.tx_event_count {
            self.tx_event_get = (index + 1) % capacity;
            self.tx_event_count -= consumed;
            if self.tx_event_count == 0 {
                self.registers[0x50 / 4] |= 1 << 11;
            }
        }
    }

    fn tx_bounds(&self) -> (u32, u32) {
        let config = self.registers[0xc0 / 4];
        let start = ((config >> 16) & 0x3f).min(32);
        let end = (start + ((config >> 24) & 0x3f)).min(32);
        (start, end)
    }

    fn tx_queue_mode(&self) -> bool {
        self.registers[0xc0 / 4] & (1 << 30) != 0
    }

    fn tx_status(&self) -> u32 {
        let (start, end) = self.tx_bounds();
        let size = end - start;
        if size == 0 {
            return 0;
        }
        let pending = self.registers[0xcc / 4];
        let free = (start..end).filter(|&slot| pending & (1 << slot) == 0);
        let free_count = free.clone().count() as u32;
        let full = free_count == 0;
        let (get, put) = if self.tx_queue_mode() {
            (0, free.into_iter().next().unwrap_or(0))
        } else {
            let put = if full {
                self.tx_put
            } else {
                (0..size)
                    .map(|offset| start + (self.tx_put - start + offset) % size)
                    .find(|&slot| pending & (1 << slot) == 0)
                    .unwrap()
            };
            (self.tx_fifo.front().copied().unwrap_or(put), put)
        };
        free_count | (get << 8) | (put << 16) | (u32::from(full) << 21)
    }

    fn request_tx(&mut self, mask: u32) {
        if self.registers[0x18 / 4] & 3 != 0 {
            return;
        }
        let (start, end) = self.tx_bounds();
        for slot in 0..end {
            let bit = 1 << slot;
            if mask & bit == 0 || self.registers[0xcc / 4] & bit != 0 {
                continue;
            }
            self.registers[0xcc / 4] |= bit;
            self.registers[0xdc / 4] &= !bit;
            self.registers[0xd8 / 4] &= !bit;
            if slot >= start && !self.tx_queue_mode() {
                self.tx_fifo.push_back(slot);
                self.tx_put = start + (slot - start + 1) % (end - start);
            }
        }
    }

    fn fifo0_capacity(&self) -> u32 {
        ((self.registers[0xa0 / 4] >> 16) & 0x7f).min(64)
    }

    #[allow(dead_code)] // Called by the loopback service pending shared wiring.
    fn fifo0_filter_header(&self, ram: &[u8], identifier: u32) -> Option<u32> {
        let global = self.registers[0x80 / 4];
        let extended = identifier & (1 << 30) != 0;
        let remote = identifier & (1 << 29) != 0;
        if remote && global & (if extended { 1 } else { 2 }) != 0 {
            return None;
        }
        if !extended {
            let configuration = self.registers[0x84 / 4];
            let base = (configuration & 0xfffc) as usize;
            let count = ((configuration >> 16) & 0xff).min(128);
            let id = (identifier >> 18) & 0x7ff;
            for index in 0..count {
                let offset = base + index as usize * 4;
                let filter = u32::from_le_bytes(ram.get(offset..offset + 4)?.try_into().ok()?);
                let action = (filter >> 27) & 7;
                let first = (filter >> 16) & 0x7ff;
                let second = filter & 0x7ff;
                let matches = match filter >> 30 {
                    0 => (first..=second).contains(&id),
                    1 => id == first || id == second,
                    2 => id & second == first & second,
                    _ => false,
                };
                if action != 0 && matches {
                    return (action == 1).then_some(index << 24);
                }
            }
        }
        let unmatched = (global >> if extended { 2 } else { 4 }) & 3;
        (unmatched == 0).then_some(1 << 31)
    }

    // Called with the board's message RAM once pending requests can be serviced.
    #[allow(dead_code)] // Shared SystemBus wiring is owned by penv-arm.
    pub(crate) fn service_loopback(&mut self, ram: &mut [u8]) {
        let control = self.registers[0x18 / 4];
        if control & 1 != 0 || control & 0x80 == 0 || self.registers[0x10 / 4] & 0x10 == 0 {
            return;
        }
        const DATA_BYTES: [usize; 8] = [8, 12, 16, 20, 24, 32, 48, 64];
        let tx = self.registers[0xc0 / 4];
        let tx_size = DATA_BYTES[(self.registers[0xc8 / 4] & 7) as usize];
        let rx_size = DATA_BYTES[(self.registers[0xbc / 4] & 7) as usize];
        let (start, end) = self.tx_bounds();
        let mut fifo_order = [0; 32];
        let fifo_count = self.tx_fifo.len();
        for (target, &slot) in fifo_order.iter_mut().zip(&self.tx_fifo) {
            *target = slot;
        }
        let direct_end = if self.tx_queue_mode() { end } else { start };
        let capacity = self.fifo0_capacity();
        for index in (0..direct_end).chain(fifo_order[..fifo_count].iter().copied()) {
            let bit = 1 << index;
            if self.registers[0xcc / 4] & bit == 0 {
                continue;
            }
            let from = (tx & 0xfffc) as usize + index as usize * (8 + tx_size);
            let Some(frame) = ram.get(from..from + 8 + tx_size) else {
                continue;
            };
            let header = u32::from_le_bytes(frame[4..8].try_into().unwrap());
            if header & (1 << 21) != 0 {
                continue; // CAN FD is outside this classical loopback path.
            }
            let payload = ((header >> 16) & 15).min(8) as usize;
            let mut element = [0; 72];
            element[..4].copy_from_slice(&frame[..4]);
            element[8..8 + payload].copy_from_slice(&frame[8..8 + payload]);
            self.registers[0xcc / 4] &= !bit;
            self.tx_fifo.retain(|&slot| slot != index);
            self.registers[0xd8 / 4] |= bit;
            if self.registers[0xe0 / 4] & bit != 0 {
                self.registers[0x50 / 4] |= 1 << 9;
            }
            let identifier = u32::from_le_bytes(element[..4].try_into().unwrap());
            self.store_tx_event(ram, identifier, header);
            let Some(filter_header) = self.fifo0_filter_header(ram, identifier) else {
                continue;
            };
            if capacity == 0 {
                continue;
            }
            element[4..8].copy_from_slice(&(filter_header | (header & 0x000f0000)).to_le_bytes());
            if self.fifo0_count == capacity {
                self.registers[0x50 / 4] |= 1 << 3;
                continue;
            }
            let to = (self.registers[0xa0 / 4] & 0xfffc) as usize
                + self.fifo0_put as usize * (8 + rx_size);
            if let Some(destination) = ram.get_mut(to..to + 8 + rx_size) {
                destination.copy_from_slice(&element[..8 + rx_size]);
                self.fifo0_put = (self.fifo0_put + 1) % capacity;
                self.fifo0_count += 1;
                self.registers[0x50 / 4] |= 1;
                if self.fifo0_count == capacity {
                    self.registers[0x50 / 4] |= 1 << 2;
                }
            }
        }
    }

    pub fn write(&mut self, address: u32, value: u32) {
        let offset = address - self.base;
        if offset == 0xf8 {
            self.acknowledge_tx_event(value);
            return;
        }
        if offset == 0xa8 {
            let capacity = self.fifo0_capacity();
            let acknowledged = value & 0x3f;
            if capacity != 0 && acknowledged < capacity && self.fifo0_count != 0 {
                let consumed = (acknowledged + capacity - self.fifo0_get) % capacity + 1;
                if consumed <= self.fifo0_count {
                    self.fifo0_get = (acknowledged + 1) % capacity;
                    self.fifo0_count -= consumed;
                }
            }
            return;
        }
        let initialized = self.registers[0x18 / 4] & 3 == 3;
        let Some(register) = self.registers.get_mut((offset / 4) as usize) else {
            return;
        };
        match offset {
            0x00 | 0x04 | 0x40 | 0x44 | 0xa4 | 0xc4 | 0xcc | 0xd8 | 0xdc | 0xf4 => {}
            0x18 => {
                let init = value & 1;
                let changes = if *register & 1 != 0 && init != 0 {
                    value & 2
                } else {
                    0
                };
                let sleep = value & (1 << 4);
                let options = if initialized { value } else { *register };
                *register = init | changes | sleep | (sleep >> 1) | (options & 0xfbe0);
            }
            0x50 => *register &= !value,
            0x54 | 0x58 | 0x5c => *register = value,
            0xc0 if initialized => {
                *register = value;
                self.tx_fifo.clear();
                self.tx_put = self.tx_bounds().0;
                self.registers[0xcc / 4] = 0;
            }
            0xf0 if initialized => {
                *register = value;
                self.tx_event_count = 0;
                self.tx_event_get = 0;
                self.tx_event_put = 0;
                self.tx_event_lost = false;
            }
            0xd0 => self.request_tx(value),
            0xd4 => {
                let cancelled = self.registers[0xcc / 4] & value;
                self.registers[0xdc / 4] |= cancelled;
                self.registers[0xcc / 4] &= !value;
                self.tx_fifo.retain(|&slot| cancelled & (1 << slot) == 0);
                if cancelled & self.registers[0xe4 / 4] != 0 {
                    self.registers[0x50 / 4] |= 1 << 10;
                }
            }
            _ if initialized => *register = value,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event_fifo_fixture() -> (Fdcan, [u8; 128]) {
        let mut can = Fdcan::new(0, [Some(19), None]);
        let mut ram = [0; 128];
        can.write(0x18, 3);
        can.write(0x18, 0xa3);
        can.write(0x10, 1 << 4);
        can.write(0xc0, (1 << 16) | 0x40);
        can.write(0xf0, (1 << 24) | (2 << 16) | 0x60);
        can.write(0x54, 1 << 12);
        can.write(0x5c, 1);
        can.write(0x18, 0xa0);
        ram[0x40..0x44].copy_from_slice(&(0x123u32 << 18).to_le_bytes());
        ram[0x44..0x48].copy_from_slice(&0x5a840000u32.to_le_bytes());
        (can, ram)
    }

    #[test]
    fn tx_event_fifo_stores_header_and_routes_new_entry_interrupt() {
        let (mut can, mut ram) = event_fifo_fixture();
        can.write(0xd0, 1);
        can.service_loopback(&mut ram);
        assert_eq!(&ram[0x60..0x64], &ram[0x40..0x44]);
        assert_eq!(
            u32::from_le_bytes(ram[0x64..0x68].try_into().unwrap()),
            0x5a840000
        );
        assert_eq!(can.read(0xf4), (1 << 16) | 1);
        assert_eq!(can.read(0x50) & 0xf000, 0x3000);
        assert!(can.interrupts().eq([19]));
        can.write(0xf8, 0);
        can.write(0x50, 0x3000);
        assert_eq!(can.read(0xf4), (1 << 16) | (1 << 8));
        assert_eq!(can.read(0x50) & (1 << 11), 1 << 11);
        assert_eq!(can.interrupts().count(), 0);
    }

    #[test]
    fn tx_event_fifo_reports_full_and_loss_without_overwriting() {
        let (mut can, mut ram) = event_fifo_fixture();
        for marker in [1u8, 2, 3] {
            ram[0x47] = marker;
            can.write(0xd0, 1);
            can.service_loopback(&mut ram);
        }
        assert_eq!(ram[0x67], 1);
        assert_eq!(ram[0x6f], 2);
        assert_eq!(can.read(0xf4), (3 << 24) | 2);
        assert_eq!(can.read(0x50) & 0xf000, 0xf000);
        can.write(0xf8, 1);
        assert_eq!(can.read(0xf4) & 0x3f, 0);
    }

    #[test]
    fn tx_fifo_wraps_and_releases_cancelled_slots() {
        let mut can = Fdcan::new(0, [None, None]);
        can.write(0x18, 3);
        can.write(0xc0, (2 << 24) | (1 << 16));
        can.write(0xd0, 1 << 1);
        assert_eq!(can.read(0xcc), 0, "requests ignored in INIT");
        can.write(0x18, 0);
        assert_eq!(can.read(0xc4), 2 | (1 << 8) | (1 << 16));
        can.write(0xd0, 1 << 1);
        assert_eq!(can.read(0xc4), 1 | (1 << 8) | (2 << 16));
        can.write(0xd0, (1 << 2) | (1 << 31));
        assert_eq!(can.read(0xcc), 0b110);
        assert_eq!(can.read(0xc4), (1 << 21) | (1 << 8) | (1 << 16));
        can.write(0xd4, 1 << 1);
        assert_eq!(can.read(0xdc), 1 << 1);
        assert_eq!(can.read(0xc4), 1 | (2 << 8) | (1 << 16));
        can.write(0xd0, 1 << 1);
        assert_eq!(can.read(0xdc), 0);
        assert_eq!(can.read(0xc4), (1 << 21) | (2 << 8) | (2 << 16));
    }

    #[test]
    fn tx_queue_reports_first_available_slot() {
        let mut can = Fdcan::new(0, [None, None]);
        can.write(0x18, 3);
        can.write(0xc0, (1 << 30) | (2 << 24) | (1 << 16));
        can.write(0x18, 0);
        can.write(0xd0, 1 << 2);
        assert_eq!(can.read(0xc4), 1 | (1 << 16));
        can.write(0xd0, 1 << 1);
        assert_eq!(can.read(0xc4), 1 << 21);
        can.write(0xd4, 1 << 2);
        assert_eq!(can.read(0xc4), 1 | (2 << 16));
    }

    fn filtered_frame(id: u32, filters: &[u32], global: u32) -> (u32, [u8; 128]) {
        let mut can = Fdcan::new(0, [None, None]);
        let mut ram = [0; 128];
        can.write(0x18, 3);
        can.write(0x18, 0xa3);
        can.write(0x10, 1 << 4);
        can.write(0x80, global);
        can.write(0x84, (filters.len() as u32) << 16);
        can.write(0xa0, (1 << 16) | 0x20);
        can.write(0xc0, (1 << 16) | 0x40);
        can.write(0x18, 0xa0);
        for (index, filter) in filters.iter().enumerate() {
            ram[index * 4..index * 4 + 4].copy_from_slice(&filter.to_le_bytes());
        }
        ram[0x40..0x44].copy_from_slice(&(id << 18).to_le_bytes());
        ram[0x44..0x48].copy_from_slice(&(1u32 << 16).to_le_bytes());
        ram[0x48] = 0x5a;
        can.write(0xd0, 1);
        can.service_loopback(&mut ram);
        (can.read(0xa4) & 0x7f, ram)
    }

    #[test]
    fn standard_mask_filter_reports_matching_index() {
        let filter = (2 << 30) | (1 << 27) | (0x123 << 16) | 0x7ff;
        let (fill, ram) = filtered_frame(0x123, &[0, filter], 2 << 4);
        assert_eq!(fill, 1);
        assert_eq!(
            u32::from_le_bytes(ram[0x24..0x28].try_into().unwrap()),
            (1 << 24) | (1 << 16)
        );
        assert_eq!(ram[0x28], 0x5a);
        assert_eq!(filtered_frame(0x124, &[filter], 2 << 4).0, 0);
    }

    #[test]
    fn first_matching_standard_filter_wins() {
        let reject_range = (3 << 27) | (0x120 << 16) | 0x125;
        let accept_dual = (1 << 30) | (1 << 27) | (0x123 << 16) | 0x126;
        assert_eq!(filtered_frame(0x123, &[reject_range, accept_dual], 0).0, 0);
        assert_eq!(
            filtered_frame(0x126, &[reject_range, accept_dual], 2 << 4).0,
            1
        );
    }

    #[test]
    fn internal_loopback_copies_classical_frame_and_sets_interrupts() {
        let mut can = Fdcan::new(0, [Some(19), None]);
        let mut ram = [0; 128];
        can.write(0x18, 3);
        can.write(0x18, 0xa3);
        can.write(0x10, 1 << 4);
        can.write(0xa0, (1 << 16) | 0x20);
        can.write(0xc0, (1 << 16) | 0x40);
        can.write(0xe0, 1);
        can.write(0x54, (1 << 9) | 1);
        can.write(0x5c, 1);
        can.write(0x18, 0xa0);
        ram[0x40..0x44].copy_from_slice(&(0x123u32 << 18).to_le_bytes());
        ram[0x44..0x48].copy_from_slice(&(4u32 << 16).to_le_bytes());
        ram[0x48..0x4c].copy_from_slice(&[1, 2, 3, 4]);
        can.write(0xd0, 1);

        can.service_loopback(&mut ram);

        assert_eq!(&ram[0x20..0x24], &ram[0x40..0x44]);
        assert_eq!(
            u32::from_le_bytes(ram[0x24..0x28].try_into().unwrap()),
            0x80040000
        );
        assert_eq!(&ram[0x28..0x2c], &[1, 2, 3, 4]);
        assert_eq!(can.read(0xcc), 0);
        assert_eq!(can.read(0xd8), 1);
        assert_eq!(can.read(0xa4) & 0x7f, 1);
        assert_eq!(can.read(0x50) & 0x201, 0x201);
        assert!(can.interrupts().eq([19]));
        can.write(0xa8, 0);
        can.write(0x50, 0x201);
        assert_eq!(can.read(0xa4) & 0x7f, 0);
        assert_eq!(can.interrupts().count(), 0);
    }
}
