/// Bosch M_CAN configuration and interrupt registers.
pub struct Fdcan {
    pub base: u32,
    pub lines: [Option<u8>; 2],
    registers: [u32; 64],
    fifo0_count: u32,
    fifo0_get: u32,
    fifo0_put: u32,
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

    fn fifo0_capacity(&self) -> u32 {
        ((self.registers[0xa0 / 4] >> 16) & 0x7f).min(64)
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
        let tx_count = (((tx >> 16) & 0x3f) + ((tx >> 24) & 0x3f)).min(32);
        let capacity = self.fifo0_capacity();
        for index in 0..tx_count {
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
            element[4..8].copy_from_slice(&(header & 0x000f0000).to_le_bytes());
            element[8..8 + payload].copy_from_slice(&frame[8..8 + payload]);
            self.registers[0xcc / 4] &= !bit;
            self.registers[0xd8 / 4] |= bit;
            if self.registers[0xe0 / 4] & bit != 0 {
                self.registers[0x50 / 4] |= 1 << 9;
            }
            let gfc = self.registers[0x80 / 4];
            let extended = element[3] & 0x40 != 0;
            let action = (gfc >> if extended { 2 } else { 4 }) & 3;
            if capacity == 0 || action != 0 {
                continue;
            }
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
            0x00 | 0x04 | 0x40 | 0x44 | 0xa4 | 0xcc | 0xd8 | 0xdc => {}
            0x18 => {
                let init = value & 1;
                let changes = if *register & 1 != 0 && init != 0 {
                    value & 2
                } else {
                    0
                };
                let sleep = value & (1 << 4);
                let options = if initialized { value } else { *register };
                *register = init | changes | sleep | (sleep >> 1) | (options & 0xf3e0);
            }
            0x50 => *register &= !value,
            0x54 | 0x58 | 0x5c => *register = value,
            0xd0 => {
                // A transmit request remains pending until cancelled or delivered.
                self.registers[0xcc / 4] |= value;
            }
            0xd4 => {
                let cancelled = self.registers[0xcc / 4] & value;
                self.registers[0xdc / 4] |= cancelled;
                self.registers[0xcc / 4] &= !value;
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

        assert_eq!(&ram[0x20..0x2c], &ram[0x40..0x4c]);
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
