use crate::bus_translation::NativePeripheral;

#[derive(Default)]
pub struct Lpc43xxGpio {
    ports: [PortState; 8],
}

#[derive(Default)]
struct PortState {
    direction: u32,
    mask: u32,
    latch: u32,
    inputs: u32,
}

impl Lpc43xxGpio {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn set_inputs(&mut self, port: usize, levels: u32) {
        self.ports[port].inputs = levels;
    }

    fn register(offset: u64) -> Option<(u64, usize)> {
        if !(0x2000..0x2380).contains(&offset) || offset % 4 != 0 || offset % 0x80 >= 32 {
            return None;
        }
        Some((offset & !0x7f, ((offset % 0x80) / 4) as usize))
    }

    pub fn read_register(&self, offset: u64) -> u32 {
        let Some((register, index)) = Self::register(offset) else {
            return 0;
        };
        let port = &self.ports[index];
        let pins = (port.latch & port.direction) | (port.inputs & !port.direction);
        match register {
            0x2000 => port.direction,
            0x2080 => port.mask,
            0x2100 | 0x2200 => pins,
            0x2180 => pins & !port.mask,
            _ => 0,
        }
    }

    pub fn write_register(&mut self, offset: u64, value: u32) {
        let Some((register, index)) = Self::register(offset) else {
            return;
        };
        let port = &mut self.ports[index];
        match register {
            0x2000 => port.direction = value,
            0x2080 => port.mask = value,
            0x2100 => port.latch = value,
            0x2180 => port.latch = (port.latch & port.mask) | (value & !port.mask),
            0x2200 => port.latch |= value,
            0x2280 => port.latch &= !value,
            0x2300 => port.latch ^= value,
            _ => {}
        }
    }
}

impl NativePeripheral for Lpc43xxGpio {
    fn read(&mut self, _: Option<&str>, offset: u64) -> u64 {
        self.read_register(offset).into()
    }

    fn write(&mut self, _: Option<&str>, offset: u64, value: u64) {
        self.write_register(offset, value as u32);
    }
}
