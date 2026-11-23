pub const READ: u8 = 1;
pub const WRITE: u8 = 2;
pub const EXECUTE: u8 = 4;
const LOCK: u8 = 0x80;
const TOR: u8 = 1;

#[derive(Clone)]
pub struct Pmp {
    config: [u8; 64],
    address: [u32; 64],
    active: u64,
    entries: usize,
}

impl Default for Pmp {
    fn default() -> Self {
        Self::new(16)
    }
}

impl Pmp {
    pub fn new(entries: usize) -> Self {
        debug_assert!(matches!(entries, 16 | 64));
        Self {
            config: [0; 64],
            address: [0; 64],
            active: 0,
            entries,
        }
    }

    pub fn read(&self, csr: u32) -> Option<u32> {
        Some(match csr {
            0x3a0..=0x3af if ((csr - 0x3a0) * 4) < self.entries as u32 => {
                let start = (csr as usize - 0x3a0) * 4;
                u32::from_le_bytes(self.config[start..start + 4].try_into().unwrap())
            }
            0x3b0..=0x3ef if csr - 0x3b0 < self.entries as u32 => {
                self.address[(csr - 0x3b0) as usize]
            }
            _ => return None,
        })
    }

    pub fn write(&mut self, csr: u32, value: u32) -> bool {
        match csr {
            0x3a0..=0x3af if ((csr - 0x3a0) * 4) < self.entries as u32 => {
                let start = (csr as usize - 0x3a0) * 4;
                for (index, config) in (start..start + 4).zip(value.to_le_bytes()) {
                    if self.config[index] & LOCK != 0 {
                        continue;
                    }
                    let mut config = config & 0x9f;
                    if config & (READ | WRITE) == WRITE {
                        config &= !WRITE;
                    }
                    self.config[index] = config;
                    self.active &= !(1 << index);
                    if mode(config) != 0 {
                        self.active |= 1 << index;
                    }
                }
            }
            0x3b0..=0x3ef if csr - 0x3b0 < self.entries as u32 => {
                let index = (csr - 0x3b0) as usize;
                let locked_by_next = self.config[..self.entries]
                    .get(index + 1)
                    .is_some_and(|&config| config & LOCK != 0 && mode(config) == TOR);
                if self.config[index] & LOCK == 0 && !locked_by_next {
                    self.address[index] = value;
                }
            }
            _ => return false,
        }
        true
    }

    pub fn permits(&self, address: u32, width: usize, permission: u8, machine: bool) -> bool {
        let start = u64::from(address);
        let end = start + width as u64;
        let mut active = self.active;
        while active != 0 {
            let index = active.trailing_zeros() as usize;
            active &= active - 1;
            let config = self.config[index];
            let (lower, upper) = self.range(index, mode(config));
            if lower >= upper || end <= lower || start >= upper {
                continue;
            }
            return start >= lower
                && end <= upper
                && ((machine && config & LOCK == 0) || config & permission == permission);
        }
        machine
    }

    fn range(&self, index: usize, mode: u8) -> (u64, u64) {
        let address = u64::from(self.address[index]);
        match mode {
            TOR => {
                let lower = if index == 0 {
                    0
                } else {
                    u64::from(self.address[index - 1]) << 2
                };
                (lower, address << 2)
            }
            2 => (address << 2, (address << 2) + 4),
            _ => {
                let mask = (1u64 << address.trailing_ones()) - 1;
                let lower = (address & !mask) << 2;
                (lower, lower + ((mask + 1) << 3))
            }
        }
    }
}

fn mode(config: u8) -> u8 {
    (config >> 3) & 3
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn honors_first_overlap_and_locked_tor_address() {
        let mut pmp = Pmp::default();
        assert!(!pmp.permits(0x1000, 4, READ, false));
        assert!(pmp.write(0x3b0, 0x400));
        assert!(pmp.write(0x3b1, 0x402));
        assert!(pmp.write(0x3a0, 0x8900)); // entry 1: locked, TOR, readable
        assert!(pmp.write(0x3b0, 0));
        assert_eq!(pmp.read(0x3b0), Some(0x400));
        assert!(pmp.permits(0x1000, 4, READ, false));
        assert!(!pmp.permits(0x1000, 4, WRITE, true));
        assert!(!pmp.permits(0x1004, 8, READ, true));
        assert!(pmp.write(0x3b2, 0x403)); // NAPOT [0x1000, 0x1020)
        assert!(pmp.write(0x3a0, 0x1b_0000)); // entry 2: readable and writable
        assert!(pmp.permits(0x1008, 4, WRITE, false));
        assert!(!pmp.permits(0x1004, 8, WRITE, false));
    }
}
