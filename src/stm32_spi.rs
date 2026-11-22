use crate::stm32_gpio_board::GpioLed;

const ENABLE: u32 = 1 << 6;
const RX_DMA_ENABLE: u32 = 1;
const TX_DMA_ENABLE: u32 = 1 << 1;
const ERROR_IRQ: u32 = 1 << 5;
const RX_IRQ: u32 = 1 << 6;
const TX_IRQ: u32 = 1 << 7;

pub struct Stp08 {
    pub leds: Vec<GpioLed>,
}

impl Stp08 {
    fn transmit(&mut self, byte: u8, report_leds: bool) {
        for led in &mut self.leds {
            led.sample(u16::from(byte), report_leds);
        }
    }
}

pub struct Stm32F0Spi {
    pub base: u32,
    pub irq: Option<u8>,
    pub dma_rx: u8,
    pub dma_tx: u8,
    pub shift_register: Option<Stp08>,
    capacity: u32,
    receive_count: u32,
    overrun: bool,
    registers: [u32; 9],
}

impl Stm32F0Spi {
    pub const SIZE: u32 = 0x400;

    pub fn new(base: u32, capacity: u32) -> Self {
        let mut registers = [0; 9];
        registers[1] = 7 << 8; // eight-bit frames
        registers[4] = 7;
        registers[8] = 2;
        Self {
            base,
            irq: None,
            dma_rx: 0,
            dma_tx: 0,
            shift_register: None,
            capacity,
            receive_count: 0,
            overrun: false,
            registers,
        }
    }

    pub fn reset(&mut self) {
        let target = self.shift_register.take();
        let (irq, dma_rx, dma_tx) = (self.irq, self.dma_rx, self.dma_tx);
        *self = Self::new(self.base, self.capacity);
        self.shift_register = target;
        (self.irq, self.dma_rx, self.dma_tx) = (irq, dma_rx, dma_tx);
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    fn frames_in_access(&self, width: usize) -> u32 {
        if width >= 2 && self.registers[1] & (0xf << 8) <= 7 << 8 {
            2
        } else {
            1
        }
    }

    fn rx_available(&self) -> bool {
        let eight_bit_threshold = self.registers[1] & (1 << 12) != 0;
        let eight_bit_frames = self.registers[1] & (0xf << 8) <= 7 << 8;
        self.receive_count
            >= if !eight_bit_threshold && eight_bit_frames && self.capacity > 1 {
                2
            } else {
                1
            }
    }

    pub fn interrupt_pending(&self) -> bool {
        let cr2 = self.registers[1];
        cr2 & TX_IRQ != 0
            || (cr2 & RX_IRQ != 0 && self.rx_available())
            || (cr2 & ERROR_IRQ != 0 && self.overrun)
    }

    pub fn pending_dma(&self) -> u8 {
        if self.registers[0] & ENABLE == 0 {
            return 0;
        }
        let cr2 = self.registers[1];
        let rx = if cr2 & RX_DMA_ENABLE != 0 && self.rx_available() {
            self.dma_rx
        } else {
            0
        };
        let tx = if cr2 & TX_DMA_ENABLE != 0 {
            self.dma_tx
        } else {
            0
        };
        rx | tx
    }

    pub fn read(&mut self, address: u32, width: usize) -> u32 {
        let offset = address - self.base;
        match offset {
            0x08 => {
                let status = 2
                    | u32::from(self.rx_available())
                    | (u32::from(self.overrun) << 6)
                    | (self.receive_count.min(3) << 9);
                if self.receive_count < self.capacity {
                    self.overrun = false;
                }
                status
            }
            0x0c => {
                self.receive_count = self
                    .receive_count
                    .saturating_sub(self.frames_in_access(width));
                0 // Both an unattached port and the STP08 provide zero on the receive line.
            }
            _ if offset & 3 == 0 => self
                .registers
                .get((offset / 4) as usize)
                .copied()
                .unwrap_or(0),
            _ => 0,
        }
    }

    pub fn write(&mut self, address: u32, width: usize, value: u32, report_leds: bool) {
        match address - self.base {
            0x00 => self.registers[0] = value & 0xffff,
            0x04 => self.registers[1] = value & 0x1fff,
            0x0c if self.registers[0] & ENABLE != 0 => {
                let frames = self.frames_in_access(width);
                if let Some(target) = self.shift_register.as_mut() {
                    target.transmit(value as u8, report_leds);
                    if frames == 2 {
                        target.transmit((value >> 8) as u8, report_leds);
                    }
                }
                let bidi_transmit = self.registers[0] & (3 << 14) == 3 << 14;
                if !bidi_transmit {
                    let count = self.receive_count.saturating_add(frames);
                    self.overrun |= count > self.capacity;
                    self.receive_count = count.min(self.capacity);
                }
            }
            0x10 => self.registers[4] = value & 0xffff,
            0x1c => self.registers[7] = value & 0xbbf,
            0x20 => self.registers[8] = value & 0x3ff,
            _ => {}
        }
    }
}
