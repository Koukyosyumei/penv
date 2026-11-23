#[derive(Clone, Copy)]
pub enum PointerEvent {
    Move(i32, i32),
    Press,
    Release,
}

#[derive(Default)]
pub struct Ft5336 {
    pub rotated: bool,
    pub inverted_x: bool,
    pub inverted_y: bool,
    pub max_x: i32,
    pub max_y: i32,
    pub irq: Option<(u32, u8)>,
    position: (u16, u16),
    touch: Touch,
    response: [u8; 6],
    len: usize,
    pending: u64,
    high: bool,
}

#[derive(Clone, Copy, Default, PartialEq)]
enum Touch {
    Up = 1,
    Contact = 2,
    #[default]
    Reserved = 3,
}

impl Ft5336 {
    pub const MODEL: &str = "Input.FT5336";

    pub fn input(&mut self, event: PointerEvent) {
        match event {
            PointerEvent::Move(mut x, mut y) => {
                if self.inverted_x {
                    x = self.max_x.wrapping_sub(x);
                }
                if self.inverted_y {
                    y = self.max_y.wrapping_sub(y);
                }
                if self.rotated {
                    std::mem::swap(&mut x, &mut y);
                }
                self.position = (x as u16, y as u16);
                if self.touch != Touch::Contact {
                    return;
                }
            }
            PointerEvent::Press => self.touch = Touch::Contact,
            PointerEvent::Release => self.touch = Touch::Up,
        }
        self.pending = self.pending.saturating_add(1);
    }

    pub fn tick(&mut self) -> Option<(u32, u8, bool)> {
        let (base, pin) = self.irq?;
        if self.high {
            self.high = false;
        } else if self.pending != 0 {
            self.pending -= 1;
            self.high = true;
        } else {
            return None;
        }
        Some((base, pin, self.high))
    }

    pub fn select(&mut self, register: u8) {
        match register {
            2 => {
                self.response[0] = u8::from(self.touch == Touch::Contact);
                self.len = 1;
            }
            3..=0x20 => {
                let point = (register - 3) / 6;
                let offset = usize::from((register - 3) % 6);
                let ((x, y), touch) = if point == 0 {
                    (self.position, self.touch)
                } else {
                    ((0, 0), Touch::Reserved)
                };
                let data = [
                    (touch as u8) << 6 | ((x >> 8) as u8 & 0xf),
                    x as u8,
                    point << 4 | ((y >> 8) as u8 & 0xf),
                    y as u8,
                    0,
                    0,
                ];
                self.len = 6 - offset;
                self.response[..self.len].copy_from_slice(&data[offset..]);
            }
            0xa8 => {
                self.response[0] = 0x51;
                self.len = 1;
            }
            // The upstream model leaves the previous response intact.
            _ => {}
        }
    }

    pub fn response(&self) -> &[u8] {
        &self.response[..self.len]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(rotated: bool, inverted_x: bool, inverted_y: bool, x: u16, y: u16) {
        let mut touch = Ft5336 {
            rotated,
            inverted_x,
            inverted_y,
            max_x: 110,
            max_y: 150,
            ..Ft5336::default()
        };
        touch.input(PointerEvent::Move(50, 60));
        touch.input(PointerEvent::Press);
        touch.select(3);
        let report = &touch.response()[..4];
        assert_eq!((report[0] & 0xf0) >> 6, 2);
        assert_eq!((u16::from(report[0] & 0xf) << 8) | u16::from(report[1]), x);
        assert_eq!((u16::from(report[2] & 0xf) << 8) | u16::from(report[3]), y);
    }

    #[test]
    fn normal_test() {
        check(false, false, false, 50, 60);
    }

    #[test]
    fn rotated_test() {
        check(true, false, false, 60, 50);
    }

    #[test]
    fn inverted_test() {
        check(false, true, false, 60, 60);
    }

    #[test]
    fn rotated_inverted_test() {
        check(true, false, true, 90, 50);
    }
}
