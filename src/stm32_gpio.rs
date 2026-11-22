pub struct Stm32Gpio {
    pub base: u32,
    pub size: u32,
    mode: u32,
    output_type: u32,
    speed: u32,
    pull: u32,
    output: u32,
    alternate: [u32; 2],
    lock: Lock,
}

#[derive(Default)]
enum Lock {
    #[default]
    Idle,
    KeySet(u16),
    KeyCleared(u16),
    AwaitRead(u16),
    Armed(u16),
}

impl Stm32Gpio {
    pub fn new(base: u32, size: u32, mode: u32, speed: u32, pull: u32) -> Self {
        Self {
            base,
            size,
            mode,
            output_type: 0,
            speed,
            pull,
            output: 0,
            alternate: [0; 2],
            lock: Lock::Idle,
        }
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < self.size
    }

    pub fn pins(&self) -> u16 {
        let mut pins = 0;
        for pin in 0..16 {
            if (self.mode >> (pin * 2)) & 3 == 1 {
                pins |= self.output & (1 << pin);
            }
        }
        pins as u16
    }

    pub fn read(&mut self, address: u32) -> u32 {
        match address - self.base {
            0x00 => self.mode,
            0x04 => self.output_type,
            0x08 => self.speed,
            0x0c => self.pull,
            0x10 => u32::from(self.pins()),
            0x14 => self.output,
            0x1c => match self.lock {
                Lock::AwaitRead(pins) | Lock::Armed(pins) => {
                    self.lock = Lock::Armed(pins);
                    (1 << 16) | u32::from(pins)
                }
                _ => {
                    self.lock = Lock::Idle;
                    0
                }
            },
            0x20 => self.alternate[0],
            0x24 => self.alternate[1],
            _ => 0,
        }
    }

    pub fn write(&mut self, address: u32, value: u32) {
        let locked = match self.lock {
            Lock::Armed(pins) => u32::from(pins),
            _ => 0,
        };
        let offset = address - self.base;
        let (register, mask) = match offset {
            0x00 => (&mut self.mode, spread(locked, 2)),
            0x04 => (&mut self.output_type, locked | 0xffff_0000),
            0x08 => (&mut self.speed, spread(locked, 2)),
            0x0c => (&mut self.pull, spread(locked, 2)),
            0x14 => {
                self.output = value & 0xffff;
                return;
            }
            0x18 => {
                self.output = (self.output & !(value >> 16)) | (value & 0xffff);
                return;
            }
            0x1c => {
                let pins = value as u16;
                let key = value & (1 << 16) != 0;
                self.lock = match self.lock {
                    Lock::Idle if key => Lock::KeySet(pins),
                    Lock::KeySet(expected) if !key && pins == expected => Lock::KeyCleared(pins),
                    Lock::KeyCleared(expected) if key && pins == expected => Lock::AwaitRead(pins),
                    Lock::Armed(pins) => Lock::Armed(pins),
                    _ => Lock::Idle,
                };
                return;
            }
            0x20 => (&mut self.alternate[0], spread(locked & 0xff, 4)),
            0x24 => (&mut self.alternate[1], spread(locked >> 8, 4)),
            0x28 => {
                self.output &= !value;
                return;
            }
            _ => return,
        };
        *register = (*register & mask) | (value & !mask);
    }
}

fn spread(mut pins: u32, width: u32) -> u32 {
    let mut mask = 0;
    while pins != 0 {
        let pin = pins.trailing_zeros();
        mask |= ((1 << width) - 1) << (pin * width);
        pins &= pins - 1;
    }
    mask
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_latch_drives_only_output_pins_and_set_wins_over_reset() {
        let mut port = Stm32Gpio::new(0, 0x400, 1, 0, 0);
        port.write(0x18, 0x0001_0003);
        assert_eq!(port.read(0x14), 3);
        assert_eq!(port.read(0x10), 1);
        port.write(0x00, 5);
        port.write(0x28, 1);
        assert_eq!(port.read(0x10), 2);
    }

    #[test]
    fn configuration_lock_preserves_only_selected_pins() {
        let mut port = Stm32Gpio::new(0, 0x400, 1, 0, 0);
        for value in [0x1_0001, 1, 0x1_0001] {
            port.write(0x1c, value);
        }
        assert_eq!(port.read(0x1c), 0x1_0001);
        port.write(0, 0xc);
        assert_eq!(port.read(0), 0xd);
    }
}
