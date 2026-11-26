// Register behavior follows Renode's STM32H7_SPI master model.
use crate::{stm32_uart::Stm32Irq, stm32h7_adc_dma::DmaRequest};

pub const SPI: &str = "SPI.STM32H7_SPI";

const SPE: u32 = 1;
const CSTART: u32 = 1 << 9;
const CSUSP: u32 = 1 << 10;
const IOLOCK: u32 = 1 << 16;
const RXP: u32 = 1;
const TXP: u32 = 1 << 1;
const DXP: u32 = 1 << 2;
const EOT: u32 = 1 << 3;
const SUSP: u32 = 1 << 11;
const TXC: u32 = 1 << 12;
const RX_DMA: u32 = 1 << 14;
const MASTER: u32 = 1 << 22;

#[derive(Default)]
struct Registers {
    cr1: u32,
    cr2: u32,
    cfg1: u32,
    cfg2: u32,
    ier: u32,
    status: u32,
    i2s: u32,
    transmitted: usize,
    tx_pending: usize,
    rx_pending: usize,
}

pub struct Spi {
    pub base: u32,
    pub rcc_bound: bool,
    pub irq: Option<Stm32Irq>,
    pub dma: Option<DmaRequest>,
    regs: Registers,
}

impl Spi {
    pub const SIZE: u32 = 0x400;
    pub const RXDR: u32 = 0x30;

    pub fn new(base: u32, rcc_bound: bool) -> Self {
        Self {
            base,
            rcc_bound,
            irq: None,
            dma: None,
            regs: Registers::default(),
        }
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    pub fn reset(&mut self) {
        self.regs = Registers::default();
    }

    pub fn interrupt(&self) -> Option<Stm32Irq> {
        let r = &self.regs;
        let pending = TXP
            | (u32::from(r.rx_pending != 0) * RXP)
            | (u32::from(r.status & (EOT | SUSP) != 0) * EOT);
        self.irq.filter(|_| r.ier & pending != 0)
    }

    pub fn read(&mut self, address: u32) -> u32 {
        let r = &mut self.regs;
        match address - self.base {
            0x00 => r.cr1,
            0x04 => r.cr2,
            0x08 => r.cfg1,
            0x0c => r.cfg2 | MASTER,
            0x10 => r.ier,
            0x14 => {
                let size = r.cr2 as u16;
                let complete = if size == 0 {
                    r.tx_pending == 0
                } else {
                    r.status & EOT != 0
                };
                r.status
                    | TXP
                    | (u32::from(r.rx_pending != 0) * (RXP | DXP))
                    | (u32::from(complete) * TXC)
                    | (u32::from(size.wrapping_sub(r.transmitted as u16)) << 16)
            }
            Self::RXDR => {
                self.consume_rx();
                0
            }
            0x50 => r.i2s,
            _ => 0,
        }
    }

    pub fn consume_rx(&mut self) {
        self.regs.rx_pending = self.regs.rx_pending.saturating_sub(1);
    }

    pub fn write(&mut self, address: u32, value: u32) -> usize {
        let r = &mut self.regs;
        match address - self.base {
            0x00 => {
                let previous = r.cr1;
                r.cr1 = (value & (SPE | 0xf900)) | ((previous | value) & CSTART);
                if r.cr1 & SPE != 0 {
                    r.cr1 |= value & IOLOCK;
                } else {
                    r.cr1 |= previous & value & IOLOCK;
                }
                if previous & !r.cr1 & SPE != 0 {
                    r.cr1 &= !CSTART;
                    r.status &= !EOT;
                    r.transmitted = 0;
                    r.tx_pending = 0;
                    r.rx_pending = 0;
                    r.cr2 &= 0xffff_0000;
                }
                if r.cr1 & !previous & CSTART != 0 {
                    r.status &= !EOT;
                }
                if value & CSUSP != 0 {
                    r.status |= EOT | SUSP;
                    r.cr1 &= !CSTART;
                }
            }
            0x04 => r.cr2 = value,
            0x08 if r.cr1 & SPE == 0 => r.cfg1 = value & 0x705f_dfff,
            0x0c if r.cr1 & SPE == 0 => r.cfg2 = value & 0xf7fe_80ff,
            0x10 => r.ier = value & 0x7ff,
            0x18 => {
                r.status &= !(value & (EOT | SUSP));
                if value & EOT != 0 {
                    r.cr1 &= !CSTART;
                    r.transmitted = 0;
                }
            }
            0x20 => r.tx_pending += 1,
            0x50 => r.i2s = value & 0x03ff_7fbe,
            _ => {}
        }

        if r.cr1 & (SPE | CSTART) != (SPE | CSTART) || r.tx_pending == 0 {
            return 0;
        }
        // An unpopulated bus returns a zero word for each transmitted frame.
        let frames = std::mem::take(&mut r.tx_pending);
        r.rx_pending += frames;
        r.transmitted += frames;
        let size = usize::from(r.cr2 as u16);
        if size != 0 && r.transmitted == size {
            r.status |= EOT;
            r.cr1 &= !CSTART;
        }
        if r.cfg1 & RX_DMA != 0 { frames } else { 0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spi4_routes_receive_dma_and_obeys_rcc_clock_and_reset() {
        let path = std::env::temp_dir().join(format!("penv-spi4-{}.repl", std::process::id()));
        std::fs::write(
            &path,
            r#"cpu: CPU.CortexM @ sysbus
    cpuType: "cortex-m0"
    nvic: nvic
nvic: IRQControllers.NVIC @ sysbus 0xE000E000
    priorityMask: 0xF0
    -> cpu@0
ram: Memory.MappedMemory @ sysbus 0x20000000
    size: 0x1000
rcc: Miscellaneous.STM32H7_RCC @ sysbus 0x58024400
dma1: DMA.STM32DMA @ {
    sysbus 0x40020000;
    rcc RegistrationPeripheralAlias.DMA1
}
dmamux1: Miscellaneous.STM32_DMAMUX @ sysbus 0x40020800
    numberOfInputRequestChannels: 115
    numberOfOutputRequestChannels: 16
    numberOfRequestGeneratorChannels: 8
    0 -> dma1@0
spi4: SPI.STM32H7_SPI @ {
    sysbus 0x40013400;
    rcc RegistrationPeripheralAlias.SPI4
}
    DMARecieve -> dmamux1@83
    IRQ -> nvic@84
"#,
        )
        .unwrap();
        let result = crate::network_repl::load(&path, None);
        std::fs::remove_file(path).unwrap();
        let mut bus = result.unwrap().bus;
        let write = |bus: &mut crate::bus::Bus, address, width, value| {
            bus.write(address, width, value, &mut std::io::sink())
                .unwrap();
        };
        let spi = 0x4001_3400;
        let rcc = 0x5802_4400;
        let dma = 0x4002_0000;
        let mux = 0x4002_0800;
        let ram = 0x2000_0000;
        assert_eq!(bus.read::<4>(spi + 0x14).unwrap(), 0);
        write(&mut bus, spi + 4, 4, 3);
        write(&mut bus, rcc + 0xf0, 4, 1 << 13);
        assert_eq!(bus.read::<4>(spi + 4).unwrap(), 0);
        assert_eq!(bus.read::<4>(spi + 0x14).unwrap(), TXC | TXP);

        write(&mut bus, rcc + 0xd8, 4, 1);
        write(&mut bus, ram, 4, 0xffff_ffff);
        write(&mut bus, dma + 0x14, 4, 2);
        write(&mut bus, dma + 0x18, 4, spi + Spi::RXDR);
        write(&mut bus, dma + 0x1c, 4, ram);
        write(&mut bus, dma + 0x10, 4, (1 << 10) | 1);
        write(&mut bus, mux, 4, 82);
        write(&mut bus, spi + 8, 4, RX_DMA | 7);
        write(&mut bus, spi + 4, 4, 3);
        write(&mut bus, spi + 0x10, 4, EOT);
        write(&mut bus, spi, 4, SPE | CSTART);
        write(&mut bus, spi + 0x20, 1, 0xaa);
        assert_eq!(bus.read::<4>(dma + 0x14).unwrap(), 2);
        assert_eq!(
            bus.read::<4>(spi + 0x14).unwrap(),
            (2 << 16) | TXP | RXP | DXP
        );
        assert_eq!(bus.read::<1>(spi + Spi::RXDR).unwrap(), 0);

        write(&mut bus, mux, 4, 83);
        write(&mut bus, spi + 0x20, 2, 0xbbcc);
        write(&mut bus, spi + 0x20, 4, 0x1122_3344);
        assert_eq!(bus.read::<4>(ram).unwrap(), 0xffff_0000);
        assert_eq!(bus.read::<4>(dma + 0x14).unwrap(), 0);
        assert_ne!(bus.read::<4>(dma).unwrap() & (1 << 5), 0);
        assert_eq!(bus.read::<4>(spi + 0x14).unwrap(), EOT | TXC | TXP);
        assert_eq!(bus.read::<4>(spi).unwrap(), SPE);
        bus.sample_nvic(&crate::nvic::ActiveExceptions::default(), false);
        assert_ne!(bus.read::<4>(0xe000_e208).unwrap() & (1 << (84 - 64)), 0);

        write(&mut bus, rcc + 0x98, 4, 1 << 13);
        assert_eq!(bus.read::<4>(spi + 0xc).unwrap(), 0);
        write(&mut bus, rcc + 0x98, 4, 0);
        assert_eq!(bus.read::<4>(spi + 0x14).unwrap(), TXC | TXP);
        assert_eq!(bus.read::<4>(spi + 0xc).unwrap(), MASTER);
        assert_eq!(bus.read::<4>(spi + 8).unwrap(), 0);
    }

    #[test]
    fn endless_transfer_ends_on_suspend_and_clears_irq_sources_independently() {
        let mut spi = Spi::new(0, false);
        spi.irq = Some(Stm32Irq::Nvic(84));
        spi.write(0x10, EOT | RXP);
        spi.write(0, SPE);
        spi.write(0x08, RX_DMA);
        assert_eq!(spi.read(0x08), 0);
        spi.write(0x20, 0xa5);
        assert_eq!(spi.read(0x14), TXP);
        spi.write(0, SPE | CSTART);
        assert_eq!(spi.read(0x14) & 0xffff, TXP | TXC | RXP | DXP);
        assert!(matches!(spi.interrupt(), Some(Stm32Irq::Nvic(84))));
        assert_eq!(spi.read(Spi::RXDR), 0);
        assert!(spi.interrupt().is_none());
        spi.write(0, SPE | CSUSP);
        assert_eq!(spi.read(0), SPE);
        assert_eq!(spi.read(0x14) & 0xffff, EOT | SUSP | TXC | TXP);
        spi.write(0x18, EOT);
        assert!(spi.interrupt().is_some());
        spi.write(0x18, SUSP);
        assert!(spi.interrupt().is_none());
    }
}
