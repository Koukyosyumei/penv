#[derive(Default)]
pub(super) struct Egu {
    events: u32,
    interrupts: u32,
    pub(super) raised: u32,
}

impl Egu {
    pub(super) fn task(offset: u32) -> bool {
        offset < 0x40 && offset & 3 == 0
    }

    pub(super) fn event(offset: u32) -> bool {
        offset.checked_sub(0x100).is_some_and(Self::task)
    }

    pub(super) fn pending(&self) -> bool {
        self.events & self.interrupts != 0
    }

    pub(super) fn read(&self, offset: u32) -> Option<u32> {
        Some(match offset {
            offset if Self::task(offset) => 0,
            offset if Self::event(offset) => (self.events >> ((offset - 0x100) / 4)) & 1,
            0x300 | 0x304 | 0x308 => self.interrupts,
            _ => return None,
        })
    }

    pub(super) fn write(&mut self, offset: u32, value: u32) -> bool {
        match offset {
            offset if Self::task(offset) => {
                if value & 1 != 0 {
                    let bit = 1 << (offset / 4);
                    self.events |= bit;
                    self.raised |= bit;
                }
            }
            offset if Self::event(offset) => {
                super::set_event(&mut self.events, 1 << ((offset - 0x100) / 4), value);
            }
            0x300 => self.interrupts = value & 0xffff,
            0x304 => self.interrupts |= value & 0xffff,
            0x308 => self.interrupts &= !value,
            _ => return false,
        }
        true
    }
}
