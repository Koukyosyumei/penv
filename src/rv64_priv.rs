use crate::{
    pmp::{EXECUTE, READ, WRITE},
    rv64::vector::VectorState,
};

const SIE: u64 = 1 << 1;
const MIE: u64 = 1 << 3;
const SPIE: u64 = 1 << 5;
const MPIE: u64 = 1 << 7;
const SPP: u64 = 1 << 8;
const MPP: u64 = 3 << 11;
pub(crate) const VS: u64 = 3 << 9;
pub const FS: u64 = 3 << 13;
const MPRV: u64 = 1 << 17;
pub const SUM: u64 = 1 << 18;
pub const MXR: u64 = 1 << 19;
const TVM: u64 = 1 << 20;
const TW: u64 = 1 << 21;
const TSR: u64 = 1 << 22;
// Fixed Renode tlib clears the defined extension fields and stores the remaining bits.
const RENODE_MENVCFG_WRITABLE: u64 = !((0x1f << 59) | (3 << 32) | 0xfd);
const SSTATUS: u64 = SIE | SPIE | SPP | VS | FS | SUM | MXR;
const MSTATUS: u64 = SSTATUS | MIE | MPIE | MPP | MPRV | TVM | TW | TSR;
const UXL: u64 = 2 << 32;
const SXL: u64 = 2 << 34;
const S_INTERRUPTS: u64 = 0x222;
const M_INTERRUPTS: u64 = 0x888;
const INTERRUPT: u64 = 1 << 63;
const SEIP: u64 = 1 << 9;

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

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum Levels {
    Machine,
    MachineUser,
    #[default]
    MachineSupervisorUser,
}

impl Levels {
    pub fn isa_bits(self) -> u64 {
        (u64::from(self != Self::Machine) << 20)
            | (u64::from(self == Self::MachineSupervisorUser) << 18)
    }

    fn lowest_mpp(self) -> u64 {
        if self == Self::Machine { MPP } else { 0 }
    }
}

#[derive(Clone, Default)]
pub struct Privilege {
    pub mode: Mode,
    pub levels: Levels,
    pub zicboz: bool,
    pub(crate) renode_menvcfg: bool,
    pub(crate) renode_vector_memory: bool,
    menvcfg: u64,
    senvcfg: u64,
    pub status: u64,
    pub satp: u64,
    pub pmp: Option<Pmp>,
    pub bpm: Option<u64>,
    pub fcsr: u8,
    pub(crate) vector: Option<Box<VectorState>>,
    pub(crate) andes_bfloat16: bool,
    pub(crate) andes_quad_mac: bool,
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
    pub(crate) cycles_per_instruction: Option<u32>,
    retired: u64,
    time: u64,
}

impl Privilege {
    pub(crate) fn save_snapshot(&self, output: &mut impl std::io::Write) -> std::io::Result<()> {
        use crate::snapshot::write_words;
        let mode = match self.mode { Mode::User => 0, Mode::Supervisor => 1, Mode::Machine => 3 };
        write_words(output, &[
            mode, self.menvcfg, self.senvcfg, self.status, self.satp, u64::from(self.fcsr),
            self.medeleg, self.mideleg, self.enable, self.pending, self.platform_interrupts, self.injected,
            self.counter_enable, self.supervisor_counter_enable, self.inhibit, self.cycle,
            self.retired, self.time, u64::from(self.bpm.is_some()), self.bpm.unwrap_or(0),
            u64::from(self.pmp.is_some()), u64::from(self.vector.is_some()),
        ])?;
        for trap in [&self.machine, &self.supervisor] {
            write_words(output, &[u64::from(trap.vector.is_some()), trap.vector.unwrap_or(0),
                trap.scratch, trap.epc, trap.cause, trap.value])?;
        }
        if let Some(pmp) = &self.pmp { pmp.save_snapshot(output)?; }
        if let Some(vector) = &self.vector { vector.save_snapshot(output)?; }
        Ok(())
    }

    pub(crate) fn load_snapshot(&mut self, input: &mut impl std::io::Read) -> std::io::Result<()> {
        use crate::snapshot::{invalid, read_bool, read_words};
        let [mode, menvcfg, senvcfg, status, satp, fcsr, medeleg, mideleg, enable, pending,
            platform_interrupts, injected, counter_enable, supervisor_counter_enable, inhibit,
            cycle, retired, time, bpm_present, bpm, pmp_present, vector_present] = read_words(input)?;
        if read_bool(pmp_present)? != self.pmp.is_some() || read_bool(vector_present)? != self.vector.is_some() {
            return Err(invalid("snapshot CPU extensions differ from the platform"));
        }
        self.mode = match mode {
            0 => Mode::User,
            1 => Mode::Supervisor,
            3 => Mode::Machine,
            _ => return Err(invalid("invalid snapshot CPU mode")),
        };
        self.menvcfg = menvcfg;
        self.senvcfg = senvcfg;
        self.status = status;
        self.satp = satp;
        self.fcsr = u8::try_from(fcsr).map_err(std::io::Error::other)?;
        self.medeleg = medeleg;
        self.mideleg = mideleg;
        self.enable = enable;
        self.pending = pending;
        self.platform_interrupts = platform_interrupts;
        self.injected = injected;
        self.counter_enable = counter_enable;
        self.supervisor_counter_enable = supervisor_counter_enable;
        self.inhibit = inhibit;
        self.cycle = cycle;
        self.retired = retired;
        self.time = time;
        self.bpm = read_bool(bpm_present)?.then_some(bpm);
        for trap in [&mut self.machine, &mut self.supervisor] {
            let [present, vector, scratch, epc, cause, value] = read_words(input)?;
            *trap = Trap { vector: read_bool(present)?.then_some(vector), scratch, epc, cause, value };
        }
        if let Some(pmp) = &mut self.pmp { pmp.load_snapshot(input)?; }
        if vector_present != 0 { self.vector = Some(Box::new(VectorState::load_snapshot(input)?)); }
        Ok(())
    }

    pub(crate) fn initialize_renode_reset_status(&mut self, floating_point: bool) {
        self.renode_menvcfg = self.levels != Levels::Machine;
        self.machine.vector.get_or_insert(u64::from(crate::csr::DEFAULT_MTVEC));
        const FS_INITIAL: u64 = 1 << 13;
        const XS_INITIAL: u64 = 1 << 15;
        self.status = MPP
            | if floating_point {
                FS_INITIAL | XS_INITIAL
            } else {
                0
            };
    }

    pub fn configure_levels(&mut self, levels: Levels) {
        self.levels = levels;
        self.status = (self.status & !MPP) | levels.lowest_mpp();
    }

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
            self.cycle = self.cycle.wrapping_add(u64::from(self.cycles_per_instruction.unwrap_or(1)));
        }
        self.inhibit & 4 == 0
    }

    pub fn retire(&mut self) {
        self.retired = self.retired.wrapping_add(1);
    }

    fn pending(&self) -> u64 {
        self.pending | self.injected
    }

    pub fn interrupt_enabled_and_pending(&self) -> bool {
        self.enable & self.pending() != 0
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
        self.status = (self.status & !(MPP | MPIE | MIE))
            | self.levels.lowest_mpp()
            | MPIE
            | ((self.status & MPIE) >> 4);
        if self.mode != Mode::Machine {
            self.status &= !MPRV;
        }
        Some(self.machine.epc & epc_mask)
    }

    pub fn sret(&mut self, epc_mask: u64) -> Option<u64> {
        if self.levels != Levels::MachineSupervisorUser
            || self.mode == Mode::User
            || (self.mode == Mode::Supervisor && self.status & TSR != 0)
        {
            return None;
        }
        self.mode = Mode::from_field((self.status & SPP) >> 8);
        self.status =
            (self.status & !(SPP | SPIE | SIE | MPRV)) | SPIE | ((self.status & SPIE) >> 4);
        Some(self.supervisor.epc & epc_mask)
    }

    pub fn allows_sfence(&self) -> bool {
        self.levels == Levels::MachineSupervisorUser
            && (self.mode == Mode::Machine
                || (self.mode == Mode::Supervisor && self.status & TVM == 0))
    }

    pub fn allows_cbo_zero(&self) -> bool {
        const CBZE: u64 = 1 << 7;
        self.zicboz
            && (self.mode == Mode::Machine
                || (self.menvcfg & CBZE != 0
                    && (self.mode == Mode::Supervisor
                        || self.levels != Levels::MachineSupervisorUser
                        || self.senvcfg & CBZE != 0)))
    }

    pub fn allows_wfi(&self) -> bool {
        self.mode != Mode::User && (self.mode == Mode::Machine || self.status & TW == 0)
    }

    pub fn set_vector(&mut self, vector: u64) {
        self.machine.write(0x05, vector);
    }

    pub(crate) fn monitor_trap_vector(
        &mut self, register: crate::csr::TrapVector, value: Option<u64>,
    ) -> Result<Option<u64>, String> {
        let trap = match register {
            crate::csr::TrapVector::Machine => &mut self.machine,
            crate::csr::TrapVector::Supervisor if self.levels == Levels::MachineSupervisorUser => {
                &mut self.supervisor
            }
            crate::csr::TrapVector::Supervisor => return Err("STVEC requires supervisor mode".into()),
        };
        if let Some(value) = value {
            trap.write(0x05, value);
            Ok(None)
        } else {
            Ok(Some(trap.vector.unwrap_or(0)))
        }
    }

    pub(crate) fn set_monitor_trap_value(&mut self, value: u64) {
        self.machine.value = value;
    }

    pub(crate) fn monitor_mepc(&mut self, value: u64) {
        self.machine.write(0x41, value);
    }

    pub(crate) fn monitor_sepc(&mut self, value: Option<u64>, alignment: u64) -> Result<Option<u64>, String> {
        if self.levels != Levels::MachineSupervisorUser {
            return Err("SEPC requires supervisor mode".into());
        }
        if let Some(value) = value {
            self.supervisor.write(0x41, value);
            Ok(None)
        } else {
            Ok(self.supervisor.read(0x41, alignment))
        }
    }

    pub(crate) fn monitor_privilege(&mut self, value: Option<u64>) -> Result<Option<u64>, String> {
        let Some(value) = value else { return Ok(Some(self.mode as u64)); };
        self.mode = match (value, self.levels) {
            (0, Levels::MachineUser | Levels::MachineSupervisorUser) => Mode::User,
            (1, Levels::MachineSupervisorUser) => Mode::Supervisor,
            (3, _) => Mode::Machine,
            _ => return Err(format!("unsupported RISC-V privilege mode {value}")),
        };
        Ok(None)
    }

    pub(crate) fn monitor_register(&self, name: &str) -> Option<u64> {
        Some(match name {
            "vstart" => return self.vector.as_ref()?.read(0x008),
            "mstatus" => self.status,
            "mepc" => self.machine.epc,
            "mcause" => self.machine.cause,
            "mtval" => self.machine.value,
            "sepc" if self.levels == Levels::MachineSupervisorUser => self.supervisor.epc,
            _ => return self.external_pmp()?.read(external_pmp_csr(name)?),
        })
    }

    pub(crate) fn write_external_pmp(&mut self, name: &str, value: u64) -> Option<()> {
        let csr = external_pmp_csr(name)?;
        let pmp = self.pmp.as_mut().filter(|pmp| pmp.allow_when_inactive)?;
        pmp.write(csr, value).then_some(())
    }

    fn external_pmp(&self) -> Option<&Pmp> {
        self.pmp.as_ref().filter(|pmp| pmp.allow_when_inactive)
    }

    pub fn read(&self, csr: u32, misa: u64, epc_mask: u64) -> Option<u64> {
        if ((csr >> 8) & 3) as u64 > self.mode as u64 {
            return None;
        }
        if self.levels != Levels::MachineSupervisorUser
            && (csr >> 8 & 3 == 1 || matches!(csr, 0x302 | 0x303))
        {
            return None;
        }
        let sd = if self.status & FS == FS || self.status & VS == VS {
            1 << 63
        } else {
            0
        };
        Some(match csr {
            1..=3 if self.status & FS != 0 && misa & (1 << 5) != 0 => match csr {
                1 => u64::from(self.fcsr & 31),
                2 => u64::from(self.fcsr >> 5),
                _ => u64::from(self.fcsr),
            },
            0x008..=0x00a | 0x00f | 0xc20..=0xc22 if self.status & VS != 0 => {
                self.vector.as_ref()?.read(csr)?
            }
            0x100 => (self.status & SSTATUS) | UXL | sd,
            0x104 => self.enable & self.mideleg,
            0x106 => self.supervisor_counter_enable,
            0x10a if self.zicboz => self.senvcfg,
            0x144 => self.pending() & self.mideleg,
            0x180 if self.allows_sfence() => self.satp,
            0x300 => {
                self.status
                    | sd
                    | match self.levels {
                        Levels::Machine => 0,
                        Levels::MachineUser => UXL,
                        Levels::MachineSupervisorUser => UXL | SXL,
                    }
            }
            0x301 => misa,
            0x302 => self.medeleg,
            0x303 => self.mideleg,
            0x304 => self.enable,
            0x306 => self.counter_enable,
            0x30a if self.zicboz || self.renode_menvcfg => self.menvcfg,
            0x310 => return None,
            0x320 => self.inhibit,
            0x344 => self.pending(),
            0xfc2 if self.andes_bfloat16 || self.andes_quad_mac => {
                (u64::from(self.andes_bfloat16) << 32)
                    | (u64::from(self.vector.is_some()) << 36)
                    | (u64::from(self.andes_quad_mac) << 52)
                    | 0x10020
            }
            0xfc3 | 0xfcb if self.andes_bfloat16 || self.andes_quad_mac => 0,
            0xfca if self.andes_bfloat16 || self.andes_quad_mac => {
                u64::from(self.andes_quad_mac) << 35
            }
            0xfc7 if self.andes_bfloat16 || self.andes_quad_mac => {
                if self.vector.is_none() || misa & (1 << 5) == 0 {
                    0
                } else if misa & (1 << 3) != 0 {
                    3 << 16
                } else {
                    2 << 16
                }
            }
            0x747 => u64::from(self.pmp.as_ref()?.mseccfg?),
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

    pub(crate) fn configure_vector(&mut self, kind: u64, avl: Option<u64>) -> Option<u64> {
        if self.status & VS == 0 && !self.renode_vector_memory {
            return None;
        }
        let length = self.vector.as_mut()?.configure(kind, avl);
        if !self.renode_vector_memory {
            self.status |= VS;
        }
        Some(length)
    }

    pub(crate) fn execute_vector(&mut self, instruction: u32, scalar: u64) -> Option<()> {
        if self.status & VS == 0 {
            return None;
        }
        self.vector.as_mut()?.execute(instruction, scalar)?;
        self.status |= VS;
        Some(())
    }

    pub fn csr_rmw_value(&self, csr: u32, read_value: u64) -> u64 {
        if csr == 0x344 {
            // MIP reads expose hardware OR software SEIP. CSRRS/CSRRC modify
            // only the software contribution, even while the PLIC is asserted.
            (read_value & !SEIP) | (self.injected & SEIP)
        } else {
            read_value
        }
    }

    fn counter_access(&self, index: u32) -> bool {
        self.mode == Mode::Machine
            || (self.counter_enable & (1 << index) != 0
                && (self.mode == Mode::Supervisor
                    || self.levels != Levels::MachineSupervisorUser
                    || self.supervisor_counter_enable & (1 << index) != 0))
    }

    pub(crate) fn set_monitor_mstatus(&mut self, value: u64, misa: u64) {
        let mut allowed = MSTATUS;
        if misa & (1 << 5) == 0 {
            allowed &= !FS;
        }
        if self.vector.is_none() {
            allowed &= !VS;
        }
        self.status = value & allowed;
        if self.levels == Levels::Machine {
            self.status |= MPP;
        } else if self.status & MPP == 2 << 11
            || (self.levels == Levels::MachineUser && self.status & MPP == 1 << 11)
        {
            self.status &= !MPP;
        }
    }

    pub fn write(&mut self, csr: u32, value: u64, misa: u64) -> bool {
        if self.mode == Mode::Machine && matches!(csr, 0x3a0..=0x3af | 0x3b0..=0x3ef) {
            return self.pmp.as_mut().is_some_and(|pmp| pmp.write(csr, value));
        }
        if (csr >> 10) == 3 || self.read(csr, misa, !1).is_none() {
            return false;
        }
        let mut extension_mask = if misa & (1 << 5) == 0 { !FS } else { u64::MAX };
        if self.vector.is_none() {
            extension_mask &= !VS;
        }
        match csr {
            1..=3 => {
                let (mask, shift) = [(31, 0), (0xe0, 5), (0xff, 0)][csr as usize - 1];
                self.fcsr = (self.fcsr & !mask) | ((value as u8) << shift & mask);
                self.status |= FS;
            }
            0x008..=0x00a | 0x00f => {
                if !self
                    .vector
                    .as_mut()
                    .is_some_and(|vector| vector.write(csr, value))
                {
                    return false;
                }
                self.status |= VS;
            }
            0x100 => self.status = (self.status & !SSTATUS) | (value & SSTATUS & extension_mask),
            0x104 => self.enable = (self.enable & !self.mideleg) | (value & self.mideleg),
            0x106 => self.supervisor_counter_enable = value & u64::from(u32::MAX),
            0x10a => self.senvcfg = value & (1 << 7),
            0x144 => {
                let writable = self.mideleg & (1 << 1);
                self.injected = (self.injected & !writable) | (value & writable);
            }
            0x180 if matches!(value >> 60, 0 | 8 | 9) => self.satp = value,
            0x180 | 0x301 | 0xb03..=0xb1f | 0x323..=0x33f => {}
            0x300 => self.set_monitor_mstatus(value, misa),
            0x302 => self.medeleg = value & 0xb3ff,
            0x303 => self.mideleg = value & S_INTERRUPTS,
            0x304 => self.enable = value & (M_INTERRUPTS | S_INTERRUPTS | self.platform_interrupts),
            0x306 => self.counter_enable = value & u64::from(u32::MAX),
            0x30a if self.renode_menvcfg => {
                if self.levels == Levels::Machine {
                    return false;
                }
                self.menvcfg = value & RENODE_MENVCFG_WRITABLE;
            }
            0x30a => self.menvcfg = value & (1 << 7),
            0x320 => self.inhibit = value & 5,
            0x344 => self.injected = value & S_INTERRUPTS,
            0x747 => {
                return self
                    .pmp
                    .as_mut()
                    .is_some_and(|pmp| pmp.write_mseccfg(value));
            }
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

fn external_pmp_csr(name: &str) -> Option<u32> {
    if let Some(index) = name.strip_prefix("pmpcfg") {
        let index: u32 = index.parse().ok()?;
        return (index < 16).then(|| 0x3a0 + index);
    }
    let index: u32 = name.strip_prefix("pmpaddr")?.parse().ok()?;
    (index < 64).then(|| 0x3b0 + index)
}

const MML: u8 = 1;
const MMWP: u8 = 2;
const RLB: u8 = 4;
const PMP_LOCK: u8 = 0x80;
const SHARED_READ_ONLY: u8 = PMP_LOCK | READ | WRITE | EXECUTE;

#[derive(Clone)]
pub struct Pmp {
    config: Vec<u8>,
    address: Vec<u64>,
    active: u64,
    address_mask: u64,
    allow_when_inactive: bool,
    pub(crate) external_path: Option<String>,
    mseccfg: Option<u8>,
}

impl Pmp {
    pub fn new(entries: usize, address_bits: u32) -> Self {
        Self {
            config: vec![0; entries],
            address: vec![0; entries],
            active: 0,
            address_mask: (1 << address_bits) - 1,
            allow_when_inactive: false,
            external_path: None,
            mseccfg: None,
        }
    }

    pub(crate) fn address_bits(&self) -> u32 {
        self.address_mask.count_ones()
    }

    pub fn external(entries: usize, address_bits: u32) -> Self {
        let mut pmp = Self::new(entries, address_bits);
        pmp.allow_when_inactive = true;
        pmp
    }

    fn save_snapshot(&self, output: &mut impl std::io::Write) -> std::io::Result<()> {
        use crate::snapshot::write_words;
        write_words(output, &[
            self.config.len() as u64, self.address_mask, self.active, self.mseccfg.map_or(256, u64::from),
        ])?;
        output.write_all(&self.config)?;
        write_words(output, &self.address)
    }

    fn load_snapshot(&mut self, input: &mut impl std::io::Read) -> std::io::Result<()> {
        use crate::snapshot::{invalid, read_words};
        let [entries, mask, active, security] = read_words(input)?;
        if entries != self.config.len() as u64 || mask != self.address_mask
            || security > 256 || (security == 256) != self.mseccfg.is_none()
        {
            return Err(invalid("snapshot PMP configuration differs from the platform"));
        }
        input.read_exact(&mut self.config)?;
        for address in &mut self.address {
            let [value] = read_words(input)?;
            if value & !mask != 0 {
                return Err(invalid("invalid snapshot PMP address"));
            }
            *address = value;
        }
        self.active = active;
        self.mseccfg = (security < 256).then_some(security as u8);
        Ok(())
    }

    pub fn enable_smepmp(&mut self) {
        self.mseccfg = Some(0);
    }

    fn write_mseccfg(&mut self, value: u64) -> bool {
        let Some(old) = self.mseccfg else {
            return false;
        };
        let mut allowed = MML | MMWP;
        if old & RLB != 0 || self.config.iter().all(|cfg| cfg & PMP_LOCK == 0) {
            allowed |= RLB;
        }
        self.mseccfg = Some((old & (MML | MMWP)) | (value as u8 & allowed));
        true
    }

    fn rule_permissions(&self, config: u8, mode: Mode) -> u8 {
        const RWX: u8 = READ | WRITE | EXECUTE;
        let machine = mode == Mode::Machine;
        let locked = config & PMP_LOCK != 0;
        if self.mseccfg.unwrap_or(0) & MML == 0 {
            return if machine && !locked {
                RWX
            } else {
                config & RWX
            };
        }
        if config & SHARED_READ_ONLY == SHARED_READ_ONLY {
            return READ;
        }
        if config & (READ | WRITE) == WRITE {
            let execute_bit = config & EXECUTE != 0;
            return if locked {
                EXECUTE | if machine && execute_bit { READ } else { 0 }
            } else {
                READ | if machine || execute_bit { WRITE } else { 0 }
            };
        }
        if machine == locked { config & RWX } else { 0 }
    }

    pub(crate) fn external_csr(&mut self, config: bool, index: u32, value: Option<u64>) -> Option<u64> {
        let limit = if config { 16 } else { 64 };
        if let Some(csr) = (index < limit).then(|| if config { 0x3a0 } else { 0x3b0 } + index)
            && self.read(csr).is_some()
        {
            return if let Some(value) = value {
                self.write(csr, value);
                None
            } else {
                self.read(csr)
            };
        }
        let action = if value.is_some() { "write to" } else { "read from" };
        let register = if config { "config" } else { "address" };
        let message = format!("Attempted to {action} invalid PMP {register} register {index}");
        crate::monitor_log::record(3, &message);
        eprintln!("{message}");
        value.is_none().then_some(0)
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
        if !self.allow_when_inactive {
            let entries = self.config.len();
            let warning = match csr {
                0x3a0..=0x3af if csr & 1 != 0 => Some(format!(
                    "Attempted illegal write to pmpcfg register with odd-numbered index ({})",
                    csr - 0x3a0,
                )),
                0x3a0..=0x3af if (csr - 0x3a0) as usize * 4 >= entries => Some(format!(
                    "Attempted illegal write to pmpcfg entry beyond entry count ({}, entry count is {entries})",
                    (csr - 0x3a0) * 4,
                )),
                0x3b0..=0x3ef if (csr - 0x3b0) as usize >= entries => Some(format!(
                    "Attempted illegal write to pmpaddr register beyond entry count ({}, entry count is {entries})",
                    csr - 0x3b0,
                )),
                _ => None,
            };
            if let Some(message) = warning {
                crate::monitor_log::record(2, &message);
                eprintln!("{message}");
                return false;
            }
        }
        let security = self.mseccfg.unwrap_or(0);
        let bypass = security & RLB != 0;
        let lockdown = security & MML != 0;
        if let Some(start) = self.config_start(csr) {
            for (index, config) in
                (start..(start + 8).min(self.config.len())).zip(value.to_le_bytes())
            {
                if !bypass && self.config[index] & PMP_LOCK != 0 {
                    continue;
                }
                let mut config = config & 0x9f;
                let shared_encoding = config & (READ | WRITE) == WRITE;
                if !lockdown && shared_encoding {
                    config &= !WRITE;
                }
                if lockdown
                    && !bypass
                    && config & PMP_LOCK != 0
                    && (config & EXECUTE != 0 || shared_encoding)
                    && config & SHARED_READ_ONLY != SHARED_READ_ONLY
                {
                    continue;
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
        if bypass || (self.config[index] & PMP_LOCK == 0 && !locked_by_next) {
            *address = value & self.address_mask;
        }
        true
    }

    pub fn permits(&self, address: u64, width: usize, permission: u8, mode: Mode) -> bool {
        let Some(end) = address.checked_add(width as u64) else {
            return false;
        };
        if self.allow_when_inactive && self.active == 0 {
            return true;
        }
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
                    && self.rule_permissions(config, mode) & permission == permission;
            }
        }
        let security = self.mseccfg.unwrap_or(0);
        mode == Mode::Machine
            && security & MMWP == 0
            && (security & MML == 0 || permission & EXECUTE == 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smepmp_lockdown_permissions_match_the_architectural_table() {
        const R: u8 = READ;
        const W: u8 = WRITE;
        const X: u8 = EXECUTE;
        let machine = [
            0,
            0,
            R | W,
            R | W,
            0,
            0,
            0,
            0,
            0,
            X,
            X,
            R | X,
            R,
            R | X,
            R | W,
            R,
        ];
        let lower = [
            0,
            X,
            R,
            R | W,
            R,
            R | X,
            R | W,
            R | W | X,
            0,
            0,
            X,
            X,
            0,
            0,
            0,
            R,
        ];
        let mut pmp = Pmp::new(16, 54);
        pmp.enable_smepmp();
        assert!(pmp.write_mseccfg(u64::from(MML | RLB)));
        assert!(pmp.write(0x3b0, 0x800));
        for (index, (&m, &su)) in machine.iter().zip(&lower).enumerate() {
            let rwx = ((index & 4) >> 2) | (index & 2) | ((index & 1) << 2);
            let cfg = (index & 8) << 4 | rwx | 0x08;
            assert!(pmp.write(0x3a0, cfg as u64));
            for (mode, expected) in [(Mode::Machine, m), (Mode::Supervisor, su), (Mode::User, su)] {
                for bit in [R, W, X, R | W] {
                    assert_eq!(
                        pmp.permits(0x1000, 4, bit, mode),
                        expected & bit == bit,
                        "{cfg:#x}"
                    );
                }
            }
        }
    }

    #[test]
    fn smepmp_bypass_is_locked_out_after_a_locked_tor_rule() {
        let mut pmp = Pmp::new(16, 54);
        pmp.enable_smepmp();
        assert!(pmp.write_mseccfg(u64::from(RLB)));
        assert!(pmp.write(0x3a0, 0x8900));
        assert!(pmp.write(0x3b0, 0x400));
        assert!(pmp.write(0x3b1, 0x800));
        assert!(pmp.write_mseccfg(u64::from(MML | MMWP)));
        assert!(pmp.write_mseccfg(u64::MAX));
        assert_eq!(pmp.mseccfg, Some(MML | MMWP));
        assert!(pmp.write_mseccfg(0));
        assert_eq!(pmp.mseccfg, Some(MML | MMWP));
        assert!(pmp.write(0x3b0, 1));
        assert!(pmp.write(0x3b1, 2));
        assert_eq!(
            (pmp.read(0x3b0), pmp.read(0x3b1)),
            (Some(0x400), Some(0x800))
        );
        assert!(pmp.write(0x3a0, 0x008c_0000));
        assert_eq!(pmp.read(0x3a0), Some(0x8900)); // executable M-only rule rejected
        assert!(pmp.permits(0x1000, 4, READ, Mode::Machine));
        assert!(!pmp.permits(0x1000, 4, READ, Mode::Supervisor));
        assert!(!pmp.permits(0x3000, 4, READ, Mode::Machine));
    }

    #[test]
    fn smepmp_unmatched_policy_and_disabled_lock() {
        let mut pmp = Pmp::new(16, 54);
        pmp.enable_smepmp();
        assert!(pmp.write(0x3a0, u64::from(PMP_LOCK))); // locked, A=OFF
        assert!(pmp.write_mseccfg(u64::from(MML | RLB)));
        assert_eq!(pmp.mseccfg, Some(MML));
        assert!(pmp.permits(0x1000, 4, READ | WRITE, Mode::Machine));
        assert!(!pmp.permits(0x1000, 4, EXECUTE, Mode::Machine));
        assert!(pmp.write_mseccfg(u64::from(MMWP)));
        assert!(!pmp.permits(0x1000, 4, READ, Mode::Machine));
    }

    #[test]
    fn external_pmp_is_bypassed_only_while_no_regions_are_active() {
        let mut pmp = Pmp::external(64, 54);
        assert!(pmp.permits(0x1000, 4, READ, Mode::Supervisor));
        assert!(pmp.write(0x3b0, 0x800));
        assert!(pmp.write(0x3a0, 0x08 | u64::from(READ)));
        assert!(pmp.permits(0x1000, 4, READ, Mode::Supervisor));
        assert!(!pmp.permits(0x1000, 4, WRITE, Mode::Supervisor));
        assert!(!pmp.permits(0x3000, 4, READ, Mode::Supervisor));
        assert!(pmp.write(0x3a0, 0));
        assert!(pmp.permits(0x3000, 4, READ, Mode::Supervisor));
    }
}
