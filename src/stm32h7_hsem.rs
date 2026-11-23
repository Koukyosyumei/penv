const LOCK: u32 = 1 << 31;
const CORE: u32 = 0xff00;
const OWNER: u32 = CORE | 0xff;
const CORTEX_M7: u32 = 3 << 8;

pub struct Hsem {
    pub base: u32,
    pub rcc_bound: bool,
    pub irq: Option<u8>,
    locks: [u32; 32],
    enabled: u32,
    released: u32,
    key: u32,
}

impl Hsem {
    pub const MODEL: &str = "Miscellaneous.STM32H7_HardwareSemaphore";
    pub const SIZE: u32 = 0x400;

    pub fn new(base: u32, rcc_bound: bool, irq: Option<u8>) -> Self {
        Self {
            base,
            rcc_bound,
            irq,
            locks: [0; 32],
            enabled: 0,
            released: 0,
            key: 0,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new(self.base, self.rcc_bound, self.irq);
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    pub fn interrupt(&self) -> Option<u8> {
        self.irq.filter(|_| self.released & self.enabled != 0)
    }

    fn register(&self, offset: u32) -> u32 {
        match offset {
            0..=0x7c => self.locks[(offset / 4) as usize],
            0x100 => self.enabled,
            0x108 => self.released,
            0x10c => self.released & self.enabled,
            0x144 => self.key,
            _ => 0,
        }
    }

    pub fn read(&mut self, address: u32, width: usize) -> u32 {
        let offset = (address - self.base) & !3;
        let value = if (0x80..=0xfc).contains(&offset) {
            let lock = &mut self.locks[((offset - 0x80) / 4) as usize];
            if *lock == 0 {
                *lock = LOCK | CORTEX_M7;
            }
            *lock
        } else {
            self.register(offset)
        };
        (value >> ((address & 3) * 8)) & (u32::MAX >> (32 - width * 8))
    }

    pub fn write(&mut self, address: u32, width: usize, value: u32) {
        let offset = (address - self.base) & !3;
        let shift = (address & 3) * 8;
        let mask = (u32::MAX >> (32 - width * 8)) << shift;
        let bits = (value << shift) & mask;
        let value = (self.register(offset) & !mask) | bits;
        match offset {
            0..=0x7c => {
                let index = (offset / 4) as usize;
                let owner = value & OWNER;
                if value & LOCK != 0 {
                    if self.locks[index] == 0 {
                        self.locks[index] = LOCK | owner;
                    }
                } else if self.locks[index] == LOCK | owner {
                    self.release(index);
                }
            }
            0x100 => self.enabled = value,
            0x104 => self.released &= !bits,
            0x140 if value & 0xffff_0000 == self.key => {
                for index in 0..self.locks.len() {
                    if self.locks[index] & (LOCK | CORE) == LOCK | (value & CORE) {
                        self.release(index);
                    }
                }
            }
            0x144 => self.key = value & 0xffff_0000,
            _ => {}
        }
    }

    fn release(&mut self, index: usize) {
        self.locks[index] = 0;
        self.released |= 1 << index;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrong_owner_cannot_release_and_notifications_follow_the_mask() {
        let mut sem = Hsem::new(0, false, Some(125));
        assert_eq!(sem.read(0x80, 4), LOCK | CORTEX_M7);
        sem.write(0, 4, LOCK | 0x103);
        sem.write(0, 4, 0x103);
        assert_eq!(sem.read(0, 4), LOCK | CORTEX_M7);
        assert_eq!(sem.read(0x108, 4), 0);
        sem.write(0, 4, CORTEX_M7);
        assert_eq!(sem.read(0, 4), 0);
        assert_eq!(sem.read(0x108, 4), 1);
        assert_eq!(sem.interrupt(), None);
        sem.write(0x100, 1, 1);
        assert_eq!(sem.interrupt(), Some(125));
        sem.write(0x104, 1, 1);
        assert_eq!(sem.interrupt(), None);
    }

    #[test]
    fn core_clear_requires_the_key_and_only_releases_matching_locks() {
        let mut sem = Hsem::new(0, false, None);
        sem.write(0x144, 4, 0x1234_0000);
        sem.write(0, 4, LOCK | 0x301);
        sem.write(4, 4, LOCK | 0x101);
        sem.write(0x140, 4, 0xabcd_0300);
        assert_eq!(sem.read(0, 4), LOCK | 0x301);
        sem.write(0x140, 4, 0x1234_0300);
        assert_eq!(sem.read(0, 4), 0);
        assert_eq!(sem.read(4, 4), LOCK | 0x101);
        assert_eq!(sem.read(0x108, 4), 1);
        sem.reset();
        assert_eq!(sem.read(4, 4), 0);
        assert_eq!(sem.read(0x108, 4), 0);
    }
}
