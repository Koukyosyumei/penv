use crate::stm32_uart::Stm32Irq;

const CMPM: u32 = 1;
const ARRM: u32 = 1 << 1;
const CMPOK: u32 = 1 << 3;
const ARROK: u32 = 1 << 4;
const REPOK: u32 = 1 << 8;
const DIEROK: u32 = 1 << 24;
const IRQ_FLAGS: u32 = CMPM | ARRM | CMPOK | ARROK | REPOK;

#[derive(Clone, Copy)]
enum Mode {
    Single,
    Continuous,
}

pub struct LpTimer {
    pub base: u32,
    pub clock: Option<u8>,
    pub irq: Option<Stm32Irq>,
    frequency: u32,
    phase: u64,
    mode: Option<Mode>,
    enabled: bool,
    status: u32,
    interrupt_enable: u32,
    config: u32,
    compare: u16,
    reload: u16,
    counter: u16,
    repetition: u8,
}

impl LpTimer {
    pub const MODEL: &str = "Timers.STM32L0_LpTimer";
    pub const SIZE: u32 = 0x400;
    pub const RCC_ALIASES: [&str; 5] = ["LPTIM1", "LPTIM2", "LPTIM3", "LPTIM4", "LPTIM5"];

    pub fn new(base: u32, clock: Option<u8>, frequency: u32, irq: Option<Stm32Irq>) -> Self {
        Self {
            base,
            clock,
            irq,
            frequency,
            phase: 0,
            mode: None,
            enabled: false,
            status: 0,
            interrupt_enable: 0,
            config: 0,
            compare: 0,
            reload: 1,
            counter: 0,
            repetition: 0,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new(self.base, self.clock, self.frequency, self.irq);
    }

    pub fn set_frequency(&mut self, frequency: u32) {
        if self.frequency != frequency {
            self.frequency = frequency;
            self.phase = 0;
        }
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    pub fn interrupt(&self) -> Option<Stm32Irq> {
        self.irq
            .filter(|_| self.status & self.interrupt_enable & IRQ_FLAGS != 0)
    }

    pub fn read(&self, address: u32) -> u32 {
        match address - self.base {
            0x00 => self.status,
            0x08 => self.interrupt_enable,
            0x0c => self.config,
            0x10 => u32::from(self.enabled),
            0x14 => self.compare.into(),
            0x18 => self.reload.into(),
            0x1c => self.counter.into(),
            0x28 => self.repetition.into(),
            _ => 0,
        }
    }

    pub fn write(&mut self, address: u32, width: usize, value: u32) {
        let word = address & !3;
        let shift = (address & 3) * 8;
        let mask = (u32::MAX >> (32 - width * 8)) << shift;
        let bits = (value << shift) & mask;
        let value = (self.read(word) & !mask) | bits;
        match word - self.base {
            0x04 => self.status &= !bits,
            0x08 => {
                self.interrupt_enable = value & 0x17f;
                self.status |= DIEROK;
            }
            0x0c => {
                if (self.config ^ value) & (7 << 9) != 0 {
                    self.phase = 0;
                }
                self.config = value & 0x01fe_eedf;
            }
            0x10 => {
                self.enabled = value & 1 != 0;
                if !self.enabled {
                    self.mode = None;
                    self.counter = 0;
                    self.phase = 0;
                } else if matches!(bits & 6, 2 | 4) {
                    self.counter = 0;
                    self.phase = 0;
                    self.mode = Some(if bits & 2 != 0 {
                        Mode::Single
                    } else {
                        Mode::Continuous
                    });
                }
            }
            0x14 => {
                self.compare = value as u16;
                self.status |= CMPOK;
            }
            0x18 => {
                self.reload = value as u16;
                self.status |= ARROK;
            }
            0x28 => {
                self.repetition = value as u8;
                self.status |= REPOK;
            }
            _ => {}
        }
    }

    pub fn tick(&mut self, steps_per_second: u32) {
        let Some(mode) = self.mode else { return };
        if self.reload == 0 || self.config & 1 != 0 {
            return;
        }
        let divisor = u64::from(steps_per_second) << ((self.config >> 9) & 7);
        self.phase += u64::from(self.frequency);
        let ticks = (self.phase / divisor) as u32;
        self.phase %= divisor;
        if ticks == 0 {
            return;
        }
        let counter = u32::from(self.counter);
        let reload = u32::from(self.reload);
        let compare = u32::from(self.compare);
        let until_reload = reload.saturating_sub(counter).max(1);
        if compare < reload && counter < compare && counter + ticks >= compare {
            self.status |= CMPM;
        }
        if ticks < until_reload {
            self.counter = (counter + ticks) as u16;
            return;
        }

        self.status |= ARRM;
        match mode {
            Mode::Single => {
                self.mode = None;
                self.counter = self.reload;
            }
            Mode::Continuous => {
                let remaining = ticks - until_reload;
                if compare != 0 && compare < reload && remaining >= compare {
                    self.status |= CMPM;
                }
                self.counter = (remaining % reload) as u16;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        bus::Bus, stm32h7_exti::H7Exti, stm32h7_net_control::NetworkControl, thumb::Thumb,
    };

    #[test]
    fn compare_enters_guest_irq_via_exti_and_rcc() {
        const BASE: u32 = 0x4000_2400;
        const RCC: u32 = 0x5802_4400;
        let mut bus = Bus::default();
        bus.map_ram(0, 0x400).unwrap();
        bus.map_nvic(0xf0, None).unwrap();
        let mut lines = [None; H7Exti::LINES];
        lines[47] = Some(93);
        bus.map_h7_exti(0x5800_0000, lines).unwrap();
        bus.map_h7_lptim(LpTimer::new(
            BASE,
            Some(1),
            1_000_000,
            Some(Stm32Irq::Exti(47)),
        ))
        .unwrap();
        bus.map_h7_network_control(NetworkControl::new(RCC, None, None))
            .unwrap();

        let write = |bus: &mut Bus, address, value| {
            bus.write(address, 4, value, &mut std::io::sink()).unwrap();
        };
        write(&mut bus, (16 + 93) * 4, 0x201);
        bus.memory(0x200, 2)
            .unwrap()
            .copy_from_slice(&0x202au16.to_le_bytes()); // movs r0, #42
        write(&mut bus, 0xe000_e108, 1 << 29);
        write(&mut bus, BASE + 0x14, 5);
        write(&mut bus, RCC + 0xe8, 1 << 9);
        assert_eq!(bus.read::<4>(BASE + 0x14).unwrap(), 0);
        write(&mut bus, BASE + 0x08, CMPM);
        write(&mut bus, BASE + 0x0c, 1 << 9); // divide by 2
        write(&mut bus, BASE + 0x14, 2);
        write(&mut bus, BASE + 0x18, 4);
        write(&mut bus, BASE + 0x10, 3);
        let mut thumb = Thumb::new(Some(0));
        let mut pc = 0x100;
        let mut registers = [0; 32];
        registers[13] = 0x400;
        for _ in 0..3 {
            bus.tick_h7_peripherals();
            assert!(
                !thumb
                    .service_interrupts(&mut pc, &mut registers, &mut bus)
                    .unwrap()
            );
        }
        bus.tick_h7_peripherals();
        assert!(
            thumb
                .service_interrupts(&mut pc, &mut registers, &mut bus)
                .unwrap()
        );
        assert_eq!(pc, 0x200);
        assert_eq!(bus.read::<4>(BASE).unwrap() & (CMPM | ARRM), CMPM);
        thumb
            .step(&mut pc, &mut registers, &mut bus, &mut std::io::sink())
            .unwrap();
        assert_eq!(registers[0], 42);

        write(&mut bus, BASE + 4, CMPM);
        for _ in 0..12 {
            bus.tick_h7_peripherals();
        }
        assert_eq!(bus.read::<4>(BASE).unwrap() & (CMPM | ARRM), ARRM);
        write(&mut bus, RCC + 0x90, 1 << 9);
        write(&mut bus, RCC + 0x90, 0);
        assert_eq!(bus.read::<4>(BASE).unwrap(), 0);
        assert_eq!(bus.read::<4>(BASE + 0x18).unwrap(), 1);
    }

    #[test]
    fn continuous_counter_repeats_compare_after_rollover() {
        let mut timer = LpTimer::new(0, None, 5_000_000, None);
        timer.write(0x14, 2, 3);
        timer.write(0x18, 2, 7);
        timer.write(0x10, 1, 5);
        timer.tick(1_000_000);
        assert_eq!(timer.read(0x1c), 5);
        assert_eq!(timer.read(0) & (CMPM | ARRM), CMPM);
        timer.write(4, 1, CMPM);
        timer.tick(1_000_000);
        assert_eq!(timer.read(0x1c), 3);
        assert_eq!(timer.read(0) & (CMPM | ARRM), CMPM | ARRM);
    }
}
