/// STM32F407 system registers used during early firmware bring-up.
#[derive(Default)]
pub struct F4System {
    debug_control: u32,
    adc_common_clock: u32,
}

impl F4System {
    pub const DBGMCU_CR: u32 = 0xe004_2004;
    pub const ADC_CCR: u32 = 0x4001_2304;
    pub const REGISTERS: [u32; 2] = [Self::DBGMCU_CR, Self::ADC_CCR];

    pub fn read(&self, address: u32, adc_enabled: bool) -> Option<u32> {
        match address {
            Self::DBGMCU_CR => Some(self.debug_control),
            Self::ADC_CCR if adc_enabled => Some(self.adc_common_clock),
            Self::ADC_CCR => Some(0),
            _ => None,
        }
    }

    pub fn write(&mut self, address: u32, value: u32, adc_enabled: bool) -> bool {
        match address {
            Self::DBGMCU_CR => self.debug_control = value & 0xe7,
            Self::ADC_CCR if adc_enabled => self.adc_common_clock = value & (3 << 16),
            Self::ADC_CCR => {}
            _ => return false,
        }
        true
    }

    pub fn reset_adc(&mut self) {
        self.adc_common_clock = 0;
    }
}
