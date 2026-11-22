#[derive(Default)]
struct Context {
    enabled: u64,
    threshold: u32,
    claimed: u64,
    output: u32,
}

pub struct Plic {
    pub base: u32,
    priority: [u32; 64],
    level: u64,
    pending: u64,
    claimed: u64,
    contexts: Vec<Context>,
    sources: u32,
    priorities_enabled: bool,
}

impl Plic {
    pub const SIZE: u64 = 0x400_0000;

    pub fn new(base: u32, sources: u32, outputs: &[u32]) -> Self {
        Self {
            base,
            priority: [0; 64],
            level: 0,
            pending: 0,
            claimed: 0,
            contexts: outputs
                .iter()
                .map(|&output| Context {
                    output,
                    ..Context::default()
                })
                .collect(),
            sources,
            priorities_enabled: true,
        }
    }

    pub fn with_priorities(mut self, enabled: bool) -> Self {
        self.priorities_enabled = enabled;
        self
    }

    pub fn set_line(&mut self, source: u32, asserted: bool) {
        let bit = 1u64 << source;
        if asserted {
            self.level |= bit;
            if self.claimed & bit == 0 {
                self.pending |= bit;
            }
        } else {
            self.level &= !bit;
        }
    }

    pub fn contains(&self, address: u32) -> bool {
        u64::from(address.wrapping_sub(self.base)) < Self::SIZE
    }

    fn select(&self, mut candidates: u64, mut priority: u32) -> u32 {
        if !self.priorities_enabled {
            return if candidates == 0 {
                0
            } else {
                candidates.trailing_zeros()
            };
        }
        let mut selected = 0;
        while candidates != 0 {
            let source = candidates.trailing_zeros();
            candidates &= candidates - 1;
            let next = self.priority[source as usize];
            if next > priority {
                priority = next;
                selected = source;
            }
        }
        selected
    }

    pub fn irq_outputs(&self) -> u32 {
        self.contexts.iter().fold(0, |outputs, context| {
            if self.select(self.pending & context.enabled, context.threshold) != 0 {
                outputs | context.output
            } else {
                outputs
            }
        })
    }

    fn context_register(&self, offset: u32) -> Option<(usize, u32)> {
        if let Some(offset) = offset.checked_sub(0x20_0000) {
            let context = (offset / 0x1000) as usize;
            (context < self.contexts.len()).then_some((context, offset % 0x1000))
        } else {
            None
        }
    }

    fn enable_register(&self, offset: u32) -> Option<(usize, u32)> {
        let offset = offset.checked_sub(0x2000)?;
        let context = (offset / 0x80) as usize;
        let word = offset % 0x80;
        (context < self.contexts.len() && word <= 4).then_some((context, word * 8))
    }

    pub fn read(&mut self, address: u32) -> u32 {
        let offset = address - self.base;
        if let Some((index, shift)) = self.enable_register(offset) {
            return (self.contexts[index].enabled >> shift) as u32;
        }
        if let Some((index, register)) = self.context_register(offset) {
            return match register {
                0 => self.contexts[index].threshold,
                4 => {
                    let source = self.select(self.pending & self.contexts[index].enabled, 0);
                    if source != 0 {
                        let bit = 1u64 << source;
                        self.pending &= !bit;
                        self.claimed |= bit;
                        self.contexts[index].claimed |= bit;
                    }
                    source
                }
                _ => 0,
            };
        }
        match offset {
            offset if offset <= self.sources * 4 => self.priority[(offset / 4) as usize],
            0x1000 => self.pending as u32,
            0x1004 => (self.pending >> 32) as u32,
            _ => 0,
        }
    }

    pub fn write(&mut self, address: u32, value: u32) {
        let offset = address - self.base;
        if let Some((index, shift)) = self.enable_register(offset) {
            let context = &mut self.contexts[index];
            let mask = (u64::MAX >> (63 - self.sources)) & !1;
            context.enabled = ((context.enabled & !(u64::from(u32::MAX) << shift))
                | (u64::from(value) << shift))
                & mask;
        } else if let Some((index, register)) = self.context_register(offset) {
            let context = &mut self.contexts[index];
            match register {
                0 if self.priorities_enabled => context.threshold = value,
                4 if value > 0 && value <= self.sources => {
                    let bit = 1u64 << value;
                    if context.enabled & context.claimed & bit != 0 {
                        context.claimed &= !bit;
                        self.claimed &= !bit;
                        self.pending |= self.level & bit;
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
}
