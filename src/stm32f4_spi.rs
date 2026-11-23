use crate::{stm32_uart::Stm32Irq, stm32h7_adc_dma::StreamTarget};

const MASTER: u32 = 1 << 2;
const SPE: u32 = 1 << 6;
const RX_ONLY: u32 = 1 << 10;
const DFF: u32 = 1 << 11;
const BIDIMODE: u32 = 1 << 15;

const RXDMAEN: u32 = 1;
const ERRIE: u32 = 1 << 5;
const RXNEIE: u32 = 1 << 6;
const TXEIE: u32 = 1 << 7;

const RXNE: u32 = 1;
const TXE: u32 = 1 << 1;
const OVR: u32 = 1 << 6;

#[derive(Clone, Copy)]
enum Ad4011Transfer {
    Idle,
    ReadConfig,
    WriteConfig,
    Sample { middle: u8, low: u8 },
    LastByte(u8),
}

pub struct Ad4011 {
    reference_uv: u32,
    input_uv: u32,
    config: u8,
    transfer: Ad4011Transfer,
}

impl Ad4011 {
    pub const MODEL: &str = "Analog.AD4011_ADC";
    pub const WIRE_MODEL: &str = "Analog.AnalogWire";
    pub const SOURCE_MODEL: &str = "Analog.ADCChannelSource";

    pub fn new(reference_uv: u32, input_uv: u32) -> Self {
        Self {
            reference_uv,
            input_uv,
            config: 1,
            transfer: Ad4011Transfer::Idle,
        }
    }

    fn exchange(&mut self, data: u8) -> u8 {
        match std::mem::replace(&mut self.transfer, Ad4011Transfer::Idle) {
            Ad4011Transfer::Idle if data & 0x14 == 0x14 => {
                self.transfer = if data & 0x40 == 0 {
                    Ad4011Transfer::WriteConfig
                } else {
                    Ad4011Transfer::ReadConfig
                };
                0
            }
            Ad4011Transfer::Idle => {
                let span = if self.config & 8 != 0 {
                    self.reference_uv * 4 / 5
                } else {
                    self.reference_uv
                };
                if self.input_uv > span {
                    self.config &= !1;
                }
                let raw = (u64::from(self.input_uv) * 131072 / u64::from(span)).min(131071);
                // The fixed upstream model returns the bottom eight bits in
                // byte three, with optional status flags overwriting bits 4–5.
                let mut low = raw as u8;
                if self.config & 0x10 != 0 {
                    low = (low & !0x30) | ((self.config & 1) << 5) | ((self.config & 8) << 1);
                    self.config |= 1;
                }
                self.transfer = Ad4011Transfer::Sample {
                    middle: (raw >> 2) as u8,
                    low,
                };
                (raw >> 10) as u8
            }
            Ad4011Transfer::ReadConfig => {
                let result = self.config;
                self.config |= 1;
                result
            }
            Ad4011Transfer::WriteConfig => {
                self.config = (self.config & 1) | (data & 0x1e);
                0
            }
            Ad4011Transfer::Sample { middle, low } => {
                self.transfer = Ad4011Transfer::LastByte(low);
                middle
            }
            Ad4011Transfer::LastByte(low) => low,
        }
    }
}

pub struct F4Spi {
    pub base: u32,
    pub irq: Option<Stm32Irq>,
    pub dma: Option<StreamTarget>,
    pub loopback: bool,
    pub adc: Option<Ad4011>,
    cr1: u32,
    cr2: u32,
    crc_polynomial: u32,
    receive: Option<u16>,
    overrun: bool,
    read_dr_since_overrun: bool,
}

impl F4Spi {
    pub const MODEL: &str = "SPI.STM32SPI";
    pub const LOOPBACK_MODEL: &str = "SPI.SPILoopback";
    pub const SIZE: u32 = 0x400;
    pub const DATA: u32 = 0x0c;

    pub fn new(base: u32) -> Self {
        Self {
            base,
            irq: None,
            dma: None,
            loopback: false,
            adc: None,
            cr1: 0,
            cr2: 0,
            crc_polynomial: 7,
            receive: None,
            overrun: false,
            read_dr_since_overrun: false,
        }
    }

    pub fn reset(&mut self) {
        let (irq, dma, loopback) = (self.irq, self.dma, self.loopback);
        let adc = self
            .adc
            .take()
            .map(|adc| Ad4011::new(adc.reference_uv, adc.input_uv));
        *self = Self::new(self.base);
        self.adc = adc;
        self.irq = irq;
        self.dma = dma;
        self.loopback = loopback;
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    pub fn interrupt(&self) -> Option<Stm32Irq> {
        self.irq.filter(|_| {
            (self.cr2 & TXEIE != 0)
                || (self.cr2 & RXNEIE != 0 && self.receive.is_some())
                || (self.cr2 & ERRIE != 0 && self.overrun)
        })
    }

    pub fn dma_value(&self) -> Option<u32> {
        if self.cr2 & RXDMAEN != 0 {
            self.receive.map(u32::from)
        } else {
            None
        }
    }

    pub fn consume_rx(&mut self) -> u16 {
        self.read_dr_since_overrun |= self.overrun;
        self.receive.take().unwrap_or(0)
    }

    pub fn read(&mut self, address: u32, width: usize) -> u32 {
        let offset = address - self.base;
        let value = match offset {
            0..=3 => self.cr1,
            4..=7 => self.cr2,
            8..=11 => {
                let status =
                    TXE | u32::from(self.receive.is_some()) * RXNE | u32::from(self.overrun) * OVR;
                if offset == 8 && std::mem::take(&mut self.read_dr_since_overrun) {
                    self.overrun = false;
                }
                status
            }
            Self::DATA => u32::from(self.consume_rx()),
            0x10..=0x13 => self.crc_polynomial,
            _ => 0,
        };
        (value >> ((offset & 3) * 8)) & (u32::MAX >> (32 - width * 8))
    }

    pub fn write(&mut self, address: u32, width: usize, value: u32) {
        let offset = address - self.base;
        let shift = (offset & 3) * 8;
        let mask = (u32::MAX >> (32 - width * 8)) << shift;
        let bits = (value << shift) & mask;
        let (register, writable) = match offset {
            0..=3 => (&mut self.cr1, 0xffff),
            4..=7 => (&mut self.cr2, 0xf7),
            0x10..=0x13 => (&mut self.crc_polynomial, 0xffff),
            Self::DATA => {
                self.transfer(bits as u16);
                return;
            }
            _ => return,
        };
        let mask = mask & writable;
        *register = (*register & !mask) | (bits & mask);
    }

    fn transfer(&mut self, value: u16) {
        if self.cr1 & (SPE | MASTER | RX_ONLY | BIDIMODE) != SPE | MASTER {
            return;
        }
        if self.receive.is_some() {
            self.overrun = true;
            self.read_dr_since_overrun = false;
            return;
        }
        // As in the H7 model, an unpopulated bus supplies zero and completes immediately.
        let mask = if self.cr1 & DFF == 0 { 0xff } else { 0xffff };
        self.receive = Some(match &mut self.adc {
            Some(adc) if mask == 0xff => u16::from(adc.exchange(value as u8)),
            Some(adc) => {
                let [high, low] = value.to_be_bytes();
                u16::from_be_bytes([adc.exchange(high), adc.exchange(low)])
            }
            None if self.loopback => value & mask,
            None => 0,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renode_ad4011_direct_voltage_sweep() {
        // AD4011.robot: Should Read Correct Voltage, including its strict tolerance.
        let mut adc = Ad4011::new(4_820_000, 0);
        for input in (0..4_820_000).step_by(500_000) {
            adc.input_uv = input;
            let raw = (u32::from(adc.exchange(0)) << 10)
                | (u32::from(adc.exchange(0)) << 2)
                | u32::from(adc.exchange(0) & 3);
            assert!((f64::from(raw) * 4.82 / 131072.0 - f64::from(input) / 1e6).abs() < 0.00005);
        }
    }

    #[test]
    fn overrun_retains_first_frame_and_clears_in_dr_sr_order() {
        let mut spi = F4Spi::new(0);
        spi.irq = Some(Stm32Irq::Nvic(35));
        spi.loopback = true;
        spi.write(4, 1, ERRIE);
        spi.write(F4Spi::DATA, 1, 0x12);
        assert_eq!(spi.read(8, 2), TXE);
        spi.write(0, 2, MASTER | SPE);
        spi.write(F4Spi::DATA, 2, 0x1234);
        spi.write(F4Spi::DATA, 1, 0xab);
        assert!(spi.interrupt().is_some());
        assert_eq!(spi.read(8, 4), TXE | RXNE | OVR);
        assert_eq!(spi.read(F4Spi::DATA, 2), 0x34);
        assert!(spi.interrupt().is_some());
        assert_eq!(spi.read(8, 4), TXE | OVR);
        assert!(spi.interrupt().is_none());
        assert_eq!(spi.read(8, 1), TXE);
    }
}
