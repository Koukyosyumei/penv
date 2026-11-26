pub struct Stm32Gpio {
    pub base: u32,
    pub name: Option<String>,
    pub size: u32,
    pub exti_routes: [u32; 16],
    external: u16,
    pub wired_inputs: u16,
    af_driver: [u16; 16],
    af_levels: [u16; 16],
    af_inverted: [u16; 16],
    reset_values: [u32; 3],
    mode: u32,
    output_type: u32,
    speed: u32,
    pull: u32,
    output: u32,
    alternate: [u32; 2],
    alternate_count: u32,
    lock: Lock,
    f1_configuration: Option<[u32; 2]>,
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
            name: None,
            size,
            exti_routes: [0; 16],
            external: 0,
            wired_inputs: 0,
            af_driver: [0; 16],
            af_levels: [0; 16],
            af_inverted: [0; 16],
            reset_values: [mode, speed, pull],
            mode,
            output_type: 0,
            speed,
            pull,
            output: 0,
            alternate: [0; 2],
            alternate_count,
            lock: Lock::Idle,
            f1_configuration: None,
        }
    }

    pub fn new_f1(base: u32, size: u32) -> Self {
        let mut port = Self::new(base, size, [0; 3], 16);
        port.f1_configuration = Some([0x4444_4444; 2]);
        port
    }

    pub(crate) fn set_f1_alternate(&mut self, pin: usize, function: u8) {
        let shift = (pin % 8) * 4;
        let select = &mut self.alternate[pin / 8];
        *select = (*select & !(15 << shift)) | (u32::from(function) << shift);
    }

    pub fn reset(&mut self) {
        [self.mode, self.speed, self.pull] = self.reset_values;
        self.output_type = 0;
        self.output = 0;
        self.alternate.fill(0);
        self.af_driver.fill(0);
        self.af_levels.fill(0);
        self.lock = Lock::Idle;
        if let Some(configuration) = &mut self.f1_configuration {
            *configuration = [0x4444_4444; 2];
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
                0 => pins |= (self.external | self.wired_inputs) & mask,
                2 => {
                    let function = ((self.alternate[pin / 8] >> ((pin % 8) * 4)) & 15) as usize;
                    let levels = if self.af_driver[function] & mask != 0 {
                        self.af_levels[function] ^ self.af_inverted[function]
                    } else {
                        self.external | self.wired_inputs
                    };
                    pins |= levels & mask;
                }
                1 => pins |= self.output as u16 & mask,
                _ => {}
            }
        }
        pins
    }

    pub fn driven_pins(&self) -> u16 {
        let mut driven = 0;
        for pin in 0..16 {
            let mask = 1 << pin;
            match (self.mode >> (pin * 2)) & 3 {
                1 => driven |= self.output as u16 & mask,
                2 => {
                    let function = ((self.alternate[pin / 8] >> ((pin % 8) * 4)) & 15) as usize;
                    driven |= (self.af_levels[function] ^ self.af_inverted[function])
                        & self.af_driver[function]
                        & mask;
                }
                _ => {}
            }
        }
        driven
    }

    pub fn alternate_count(&self) -> u32 {
        self.alternate_count
    }

    pub fn invert_alternate(&mut self, pin: u8, function: u8) {
        self.af_inverted[usize::from(function)] |= 1 << pin;
    }

    pub fn set_alternate_output(&mut self, pin: u8, function: u8, level: Option<bool>) {
        let bit = 1 << pin;
        let function = usize::from(function);
        let driven = &mut self.af_driver[function];
        *driven = (*driven & !bit) | (u16::from(level.is_some()) << pin);
        let active = &mut self.af_levels[function];
        *active = (*active & !bit) | (u16::from(level.unwrap_or(false)) << pin);
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
        if let Some(configuration) = self.f1_configuration {
            return match address - self.base {
                0x00 => configuration[0],
                0x04 => configuration[1],
                0x08 => u32::from(self.pins()),
                0x0c => self.output,
                _ => 0,
            };
        }
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
        if self.f1_configuration.is_some() {
            self.write_f1(address - self.base, value);
            return;
        }
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
    fn write_f1(&mut self, offset: u32, value: u32) {
        match offset {
            0x00 | 0x04 => {
                let half = (offset / 4) as usize;
                self.f1_configuration.as_mut().unwrap()[half] = value;
                for index in 0..8 {
                    let pin = half * 8 + index;
                    let configuration = (value >> (index * 4)) & 15;
                    let mode = match (configuration & 3, configuration & 8) {
                        (0, _) => 0,
                        (_, 0) => 1,
                        _ => 2,
                    };
                    self.mode = (self.mode & !(3 << (pin * 2))) | (mode << (pin * 2));
                }
            }
            0x0c => self.output = value & 0xffff,
            0x10 => self.output = (self.output & !(value >> 16)) | (value & 0xffff),
            0x14 => self.output &= !(value & 0xffff),
            _ => {}
        }
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
    fn f1_afio_switches_timer_outputs_and_exti_inputs() {
        let mut ports = [
            Stm32Gpio::new_f1(0x100, 0x400),
            Stm32Gpio::new_f1(0x500, 0x400),
        ];
        let mut afio = F1Afio::new(
            0x2000,
            [0x100, 0x500, 0x900, 0xd00, 0x1100, 0x1500, 0x1900],
            std::array::from_fn(|pin| 1 << pin),
        );
        afio.apply(&mut ports);
        ports[0].write(0x100, 0x4444_4449);
        ports[0].set_alternate_output(0, 2, Some(true));
        assert_eq!(ports[0].read(0x108) & 1, 1);
        assert_eq!(ports[0].exti_levels() & 1, 1);

        afio.write(0x2004, 1 << 8);
        afio.write(0x2008, 1);
        afio.apply(&mut ports);
        assert_eq!(ports[0].read(0x108) & 1, 0);
        ports[1].set_external(0, true);
        assert_eq!(ports[1].exti_levels() & 1, 1);
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

/// GPIO alternate-function and EXTI routing registers on STM32F1.
pub(crate) struct F1Afio {
    pub(crate) base: u32,
    pub(crate) ports: [u32; 7],
    pub(crate) exti: [u32; 16],
    event: u32,
    map: [u32; 2],
    external: [u32; 4],
}

const TIM1: &[&[(usize, usize)]] = &[
    &[(0, 8), (0, 9), (0, 10), (0, 11), (1, 13), (1, 14), (1, 15)],
    &[(0, 8), (0, 9), (0, 10), (0, 11), (0, 7), (1, 0), (1, 1)],
    &[],
    &[(4, 9), (4, 11), (4, 13), (4, 14), (4, 8), (4, 10), (4, 12)],
];
const TIM2: &[&[(usize, usize)]] = &[
    &[(0, 0), (0, 1), (0, 2), (0, 3)],
    &[(0, 15), (1, 3), (0, 2), (0, 3)],
    &[(0, 0), (0, 1), (1, 10), (1, 11)],
    &[(0, 15), (1, 3), (1, 10), (1, 11)],
];
const TIM3: &[&[(usize, usize)]] = &[
    &[(0, 6), (0, 7), (1, 0), (1, 1)],
    &[],
    &[(1, 4), (1, 5), (1, 0), (1, 1)],
    &[(2, 6), (2, 7), (2, 8), (2, 9)],
];
const TIM4: &[&[(usize, usize)]] = &[
    &[(1, 6), (1, 7), (1, 8), (1, 9)],
    &[(3, 12), (3, 13), (3, 14), (3, 15)],
];

impl F1Afio {
    pub(crate) const SIZE: u32 = 0x400;

    pub(crate) fn new(base: u32, ports: [u32; 7], exti: [u32; 16]) -> Self {
        Self {
            base,
            ports,
            exti,
            event: 0,
            map: [0; 2],
            external: [0; 4],
        }
    }

    pub(crate) fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    pub(crate) fn reset(&mut self) {
        self.event = 0;
        self.map.fill(0);
        self.external.fill(0);
    }

    pub(crate) fn read(&self, address: u32) -> u32 {
        match address - self.base {
            0x00 => self.event,
            0x04 => self.map[0],
            offset @ (0x08 | 0x0c | 0x10 | 0x14) => self.external[(offset as usize - 8) / 4],
            0x1c => self.map[1],
            _ => 0,
        }
    }

    pub(crate) fn write(&mut self, address: u32, value: u32) -> bool {
        match address - self.base {
            0x00 => self.event = value & 0x7f,
            0x04 => self.map[0] = value & 0x77ff_ffff,
            offset @ (0x08 | 0x0c | 0x10 | 0x14) => {
                self.external[(offset as usize - 8) / 4] = value & 0xffff
            }
            0x1c => self.map[1] = value & 0x3fff,
            _ => return false,
        }
        true
    }

    pub(crate) fn apply(&self, gpios: &mut [Stm32Gpio]) {
        let mut alternate = [[0; 16]; 7];
        let route = |alternate: &mut [[u8; 16]; 7], timer, pins: &[(usize, usize)]| {
            for &(port, pin) in pins {
                alternate[port][pin] = timer;
            }
        };
        for (timer, shift, mask, choices) in [
            (1, 6, 3, TIM1),
            (2, 8, 3, TIM2),
            (3, 10, 3, TIM3),
            (4, 12, 1, TIM4),
        ] {
            route(
                &mut alternate,
                timer,
                choices[((self.map[0] >> shift) & mask) as usize],
            );
        }
        route(&mut alternate, 5, &[(0, 3)]);
        for (timer, bit, primary, remapped) in [
            (9, 5, &[(0, 2), (0, 3)][..], &[(4, 5), (4, 6)][..]),
            (10, 6, &[(1, 8)], &[(5, 6)]),
            (11, 7, &[(1, 9)], &[(5, 7)]),
            (13, 8, &[(0, 6)], &[(5, 8)]),
            (14, 9, &[(0, 7)], &[(5, 9)]),
        ] {
            if self.map[1] & (1 << bit) != 0 {
                route(&mut alternate, timer, remapped);
            } else {
                for &(port, pin) in primary {
                    if alternate[port][pin] == 0 {
                        alternate[port][pin] = timer;
                    }
                }
            }
        }
        for (port, &base) in self.ports.iter().enumerate() {
            let Some(gpio) = gpios.iter_mut().find(|gpio| gpio.base == base) else {
                continue;
            };
            for pin in 0..16 {
                gpio.set_f1_alternate(pin, alternate[port][pin]);
                let selected = (self.external[pin / 4] >> ((pin % 4) * 4)) & 15;
                gpio.exti_routes[pin] = if selected as usize == port {
                    self.exti[pin]
                } else {
                    0
                };
            }
        }
    }
}
