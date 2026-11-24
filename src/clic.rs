//! RV32 core-local interrupt controller with the legacy CLIC configuration layout.
use std::collections::BTreeSet;

#[derive(Clone, Copy)]
pub(crate) struct Interrupt {
    pub id: u32,
    pub level: u8,
    pub vectored: bool,
    pub supervisor: bool,
}

pub(crate) struct Clic {
    pub base: u32,
    configuration: u8,
    clint_lines: u32,
    inputs: Box<[[u8; 4]]>,
    pending: BTreeSet<u32>,
    best: Option<u32>,
    acknowledged: bool,
}

impl Clic {
    pub const SIZE: u32 = 0x10000;

    pub fn new(base: u32) -> Self {
        Self {
            base,
            configuration: 0x51,
            clint_lines: 0,
            inputs: vec![[0, 0, 0xc0, 0]; 4096].into_boxed_slice(),
            pending: BTreeSet::new(),
            best: None,
            acknowledged: false,
        }
    }

    pub fn offset(&self, address: u32, width: usize) -> Option<u32> {
        address
            .checked_sub(self.base)
            .filter(|&offset| u64::from(offset) + width as u64 <= u64::from(Self::SIZE))
    }

    pub fn read(&self, offset: u32, width: usize) -> u32 {
        if offset == 4 && width == 4 {
            return (8 << 21) | 4096;
        }
        (0..width).fold(0, |value, lane| {
            let addr = offset + lane as u32;
            let byte = match addr {
                0 => self.configuration,
                0x1000..=0x4fff => self.inputs[((addr - 0x1000) / 4) as usize][addr as usize & 3],
                _ => 0,
            };
            value | (u32::from(byte) << (lane * 8))
        })
    }

    pub fn write(&mut self, offset: u32, width: usize, value: u32) {
        for lane in 0..width {
            let addr = offset + lane as u32;
            let byte = (value >> (lane * 8)) as u8;
            match addr {
                0 => {
                    let levels = ((byte >> 1) & 15).min(8);
                    self.configuration = 1 | (levels << 1) | (((byte >> 5) & 3).min(2) << 5);
                }
                0x1000..=0x4fff => {
                    let id = (addr - 0x1000) / 4;
                    let entry = &mut self.inputs[id as usize];
                    match addr & 3 {
                        0 if entry[2] & 2 != 0 => entry[0] = byte & 1,
                        0 => {}
                        1 => entry[1] = byte & 1,
                        2 => entry[2] = byte & 0xc7,
                        3 => entry[3] = byte,
                        _ => unreachable!(),
                    }
                    self.update_pending(id);
                }
                _ => {}
            }
        }
        self.select();
    }

    pub fn sample_clint(&mut self, pending: u32) {
        let changed = pending ^ self.clint_lines;
        self.clint_lines = pending;
        for id in [3, 7] {
            if changed & (1 << id) != 0 {
                self.set_line(id, pending & (1 << id) != 0);
            }
        }
    }

    fn set_line(&mut self, id: u32, asserted: bool) {
        let entry = &mut self.inputs[id as usize];
        let active = u8::from(asserted ^ (entry[2] & 4 != 0));
        if entry[2] & 2 != 0 {
            entry[0] |= active;
        } else {
            entry[0] = active;
        }
        self.update_pending(id);
        self.select();
    }

    fn update_pending(&mut self, id: u32) {
        let entry = self.inputs[id as usize];
        if entry[0] != 0 && entry[1] != 0 {
            self.pending.insert(id);
        } else {
            self.pending.remove(&id);
        }
    }

    fn rank(&self, id: u32) -> (u8, u8, u8) {
        let entry = self.inputs[id as usize];
        let mode = match (self.configuration >> 5) & 3 {
            0 => 3,
            1 => {
                if entry[2] & 0x80 != 0 {
                    3
                } else {
                    1
                }
            }
            _ => entry[2] >> 6,
        };
        let priority_mask = ((1u16 << (8 - ((self.configuration >> 1) & 15))) - 1) as u8;
        (mode, entry[3] | priority_mask, entry[3] & priority_mask)
    }

    fn select(&mut self) {
        let mut best = self.best.filter(|id| self.pending.contains(id));
        for &id in &self.pending {
            if best.is_none_or(|previous| {
                let (mode, level, priority) = self.rank(id);
                let (old_mode, old_level, old_priority) = self.rank(previous);
                mode > old_mode
                    || level > old_level
                    || (!self.acknowledged
                        && (mode, level) == (old_mode, old_level)
                        && priority > old_priority)
            }) {
                best = Some(id);
            }
        }
        self.best = best;
        if best.is_none() {
            self.acknowledged = false;
        }
    }

    pub fn interrupt(&self) -> Option<Interrupt> {
        let id = self.best?;
        let (mode, level, _) = self.rank(id);
        matches!(mode, 1 | 3).then_some(Interrupt {
            supervisor: mode == 1,
            id,
            level,
            vectored: self.inputs[id as usize][2] & 1 != 0,
        })
    }

    pub fn acknowledge(&mut self) {
        self.acknowledged = true;
    }

    pub fn clear_edge(&mut self, id: u32) {
        if self.inputs[id as usize][2] & 2 != 0 {
            self.inputs[id as usize][0] = 0;
            self.update_pending(id);
            self.select();
        }
    }
}
