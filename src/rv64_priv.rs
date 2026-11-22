use crate::pmp::{EXECUTE, READ, WRITE};

const SIE: u64 = 1 << 1;
const MIE: u64 = 1 << 3;
const SPIE: u64 = 1 << 5;
const MPIE: u64 = 1 << 7;
const SPP: u64 = 1 << 8;
const MPP: u64 = 3 << 11;
pub const FS: u64 = 3 << 13;
const MPRV: u64 = 1 << 17;
pub const SUM: u64 = 1 << 18;
pub const MXR: u64 = 1 << 19;
const TVM: u64 = 1 << 20;
const TW: u64 = 1 << 21;
const TSR: u64 = 1 << 22;
const SSTATUS: u64 = SIE | SPIE | SPP | FS | SUM | MXR;
const MSTATUS: u64 = SSTATUS | MIE | MPIE | MPP | MPRV | TVM | TW | TSR;
const UXL: u64 = 2 << 32;
const SXL: u64 = 2 << 34;
const S_INTERRUPTS: u64 = 0x222;
const M_INTERRUPTS: u64 = 0x888;
const INTERRUPT: u64 = 1 << 63;

#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Mode {
    User = 0,
    Supervisor = 1,
    #[default]
    Machine = 3,
}

impl Mode {
    fn from_field(value: u64) -> Self {
        match value {
            1 => Self::Supervisor,
            3 => Self::Machine,
            _ => Self::User,
        }
    }
}

#[derive(Clone, Default)]
struct Trap {
    vector: Option<u64>,
    scratch: u64,
    epc: u64,
    cause: u64,
    value: u64,
}

impl Trap {
    fn enter(&mut self, pc: u64, cause: u64, value: u64) -> Option<u64> {
        let vector = self.vector?;
        self.epc = pc;
        self.cause = cause;
        self.value = value;
        let offset = if vector & 1 != 0 && cause & INTERRUPT != 0 {
            (cause & !INTERRUPT) * 4
        } else {
            0
        };
        Some((vector & !3).wrapping_add(offset))
    }

    fn read(&self, index: u32, epc_mask: u64) -> Option<u64> {
        Some(match index {
            0x05 => self.vector.unwrap_or(0),
            0x40 => self.scratch,
            0x41 => self.epc & epc_mask,
            0x42 => self.cause,
            0x43 => self.value,
            _ => return None,
        })
    }

    fn write(&mut self, index: u32, value: u64) -> bool {
        match index {
            0x05 => self.vector = Some(value & !2),
            0x40 => self.scratch = value,
            0x41 => self.epc = value & !1,
            0x42 => self.cause = value,
            0x43 => self.value = value,
            _ => return false,
        }
        true
    }
}

#[derive(Clone, Default)]
pub struct Privilege {
    pub mode: Mode,
    pub status: u64,
    pub satp: u64,
    pub pmp: Option<Pmp>,
    pub bpm: Option<u64>,
    pub fcsr: u8,
    machine: Trap,
    supervisor: Trap,
    medeleg: u64,
    mideleg: u64,
    enable: u64,
    pending: u64,
    platform_interrupts: u64,
    injected: u64,
    counter_enable: u64,
    supervisor_counter_enable: u64,
    inhibit: u64,
    cycle: u64,
    retired: u64,
    time: u64,
}

impl Privilege {
    pub fn add_interrupt(&mut self, pin: u32) {
        self.platform_interrupts |= 1 << pin;
    }

    pub fn effective_mode(&self, permission: u8) -> Mode {
        if permission != EXECUTE && self.mode == Mode::Machine && self.status & MPRV != 0 {
            Mode::from_field((self.status & MPP) >> 11)
        } else {
            self.mode
        }
    }

    pub fn tick(&mut self, pending: u64, time: u64) -> bool {
        self.pending = pending;
        self.time = time;
        if self.inhibit & 1 == 0 {
            self.cycle = self.cycle.wrapping_add(1);
        }
        self.inhibit & 4 == 0
    }

    pub fn retire(&mut self) {
        self.retired = self.retired.wrapping_add(1);
    }

    fn pending(&self) -> u64 {
        self.pending | self.injected
    }

    pub fn interrupt(&mut self, pc: u64) -> Option<u64> {
        let enabled = self.enable & self.pending();
        let machine = if self.mode < Mode::Machine || self.status & MIE != 0 {
            enabled & !self.mideleg
        } else {
            0
        };
        let supervisor = if self.mode < Mode::Supervisor
            || (self.mode == Mode::Supervisor && self.status & SIE != 0)
        {
            enabled & self.mideleg
        } else {
            0
        };
        let candidates = if machine == 0 { supervisor } else { machine };
        let platform = candidates & self.platform_interrupts;
        let code = if platform != 0 {
            u64::from(platform.trailing_zeros())
        } else {
            [11, 3, 7, 9, 1, 5]
                .into_iter()
                .find(|code| candidates & (1 << code) != 0)?
        };
        self.trap(pc, INTERRUPT | code, 0)
    }

    pub fn trap(&mut self, pc: u64, cause: u64, value: u64) -> Option<u64> {
        let delegation = if cause & INTERRUPT != 0 {
            self.mideleg
        } else {
            self.medeleg
        };
        if self.mode != Mode::Machine && delegation & (1 << (cause & 63)) != 0 {
            let target = self.supervisor.enter(pc, cause, value)?;
            let previous = if self.mode == Mode::Supervisor {
                SPP
            } else {
                0
            };
            self.status =
                (self.status & !(SPP | SPIE | SIE)) | previous | ((self.status & SIE) << 4);
            self.mode = Mode::Supervisor;
            return Some(target);
        }
        let target = self.machine.enter(pc, cause, value)?;
        self.status = (self.status & !(MPP | MPIE | MIE))
            | ((self.mode as u64) << 11)
            | ((self.status & MIE) << 4);
        self.mode = Mode::Machine;
        Some(target)
    }

    pub fn mret(&mut self, epc_mask: u64) -> Option<u64> {
        if self.mode != Mode::Machine {
            return None;
        }
        self.mode = Mode::from_field((self.status & MPP) >> 11);
        self.status = (self.status & !(MPP | MPIE | MIE)) | MPIE | ((self.status & MPIE) >> 4);
        if self.mode != Mode::Machine {
            self.status &= !MPRV;
        }
        Some(self.machine.epc & epc_mask)
    }

    pub fn sret(&mut self, epc_mask: u64) -> Option<u64> {
        if self.mode == Mode::User || (self.mode == Mode::Supervisor && self.status & TSR != 0) {
            return None;
        }
        self.mode = Mode::from_field((self.status & SPP) >> 8);
        self.status =
            (self.status & !(SPP | SPIE | SIE | MPRV)) | SPIE | ((self.status & SPIE) >> 4);
        Some(self.supervisor.epc & epc_mask)
    }

    pub fn allows_sfence(&self) -> bool {
        self.mode == Mode::Machine || (self.mode == Mode::Supervisor && self.status & TVM == 0)
    }

    pub fn allows_wfi(&self) -> bool {
        self.mode != Mode::User && (self.mode == Mode::Machine || self.status & TW == 0)
    }

    pub fn set_vector(&mut self, vector: u64) {
        self.machine.write(0x05, vector);
    }

    pub fn read(&self, csr: u32, misa: u64, epc_mask: u64) -> Option<u64> {
        if ((csr >> 8) & 3) as u64 > self.mode as u64 {
            return None;
        }
        let sd = if self.status & FS == FS { 1 << 63 } else { 0 };
        Some(match csr {
            1..=3 if self.status & FS != 0 && misa & (1 << 5) != 0 => match csr {
                1 => u64::from(self.fcsr & 31),
                2 => u64::from(self.fcsr >> 5),
                _ => u64::from(self.fcsr),
            },
            0x100 => (self.status & SSTATUS) | UXL | sd,
            0x104 => self.enable & self.mideleg,
            0x106 => self.supervisor_counter_enable,
            0x144 => self.pending() & self.mideleg,
            0x180 if self.allows_sfence() => self.satp,
            0x300 => self.status | UXL | SXL | sd,
            0x301 => misa,
            0x302 => self.medeleg,
            0x303 => self.mideleg,
            0x304 => self.enable,
            0x306 => self.counter_enable,
            0x310 => return None,
            0x320 => self.inhibit,
            0x344 => self.pending(),
            0x7c0 => self.bpm?,
            0xb00 => self.cycle,
            0xb02 => self.retired,
            0xc00..=0xc1f if self.counter_access(csr & 31) => match csr {
                0xc00 => self.cycle,
                0xc01 => self.time,
                0xc02 => self.retired,
                _ => 0,
            },
            0xb03..=0xb1f | 0x323..=0x33f | 0xf11..=0xf13 | 0xf15 => 0,
            0x105 | 0x140..=0x143 => self.supervisor.read(csr & 0xff, epc_mask)?,
            0x305 | 0x340..=0x343 | 0x3a0..=0x3af | 0x3b0..=0x3ef => self
                .machine
                .read(csr & 0xff, epc_mask)
                .or_else(|| self.pmp.as_ref()?.read(csr))?,
            _ => return None,
        })
    }

    fn counter_access(&self, index: u32) -> bool {
        self.mode == Mode::Machine
            || (self.counter_enable & (1 << index) != 0
                && (self.mode == Mode::Supervisor
                    || self.supervisor_counter_enable & (1 << index) != 0))
    }

    pub fn write(&mut self, csr: u32, value: u64, misa: u64) -> bool {
        if (csr >> 10) == 3 || self.read(csr, misa, !1).is_none() {
            return false;
        }
        let fp_mask = if misa & (1 << 5) == 0 { !FS } else { u64::MAX };
        match csr {
            1..=3 => {
                let (mask, shift) = [(31, 0), (0xe0, 5), (0xff, 0)][csr as usize - 1];
                self.fcsr = (self.fcsr & !mask) | ((value as u8) << shift & mask);
                self.status |= FS;
            }
            0x100 => self.status = (self.status & !SSTATUS) | (value & SSTATUS & fp_mask),
            0x104 => self.enable = (self.enable & !self.mideleg) | (value & self.mideleg),
            0x106 => self.supervisor_counter_enable = value & u64::from(u32::MAX),
            0x144 => {
                let writable = self.mideleg & (1 << 1);
                self.injected = (self.injected & !writable) | (value & writable);
            }
            0x180 if matches!(value >> 60, 0 | 8 | 9) => self.satp = value,
            0x180 | 0x301 | 0xb03..=0xb1f | 0x323..=0x33f => {}
            0x300 => {
                self.status = value & MSTATUS & fp_mask;
                if self.status & MPP == 2 << 11 {
                    self.status &= !MPP;
                }
            }
            0x302 => self.medeleg = value & 0xb3ff,
            0x303 => self.mideleg = value & S_INTERRUPTS,
            0x304 => self.enable = value & (M_INTERRUPTS | S_INTERRUPTS | self.platform_interrupts),
            0x306 => self.counter_enable = value & u64::from(u32::MAX),
            0x320 => self.inhibit = value & 5,
            0x344 => self.injected = value & S_INTERRUPTS,
            0x7c0 => self.bpm = Some(value),
            0xb00 => self.cycle = value,
            0xb02 => self.retired = value,
            0x105 | 0x140..=0x143 => return self.supervisor.write(csr & 0xff, value),
            0x305 | 0x340..=0x343 | 0x3a0..=0x3af | 0x3b0..=0x3ef => {
                return self.machine.write(csr & 0xff, value)
                    || self.pmp.as_mut().is_some_and(|pmp| pmp.write(csr, value));
            }
            _ => return false,
        }
        true
    }
}

#[derive(Clone)]
pub struct Pmp {
    config: Vec<u8>,
    address: Vec<u64>,
    active: u64,
    address_mask: u64,
}

impl Pmp {
    pub fn new(entries: usize, address_bits: u32) -> Self {
        Self {
            config: vec![0; entries],
            address: vec![0; entries],
            active: 0,
            address_mask: (1 << address_bits) - 1,
        }
    }

    fn config_start(&self, csr: u32) -> Option<usize> {
        let start = (csr.checked_sub(0x3a0)? as usize) * 4;
        (csr <= 0x3ae && csr & 1 == 0 && start < self.config.len()).then_some(start)
    }

    fn read(&self, csr: u32) -> Option<u64> {
        if let Some(start) = self.config_start(csr) {
            let config = &self.config[start..(start + 8).min(self.config.len())];
            let mut bytes = [0; 8];
            bytes[..config.len()].copy_from_slice(config);
            return Some(u64::from_le_bytes(bytes));
        }
        self.address.get(csr.checked_sub(0x3b0)? as usize).copied()
    }

    fn write(&mut self, csr: u32, value: u64) -> bool {
        if let Some(start) = self.config_start(csr) {
            for (index, config) in
                (start..(start + 8).min(self.config.len())).zip(value.to_le_bytes())
            {
                if self.config[index] & 0x80 != 0 {
                    continue;
                }
                let mut config = config & 0x9f;
                if config & (READ | WRITE) == WRITE {
                    config &= !WRITE;
                }
                self.config[index] = config;
                self.active &= !(1 << index);
                if config & 0x18 != 0 {
                    self.active |= 1 << index;
                }
            }
            return true;
        }

        let Some(index) = csr.checked_sub(0x3b0).map(|index| index as usize) else {
            return false;
        };
        let Some(address) = self.address.get_mut(index) else {
            return false;
        };
        let locked_by_next = self
            .config
            .get(index + 1)
            .is_some_and(|config| config & 0x98 == 0x88);
        if self.config[index] & 0x80 == 0 && !locked_by_next {
            *address = value & self.address_mask;
        }
        true
    }

    pub fn permits(&self, address: u64, width: usize, permission: u8, mode: Mode) -> bool {
        let Some(end) = address.checked_add(width as u64) else {
            return false;
        };
        let mut active = self.active;
        while active != 0 {
            let index = active.trailing_zeros() as usize;
            active &= active - 1;
            let config = self.config[index];
            let encoded = self.address[index];
            let (lower, upper) = match (config >> 3) & 3 {
                1 => (
                    index
                        .checked_sub(1)
                        .map_or(0, |previous| self.address[previous] << 2),
                    encoded << 2,
                ),
                2 => (encoded << 2, (encoded << 2) + 4),
                _ => {
                    let mask = (1 << encoded.trailing_ones()) - 1;
                    let lower = (encoded & !mask) << 2;
                    (lower, lower + ((mask + 1) << 3))
                }
            };
            if lower < upper && address < upper && lower < end {
                return address >= lower
                    && end <= upper
                    && ((mode == Mode::Machine && config & 0x80 == 0)
                        || config & permission == permission);
            }
        }
        mode == Mode::Machine
    }
}
