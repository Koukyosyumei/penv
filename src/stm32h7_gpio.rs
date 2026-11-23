pub struct Stm32Gpio {
    pub base: u32,
    pub size: u32,
    pub rcc_pin: Option<u8>,
    pub syscfg_port: Option<u8>,
    pub syscfg_inputs: [Option<u8>; 16],
    external: u16,
    timer_inputs: Vec<(u8, u32, u8)>,
    published_timer_pins: u16,
    alternate_drive: u16,
    alternate_driven: u16,
    reset_values: [u32; 3],
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
            rcc_pin: None,
            syscfg_port: None,
            syscfg_inputs: [None; 16],
            external: 0,
            timer_inputs: Vec::new(),
            published_timer_pins: 0,
            alternate_drive: 0,
            alternate_driven: 0,
            reset_values: [mode, speed, pull],
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

    pub fn reset(&mut self) {
        [self.mode, self.speed, self.pull] = self.reset_values;
        self.output_type = 0;
        self.output = 0;
        self.alternate.fill(0);
        self.lock = Lock::Idle;
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < self.size
    }

    pub fn set_external(&mut self, pin: u8, high: bool) {
        let bit = 1 << pin;
        self.external = (self.external & !bit) | (u16::from(high) << pin);
    }

    pub fn drive_alternate(&mut self, pin: u8, level: Option<bool>) {
        let bit = 1 << pin;
        self.alternate_driven &= !bit;
        self.alternate_drive &= !bit;
        if let Some(level) = level {
            self.alternate_driven |= bit;
            self.alternate_drive |= u16::from(level) << pin;
        }
    }

    pub fn alternate_function(&self, pin: u8) -> Option<usize> {
        ((self.mode >> (pin * 2)) & 3 == 2)
            .then(|| ((self.alternate[usize::from(pin / 8)] >> ((pin % 8) * 4)) & 15) as usize)
    }

    pub fn connect_timer_input(&mut self, pin: u8, base: u32, channel: u8) -> Result<(), String> {
        let route = (pin, base, channel);
        if self.timer_inputs.contains(&route) {
            return Err("duplicate GPIO to STM32 timer connection".into());
        }
        self.timer_inputs.push(route);
        Ok(())
    }

    pub fn timer_input_changes(&mut self) -> impl Iterator<Item = (u32, u8, bool)> + '_ {
        let pins = if self.timer_inputs.is_empty() {
            0
        } else {
            self.pins()
        };
        let changed = pins ^ std::mem::replace(&mut self.published_timer_pins, pins);
        self.timer_inputs
            .iter()
            .filter_map(move |&(pin, base, channel)| {
                let bit = 1 << pin;
                (changed & bit != 0).then_some((base, channel, pins & bit != 0))
            })
    }

    pub fn pins(&self) -> u16 {
        let mut pins = 0;
        let mut active = self.external | self.output as u16 | self.alternate_drive;
        while active != 0 {
            let pin = active.trailing_zeros();
            active &= active - 1;
            let mask = 1 << pin;
            match (self.mode >> (pin * 2)) & 3 {
                0 => pins |= self.external & mask,
                2 => {
                    pins |= (self.alternate_drive | (self.external & !self.alternate_driven)) & mask
                }
                1 => pins |= self.output as u16 & mask,
                _ => {}
            }
        }
        pins
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
    use super::Stm32Gpio;

    #[test]
    fn pin_edges_fan_out_to_timer_inputs() {
        let mut port = Stm32Gpio::new(0x58022800, 0x400, [1, 0, 0], 16);
        port.connect_timer_input(0, 0x40010000, 0).unwrap();
        port.connect_timer_input(0, 0x40010400, 2).unwrap();
        assert!(port.connect_timer_input(0, 0x40010000, 0).is_err());

        assert_eq!(port.timer_input_changes().count(), 0);
        port.write(port.base + 0x18, 1);
        assert!(
            port.timer_input_changes()
                .eq([(0x40010000, 0, true), (0x40010400, 2, true),])
        );
        assert_eq!(port.timer_input_changes().count(), 0);
        port.write(port.base + 0x18, 1 << 16);
        assert!(
            port.timer_input_changes()
                .eq([(0x40010000, 0, false), (0x40010400, 2, false),])
        );
    }
}
