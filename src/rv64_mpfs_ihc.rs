const HARTS: usize = 5;
const WORDS: usize = 4;
const REMOTE_PENDING: u32 = 1;
const INCOMING_PENDING: u32 = 1 << 1;
const MESSAGE_IRQ: u32 = 1 << 2;
const ACK: u32 = 1 << 3;
const ACK_CLEAR: u32 = 1 << 4;
const ACK_IRQ: u32 = 1 << 5;

#[derive(Clone, Copy, Default)]
struct Channel {
    control: u32,
    hart_id: u32,
    input: [u32; WORDS],
    output: [u32; WORDS],
}

#[derive(Default)]
pub struct FireIhc {
    channels: [[Channel; HARTS]; HARTS],
    irq_enable: [u32; HARTS],
}

impl FireIhc {
    pub const BASE: u32 = 0x5000_0000;
    pub const SIZE: u64 = 0x1900;
    pub const FIRST_IRQ: usize = 181;

    fn select(address: u32, width: usize) -> Option<(usize, usize, u32)> {
        let offset = address.checked_sub(Self::BASE)?;
        if width != 4 || !offset.is_multiple_of(4) || u64::from(offset) >= Self::SIZE {
            return None;
        }
        let local = (offset / 0x500) as usize;
        let slot = ((offset % 0x500) / 0x100) as usize;
        let remote = slot + usize::from(slot >= local);
        Some((local, remote, offset % 0x100))
    }

    fn available(&self, local: usize, for_irq: bool) -> u32 {
        let mut available = 0;
        for (remote, channel) in self.channels[local].iter().enumerate() {
            if channel.control & INCOMING_PENDING != 0
                && (!for_irq || channel.control & MESSAGE_IRQ != 0)
            {
                available |= 1 << (remote * 2);
            }
            if channel.control & ACK != 0 && (!for_irq || channel.control & ACK_IRQ != 0) {
                available |= 2 << (remote * 2);
            }
        }
        available
    }

    pub fn interrupts(&self) -> impl Iterator<Item = (usize, bool)> + '_ {
        (0..HARTS).map(|local| {
            (
                Self::FIRST_IRQ - local,
                self.available(local, true) & self.irq_enable[local] != 0,
            )
        })
    }

    pub fn read(&self, address: u32, width: usize) -> Option<u32> {
        let (local, remote, offset) = Self::select(address, width)?;
        if remote == HARTS {
            return Some(match offset {
                4 => self.irq_enable[local],
                8 => self.available(local, false),
                _ => 0,
            });
        }
        let channel = &self.channels[local][remote];
        Some(match offset {
            4 => channel.control,
            8 => channel.hart_id,
            0xc => WORDS as u32,
            0x20..=0x2c => channel.input[((offset - 0x20) / 4) as usize],
            0x30..=0x3c => channel.output[((offset - 0x30) / 4) as usize],
            _ => 0,
        })
    }

    pub fn write(&mut self, address: u32, width: usize, value: u32) -> Option<()> {
        let (local, remote, offset) = Self::select(address, width)?;
        if remote == HARTS {
            if offset == 4 {
                self.irq_enable[local] = value & 0x3ff;
            }
            return Some(());
        }
        match offset {
            4 => self.write_control(local, remote, value),
            8 => self.channels[local][remote].hart_id = value,
            0x30..=0x3c => {
                self.channels[local][remote].output[((offset - 0x30) / 4) as usize] = value;
            }
            _ => {}
        }
        Some(())
    }

    fn write_control(&mut self, local: usize, remote: usize, value: u32) {
        let channel = &mut self.channels[local][remote];
        let previous = channel.control;
        channel.control = (previous & !(MESSAGE_IRQ | ACK_IRQ)) | (value & (MESSAGE_IRQ | ACK_IRQ));
        if value & ACK_CLEAR == 0 {
            channel.control &= !(ACK | ACK_CLEAR);
        }
        if previous & INCOMING_PENDING != 0 && value & INCOMING_PENDING == 0 {
            channel.control &= !INCOMING_PENDING;
            let peer = &mut self.channels[remote][local];
            peer.control &= !REMOTE_PENDING;
            if value & ACK != 0 {
                peer.control |= ACK | ACK_CLEAR;
            }
        }
        if previous & REMOTE_PENDING == 0 && value & REMOTE_PENDING != 0 {
            let channel = &mut self.channels[local][remote];
            channel.control |= REMOTE_PENDING;
            let message = channel.output;
            let peer = &mut self.channels[remote][local];
            peer.input = message;
            peer.control |= INCOMING_PENDING;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fire_ihc_transfers_messages_and_acknowledges_the_sender() {
        let mut ihc = FireIhc::default();
        let base = FireIhc::BASE;
        for (offset, value) in [
            (0x904, 1),
            (0x404, 1 << 3),
            (0x504, MESSAGE_IRQ),
            (0x30, 0x1234_abcd),
            (4, REMOTE_PENDING | ACK_IRQ),
        ] {
            assert_eq!(ihc.write(base + offset, 4, value), Some(()));
        }
        assert_eq!(ihc.read(base + 0x520, 4), Some(0x1234_abcd));
        assert_eq!(ihc.read(base + 0x908, 4), Some(1));
        assert_eq!(ihc.interrupts().nth(1), Some((180, true)));
        ihc.write(base + 0x504, 4, MESSAGE_IRQ | ACK).unwrap();
        assert_eq!(ihc.read(base + 0x908, 4), Some(0));
        assert_eq!(ihc.read(base + 4, 4), Some(ACK_IRQ | ACK | ACK_CLEAR));
        assert_eq!(ihc.interrupts().next(), Some((181, true)));
        ihc.write(base + 4, 4, ACK_IRQ).unwrap();
        assert_eq!(ihc.interrupts().next(), Some((181, false)));
    }
}
