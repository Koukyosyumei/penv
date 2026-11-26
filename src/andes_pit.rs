#[derive(Default)]
struct Channel {
    control: u32,
    reload: u32,
    remaining: u64,
    enabled: bool,
}

pub(crate) struct AndesPit {
    pub base: u32,
    pub name: String,
    pub irq: Option<(usize, u32)>,
    frequency: u32,
    phase: u64,
    channels: Vec<Channel>,
    interrupt_enable: u32,
    interrupt_status: u32,
    enable: u32,
}

impl AndesPit {
    pub const SIZE: u32 = 0x1000;

    pub(crate) fn reset(&mut self) {
        self.phase = 0;
        self.interrupt_enable = 0;
        self.interrupt_status = 0;
        self.enable = 0;
        for channel in &mut self.channels { *channel = Channel::default(); }
    }

    pub fn new(
        base: u32,
        name: String,
        frequency: u32,
        count: usize,
        irq: Option<(usize, u32)>,
    ) -> Self {
        Self {
            base,
            name,
            irq,
            frequency,
            phase: 0,
            channels: (0..count).map(|_| Channel::default()).collect(),
            interrupt_enable: 0,
            interrupt_status: 0,
            enable: 0,
        }
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    pub fn interrupt(&self) -> bool {
        self.interrupt_enable & self.interrupt_status != 0
    }

    fn record_irq_change(&self, was_asserted: bool) {
        let asserted = self.interrupt();
        if was_asserted != asserted {
            let value = if asserted { "True" } else { "False" };
            crate::monitor_log::record(0, &format!("{}: Setting IRQ to {value}", self.name));
        }
    }

    pub fn read(&self, address: u32) -> u32 {
        let offset = address - self.base;
        match offset {
            0 => 0x0303_1000,
            0x10 => self.channels.len() as u32,
            0x14 => self.interrupt_enable,
            0x18 => self.interrupt_status,
            0x1c => self.enable,
            0x20..=0x5c => {
                let Some(channel) = self.channels.get(((offset - 0x20) / 0x10) as usize) else {
                    return 0;
                };
                match offset % 0x10 {
                    0 => channel.control,
                    4 => channel.reload,
                    8 if channel.control & 7 == 1 => channel.remaining as u32,
                    _ => 0,
                }
            }
            _ => 0,
        }
    }

    pub fn write(&mut self, address: u32, value: u32) {
        let offset = address - self.base;
        let was_asserted = self.interrupt();
        let mask = (1 << (self.channels.len() * 4)) - 1;
        match offset {
            0x14 => self.interrupt_enable = value & mask,
            0x18 => self.interrupt_status &= !value,
            0x1c => {
                let changed = self.enable ^ value;
                self.enable = value & mask;
                for (index, channel) in self.channels.iter_mut().enumerate() {
                    let shift = index * 4;
                    if (changed >> shift) & 0xf != 0 && channel.control & 7 == 1 {
                        channel.remaining = u64::from(channel.reload) + 1;
                        channel.enabled = self.enable & (1 << shift) != 0;
                    }
                }
            }
            0x20..=0x5c => {
                if let Some(channel) = self.channels.get_mut(((offset - 0x20) / 0x10) as usize) {
                    match offset % 0x10 {
                        0 => channel.control = value & 0x1f,
                        4 => channel.reload = value,
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        self.record_irq_change(was_asserted);
    }

    pub(crate) fn set_frequency(&mut self, frequency: u32) {
        self.frequency = frequency;
    }

    pub(crate) fn can_advance(&self) -> bool {
        self.enable != 0
    }

    pub fn tick(&mut self, steps_per_second: u32) {
        if self.enable == 0 {
            return;
        }
        self.phase += u64::from(self.frequency);
        let cycles = self.phase / u64::from(steps_per_second);
        self.phase %= u64::from(steps_per_second);
        if cycles == 0 {
            return;
        }
        let was_asserted = self.interrupt();
        for (index, channel) in self.channels.iter_mut().enumerate() {
            if !channel.enabled || channel.control & 7 != 1 {
                continue;
            }
            if channel.remaining > cycles {
                channel.remaining -= cycles;
                continue;
            }
            let reload = u64::from(channel.reload) + 1;
            channel.remaining = reload - (cycles - channel.remaining) % reload;
            self.interrupt_status |= self.interrupt_enable & (1 << (index * 4));
            crate::monitor_log::record(0, &format!("{}: Channel {index} timer 0 fired", self.name));
        }
        self.record_irq_change(was_asserted);
    }
}

#[cfg(test)]
mod tests {
    use super::AndesPit;

    #[test]
    fn clocked_channel_sets_and_clears_its_interrupt_without_disabling_the_timer() {
        let mut pit = AndesPit::new(0, "pit".into(), 1, 4, None);
        pit.write(0x30, 1);
        pit.write(0x34, 2);
        pit.write(0x14, 1 << 4);
        pit.write(0x1c, 1 << 4);
        assert_eq!(pit.read(0x38), 3);
        for _ in 0..5 {
            pit.tick(2);
        }
        assert!(!pit.interrupt());
        assert_eq!(pit.read(0x38), 1);
        pit.tick(2);
        assert!(pit.interrupt());
        assert_eq!(pit.read(0x18), 1 << 4);
        pit.write(0x18, 1 << 4);
        assert!(!pit.interrupt());
        assert_eq!(pit.read(0x38), 3);
    }
}
