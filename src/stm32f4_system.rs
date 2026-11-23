use crate::stm32f4_rcc::F4Rcc;

/// STM32F407 system registers used during early firmware bring-up.
#[derive(Default)]
pub struct F4System {
    debug_control: u32,
    adc_common_clock: u32,
    rmii: u32,
    exti_select: [u16; 4],
}

impl F4System {
    pub const DBGMCU_CR: u32 = 0xe004_2004;
    pub const ADC_CCR: u32 = 0x4001_2304;
    pub const SYSCFG: u32 = 0x4001_3800;
    const PMC: u32 = Self::SYSCFG + 4;
    const EXTICR: u32 = Self::SYSCFG + 8;
    pub const REGISTERS: [u32; 7] = [
        Self::DBGMCU_CR,
        Self::ADC_CCR,
        Self::PMC,
        Self::EXTICR,
        Self::EXTICR + 4,
        Self::EXTICR + 8,
        Self::EXTICR + 12,
    ];

    fn exticr(address: u32) -> Option<usize> {
        let offset = address.wrapping_sub(Self::EXTICR);
        (offset < 16 && offset & 3 == 0).then_some((offset / 4) as usize)
    }

    pub fn exti_port(&self, line: u8) -> u32 {
        u32::from((self.exti_select[usize::from(line / 4)] >> ((line % 4) * 4)) & 15)
    }

    pub fn reset_syscfg(&mut self) {
        self.rmii = 0;
        self.exti_select = [0; 4];
    }

    pub fn read(&self, address: u32, rcc: &F4Rcc) -> Option<u32> {
        match address {
            Self::DBGMCU_CR => Some(self.debug_control),
            Self::ADC_CCR if rcc.adc_common_enabled() => Some(self.adc_common_clock),
            Self::ADC_CCR => Some(0),
            Self::PMC => Some(if rcc.enabled(Self::SYSCFG) {
                self.rmii
            } else {
                0
            }),
            _ => Self::exticr(address).map(|index| {
                if rcc.enabled(Self::SYSCFG) {
                    u32::from(self.exti_select[index])
                } else {
                    0
                }
            }),
        }
    }

    pub fn write(&mut self, address: u32, value: u32, rcc: &F4Rcc) -> bool {
        match address {
            Self::DBGMCU_CR => self.debug_control = value & 0xe7,
            Self::ADC_CCR if rcc.adc_common_enabled() => self.adc_common_clock = value & (3 << 16),
            Self::ADC_CCR => {}
            Self::PMC if rcc.enabled(Self::SYSCFG) => self.rmii = value & (1 << 23),
            Self::PMC => {}
            _ => {
                let Some(index) = Self::exticr(address) else {
                    return false;
                };
                if rcc.enabled(Self::SYSCFG) {
                    self.exti_select[index] = value as u16;
                }
            }
        }
        true
    }

    pub fn reset_adc(&mut self) {
        self.adc_common_clock = 0;
    }
}
