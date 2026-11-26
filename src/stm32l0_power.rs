pub(crate) struct Stm32l0Power {
    pub base: u32,
    pub name: Option<String>,
    pub exti: Option<u8>,
    control: u32,
    voltage: f64,
    voltage_low: bool,
    irq: bool,
}

impl Stm32l0Power {
    pub const SIZE: u32 = 0x400;

    pub fn new(base: u32, exti: Option<u8>) -> Self {
        Self {
            base,
            name: None,
            exti,
            control: 0x1000,
            voltage: 3.3,
            voltage_low: false,
            irq: false,
        }
    }

    pub fn reset(&mut self) {
        let name = self.name.take();
        *self = Self::new(self.base, self.exti);
        self.name = name;
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    pub fn read(&self, address: u32) -> u32 {
        match address - self.base {
            0 => self.control,
            4 => 8 | (u32::from(self.irq) << 2),
            _ => 0,
        }
    }

    pub fn write(&mut self, address: u32, value: u32) {
        if address == self.base {
            let vos = value & 0x1800;
            self.control = (value & 0x1f1) | if vos == 0 { self.control & 0x1800 } else { vos };
            self.set_voltage(self.voltage);
        }
    }

    pub fn set_voltage(&mut self, voltage: f64) {
        self.voltage = voltage;
        let level = (self.control >> 5) & 7;
        if level < 7 {
            let threshold = 1.9 + f64::from(level) * 0.2;
            if voltage < threshold - 0.1 {
                self.voltage_low = true;
            } else if voltage > threshold + 0.1 {
                self.voltage_low = false;
            }
        }
        self.irq = self.voltage_low && self.control & 0x10 != 0;
    }

    pub fn interrupt(&self) -> bool {
        self.irq
    }
}

#[cfg(test)]
mod tests {
    use super::Stm32l0Power;

    #[test]
    fn pvd_respects_enable_and_threshold_hysteresis() {
        let mut power = Stm32l0Power::new(0, Some(16));
        power.set_voltage(1.7);
        assert!(!power.interrupt());
        power.write(0, 0x1010);
        assert!(power.interrupt());
        power.set_voltage(1.95);
        assert!(power.interrupt());
        power.set_voltage(2.1);
        assert!(!power.interrupt());
    }
}
