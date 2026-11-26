use crate::nvic::{ActiveExceptions, IRQ_LINES, Nvic};

const LINK_BASE: usize = 0x300 / 4;
const EVENT_MASK: u32 = 0x1ff;
const PENDING: u32 = 1 << 16;

pub(crate) struct Icu {
    pub(crate) base: u32,
    pub(crate) name: String,
    triggers: Vec<u8>,
    levels: Vec<bool>,
    links: Vec<u16>,
    pending: [u64; IRQ_LINES.div_ceil(64)],
}

impl Icu {
    pub(crate) const SIZE: u32 = 0x1000;

    pub(crate) fn new(
        base: u32,
        name: String,
        external: usize,
        highest_event: usize,
        outputs: usize,
    ) -> Result<Self, String> {
        if highest_event >= 512 || external > 64 || external > highest_event || outputs > IRQ_LINES
        {
            return Err("invalid Renesas RA ICU event or interrupt count".into());
        }
        Ok(Self {
            base,
            name,
            triggers: vec![0; external],
            levels: vec![false; highest_event + 1],
            links: vec![0; outputs],
            pending: [0; IRQ_LINES.div_ceil(64)],
        })
    }

    pub(crate) fn output_count(&self) -> usize {
        self.links.len()
    }

    pub(crate) fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    pub(crate) fn reset(&mut self) {
        self.triggers.fill(0);
        self.levels.fill(false);
        self.links.fill(0);
        self.pending.fill(0);
    }

    fn triggered(&self, event: usize, previous: bool, current: bool) -> bool {
        match event
            .checked_sub(1)
            .and_then(|index| self.triggers.get(index))
        {
            Some(0) => previous && !current,
            Some(1) => !previous && current,
            Some(2) => previous != current,
            Some(3) => !current,
            _ => event != 0 && current,
        }
    }

    pub(crate) fn set_event(&mut self, event: usize, current: bool) {
        let Some(level) = self.levels.get_mut(event) else {
            return;
        };
        let previous = std::mem::replace(level, current);
        if self.triggered(event, previous, current) {
            for (line, &link) in self.links.iter().enumerate() {
                if usize::from(link) == event {
                    self.pending[line / 64] |= 1 << (line % 64);
                }
            }
        }
    }

    pub(crate) fn has_pending(&self) -> bool {
        self.pending.iter().any(|&pending| pending != 0)
    }

    pub(crate) fn sample(&self, nvic: &mut Nvic, active: &ActiveExceptions) {
        for (word, mut pending) in self.pending.into_iter().enumerate() {
            while pending != 0 {
                let bit = pending.trailing_zeros();
                nvic.set_line((word * 64) as u8 + bit as u8, true, active);
                pending &= pending - 1;
            }
        }
    }

    pub(crate) fn read(&self, address: u32, width: usize) -> u32 {
        let offset = (address - self.base) as usize;
        let register = offset / 4;
        let value = if let Some(trigger) = self.triggers.get(register) {
            u32::from(*trigger)
        } else if let Some(line) = register
            .checked_sub(LINK_BASE)
            .filter(|&line| line < self.links.len())
        {
            let pending = (self.pending[line / 64] >> (line % 64)) & 1;
            u32::from(self.links[line]) | (pending as u32 * PENDING)
        } else {
            0
        };
        (value >> (8 * (offset % 4))) & (u32::MAX >> (32 - width * 8))
    }

    pub(crate) fn write(&mut self, address: u32, width: usize, value: u32) {
        let offset = (address - self.base) as usize;
        let register = offset / 4;
        let shift = 8 * (offset % 4);
        let mask = (u32::MAX >> (32 - width * 8)) << shift;
        let value = value << shift;
        if let Some(trigger) = self.triggers.get_mut(register) {
            if mask & 3 != 0 {
                let new = (value & 3) as u8;
                if *trigger != new {
                    *trigger = new;
                    let event = register + 1;
                    self.set_event(event, self.levels[event]);
                }
            }
        } else if let Some(line) = register
            .checked_sub(LINK_BASE)
            .filter(|&line| line < self.links.len())
        {
            let old = self.read(address & !3, 4);
            let clear_pending = mask & PENDING != 0 && value & PENDING == 0;
            let link_mask = mask & EVENT_MASK;
            let new = (old & !link_mask) | (value & link_mask);
            let new = if clear_pending { new & !PENDING } else { new };
            if old == new {
                return;
            }
            self.links[line] = (new & EVENT_MASK) as u16;
            let bit = 1 << (line % 64);
            if new & PENDING == 0 {
                self.pending[line / 64] &= !bit;
            }
            let event = usize::from(self.links[line]);
            if let Some(&state) = self.levels.get(event)
                && self.triggered(event, state, state)
            {
                self.pending[line / 64] |= bit;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latches_programmed_external_edges_and_clears_pending_by_byte() {
        let mut icu = Icu::new(0x4000_c000, "icu".into(), 16, 0x1da, 96).unwrap();
        let link = icu.base + 0x300 + 40 * 4;
        icu.write(icu.base, 1, 1);
        icu.write(link, 2, 1);
        icu.set_event(1, true);
        icu.set_event(1, false);
        assert_eq!(icu.read(link, 4), 1 | PENDING);
        let mut nvic = Nvic::new(0xff, None);
        nvic.set_external_lines(96);
        icu.sample(&mut nvic, &ActiveExceptions::default());
        assert_eq!(nvic.read(0xe000_e204, 4), Some(1 << 8));

        icu.write(link + 2, 1, 0);
        assert_eq!(icu.read(link, 4), 1);
        icu.set_event(1, true);
        assert_eq!(icu.read(link, 4), 1 | PENDING);
        icu.reset();
        assert_eq!(icu.read(link, 4), 0);
    }
}
