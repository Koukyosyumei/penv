struct Context {
    enabled: Vec<u32>,
    threshold: u32,
    output: u32,
}

pub struct Plic {
    pub base: u32,
    pub size: u64,
    priority: Vec<u32>,
    level: Vec<u32>,
    pending: Vec<u32>,
    claimed_by: Vec<Option<usize>>,
    contexts: Vec<Context>,
    sources: u32,
    priorities_enabled: bool,
    andes: bool,
}

impl Plic {
    pub const SIZE: u64 = 0x400_0000;

    pub fn new(base: u32, sources: u32, outputs: &[u32]) -> Self {
        let words = (sources as usize + 1).div_ceil(32);
        Self {
            base,
            size: Self::SIZE,
            priority: vec![0; sources as usize + 1],
            level: vec![0; words],
            pending: vec![0; words],
            claimed_by: vec![None; sources as usize + 1],
            contexts: outputs
                .iter()
                .map(|&output| Context {
                    enabled: vec![0; words],
                    threshold: 0,
                    output,
                })
                .collect(),
            sources,
            priorities_enabled: true,
            andes: false,
        }
    }

    pub fn with_priorities(mut self, enabled: bool) -> Self {
        self.priorities_enabled = enabled;
        self
    }

    pub(crate) fn with_andes_range(mut self, size: u64) -> Self {
        self.size = size;
        self.andes = true;
        self
    }

    pub fn set_line(&mut self, source: u32, asserted: bool) {
        if source == 0 || source > self.sources {
            return;
        }
        let (word, bit) = Self::source_bit(source);
        if asserted {
            self.level[word] |= bit;
            if self.claimed_by[source as usize].is_none() {
                self.pending[word] |= bit;
            }
        } else {
            self.level[word] &= !bit;
        }
    }

    pub(crate) fn line_is_asserted(&self, source: u32) -> bool {
        let (word, bit) = Self::source_bit(source);
        self.level.get(word).is_some_and(|level| level & bit != 0)
    }

    fn source_bit(source: u32) -> (usize, u32) {
        ((source / 32) as usize, 1 << (source % 32))
    }

    pub fn contains(&self, address: u32) -> bool {
        u64::from(address.wrapping_sub(self.base)) < self.size
    }

    fn select(&self, context: &Context, mut priority: u32) -> u32 {
        let mut selected = 0;
        for (word, (&pending, &enabled)) in self.pending.iter().zip(&context.enabled).enumerate() {
            let mut candidates = pending & enabled;
            while candidates != 0 {
                let bit = candidates.trailing_zeros();
                let source = word as u32 * 32 + bit;
                if !self.priorities_enabled {
                    return source;
                }
                let next = self.priority[source as usize];
                if next > priority {
                    priority = next;
                    selected = source;
                }
                candidates &= candidates - 1;
            }
        }
        selected
    }

    pub fn irq_outputs(&self) -> u32 {
        self.contexts.iter().fold(0, |outputs, context| {
            if context.output != 0 && self.select(context, context.threshold) != 0 {
                outputs | context.output
            } else {
                outputs
            }
        })
    }

    fn context_register(&self, offset: u32) -> Option<(usize, u32)> {
        let offset = offset.checked_sub(0x20_0000)?;
        let context = (offset / 0x1000) as usize;
        (context < self.contexts.len()).then_some((context, offset % 0x1000))
    }

    fn enable_register(&self, offset: u32) -> Option<(usize, usize)> {
        let offset = offset.checked_sub(0x2000)?;
        let context = (offset / 0x80) as usize;
        let word = (offset % 0x80 / 4) as usize;
        (context < self.contexts.len() && word < self.pending.len()).then_some((context, word))
    }

    pub fn read(&mut self, address: u32) -> u32 {
        let offset = address - self.base;
        if offset & 3 != 0 {
            return 0;
        }
        if let Some((index, word)) = self.enable_register(offset) {
            return self.contexts[index].enabled[word];
        }
        if let Some((index, register)) = self.context_register(offset) {
            return match register {
                0 => self.contexts[index].threshold,
                4 => {
                    let source = self.select(&self.contexts[index], 0);
                    if source != 0 {
                        let (word, bit) = Self::source_bit(source);
                        self.pending[word] &= !bit;
                        self.claimed_by[source as usize] = Some(index);
                    }
                    source
                }
                _ => 0,
            };
        }
        match offset {
            offset if offset <= self.sources * 4 => self.priority[(offset / 4) as usize],
            0x1000..=0x107c => self
                .pending
                .get(((offset - 0x1000) / 4) as usize)
                .copied()
                .unwrap_or(0),
            // Andes exposes a read-only trigger mode per source. All inputs are level-triggered.
            0x1080..=0x10fc if self.andes => 0,
            _ => 0,
        }
    }

    pub fn write(&mut self, address: u32, value: u32) {
        let offset = address - self.base;
        if offset & 3 != 0 {
            return;
        }
        if let Some((index, word)) = self.enable_register(offset) {
            let bits = (self.sources + 1 - word as u32 * 32).min(32);
            let mask = (u32::MAX >> (32 - bits)) & if word == 0 { !1 } else { u32::MAX };
            self.contexts[index].enabled[word] = value & mask;
        } else if let Some((index, register)) = self.context_register(offset) {
            let context = &mut self.contexts[index];
            match register {
                0 if self.priorities_enabled => context.threshold = value,
                4 if value > 0 && value <= self.sources => {
                    let (word, bit) = Self::source_bit(value);
                    if context.enabled[word] & bit != 0
                        && self.claimed_by[value as usize] == Some(index)
                    {
                        self.claimed_by[value as usize] = None;
                        self.pending[word] |= self.level[word] & bit;
                    }
                }
                _ => {}
            }
        } else if self.priorities_enabled && offset != 0 && offset <= self.sources * 4 {
            self.priority[(offset / 4) as usize] = value;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn andes_plic_handles_the_last_source_and_a_high_context() {
        let mut outputs = [0; 16];
        outputs[15] = 1 << 11;
        let mut plic = Plic::new(0, 1023, &outputs).with_andes_range(0x400000);
        plic.write(1023 * 4, 5);
        plic.write(0x2000 + 15 * 0x80 + 31 * 4, 1 << 31);
        plic.set_line(1023, true);
        assert_eq!(plic.read(0x107c), 1 << 31);
        assert_eq!(plic.read(0x10fc), 0);
        assert_eq!(plic.irq_outputs(), 1 << 11);
        assert_eq!(plic.read(0x20f004), 1023);
        assert_eq!(plic.irq_outputs(), 0);
        plic.write(0x20f004, 1023);
        assert_eq!(plic.irq_outputs(), 1 << 11);
    }

    #[test]
    fn shares_pending_sources_across_independent_contexts() {
        let mut plic = Plic::new(0, 52, &[1 << 11, 1 << 9]);
        plic.write(12, 1);
        plic.write(0x2000, 1 << 3);
        plic.write(0x2080, 1 << 3);
        plic.write(0x200000, 1);
        plic.set_line(3, true);
        assert_eq!(plic.irq_outputs(), 1 << 9);
        assert_eq!(plic.read(0x201004), 3);
        assert_eq!(plic.read(0x200004), 0);
        plic.write(0x200004, 3);
        assert_eq!(plic.irq_outputs(), 0);
        plic.write(0x201004, 3);
        assert_eq!(plic.irq_outputs(), 1 << 9);
    }

    #[test]
    fn claims_latched_source_and_requeues_asserted_levels_on_completion() {
        let mut plic = Plic::new(0, 52, &[1]);
        plic.write(12, 2);
        plic.write(16, 2);
        plic.write(0x2000, (1 << 3) | (1 << 4));
        plic.set_line(4, true);
        plic.set_line(3, true);
        plic.set_line(3, false);
        assert!(plic.irq_outputs() != 0);
        assert_eq!(plic.read(0x200004), 3);
        assert_eq!(plic.read(0x200004), 4);
        assert_eq!(plic.irq_outputs(), 0);
        plic.write(0x200004, 3);
        assert_eq!(plic.irq_outputs(), 0);
        plic.write(0x200004, 4);
        assert!(plic.irq_outputs() != 0);
        assert_eq!(plic.read(0x200004), 4);
    }

    #[test]
    fn virt_plic_masks_threshold_notifications_and_high_source_enables() {
        // Source count and machine context from Renode riscv_virt.repl.
        let mut plic = Plic::new(0, 52, &[1 << 11]);
        plic.write(52 * 4, 3);
        plic.write(0x2004, u32::MAX);
        assert_eq!(plic.read(0x2004), (1 << 21) - 1);
        plic.write(0x200000, 3);
        plic.set_line(52, true);
        assert_eq!(plic.irq_outputs(), 0);
        plic.write(0x200000, 2);
        assert_eq!(plic.irq_outputs(), 1 << 11);
        assert_eq!(plic.read(0x200004), 52);
        plic.set_line(52, false);
        plic.write(0x200004, 52);
        assert_eq!(plic.read(0x200004), 0);
        assert_eq!(plic.irq_outputs(), 0);
    }
}
