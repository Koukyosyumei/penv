use std::collections::VecDeque;

const RECEIVE: u32 = 1;
const TRANSMIT: u32 = 1 << 4;
const RECEIVE_IRQ: u32 = 1 << 16;
const TRANSMIT_IRQ: u32 = 1 << 20;
const TRANSMIT_END_IRQ: u32 = 1 << 21;

pub(crate) struct Sci {
    pub(crate) base: u32,
    pub(crate) name: String,
    routes: [Option<usize>; 3],
    control: [u32; 4],
    input: VecDeque<u8>,
    pending: u8,
    irq_delay: u32,
}

impl Sci {
    pub(crate) const SIZE: u32 = 0x100;

    pub(crate) fn new(base: u32, name: String, routes: [Option<usize>; 3]) -> Self {
        Self {
            base,
            name,
            routes,
            control: [0; 4],
            input: VecDeque::new(),
            pending: 0,
            irq_delay: 0,
        }
    }

    pub(crate) fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    pub(crate) fn reset(&mut self) {
        self.control.fill(0);
        self.input.clear();
        self.pending = 0;
        self.irq_delay = 0;
    }

    fn uart_mode(&self) -> bool {
        self.control[3] & (7 << 16) == 0
    }

    fn update_interrupts(&mut self) {
        if self.uart_mode() {
            let control = self.control[0];
            self.pending |= u8::from(!self.input.is_empty() && control & RECEIVE_IRQ != 0)
                | (u8::from(control & TRANSMIT_IRQ != 0) << 1)
                | (u8::from(control & TRANSMIT_END_IRQ != 0) << 2);
        }
    }

    pub(crate) fn queue_input(&mut self, bytes: impl Iterator<Item = u8>) {
        if self.uart_mode() && self.control[0] & RECEIVE != 0 {
            self.input.extend(bytes);
            self.update_interrupts();
        }
    }

    pub(crate) fn tick(&mut self, mut pulse: impl FnMut(usize)) {
        if self.irq_delay != 0 {
            self.irq_delay -= 1;
            if self.irq_delay == 0 {
                self.update_interrupts();
            }
        }
        let pending = std::mem::take(&mut self.pending);
        for (index, route) in self.routes.iter().enumerate() {
            if pending & (1 << index) != 0
                && let Some(event) = route
            {
                pulse(*event);
            }
        }
    }

    pub(crate) fn read(&mut self, address: u32) -> u32 {
        match address - self.base {
            0 => {
                let value = self.input.pop_front().unwrap_or(0);
                self.update_interrupts();
                value.into()
            }
            offset @ (8 | 0xc | 0x10 | 0x14) => self.control[((offset - 8) / 4) as usize],
            0x1c => self.control[0] & (TRANSMIT | RECEIVE),
            0x48 => 1 << 30,
            _ => 0,
        }
    }

    pub(crate) fn write(&mut self, address: u32, value: u32, step_hz: u32) -> Option<u8> {
        match address - self.base {
            4 if self.uart_mode() && self.control[0] & TRANSMIT != 0 => {
                self.update_interrupts();
                return Some(value as u8);
            }
            8 => {
                let control =
                    value & (RECEIVE | TRANSMIT | RECEIVE_IRQ | TRANSMIT_IRQ | TRANSMIT_END_IRQ);
                if self.control[0] != control {
                    self.control[0] = control;
                    if control & (TRANSMIT | TRANSMIT_IRQ) == TRANSMIT | TRANSMIT_IRQ {
                        self.irq_delay = step_hz.div_ceil(1000);
                    } else {
                        self.update_interrupts();
                    }
                }
            }
            0xc => self.control[1] = value & (3 << 8),
            0x10 => self.control[2] = value & ((3 << 20) | (0xff << 8)),
            0x14 => self.control[3] = value & ((7 << 16) | (1 << 14)),
            _ => {}
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transfers_uart_bytes_and_delivers_selected_events() {
        let mut sci = Sci::new(0, "sci0".into(), [Some(10), Some(11), Some(12)]);
        sci.queue_input([1].into_iter());
        assert_eq!(sci.read(0), 0);
        assert_eq!(sci.write(4, b'A'.into(), 2_000), None);
        sci.write(
            8,
            RECEIVE | RECEIVE_IRQ | TRANSMIT | TRANSMIT_IRQ | TRANSMIT_END_IRQ,
            2_000,
        );
        let mut events = Vec::new();
        sci.tick(|event| events.push(event));
        assert!(events.is_empty());
        sci.tick(|event| events.push(event));
        assert_eq!(events, [11, 12]);
        sci.queue_input([b'B'].into_iter());
        assert_eq!(sci.read(0), u32::from(b'B'));
        assert_eq!(sci.write(4, b'C'.into(), 2_000), Some(b'C'));
        sci.tick(|event| events.push(event));
        assert_eq!(events, [11, 12, 10, 11, 12]);
    }
}
