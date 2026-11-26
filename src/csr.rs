use crate::{
    andes_csr::AndesCsrs,
    clint::{SOFTWARE, TIMER},
    pmp::Pmp,
    vector_bootstrap::VectorBootstrap,
};

const MIE: u32 = 1 << 3;
const MPIE: u32 = 1 << 7;
const MPRV: u32 = 1 << 17;
const MPP: u32 = 3 << 11;
pub const DEFAULT_MTVEC: u32 = 0x1010;
pub const EXTERNAL: u32 = 1 << 11;
const MACHINE_INTERRUPTS: u32 = SOFTWARE | TIMER | EXTERNAL;

const SSIP: u32 = 1 << 1;
const STIP: u32 = 1 << 5;
const SEIP: u32 = 1 << 9;
const SUPERVISOR_INTERRUPTS: u32 = SSIP | STIP | SEIP;
const INTERRUPT: u32 = 1 << 31;
const SIE: u32 = 1 << 1;
const SPIE: u32 = 1 << 5;
const SPP: u32 = 1 << 8;
const SUM: u32 = 1 << 18;
const MXR: u32 = 1 << 19;
const TVM: u32 = 1 << 20;
const TSR: u32 = 1 << 22;
const VS: u32 = 3 << 9;
const FS: u32 = 3 << 13;
const SD: u32 = 1 << 31;
const SSTATUS: u32 = SIE | SPIE | SPP | SUM | MXR;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrapVector {
    Machine,
    Supervisor,
}

impl TrapVector {
    pub fn name(self) -> &'static str {
        match self {
            Self::Machine => "MTVEC",
            Self::Supervisor => "STVEC",
        }
    }
}

#[derive(Clone, Copy)]
pub enum InterruptMode {
    Auto,
    Direct,
    Vectored,
}

impl InterruptMode {
    fn correct(self, value: u32) -> u32 {
        match self {
            Self::Auto => value & !2,
            Self::Direct => value & !3,
            Self::Vectored => (value & !3) | 1,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Privilege {
    User = 0,
    Supervisor = 1,
    Machine = 3,
}

#[derive(Clone, Copy, Default)]
struct AddressTrigger {
    control: u32,
    address: u32,
}

#[derive(Clone, Default)]
struct Triggers {
    selected: usize,
    entries: [AddressTrigger; 2],
}

const MCONTROL: u32 = 2 << 28;
const MCONTROL_FIELDS: u32 = 0x5f; // M/S/U and execute/store/load

#[derive(Clone, Default)]
struct Supervisor {
    delegation: u32,
    interrupt_delegation: u32,
    injected_interrupts: u32,
    satp: u32,
    vector: u32,
    scratch: u32,
    epc: u32,
    cause: u32,
    value: u32,
}

const CYCLE_ENABLE: u32 = 1;
const INSTRET_ENABLE: u32 = 4;

#[derive(Clone, Default)]
struct Counters {
    cycle: u64,
    time: u64,
    instret: u64,
    inhibit: u32,
    machine_enable: u32,
    supervisor_enable: u32,
    instret_written: bool,
}

fn counter_half(value: u64, high: bool) -> u32 {
    if high {
        (value >> 32) as u32
    } else {
        value as u32
    }
}

fn set_counter_half(counter: &mut u64, value: u32, high: bool) {
    let shift = if high { 32 } else { 0 };
    *counter = (*counter & !(u64::from(u32::MAX) << shift)) | (u64::from(value) << shift);
}

const CLIC_CAUSE_ALIAS: u32 = 0x3800_0000;

#[derive(Clone, Copy, Default)]
struct ClicState {
    table: u32,
    threshold: u8,
    level: u8,
}

#[derive(Clone)]
pub struct Csr {
    pub hart_id: usize,
    pub m_extension: bool,
    pub zmmul: bool,
    pub a_extension: bool,
    pub c_extension: bool,
    pub compressed_single: bool,
    pub compressed_double: bool,
    pub embedded_base: bool,
    pub sscofpmf: bool,
    pub andes: Option<AndesCsrs>,
    status: u32,
    pub f_extension: bool,
    pub d_extension: bool,
    pub fcsr: u8,
    vector_bootstrap: Option<VectorBootstrap>,
    counters: Counters,
    user_supported: bool,
    pmp: Option<Box<Pmp>>,
    privilege: Privilege,
    supervisor: Option<Supervisor>,
    triggers: Option<Box<Triggers>>,
    interrupt_enable: u32,
    pending: u32,
    pub ibex: bool,
    pub vex: bool,
    pub minerva: bool,
    pub local_irq_mask: Option<u32>,
    local_irq_pending: u32,
    pub vex_supervisor_irq_mask: u32,
    vex_supervisor_irq_pending: u32,
    local_irq_levels: u64,
    vex_dcache_info: u32,
    vector: Option<u32>,
    interrupt_mode: Option<InterruptMode>,
    clic: Option<[ClicState; 2]>,
    clic_iselect: u32,
    scratch: u32,
    exception_pc: u32,
    cause: u32,
    trap_value: u32,
}

impl Default for Csr {
    fn default() -> Self {
        Self {
            hart_id: 0,
            m_extension: true,
            zmmul: false,
            a_extension: false,
            c_extension: false,
            compressed_single: true,
            compressed_double: true,
            embedded_base: false,
            sscofpmf: false,
            andes: None,
            status: MPP,
            f_extension: false,
            d_extension: false,
            fcsr: 0,
            vector_bootstrap: None,
            counters: Counters::default(),
            user_supported: false,
            pmp: None,
            privilege: Privilege::Machine,
            supervisor: None,
            triggers: None,
            interrupt_enable: 0,
            pending: 0,
            ibex: false,
            vex: false,
            minerva: false,
            local_irq_mask: None,
            local_irq_pending: 0,
            vex_supervisor_irq_mask: 0,
            vex_supervisor_irq_pending: 0,
            local_irq_levels: 0,
            vex_dcache_info: 0,
            vector: None,
            interrupt_mode: None,
            clic: None,
            clic_iselect: 0,
            scratch: 0,
            exception_pc: 0,
            cause: 0,
            trap_value: 0,
        }
    }
}

impl Csr {
    pub fn tick_counters(&mut self, timer: u64) -> bool {
        let counters = &mut self.counters;
        if counters.inhibit & CYCLE_ENABLE == 0 {
            counters.cycle = counters.cycle.wrapping_add(1);
        }
        counters.time = timer;
        counters.instret_written = false;
        counters.inhibit & INSTRET_ENABLE == 0
    }

    pub fn retire(&mut self, counting: bool) {
        let counters = &mut self.counters;
        if counting && !counters.instret_written {
            counters.instret = counters.instret.wrapping_add(1);
        }
    }

    fn counter_accessible(&self, address: u32) -> bool {
        if self.privilege == Privilege::Machine {
            return true;
        }
        let bit = 1 << (address & 31);
        self.counters.machine_enable & bit != 0
            && (self.privilege != Privilege::User
                || self.supervisor.is_none()
                || self.counters.supervisor_enable & bit != 0)
    }

    pub fn enable_user_mode(&mut self) {
        self.user_supported = true;
    }

    pub fn enable_supervisor_mode(&mut self) {
        self.supervisor = Some(Supervisor::default());
    }

    pub fn enable_address_triggers(&mut self) {
        self.triggers = Some(Box::default());
    }

    pub fn address_trigger_matches(&self, address: u32, access: u8) -> bool {
        let mode = 1 << (3 + self.privilege as u32);
        self.triggers.as_ref().is_some_and(|triggers| {
            triggers.entries.iter().any(|trigger| {
                trigger.address == address
                    && trigger.control & mode != 0
                    && trigger.control & u32::from(access) != 0
            })
        })
    }

    pub fn enable_smepmp(&mut self) {
        self.pmp
            .as_mut()
            .expect("Smepmp requires PMP")
            .enable_smepmp();
    }

    pub fn enable_pmp(&mut self, entries: usize) {
        self.pmp = Some(Box::new(Pmp::new(entries)));
    }

    pub fn attach_external_pmp(&mut self, path: String, entries: usize) {
        let mut pmp = Pmp::new(entries);
        pmp.external_path = Some(path);
        self.pmp = Some(Box::new(pmp));
    }

    pub fn external_pmp(&mut self, path: &str) -> Option<&mut Pmp> {
        self.pmp
            .as_deref_mut()
            .filter(|pmp| pmp.external_path.as_deref() == Some(path))
    }

    fn effective_privilege(&self, fetch: bool) -> Privilege {
        if self.privilege == Privilege::Machine && !fetch && self.status & MPRV != 0 {
            self.decode_privilege((self.status & MPP) >> 11)
        } else {
            self.privilege
        }
    }

    pub fn permits(&self, address: u32, width: usize, permissions: u8, fetch: bool) -> bool {
        let machine = self.effective_privilege(fetch) == Privilege::Machine;
        self.pmp
            .as_ref()
            .is_none_or(|pmp| pmp.permits(address, width, permissions, machine))
    }

    pub fn permits_page_table(&self, address: u32, permissions: u8) -> bool {
        self.pmp
            .as_ref()
            .is_none_or(|pmp| pmp.permits(address, 4, permissions, false))
    }

    pub fn translation(&self, fetch: bool) -> Option<(u32, bool, bool, bool)> {
        let privilege = self.effective_privilege(fetch);
        let satp = self.supervisor.as_ref()?.satp;
        (privilege != Privilege::Machine && satp >> 31 != 0).then_some((
            satp,
            privilege == Privilege::User,
            self.status & SUM != 0,
            self.status & MXR != 0,
        ))
    }

    pub fn can_sfence(&self) -> bool {
        self.supervisor.is_some()
            && match self.privilege {
                Privilege::Machine => true,
                Privilege::Supervisor => self.status & TVM == 0,
                Privilege::User => false,
            }
    }

    pub(crate) fn monitor_privilege(&mut self, value: Option<u64>) -> Result<Option<u64>, String> {
        let Some(value) = value else { return Ok(Some(self.privilege as u64)); };
        self.privilege = match value {
            0 if self.user_supported => Privilege::User,
            1 if self.supervisor.is_some() => Privilege::Supervisor,
            3 => Privilege::Machine,
            _ => return Err(format!("unsupported RISC-V privilege mode {value}")),
        };
        Ok(None)
    }

    pub fn in_user_mode(&self) -> bool {
        self.privilege == Privilege::User
    }

    pub fn ecall_cause(&self) -> u32 {
        8 + self.privilege as u32
    }

    pub fn floating_enabled(&self) -> bool {
        self.f_extension && self.status & FS != 0
    }

    pub fn floating_dirty(&mut self) {
        self.status |= FS;
    }

    pub fn enable_vector_bootstrap(&mut self) {
        self.vector_bootstrap = Some(VectorBootstrap::default());
    }

    pub fn vector_configured(&self) -> bool {
        self.vector_bootstrap.is_some()
    }

    pub fn configure_vector(&mut self, insn: u32, avl: u32, kind: u32) -> Option<u32> {
        if self.status & VS == 0 {
            return None;
        }
        let length = self.vector_bootstrap.as_mut()?.configure(insn, avl, kind)?;
        self.status |= VS;
        Some(length)
    }

    fn extension_status_mask(&self) -> u32 {
        (if self.f_extension { FS } else { 0 })
            | (if self.vector_bootstrap.is_some() {
                VS
            } else {
                0
            })
    }

    fn status_summary(&self) -> u32 {
        if self.status & FS == FS || self.status & VS == VS {
            SD
        } else {
            0
        }
    }

    pub(crate) fn enable_clic(&mut self) {
        self.clic = Some([ClicState::default(); 2]);
    }

    pub(crate) fn clic_return_uses_vector_entry(&self, supervisor: bool) -> bool {
        self.clic_mode() && self.clic_cause(supervisor) & 0x4000_0000 != 0
    }

    fn clic_cause(&self, supervisor: bool) -> u32 {
        if supervisor { self.supervisor.as_ref().map_or(0, |s| s.cause) } else { self.cause }
    }

    pub(crate) fn clic_mode(&self) -> bool {
        self.clic.is_some() && self.vector.is_some_and(|vector| vector & 3 == 3)
    }

    pub(crate) fn clic_interrupt(
        &mut self, pc: u32, interrupt: crate::clic::Interrupt,
    ) -> Option<(u32, bool)> {
        let supervisor = interrupt.supervisor;
        let (mode, enabled) = if supervisor {
            self.supervisor.as_ref()?;
            (Privilege::Supervisor, SIE)
        } else {
            (Privilege::Machine, MIE)
        };
        let index = usize::from(supervisor);
        let state = self.clic.as_ref()?[index];
        if !self.clic_mode() || self.privilege as u32 > mode as u32
            || (self.privilege == mode && (self.status & enabled == 0
                || interrupt.level <= state.threshold.max(state.level)))
        {
            return None;
        }
        let entry = state.table.wrapping_add(interrupt.id * 4);
        let cause = INTERRUPT | interrupt.id;
        let target = if supervisor {
            self.supervisor_trap(pc, cause, None)?
        } else {
            self.trap(pc, cause, self.trap_value)?
        };
        self.clic.as_mut().unwrap()[index].level = interrupt.level;
        Some(if interrupt.vectored { (entry, true) } else { (target, false) })
    }

    pub(crate) fn clic_next_interrupt(
        &mut self, instruction: u32, register_value: u32, controller: &mut crate::clic::Clic,
    ) -> Option<u32> {
        let supervisor = instruction >> 20 == 0x145;
        let mode = if supervisor { Privilege::Supervisor } else { Privilege::Machine };
        let enabled = if supervisor { SIE } else { MIE };
        if !self.clic_mode() || (self.privilege as u32) < mode as u32
            || (supervisor && self.supervisor.is_none())
        {
            return None;
        }
        let operation = (instruction >> 12) & 7;
        let source = (instruction >> 15) & 31;
        let operand = if operation & 4 != 0 { source } else { register_value } & 0x1f;
        match operation & 3 {
            2 => self.status |= operand & enabled,
            3 => self.status &= !(operand & enabled),
            _ => return None,
        }
        let previous_level = (self.clic_cause(supervisor) >> 16) as u8;
        let state = &mut self.clic.as_mut().unwrap()[usize::from(supervisor)];
        let Some(interrupt) = controller.interrupt().filter(|irq| {
            irq.supervisor == supervisor && !irq.vectored
                && irq.level > previous_level.max(state.threshold)
        }) else {
            return Some(0);
        };
        if operand != 0 {
            state.level = interrupt.level;
            let cause = if supervisor { &mut self.supervisor.as_mut()?.cause } else { &mut self.cause };
            *cause = (*cause & !0xfff) | INTERRUPT | interrupt.id;
            controller.clear_edge(interrupt.id);
        }
        Some(state.table.wrapping_add(interrupt.id * 4))
    }

    pub(crate) fn clic_indirect_access(
        &self, instruction: u32, register_value: u32, controller: &mut crate::clic::Clic,
    ) -> Option<u32> {
        if self.privilege != Privilege::Machine {
            return None;
        }
        let register = (instruction >> 20) - 0x351;
        let operation = (instruction >> 12) & 7;
        let source = (instruction >> 15) & 31;
        let operand = if operation & 4 != 0 { source } else { register_value };
        let previous = controller.read_indirect(self.clic_iselect, register);
        let value = match operation & 3 {
            1 => operand,
            2 | 3 if source == 0 => return Some(previous),
            2 => previous | operand,
            3 => previous & !operand,
            _ => return None,
        };
        controller.write_indirect(self.clic_iselect, register, value);
        Some(previous)
    }

    pub fn access(&mut self, instruction: u32, register_value: u32) -> Result<u32, String> {
        let address = instruction >> 20;
        let operation = (instruction >> 12) & 7;
        let source = (instruction >> 15) & 31;
        let operand = if operation & 4 != 0 {
            source
        } else {
            register_value
        };
        let error = || format!("unsupported CSR access at {address:#05x}");
        if ((address >> 8) & 3) > self.privilege as u32
            || (address == 0x180
                && self.privilege == Privilege::Supervisor
                && self.status & TVM != 0)
        {
            return Err(error());
        }
        if operation & 3 == 1
            && (instruction >> 7) & 31 == 0
            && self
                .pmp
                .as_ref()
                .is_some_and(|pmp| pmp.is_external_csr(address))
        {
            return self.write(address, operand).then_some(0).ok_or_else(error);
        }
        if operation & 3 == 1
            && (instruction >> 7) & 31 == 0
            && self
                .andes
                .as_mut()
                .is_some_and(|andes| andes.write(address, operand))
        {
            return Ok(0);
        }
        if self.clic_mode() && operation & 3 == 1 && matches!(address, 0x148 | 0x149 | 0x348 | 0x349) {
            let supervisor = address & 0x200 == 0;
            let swap = if address & 1 != 0 {
                ((self.clic_cause(supervisor) >> 16) as u8 == 0)
                    != (self.clic.as_ref().unwrap()[usize::from(supervisor)].level == 0)
            } else if supervisor {
                self.privilege as u32 != u32::from(self.status & SPP != 0)
            } else {
                self.privilege != self.decode_privilege((self.status & MPP) >> 11)
            };
            let scratch = if supervisor { &mut self.supervisor.as_mut().ok_or_else(error)?.scratch }
                else { &mut self.scratch };
            return Ok(if swap { std::mem::replace(scratch, operand) } else { operand });
        }
        let previous = self.read(address).ok_or_else(error)?;
        // A hardware SEIP level is visible to CSR reads but must not become
        // latched as a software-injected interrupt by CSRRS/CSRRC.
        let writable_previous = if matches!(address, 0x144 | 0x344) {
            self.supervisor
                .as_ref()
                .map_or(0, |s| s.injected_interrupts)
        } else {
            previous
        };
        let value = match operation & 3 {
            1 => operand,
            2 | 3 if source == 0 => return Ok(previous),
            2 => writable_previous | operand,
            3 => writable_previous & !operand,
            _ => return Err(error()),
        };
        self.write(address, value)
            .then_some(previous)
            .ok_or_else(error)
    }

    pub(crate) fn set_monitor_trap_value(&mut self, value: u32) {
        self.write(0x343, value);
    }

    pub(crate) fn monitor_trap_register(&self, name: &str) -> Option<u32> {
        self.read(match name {
            "mepc" => 0x341,
            "mstatus" => 0x300,
            "mcause" => 0x342,
            "mtval" => 0x343,
            _ => return None,
        })
    }

    fn read(&self, address: u32) -> Option<u32> {
        Some(match address {
            0x008..=0x00a | 0x00f | 0xc20..=0xc22 if self.status & VS != 0 => {
                return self.vector_bootstrap.as_ref()?.read(address);
            }
            0x001 if self.floating_enabled() => u32::from(self.fcsr & 31),
            0x002 if self.floating_enabled() => u32::from(self.fcsr >> 5),
            0x003 if self.floating_enabled() => u32::from(self.fcsr),
            0x100 if self.supervisor.is_some() => {
                self.status & (SSTATUS | self.extension_status_mask()) | self.status_summary()
            }
            0x104 | 0x144 | 0x303 if self.clic_mode() => 0,
            0x104 => self.interrupt_enable & self.supervisor.as_ref()?.interrupt_delegation,
            0x144 => self.pending_interrupts() & self.supervisor.as_ref()?.interrupt_delegation,
            0x303 => self.supervisor.as_ref()?.interrupt_delegation,
            0x106 if self.supervisor.is_some() => self.counters.supervisor_enable,
            0x105 => self.supervisor.as_ref()?.vector | if self.clic_mode() { 3 } else { 0 },
            0x107 => self.clic.as_ref()?[1].table,
            0x147 => u32::from(self.clic.as_ref()?[1].threshold),
            0xdb1 => u32::from(self.clic.as_ref()?[1].level) << 8,
            0x142 if self.clic_mode() => (self.supervisor.as_ref()?.cause & !CLIC_CAUSE_ALIAS)
                | ((self.status & SPP) << 20) | ((self.status & SPIE) << 22),
            0x140 => self.supervisor.as_ref()?.scratch,
            0x141 => self.supervisor.as_ref()?.epc,
            0x142 => self.supervisor.as_ref()?.cause,
            0x143 => self.supervisor.as_ref()?.value,
            0x180 => self.supervisor.as_ref()?.satp,
            0x302 => self.supervisor.as_ref()?.delegation,
            0x300 => self.status | self.status_summary(),
            0x301 => {
                (1 << 30)
                    | (u32::from(self.m_extension) << 12)
                    | (if self.embedded_base { 1 << 4 } else { 1 << 8 })
                    | u32::from(self.a_extension)
                    | (u32::from(
                        self.c_extension
                            && (!self.f_extension || self.compressed_single)
                            && (!self.d_extension || self.compressed_double),
                    ) << 2)
                    | (u32::from(self.f_extension) << 5)
                    | (u32::from(self.d_extension) << 3)
                    | (u32::from(self.user_supported) << 20)
                    | (u32::from(self.supervisor.is_some()) << 18)
            }
            0x304 if self.clic_mode() => 0,
            0x344 if self.clic_mode() => 0,
            0x350 if self.clic.is_some() => self.clic_iselect,
            0x307 => self.clic.as_ref()?[0].table,
            0x347 => u32::from(self.clic.as_ref()?[0].threshold),
            0xfb1 => (u32::from(self.clic.as_ref()?[0].level) << 24)
                | (u32::from(self.clic.as_ref()?[1].level) << 8),
            0x342 if self.clic_mode() => (self.cause & !CLIC_CAUSE_ALIAS)
                | ((self.status & MPP) << 17) | ((self.status & MPIE) << 20),
            0x304 => self.interrupt_enable,
            0x305 => self.vector.unwrap_or(0),
            0x306 => self.counters.machine_enable,
            0x320 => self.counters.inhibit,
            // Fixed Renode models these HPM event selectors as hardwired zero.
            0x330 if self.minerva => self.local_irq_mask?,
            0x323..=0x33f => 0,
            0x723..=0x73f | 0xda0 if self.sscofpmf => 0,
            0xb00 | 0xb80 => counter_half(self.counters.cycle, address & 0x80 != 0),
            0xb02 | 0xb82 => counter_half(self.counters.instret, address & 0x80 != 0),
            0xc00..=0xc02 | 0xc80..=0xc82 if self.counter_accessible(address) => {
                let value = match address & 3 {
                    0 => self.counters.cycle,
                    1 => self.counters.time,
                    _ => self.counters.instret,
                };
                counter_half(value, address & 0x80 != 0)
            }
            // Renode exposes unimplemented HPM counters as zero.
            0xb03..=0xb1f | 0xb83..=0xb9f => 0,
            0xc03..=0xc1f | 0xc83..=0xc9f if self.counter_accessible(address) => 0,
            0x310 | 0xf11..=0xf13 | 0xf15 => 0,
            0xf14 => self.hart_id as u32,
            0x340 => self.scratch,
            0x341 => self.exception_pc,
            0x342 => self.cause,
            0x343 => self.trap_value,
            0x344 => self.pending_interrupts(),
            0x7a0 => self.triggers.as_ref()?.selected as u32,
            0x7a1 => {
                let triggers = self.triggers.as_ref()?;
                MCONTROL | triggers.entries[triggers.selected].control
            }
            0x7a2 => {
                let triggers = self.triggers.as_ref()?;
                triggers.entries[triggers.selected].address
            }
            0x7a4 if self.triggers.is_some() => 1 << 2,
            0x7c0 if self.ibex => 0,
            0x9c0 if self.vex && self.local_irq_mask.is_some() => self.vex_supervisor_irq_mask,
            0xbc0 if self.vex => self.local_irq_mask?,
            0x360 if self.minerva => self.local_irq_pending,
            0xdc0 if self.vex && self.local_irq_mask.is_some() => self.vex_supervisor_irq_pending,
            0xcc0 if self.vex && self.local_irq_mask.is_some() => self.vex_dcache_info,
            0xfc0 if self.vex && self.local_irq_mask.is_some() => self.local_irq_pending,
            _ => {
                return self
                    .andes
                    .as_ref()
                    .and_then(|andes| andes.read(address))
                    .or_else(|| self.pmp.as_ref()?.read(address));
            }
        })
    }

    fn write(&mut self, address: u32, value: u32) -> bool {
        match address {
            0x008..=0x00a | 0x00f
                if self.status & VS != 0
                    && self
                        .vector_bootstrap
                        .as_mut()
                        .is_some_and(|state| state.write(address, value)) =>
            {
                self.status |= VS;
            }
            0x001..=0x003 if self.floating_enabled() => {
                let value = value as u8;
                self.fcsr = match address {
                    1 => (self.fcsr & !31) | (value & 31),
                    2 => (self.fcsr & 31) | (value << 5),
                    _ => value,
                };
                self.floating_dirty();
            }
            0x100 if self.supervisor.is_some() => {
                let mask = SSTATUS | self.extension_status_mask();
                self.status = (self.status & !mask) | (value & mask);
            }
            0x104 | 0x144 | 0x303 if self.clic_mode() => {},
            0x104 if let Some(supervisor) = self.supervisor.as_ref() => {
                let mask = supervisor.interrupt_delegation;
                self.interrupt_enable = (self.interrupt_enable & !mask) | (value & mask);
            }
            0x144 if let Some(supervisor) = self.supervisor.as_mut() => {
                // The legacy LiteX machine-mode SBI forwards MTIP by writing STIP
                // through SIP on VexRiscv's built-in interrupt controller.
                let writable = if self.vex
                    && self.local_irq_mask.is_some()
                    && self.privilege == Privilege::Machine
                {
                    SSIP | STIP
                } else {
                    SSIP
                };
                let mask = supervisor.interrupt_delegation & writable;
                supervisor.injected_interrupts =
                    (supervisor.injected_interrupts & !mask) | (value & mask);
            }
            0x303 if let Some(supervisor) = self.supervisor.as_mut() => {
                supervisor.interrupt_delegation = value & SUPERVISOR_INTERRUPTS;
            }
            0x107 if let Some(clic) = self.clic.as_mut() => clic[1].table = value & !63,
            0x147 if let Some(clic) = self.clic.as_mut() => clic[1].threshold = value as u8,
            0x142 if self.clic_mode() && self.supervisor.is_some() => {
                self.supervisor.as_mut().unwrap().cause = value;
                self.status = (self.status & !(SPP | SPIE))
                    | ((value >> 20) & SPP) | ((value >> 22) & SPIE);
            }
            0x105 if self.supervisor.is_some() => {
                let vector = self.correct_vector("STVEC", value, true);
                self.supervisor.as_mut().unwrap().vector = vector;
            }
            0x140..=0x143 | 0x180 | 0x302 if self.supervisor.is_some() => {
                let alignment = self.epc_mask();
                let supervisor = self.supervisor.as_mut().unwrap();
                match address {
                    0x140 => supervisor.scratch = value,
                    0x141 => supervisor.epc = value & alignment,
                    0x142 => supervisor.cause = value,
                    0x143 => supervisor.value = value,
                    0x180 => supervisor.satp = value,
                    0x302 => supervisor.delegation = value & 0xb1ff,
                    _ => unreachable!(),
                }
            }
            0x300 => {
                let privilege = self.decode_privilege((value & MPP) >> 11);
                let mprv = if self.user_supported { value & MPRV } else { 0 };
                let supervisor = if self.supervisor.is_some() {
                    value & (SSTATUS | TVM | TSR)
                } else {
                    0
                };
                self.status = (value & (MIE | MPIE))
                    | ((privilege as u32) << 11)
                    | mprv
                    | supervisor
                    | (value & self.extension_status_mask());
            }
            0x304 | 0x344 if self.clic_mode() => {},
            0x350 if self.clic.is_some() => self.clic_iselect = value,
            0x307 if let Some(clic) = self.clic.as_mut() => clic[0].table = value & !63,
            0x347 if let Some(clic) = self.clic.as_mut() => clic[0].threshold = value as u8,
            0x304 => {
                let supervisor_mask = if self.supervisor.is_some() {
                    SUPERVISOR_INTERRUPTS
                } else {
                    0
                };
                self.interrupt_enable = value & (MACHINE_INTERRUPTS | supervisor_mask);
            }
            0x305 => self.vector = Some(self.correct_vector("MTVEC", value, true)),
            0x306 => self.counters.machine_enable = value,
            0x106 if self.supervisor.is_some() => {
                self.counters.supervisor_enable = value;
            }
            0x330 if self.minerva => self.local_irq_mask = Some(value),
            0x323..=0x33f => {}
            0x723..=0x73f if self.sscofpmf => {}
            0x320 => self.counters.inhibit = value & (CYCLE_ENABLE | INSTRET_ENABLE),
            0xb00 | 0xb80 => set_counter_half(&mut self.counters.cycle, value, address & 0x80 != 0),
            0xb02 | 0xb82 => {
                set_counter_half(&mut self.counters.instret, value, address & 0x80 != 0);
                self.counters.instret_written = true;
            }
            0x7a0..=0x7a2 if let Some(triggers) = self.triggers.as_mut() => {
                match address {
                    0x7a0 if (value as usize) < triggers.entries.len() => {
                        triggers.selected = value as usize;
                    }
                    0x7a0 => {}
                    0x7a1 => {
                        // Only exact-address matches with the breakpoint action are implemented.
                        triggers.entries[triggers.selected].control =
                            if value & !MCONTROL_FIELDS == MCONTROL {
                                value & MCONTROL_FIELDS
                            } else {
                                0
                            };
                    }
                    0x7a2 => triggers.entries[triggers.selected].address = value,
                    _ => unreachable!(),
                }
            }
            0x7c0 if self.ibex => {}
            0x360 if self.minerva => self.local_irq_pending = value,
            0x9c0 if self.vex && self.local_irq_mask.is_some() => {
                self.vex_supervisor_irq_mask = value
            }
            0xbc0 if self.vex && self.local_irq_mask.is_some() => self.local_irq_mask = Some(value),
            0xdc0 if self.vex && self.local_irq_mask.is_some() => {
                self.vex_supervisor_irq_pending = value
            }
            0xfc0 if self.vex && self.local_irq_mask.is_some() => self.local_irq_pending = value,
            0xcc0 if self.vex && self.local_irq_mask.is_some() => self.vex_dcache_info = value,
            0x340 => self.scratch = value,
            0x341 => self.exception_pc = value & self.epc_mask(),
            0x342 => {
                self.cause = value;
                if self.clic_mode() {
                    let mpp = (value >> 28) & 3;
                    let allowed = mpp == 3 || (mpp == 0 && self.user_supported)
                        || (mpp == 1 && self.supervisor.is_some());
                    let mpp = if allowed { mpp << 11 } else { self.status & MPP };
                    self.status = (self.status & !(MPP | MPIE)) | mpp | ((value >> 20) & MPIE);
                }
            }
            0x343 => self.trap_value = value,
            0x344 if let Some(supervisor) = self.supervisor.as_mut() => {
                supervisor.injected_interrupts = value & SUPERVISOR_INTERRUPTS;
            }
            0x301 | 0x310 | 0x344 => {} // Fixed WARL fields.
            _ => {
                return self
                    .andes
                    .as_mut()
                    .is_some_and(|andes| andes.write(address, value))
                    || self
                        .pmp
                        .as_mut()
                        .is_some_and(|pmp| pmp.write(address, value));
            }
        }
        true
    }

    fn epc_mask(&self) -> u32 {
        if self.c_extension { !1 } else { !3 }
    }

    fn decode_privilege(&self, value: u32) -> Privilege {
        match value {
            0 if self.user_supported => Privilege::User,
            1 if self.supervisor.is_some() => Privilege::Supervisor,
            _ => Privilege::Machine,
        }
    }

    pub fn set_mstatus(&mut self, value: u32) {
        self.write(0x300, value);
    }

    pub fn configure_interrupt_mode(&mut self, mode: InterruptMode) {
        self.interrupt_mode = Some(mode);
        let machine = mode.correct(DEFAULT_MTVEC);
        let supervisor = mode.correct(0);
        if matches!(mode, InterruptMode::Vectored) {
            eprintln!("Vectored interrupt mode set - updating MTVEC from 0x1010 to {machine:#x}");
            eprintln!("Vectored interrupt mode set - updating STVEC from 0x0 to {supervisor:#x}");
        }
        self.vector = Some(machine);
        if let Some(state) = self.supervisor.as_mut() {
            state.vector = supervisor;
        }
    }

    fn correct_vector(&self, register: &str, value: u32, software: bool) -> u32 {
        if register == "MTVEC" && self.clic.is_some() && value & 3 == 3 {
            return (value & !63) | (value & 3);
        }
        let Some(mode) = self.interrupt_mode else {
            return value & !2;
        };
        let corrected = mode.correct(value);
        if corrected != value {
            if software {
                eprintln!(
                    "{register} value written to CSR corrected from {value:#x} to {corrected:#x}"
                );
            } else {
                eprintln!("modifying {register} from {value:#x} to {corrected:#x}");
            }
        }
        corrected
    }

    pub fn monitor_trap_vector(
        &mut self,
        register: TrapVector,
        value: Option<u32>,
    ) -> Result<Option<u32>, String> {
        if register == TrapVector::Supervisor && self.supervisor.is_none() {
            return Err("STVEC requires supervisor mode".into());
        }
        if let Some(value) = value {
            let corrected = self.correct_vector(register.name(), value, false);
            match register {
                TrapVector::Machine => self.vector = Some(corrected),
                TrapVector::Supervisor => self.supervisor.as_mut().unwrap().vector = corrected,
            }
            return Ok(None);
        }
        Ok(Some(match register {
            TrapVector::Machine => self.vector.unwrap_or(0),
            TrapVector::Supervisor => self.supervisor.as_ref().unwrap().vector,
        }))
    }

    pub fn set_vector(&mut self, value: u32) {
        self.vector = Some(self.correct_vector("MTVEC", value, false));
    }

    pub fn trap(&mut self, pc: u32, cause: u32, value: u32) -> Option<u32> {
        let code = cause & !INTERRUPT;
        let clic = self.clic_mode();
        if self.privilege != Privilege::Machine
            && code < 32
            && (!clic || cause & INTERRUPT == 0)
            && let Some(supervisor) = self.supervisor.as_ref()
            && (if cause & INTERRUPT == 0 {
                supervisor.delegation
            } else {
                supervisor.interrupt_delegation
            }) & (1 << code)
                != 0
        {
            return self.supervisor_trap(pc, cause, (!clic || !matches!(code, 3 | 8..=11)).then_some(value));
        }
        let vector = self.vector?;
        self.exception_pc = pc;
        self.cause = cause;
        if self.clic_mode() {
            self.cause |= u32::from(self.clic.as_ref().unwrap()[0].level) << 16;
        }
        if !clic || cause & INTERRUPT != 0 || !matches!(code, 3 | 8..=11) {
            self.trap_value = value;
        }
        self.status = (self.status & !(MPP | MPIE | MIE))
            | ((self.privilege as u32) << 11)
            | ((self.status & MIE) << 4);
        self.privilege = Privilege::Machine;
        Some(if self.clic_mode() { vector & !63 } else { vector_target(vector, cause) })
    }

    fn supervisor_trap(&mut self, pc: u32, cause: u32, value: Option<u32>) -> Option<u32> {
        let clic = self.clic_mode();
        let level = if clic { u32::from(self.clic.as_ref()?[1].level) << 16 } else { 0 };
        let supervisor = self.supervisor.as_mut()?;
        supervisor.epc = pc;
        supervisor.cause = cause | level;
        if let Some(value) = value {
            supervisor.value = value;
        }
        let spp = if self.privilege == Privilege::Supervisor { SPP } else { 0 };
        self.status = (self.status & !(SPP | SPIE | SIE)) | spp | ((self.status & SIE) << 4);
        self.privilege = Privilege::Supervisor;
        Some(if clic { supervisor.vector & !63 } else { vector_target(supervisor.vector, cause) })
    }

    fn pending_interrupts(&self) -> u32 {
        self.pending
            | self
                .supervisor
                .as_ref()
                .map_or(0, |s| s.injected_interrupts)
    }

    pub fn sample_local_irq(&mut self, levels: u64) -> u32 {
        let Some(mask) = self.local_irq_mask else {
            return levels as u32;
        };
        let changed = levels ^ self.local_irq_levels;
        if changed != 0 {
            self.local_irq_levels = levels;
            self.local_irq_pending =
                (self.local_irq_pending & !(changed as u32)) | (levels as u32 & changed as u32);
            let changed = (changed >> 32) as u32;
            self.vex_supervisor_irq_pending =
                (self.vex_supervisor_irq_pending & !changed) | ((levels >> 32) as u32 & changed);
        }
        (u32::from(self.local_irq_pending & mask != 0) * EXTERNAL)
            | (u32::from(self.vex_supervisor_irq_pending & self.vex_supervisor_irq_mask != 0)
                * SEIP)
    }

    pub fn has_local_irq_to_clear(&self) -> bool {
        self.local_irq_mask.is_some() && self.pending & (EXTERNAL | SEIP) != 0
    }

    pub fn has_injected_interrupts(&self) -> bool {
        self.supervisor
            .as_ref()
            .is_some_and(|s| s.injected_interrupts != 0)
    }

    pub(crate) fn wfi_wakeup_pending(&self, pending: u32) -> bool {
        let injected = self.supervisor.as_ref().map_or(0, |s| s.injected_interrupts);
        (pending | injected) & self.interrupt_enable != 0
    }

    pub fn interrupt(&mut self, pc: u32, pending: u32) -> Option<u32> {
        self.pending = pending;
        let enabled = self.pending_interrupts() & self.interrupt_enable;
        let delegation = self
            .supervisor
            .as_ref()
            .map_or(0, |s| s.interrupt_delegation);
        let machine = if self.privilege != Privilege::Machine || self.status & MIE != 0 {
            enabled & !delegation
        } else {
            0
        };
        let supervisor = if self.privilege == Privilege::User
            || (self.privilege == Privilege::Supervisor && self.status & SIE != 0)
        {
            enabled & delegation
        } else {
            0
        };
        let candidates = if machine != 0 { machine } else { supervisor };
        let bit = [EXTERNAL, SOFTWARE, TIMER, SEIP, SSIP, STIP]
            .into_iter()
            .find(|bit| candidates & bit != 0)?;
        self.trap(pc, INTERRUPT | bit.trailing_zeros(), 0)
    }

    pub fn return_from_trap(&mut self) -> Option<u32> {
        if self.privilege != Privilege::Machine {
            return None;
        }
        if self.clic_mode() {
            self.clic.as_mut().unwrap()[0].level = (self.cause >> 16) as u8;
        }
        self.privilege = self.decode_privilege((self.status & MPP) >> 11);
        let least_privilege = if self.user_supported { 0 } else { MPP };
        self.status = (self.status & !(MPP | MPIE | MIE))
            | ((self.status & MPIE) >> 4)
            | MPIE
            | least_privilege;
        if self.privilege != Privilege::Machine {
            self.status &= !MPRV;
        }
        Some(self.exception_pc)
    }

    pub fn return_from_supervisor_trap(&mut self) -> Option<u32> {
        if self.privilege == Privilege::User
            || (self.privilege == Privilege::Supervisor && self.status & TSR != 0)
        {
            return None;
        }
        let epc = self.supervisor.as_ref()?.epc;
        if self.clic_mode() {
            self.clic.as_mut().unwrap()[1].level = (self.supervisor.as_ref()?.cause >> 16) as u8;
        }
        self.privilege = if self.status & SPP == 0 {
            Privilege::User
        } else {
            Privilege::Supervisor
        };
        self.status =
            (self.status & !(SPP | SPIE | SIE | MPRV)) | ((self.status & SPIE) >> 4) | SPIE;
        Some(epc)
    }
}

fn vector_target(vector: u32, cause: u32) -> u32 {
    let offset = if vector & 1 != 0 && cause & INTERRUPT != 0 {
        (cause & !INTERRUPT) * 4
    } else {
        0
    };
    (vector & !3).wrapping_add(offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hpm_counter_access_uses_each_counter_enable_bit() {
        let mut csr = Csr::default();
        csr.enable_supervisor_mode();
        csr.enable_user_mode();
        let read = |csr: &mut Csr, address: u32| csr.access((address << 20) | 0x20f3, 0);

        assert_eq!(read(&mut csr, 0xb9f).unwrap(), 0);
        assert_eq!(read(&mut csr, 0xc1f).unwrap(), 0);
        csr.access(0x3060_9073, 1 << 31).unwrap();
        csr.privilege = Privilege::Supervisor;
        assert_eq!(read(&mut csr, 0xc9f).unwrap(), 0);
        assert!(read(&mut csr, 0xc83).is_err());
        assert!(read(&mut csr, 0xb9f).is_err());
        csr.privilege = Privilege::User;
        assert!(read(&mut csr, 0xc9f).is_err());
        csr.privilege = Privilege::Supervisor;
        csr.access(0x1060_9073, 1 << 31).unwrap();
        csr.privilege = Privilege::User;
        assert_eq!(read(&mut csr, 0xc9f).unwrap(), 0);
        assert!(csr.access(0xc9f0_9073, 1).is_err());
    }

    #[test]
    fn vector_bootstrap_gates_csrs_and_does_not_advertise_v() {
        let mut csr = Csr::default();
        csr.enable_supervisor_mode();
        assert!(csr.write(0x300, VS));
        assert_eq!(csr.read(0x300).unwrap() & (VS | SD), 0);

        csr.enable_vector_bootstrap();
        assert_eq!(csr.read(0xc22), None);
        assert!(!csr.write(0x00f, 7));
        assert_eq!(csr.read(0x301).unwrap() & (1 << 21), 0);
        assert!(csr.write(0x100, 1 << 9));
        assert_eq!(csr.read(0xc22), Some(128));
        assert_eq!(csr.read(0x100).unwrap() & SD, 0);
        assert!(!csr.write(0xc22, 1));
        assert!(csr.write(0x00f, u32::MAX));
        assert_eq!(csr.read(0x009), Some(1));
        assert_eq!(csr.read(0x00a), Some(3));
        assert_eq!(csr.read(0x300).unwrap() & (VS | SD), VS | SD);
        assert!(csr.write(0x100, 0));
        assert_eq!(csr.read(0x00f), None);
        assert_eq!(csr.configure_vector(0x0000_70d7, 0, 0), None);
    }

    #[test]
    fn floating_csrs_follow_fs_and_share_rounding_and_flags() {
        let mut csr = Csr::default();
        csr.enable_supervisor_mode();
        assert!(csr.write(0x300, MPP | FS));
        assert_eq!(csr.read(0x300).unwrap() & (FS | SD), 0);
        assert!(!csr.write(0x003, 0xff));

        csr.f_extension = true;
        assert_eq!(csr.read(0x301).unwrap() & ((1 << 5) | (1 << 3)), 1 << 5);
        assert!(csr.write(0x100, 1 << 13));
        assert_eq!(csr.read(0x100).unwrap() & SD, 0);
        assert!(csr.write(0x002, 4));
        assert!(csr.write(0x001, 0x25));
        assert_eq!(csr.read(0x003), Some(0x85));
        assert_eq!(csr.read(0x100).unwrap() & (FS | SD), FS | SD);
        assert!(csr.write(0x100, 0));
        assert_eq!(csr.read(0x003), None);
        assert_eq!(csr.read(0x300).unwrap() & (FS | SD), 0);
    }

    #[test]
    fn legacy_vex_machine_sbi_can_forward_a_timer_interrupt_through_sip() {
        for legacy_vex in [false, true] {
            let mut csr = Csr::default();
            csr.enable_supervisor_mode();
            csr.vex = legacy_vex;
            csr.local_irq_mask = legacy_vex.then_some(0);
            csr.access(0x3030_9073, STIP).unwrap();
            csr.access(0x1440_a073, STIP).unwrap(); // csrs sip, ra
            assert_eq!(
                csr.access(0x1440_2073, 0).unwrap(),
                if legacy_vex { STIP } else { 0 }
            );

            csr.access(0x1440_b073, STIP).unwrap(); // csrc sip, ra
            assert_eq!(csr.access(0x1440_2073, 0).unwrap(), 0);
            csr.access(0x3000_9073, 1 << 11).unwrap(); // MPP=S
            csr.return_from_trap().unwrap();
            csr.access(0x1440_a073, STIP).unwrap();
            assert_eq!(csr.access(0x1440_2073, 0).unwrap(), 0);
        }
    }
}
