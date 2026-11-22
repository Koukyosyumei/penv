pub struct Stm32Gpio {
    pub base: u32,
    pub size: u32,
    pub exti_routes: [u32; 16],
    external: u16,
    mode: u32,
    output_type: u32,
    speed: u32,
    pull: u32,
    output: u32,
    alternate: [u32; 2],
    alternate_count: u32,
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
    pub fn new(base: u32, size: u32, [mode, speed, pull]: [u32; 3], alternate_count: u32) -> Self {
        Self {
            base,
            size,
            exti_routes: [0; 16],
            external: 0,
            mode,
            output_type: 0,
            speed,
            pull,
            output: 0,
            alternate: [0; 2],
            alternate_count,
            lock: Lock::Idle,
        }
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < self.size
    }

    pub fn set_external(&mut self, pin: u8, high: bool) {
        let bit = 1 << pin;
        self.external = (self.external & !bit) | (u16::from(high) << pin);
    }

    pub fn pins(&self) -> u16 {
        let mut pins = 0;
        for pin in 0..16 {
            let mask = 1 << pin;
            match (self.mode >> (pin * 2)) & 3 {
                0 | 2 => pins |= self.external & mask,
                1 => pins |= self.output as u16 & mask,
                _ => {}
            }
        }
        pins
    }

    pub fn exti_levels(&self) -> u32 {
        let mut pins = self.pins();
        let mut levels = 0;
        while pins != 0 {
            levels |= self.exti_routes[pins.trailing_zeros() as usize];
            pins &= pins - 1;
        }
        levels
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

    pub fn read_halfword(&mut self, address: u32) -> u32 {
        (self.read(address & !3) >> ((address & 2) * 8)) & 0xffff
    }

    pub fn write_halfword(&mut self, address: u32, value: u32) {
        let word = address & !3;
        let shift = (address & 2) * 8;
        let preserved = self.read(word) & !(0xffff << shift);
        self.write(word, preserved | ((value & 0xffff) << shift));
    }

    pub fn write(&mut self, address: u32, value: u32) {
        let locked = match self.lock {
            Lock::Armed(pins) => u32::from(pins),
            _ => 0,
        };
        let offset = address - self.base;
        let (register, mut mask) = match offset {
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
        if matches!(offset, 0x20 | 0x24) && self.alternate_count < 16 {
            for shift in (0..32).step_by(4) {
                if (value >> shift) & 0xf >= self.alternate_count {
                    mask |= 0xf << shift;
                }
            }
        }
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
        let mut port = Stm32Gpio::new(0, 0x400, [1, 0, 0], 16);
        port.write(0x18, 0x0001_0003);
        assert_eq!(port.read(0x14), 3);
        assert_eq!(port.read(0x10), 1);
        port.write(0x00, 5);
        port.write(0x28, 1);
        assert_eq!(port.read(0x10), 2);
    }

    #[test]
    fn halfword_writes_preserve_other_fields_and_reject_invalid_functions() {
        let mut port = Stm32Gpio::new(0, 0x400, [0, 0, 0], 8);
        port.write(0x20, 0x7654_3210);
        port.write_halfword(0x22, 0x1f98);
        assert_eq!(port.read(0x20), 0x1654_3210);
        assert_eq!(port.read_halfword(0x22), 0x1654);
    }

    #[test]
    fn configuration_lock_preserves_only_selected_pins() {
        let mut port = Stm32Gpio::new(0, 0x400, [1, 0, 0], 16);
        for value in [0x1_0001, 1, 0x1_0001] {
            port.write(0x1c, value);
        }
        assert_eq!(port.read(0x1c), 0x1_0001);
        port.write(0, 0xc);
        assert_eq!(port.read(0), 0xd);
    }
}
