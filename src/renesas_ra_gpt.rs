const STRIDE: u32 = 0x100;
const OVERFLOW: u32 = 1 << 6;

struct Channel {
    mask: u32,
    value: u32,
    cycle: u32,
    enabled: bool,
    upward: bool,
    overflow: bool,
    fraction: u64,
}

impl Channel {
    fn new(mask: u32) -> Self {
        Self {
            mask,
            value: mask,
            cycle: mask,
            enabled: false,
            upward: false,
            overflow: false,
            fraction: 0,
        }
    }

    fn tick(&mut self, frequency: u32, step_hz: u32) -> Option<usize> {
        if !self.enabled {
            return None;
        }
        self.fraction += u64::from(frequency);
        let ticks = self.fraction / u64::from(step_hz);
        self.fraction %= u64::from(step_hz);
        if ticks == 0 {
            return None;
        }
        let period = u64::from(self.cycle).max(1);
        let distance = if self.upward {
            u64::from(self.cycle.saturating_sub(self.value)).max(1)
        } else {
            u64::from(self.value).max(1)
        };
        if ticks < distance {
            if self.upward {
                self.value += ticks as u32;
            } else {
                self.value -= ticks as u32;
            }
            return None;
        }
        let remaining = ((ticks - distance) % period) as u32;
        if self.upward {
            self.overflow = true;
            self.value = remaining;
            Some(6)
        } else {
            self.value = self.cycle.saturating_sub(remaining);
            Some(7)
        }
    }
}

pub(crate) struct Gpt {
    pub(crate) base: u32,
    pub(crate) size: u32,
    frequency: u32,
    channels: Vec<Channel>,
    irq_events: Vec<Option<usize>>,
}

impl Gpt {
    pub(crate) fn new(
        base: u32,
        wide: u32,
        narrow: u32,
        common: u32,
        frequency: u32,
    ) -> Result<Self, String> {
        let count = wide
            .checked_add(narrow)
            .filter(|&n| (1..=32).contains(&n))
            .ok_or("invalid GPT channel count")?;
        let size = common
            .checked_add(STRIDE)
            .filter(|_| common >= count * STRIDE && common % STRIDE == 0)
            .ok_or("invalid GPT common register offset")?;
        if frequency == 0 || base % 4 != 0 {
            return Err("invalid GPT base or clock frequency".into());
        }
        let channels = (0..count)
            .map(|i| Channel::new(if i < wide { u32::MAX } else { 0xffff }))
            .collect();
        Ok(Self {
            base,
            size,
            frequency,
            channels,
            irq_events: vec![None; count as usize * 9],
        })
    }

    pub(crate) fn connect(&mut self, output: usize, event: usize) -> Result<(), String> {
        let line = self
            .irq_events
            .get_mut(output)
            .ok_or("GPT output index out of range")?;
        if line.is_some() {
            return Err("duplicate GPT output wiring".into());
        }
        *line = Some(event);
        Ok(())
    }

    pub(crate) fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < self.size
    }

    pub(crate) fn running(&self) -> bool {
        self.channels.iter().any(|channel| channel.enabled)
    }

    pub(crate) fn reset(&mut self) {
        for channel in &mut self.channels {
            *channel = Channel::new(channel.mask);
        }
    }

    pub(crate) fn tick(&mut self, step_hz: u32, mut pulse: impl FnMut(usize)) {
        for (index, channel) in self.channels.iter_mut().enumerate() {
            if let Some(line) = channel.tick(self.frequency, step_hz)
                && let Some(event) = self.irq_events[index * 9 + line]
            {
                pulse(event);
            }
        }
    }

    pub(crate) fn read(&self, address: u32) -> u32 {
        let offset = address - self.base;
        let Some(channel) = self.channels.get((offset / STRIDE) as usize) else {
            return 0;
        };
        match offset % STRIDE {
            0x04 | 0x08 => {
                let running = offset % STRIDE == 0x04;
                self.channels.iter().enumerate().fold(0, |mask, (i, ch)| {
                    mask | (u32::from(ch.enabled == running) << i)
                })
            }
            0x30 => u32::from(channel.upward),
            0x3c => u32::from(channel.overflow) * OVERFLOW,
            0x48 => channel.value,
            0x64 => channel.cycle,
            _ => 0,
        }
    }

    pub(crate) fn write(&mut self, address: u32, value: u32) -> Option<usize> {
        let offset = address - self.base;
        let index = (offset / STRIDE) as usize;
        let channel = self.channels.get_mut(index)?;
        match offset % STRIDE {
            0x04 | 0x08 => {
                let enable = offset % STRIDE == 0x04;
                for (i, channel) in self.channels.iter_mut().enumerate() {
                    if value & (1 << i) != 0 {
                        channel.enabled = enable;
                    }
                }
            }
            0x30 => channel.upward = value & 1 != 0,
            0x3c if value & OVERFLOW == 0 => channel.overflow = false,
            0x48 if !channel.enabled => {
                let value = value & channel.mask;
                channel.overflow = value >= channel.cycle;
                if channel.overflow {
                    channel.value = if channel.upward { 0 } else { channel.cycle };
                    return self.irq_events[index * 9 + 6];
                }
                channel.value = value;
            }
            0x64 => channel.cycle = value & channel.mask,
            _ => {}
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_selected_channels_and_routes_period_expiry() {
        let mut timer = Gpt::new(0, 1, 1, 0x300, 4).unwrap();
        timer.connect(6, 101).unwrap();
        timer.write(0x30, 1);
        timer.write(0x64, 3);
        timer.write(0x48, 0);
        timer.write(0x104, 1);
        let mut events = Vec::new();
        for _ in 0..5 {
            timer.tick(8, |event| events.push(event));
        }
        assert_eq!(timer.read(0x48), 2);
        assert!(events.is_empty());
        timer.tick(8, |event| events.push(event));
        assert_eq!(events, [101]);
        assert_eq!(timer.read(0x3c), OVERFLOW);
        assert_eq!(timer.read(0x164), 0xffff);
    }
}
