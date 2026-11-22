const CPU_HZ: u64 = 100_000_000;

pub struct Timer {
    wide_csr: bool,
    frequency: u32,
    fraction: u64,
    uptime: u64,
    uptime_latched: u64,
    load: u32,
    reload: u32,
    remaining: u32,
    latched: u32,
    enabled: bool,
    pending: bool,
    event_enabled: bool,
}

impl Timer {
    pub fn new(frequency: u32, wide_csr: bool) -> Self {
        Self {
            wide_csr,
            frequency,
            fraction: 0,
            uptime: 0,
            uptime_latched: 0,
            load: 0,
            reload: 0,
            remaining: 0,
            latched: 0,
            enabled: false,
            pending: false,
            event_enabled: false,
        }
    }

    pub fn size(&self) -> u32 {
        if self.wide_csr { 0x2c } else { 0x68 }
    }

    pub fn tick(&mut self) {
        let scaled = self.fraction + u64::from(self.frequency);
        let ticks = scaled / CPU_HZ;
        self.fraction = scaled % CPU_HZ;
        self.uptime = self.uptime.wrapping_add(ticks);
        if !self.enabled || ticks == 0 {
            return;
        }
        if ticks < u64::from(self.remaining) {
            self.remaining -= ticks as u32;
            return;
        }
        self.pending = true;
        if self.reload == 0 {
            self.enabled = false;
            self.remaining = 0;
        } else {
            let extra = (ticks - u64::from(self.remaining)) % u64::from(self.reload);
            self.remaining = self.reload - extra as u32;
        }
    }

    pub fn interrupt_pending(&self) -> bool {
        self.pending && self.event_enabled
    }

    // The narrow layout partitions multi-byte fields into MSB-first CSRs.
    fn register(&self, offset: u32) -> (u32, u32, u32) {
        if self.wide_csr {
            return (offset, 0, u32::MAX);
        }
        let (register, shift) = match offset {
            0x00..=0x0c => (0x00, (0x0c - offset) * 2),
            0x10..=0x1c => (0x04, (0x1c - offset) * 2),
            0x20..=0x24 => (offset - 0x18, 0),
            0x28..=0x34 => (0x10, (0x34 - offset) * 2),
            0x38..=0x44 => (offset - 0x24, 0),
            0x48..=0x54 => (0x24, (0x54 - offset) * 2),
            0x58..=0x64 => (0x28, (0x64 - offset) * 2),
            _ => unreachable!("invalid LiteX timer register"),
        };
        (register, shift, 0xff)
    }

    pub fn read(&self, offset: u32, width: usize) -> u32 {
        let (register, shift, mask) = self.register(offset & !3);
        let value = match register {
            0x00 => self.load,
            0x04 => self.reload,
            0x08 => u32::from(self.enabled),
            0x10 => self.latched,
            0x14 => u32::from(self.remaining == 0),
            0x18 => u32::from(self.pending),
            0x1c => u32::from(self.event_enabled),
            0x24 => (self.uptime_latched >> 32) as u32,
            0x28 => self.uptime_latched as u32,
            _ => 0,
        };
        let value = (value >> shift) & mask;
        if width == 1 {
            (value >> ((offset & 3) * 8)) & 0xff
        } else {
            value
        }
    }

    pub fn write(&mut self, offset: u32, width: usize, value: u32) {
        let (register, shift, mut mask) = self.register(offset & !3);
        let lane = (offset & 3) * 8;
        let value = if width == 1 {
            mask &= 0xff << lane;
            (value & 0xff) << lane
        } else {
            value
        };
        let value = value & mask;
        match register {
            0x00 => self.load = (self.load & !(mask << shift)) | (value << shift),
            0x04 => self.reload = (self.reload & !(mask << shift)) | (value << shift),
            _ if mask & 1 == 0 => {}
            0x08 => {
                let enable = value & 1 != 0;
                if enable && !self.enabled {
                    self.remaining = self.load;
                }
                self.enabled = enable;
            }
            0x0c if value & 1 != 0 => self.latched = self.remaining,
            0x18 if value & 1 != 0 => self.pending = false,
            0x1c => self.event_enabled = value & 1 != 0,
            0x20 if value & 1 != 0 => self.uptime_latched = self.uptime,
            _ => {}
        }
    }
}

pub struct SocController {
    pub base: u32,
    scratch: [u8; 4],
}

impl SocController {
    pub const SIZE: u32 = 0x100;

    pub fn new(base: u32) -> Self {
        Self {
            base,
            scratch: [0x12, 0x34, 0x56, 0x78],
        }
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    pub fn read(&self, address: u32) -> u32 {
        let offset = address - self.base;
        match offset {
            4..=0x10 => u32::from(self.scratch[(offset / 4 - 1) as usize]),
            _ => 0,
        }
    }

    pub fn write(&mut self, address: u32, value: u32) {
        let offset = address - self.base;
        if (4..=0x10).contains(&offset) {
            self.scratch[(offset / 4 - 1) as usize] = value as u8;
        }
    }
}

#[derive(Default)]
pub struct Spi {
    bits_per_word: u8,
    start: bool,
    mosi: u8,
    miso: u8,
    chip_select: bool,
    loopback: bool,
}

impl Spi {
    pub fn read(&self, offset: u32) -> u32 {
        match offset {
            0x00 => u32::from(self.bits_per_word),
            0x04 => u32::from(self.start),
            0x08 => 1,
            0x0c => u32::from(self.mosi),
            0x10 => u32::from(self.miso),
            0x14 => u32::from(self.chip_select),
            0x18 => u32::from(self.loopback),
            _ => 0,
        }
    }

    pub fn write(&mut self, offset: u32, value: u32) {
        match offset {
            0x00 => self.bits_per_word = value as u8,
            0x04 => {
                self.start = value & 1 != 0;
                if self.start {
                    self.miso = if self.loopback { self.mosi } else { 0xff };
                }
            }
            0x0c => self.mosi = value as u8,
            0x10 => self.miso = value as u8,
            0x14 => self.chip_select = value & 1 != 0,
            0x18 => self.loopback = value & 1 != 0,
            _ => {}
        }
    }
}

pub enum Gpio {
    In,
    Out(u32),
    InOut(u32),
}

impl Gpio {
    pub fn read(&self, offset: u32) -> u32 {
        match (self, offset) {
            (Self::Out(value), 0) | (Self::InOut(value), 4) => *value,
            _ => 0,
        }
    }

    pub fn write(&mut self, offset: u32, value: u32) {
        match (self, offset) {
            (Self::Out(output), 0) | (Self::InOut(output), 4) => *output = value,
            _ => {}
        }
    }
}

pub struct Mmcm {
    registers: [u16; 0x50],
    address: u8,
    data_write: u16,
    data_read: u16,
    ready: bool,
}

impl Default for Mmcm {
    fn default() -> Self {
        Self {
            registers: [0; 0x50],
            address: 0,
            data_write: 0,
            data_read: 0,
            ready: false,
        }
    }
}

impl Mmcm {
    fn read(&self, offset: u32) -> u32 {
        let value = match offset & !3 {
            0x04 => 1,
            0x10 => u32::from(self.ready),
            0x14 => u32::from(self.address),
            0x1c => u32::from(self.data_read),
            _ => 0,
        };
        value >> ((offset & 3) * 8)
    }

    fn write(&mut self, offset: u32, width: usize, value: u32) {
        let lane = (offset & 3) * 8;
        let trigger = lane == 0 && value & 1 != 0;
        match offset & !3 {
            0x00 if trigger => {
                self.address = 0;
                self.data_write = 0;
                self.data_read = 0;
                self.ready = false;
            }
            0x08 if trigger => {
                if let Some(value) = self.registers.get(usize::from(self.address)) {
                    self.data_read = *value;
                    self.ready = true;
                }
            }
            0x0c if trigger => {
                if let Some(register) = self.registers.get_mut(usize::from(self.address)) {
                    *register = self.data_write;
                    self.ready = true;
                }
            }
            0x10 if lane == 0 => self.ready = trigger,
            0x14 if lane == 0 => self.address = value as u8,
            0x18 if lane < 16 => {
                let mask = if width == 1 { 0xff << lane } else { 0xffff };
                self.data_write = (self.data_write & !mask) | ((value << lane) as u16 & mask);
            }
            _ => {}
        }
    }
}

pub enum IoDevice {
    Spi(Spi),
    Gpio(Gpio),
    Mmcm(Box<Mmcm>),
}

impl IoDevice {
    pub fn size(&self) -> u32 {
        match self {
            Self::Spi(_) => 0x50,
            Self::Mmcm(_) => 0x100,
            Self::Gpio(Gpio::InOut(_)) => 8,
            Self::Gpio(_) => 4,
        }
    }

    pub fn offset(&self, bases: &[u32], address: u32, width: usize) -> Option<u32> {
        if width != 4 && !matches!(self, Self::Mmcm(_)) {
            return None;
        }
        bases.iter().find_map(|&base| {
            let offset = address.checked_sub(base)?;
            (width <= self.size().checked_sub(offset)? as usize).then_some(offset)
        })
    }

    pub fn read(&self, offset: u32, width: usize) -> u32 {
        let value = match self {
            Self::Spi(spi) => spi.read(offset),
            Self::Gpio(gpio) => gpio.read(offset),
            Self::Mmcm(mmcm) => mmcm.read(offset),
        };
        value & (u32::MAX >> ((4 - width) * 8))
    }

    pub fn write(&mut self, offset: u32, width: usize, value: u32) {
        match self {
            Self::Spi(spi) => spi.write(offset, value),
            Self::Gpio(gpio) => gpio.write(offset, value),
            Self::Mmcm(mmcm) => mmcm.write(offset, width, value),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mmcm_reset_keeps_internal_registers() {
        let mut mmcm = Mmcm::default();
        assert_eq!(mmcm.read(4), 1);
        mmcm.write(0x14, 4, 0x4f);
        mmcm.write(0x18, 1, 0x34);
        mmcm.write(0x19, 1, 0x12);
        mmcm.write(0x0c, 4, 1);
        assert_eq!(mmcm.read(0x10), 1);
        mmcm.write(0, 4, 1);
        assert_eq!(mmcm.read(0x10), 0);
        mmcm.write(0x14, 4, 0x4f);
        mmcm.write(0x08, 4, 1);
        assert_eq!(mmcm.read(0x1c), 0x1234);
    }

    #[test]
    fn narrow_timer_latches_count_and_reloads() {
        let mut timer = Timer::new(50_000_000, false);
        timer.write(0xc, 1, 3);
        timer.write(0x1c, 1, 2);
        timer.write(0x40, 4, 1);
        timer.write(0x20, 4, 1);
        for _ in 0..4 {
            timer.tick();
        }
        timer.write(0x24, 4, 1);
        assert_eq!(timer.read(0x34, 4), 1);
        assert!(!timer.interrupt_pending());
        timer.tick();
        timer.tick();
        assert!(timer.interrupt_pending());
        timer.write(0x3c, 4, 1);
        assert!(!timer.interrupt_pending());
        timer.write(0x24, 4, 1);
        assert_eq!(timer.read(0x34, 4), 2);
        timer.write(0x44, 4, 1);
        assert_eq!(timer.read(0x64, 4), 3);
    }

    #[test]
    fn wide_timer_preserves_byte_lanes_and_expires_once() {
        let mut timer = Timer::new(100_000_000, true);
        timer.write(0, 4, 0x100);
        timer.write(1, 1, 0);
        timer.write(0, 1, 1);
        assert_eq!(timer.read(0, 4), 1);
        timer.write(8, 4, 1);
        timer.write(0x1c, 4, 1);
        timer.tick();
        assert_eq!(timer.read(8, 4), 0);
        assert!(timer.interrupt_pending());
        timer.write(0x18, 4, 1);
        timer.tick();
        assert!(!timer.interrupt_pending());
    }
}
