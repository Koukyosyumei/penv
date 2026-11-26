//! ARM interrupt distributor, private interrupt banks, and CPU arbitration.

const PRIVATE_INTERRUPTS: usize = 32;
pub(crate) const SPURIOUS_INTERRUPT: u16 = 1023;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Version {
    V2,
    V3,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum WindowKind {
    Distributor,
    CpuInterface,
    Redistributor(usize),
}

pub(crate) struct Window {
    pub(crate) base: u32,
    pub(crate) size: u32,
    pub(crate) kind: WindowKind,
}

impl Window {
    pub(crate) fn offset(&self, address: u32) -> Option<u32> {
        let offset = address.wrapping_sub(self.base);
        (offset < self.size).then_some(offset)
    }
}

#[derive(Clone, Copy)]
struct Interrupt {
    group: usize,
    priority: u8,
    targets: u8,
    route: u64,
    enabled: bool,
    pending: bool,
    active: bool,
    asserted: bool,
    edge: bool,
}

impl Default for Interrupt {
    fn default() -> Self {
        Self {
            group: 0,
            priority: 0,
            targets: 1,
            route: 0,
            enabled: false,
            pending: false,
            active: false,
            asserted: false,
            edge: false,
        }
    }
}

impl Interrupt {
    fn set_input(&mut self, asserted: bool) {
        if self.edge && asserted && !self.asserted {
            self.pending = true;
        }
        self.asserted = asserted;
    }

    fn pending(self) -> bool {
        self.pending || self.asserted && !self.edge
    }
}

struct CpuInterface {
    affinity: u32,
    private: [Interrupt; PRIVATE_INTERRUPTS],
    enabled_groups: u8,
    priority_mask: u8,
    binary_point: [u8; 2],
    active: Vec<(u16, u8)>,
    processor_sleep: bool,
}

impl CpuInterface {
    fn new(affinity: u32) -> Self {
        let mut private = [Interrupt::default(); PRIVATE_INTERRUPTS];
        for interrupt in &mut private[..16] {
            interrupt.enabled = true;
            interrupt.edge = true;
        }
        Self {
            affinity,
            private,
            enabled_groups: 0,
            priority_mask: 0,
            binary_point: [0; 2],
            active: Vec::new(),
            processor_sleep: true,
        }
    }
}

pub(crate) struct Gic {
    version: Version,
    distributor: u32,
    shared: Vec<Interrupt>,
    cpus: Vec<CpuInterface>,
}

impl Gic {
    pub(crate) fn new(
        version: Version,
        shared_count: usize,
        affinities: &[u32],
    ) -> Result<Self, String> {
        if !(1..=988).contains(&shared_count) || affinities.is_empty() || affinities.len() > 8 {
            return Err("invalid ARM interrupt controller size".into());
        }
        Ok(Self {
            version,
            distributor: 0,
            shared: vec![Interrupt::default(); shared_count],
            cpus: affinities.iter().copied().map(CpuInterface::new).collect(),
        })
    }

    fn interrupt(&self, cpu: usize, id: usize) -> Option<&Interrupt> {
        if id < PRIVATE_INTERRUPTS {
            self.cpus.get(cpu)?.private.get(id)
        } else {
            self.shared.get(id - PRIVATE_INTERRUPTS)
        }
    }

    fn interrupt_mut(&mut self, cpu: usize, id: usize) -> Option<&mut Interrupt> {
        if id < PRIVATE_INTERRUPTS {
            self.cpus.get_mut(cpu)?.private.get_mut(id)
        } else {
            self.shared.get_mut(id - PRIVATE_INTERRUPTS)
        }
    }

    pub(crate) fn set_private(&mut self, cpu: usize, id: usize, asserted: bool) {
        if id < PRIVATE_INTERRUPTS
            && let Some(interrupt) = self.interrupt_mut(cpu, id)
        {
            interrupt.set_input(asserted);
        }
    }

    pub(crate) fn set_shared(&mut self, input: usize, asserted: bool) {
        if let Some(interrupt) = self.shared.get_mut(input) {
            interrupt.set_input(asserted);
        }
    }

    fn routed(&self, cpu: usize, interrupt: &Interrupt) -> bool {
        if self.version == Version::V3 && self.distributor & 0x30 != 0 {
            !self.cpus[cpu].processor_sleep
                && (interrupt.route & (1 << 31) != 0
                    || (interrupt.route & 0x00ff_ffff) as u32
                        == self.cpus[cpu].affinity & 0x00ff_ffff)
        } else {
            interrupt.targets & (1 << cpu) != 0
        }
    }

    pub(crate) fn highest_pending(&self, cpu: usize, group: Option<usize>) -> u16 {
        let Some(interface) = self.cpus.get(cpu) else {
            return SPURIOUS_INTERRUPT;
        };
        if interface.enabled_groups & self.distributor as u8 == 0 || interface.priority_mask == 0 {
            return SPURIOUS_INTERRUPT;
        }
        let running = interface
            .active
            .last()
            .map_or(0x100, |(_, priority)| u16::from(*priority));
        let eligible = |interrupt: &Interrupt| {
            let group_bit = 1 << interrupt.group;
            let preemption_shift = interface.binary_point[interrupt.group].saturating_add(1);
            interrupt.enabled
                && interrupt.pending()
                && !interrupt.active
                && interface.enabled_groups & group_bit != 0
                && self.distributor & u32::from(group_bit) != 0
                && group.is_none_or(|group| group == interrupt.group)
                && interrupt.priority < interface.priority_mask
                && (u16::from(interrupt.priority) >> preemption_shift)
                    < (running >> preemption_shift)
        };
        interface
            .private
            .iter()
            .enumerate()
            .filter(|(_, interrupt)| eligible(interrupt))
            .chain(
                self.shared
                    .iter()
                    .enumerate()
                    .filter_map(|(index, interrupt)| {
                        (eligible(interrupt) && self.routed(cpu, interrupt))
                            .then_some((index + PRIVATE_INTERRUPTS, interrupt))
                    }),
            )
            .min_by_key(|(id, interrupt)| (interrupt.priority, *id))
            .map_or(SPURIOUS_INTERRUPT, |(id, _)| id as u16)
    }

    pub(crate) fn cpu_inputs(&self, cpu: usize) -> (bool, bool) {
        match self.version {
            Version::V2 => (self.highest_pending(cpu, None) != SPURIOUS_INTERRUPT, false),
            Version::V3 => (
                self.highest_pending(cpu, Some(1)) != SPURIOUS_INTERRUPT,
                self.highest_pending(cpu, Some(0)) != SPURIOUS_INTERRUPT,
            ),
        }
    }

    pub(crate) fn acknowledge(&mut self, cpu: usize, group: Option<usize>) -> u16 {
        let id = self.highest_pending(cpu, group);
        if id != SPURIOUS_INTERRUPT {
            let interrupt = self.interrupt_mut(cpu, usize::from(id)).unwrap();
            interrupt.pending = false;
            interrupt.active = true;
            let priority = interrupt.priority;
            self.cpus[cpu].active.push((id, priority));
        }
        id
    }

    pub(crate) fn end_interrupt(&mut self, cpu: usize, id: u16) {
        let Some(interface) = self.cpus.get_mut(cpu) else {
            return;
        };
        if interface
            .active
            .last()
            .is_some_and(|(active, _)| *active == id)
        {
            interface.active.pop();
            if let Some(interrupt) = self.interrupt_mut(cpu, usize::from(id)) {
                interrupt.active = false;
            }
        }
    }

    pub(crate) fn set_cpu_groups(&mut self, cpu: usize, mask: u8) {
        if let Some(interface) = self.cpus.get_mut(cpu) {
            interface.enabled_groups = mask & 3;
        }
    }

    pub(crate) fn set_priority_mask(&mut self, cpu: usize, mask: u8) {
        if let Some(interface) = self.cpus.get_mut(cpu) {
            interface.priority_mask = mask;
        }
    }

    pub(crate) fn access_aarch32(
        &mut self,
        cpu: usize,
        register: u32,
        value: Option<u32>,
    ) -> Option<Option<u32>> {
        if self.version != Version::V3 {
            return None;
        }
        let interface = self.cpus.get_mut(cpu)?;
        let reply = match (register, value) {
            (0x0004_0006, None) => u32::from(interface.priority_mask), // ICC_PMR
            (0x0004_0006, Some(value)) => {
                return Some({
                    interface.priority_mask = value as u8;
                    None
                });
            }
            (0x000c_0008 | 0x000c_000c, None) => {
                let group = usize::from(register & 4 != 0);
                u32::from(self.acknowledge(cpu, Some(group))) // ICC_IAR0/1
            }
            (0x000c_0028 | 0x000c_002c, Some(value)) => {
                self.end_interrupt(cpu, (value & 0x3ff) as u16); // ICC_EOIR0/1
                return Some(None);
            }
            (0x000c_0048 | 0x000c_004c, None) => {
                let group = usize::from(register & 4 != 0);
                u32::from(self.highest_pending(cpu, Some(group))) // ICC_HPPIR0/1
            }
            (0x000c_0068 | 0x000c_006c, value) => {
                // ICC_BPR0/1
                let group = usize::from(register & 4 != 0);
                if let Some(value) = value {
                    interface.binary_point[group] = value as u8 & 7;
                    return Some(None);
                }
                u32::from(interface.binary_point[group])
            }
            (0x000c_006b, None) => interface
                .active
                .last()
                .map_or(0xff, |(_, priority)| u32::from(*priority)), // ICC_RPR
            (0x000c_00cc | 0x000c_00ec, value) => {
                // ICC_IGRPEN0/1
                let group = u32::from(register & 0x20 != 0);
                if let Some(value) = value {
                    interface.enabled_groups =
                        (interface.enabled_groups & !(1 << group)) | (((value & 1) as u8) << group);
                    return Some(None);
                }
                u32::from(interface.enabled_groups >> group) & 1
            }
            (0x000c_00ac | 0x008c_00a9, value) => return Some(value.is_none().then_some(1)), // ICC_SRE
            _ => return None,
        };
        Some(Some(reply))
    }

    pub(crate) fn read_window(&mut self, cpu: usize, kind: WindowKind, offset: u32) -> u32 {
        match kind {
            WindowKind::Distributor => self.read_distributor(cpu, offset),
            WindowKind::CpuInterface => self.read_cpu_interface(cpu, offset),
            WindowKind::Redistributor(target) => self.read_redistributor(target, offset),
        }
    }

    pub(crate) fn write_window(&mut self, cpu: usize, kind: WindowKind, offset: u32, value: u32) {
        match kind {
            WindowKind::Distributor => self.write_distributor(cpu, offset, value),
            WindowKind::CpuInterface => self.write_cpu_interface(cpu, offset, value),
            WindowKind::Redistributor(target) => self.write_redistributor(target, offset, value),
        }
    }

    fn read_bank(&self, cpu: usize, word: usize, read: impl Fn(Interrupt) -> bool) -> u32 {
        (0..32).fold(0, |result, bit| {
            result
                | (u32::from(
                    self.interrupt(cpu, word * 32 + bit)
                        .is_some_and(|irq| read(*irq)),
                ) << bit)
        })
    }

    fn write_bank(
        &mut self,
        cpu: usize,
        word: usize,
        bits: u32,
        mut write: impl FnMut(&mut Interrupt, bool),
    ) {
        for bit in 0..32 {
            if let Some(interrupt) = self.interrupt_mut(cpu, word * 32 + bit) {
                write(interrupt, bits & (1 << bit) != 0);
            }
        }
    }

    fn read_register(&self, cpu: usize, offset: u32) -> Option<u32> {
        let word = ((offset & 0x7f) / 4) as usize;
        Some(match offset {
            0x080..=0x0fc => self.read_bank(cpu, word, |irq| irq.group != 0),
            0x100..=0x1fc => self.read_bank(cpu, word, |irq| irq.enabled),
            0x200..=0x2fc => self.read_bank(cpu, word, Interrupt::pending),
            0x300..=0x3fc => self.read_bank(cpu, word, |irq| irq.active),
            0x400..=0x7fc | 0x800..=0xbfc => {
                let first = (offset & 0x3ff) as usize;
                (0..4).fold(0, |result, byte| {
                    let value = self.interrupt(cpu, first + byte).map_or(0, |irq| {
                        if offset < 0x800 {
                            irq.priority
                        } else if first + byte < 32 {
                            1 << cpu
                        } else {
                            irq.targets
                        }
                    });
                    result | (u32::from(value) << (8 * byte))
                })
            }
            0xc00..=0xcfc => {
                let first = ((offset - 0xc00) as usize / 4) * 16;
                (0..16).fold(0, |result, field| {
                    let edge = self
                        .interrupt(cpu, first + field)
                        .is_some_and(|irq| irq.edge);
                    result | (u32::from(edge) << (2 * field + 1))
                })
            }
            _ => return None,
        })
    }

    fn write_register(&mut self, cpu: usize, offset: u32, value: u32) -> bool {
        let word = ((offset & 0x7f) / 4) as usize;
        match offset {
            0x080..=0x0fc => {
                self.write_bank(cpu, word, value, |irq, set| irq.group = usize::from(set))
            }
            0x100..=0x17c => self.write_bank(cpu, word, value, |irq, set| irq.enabled |= set),
            0x180..=0x1fc => self.write_bank(cpu, word, value, |irq, set| irq.enabled &= !set),
            0x200..=0x27c => self.write_bank(cpu, word, value, |irq, set| irq.pending |= set),
            0x280..=0x2fc => self.write_bank(cpu, word, value, |irq, set| irq.pending &= !set),
            0x300..=0x37c => self.write_bank(cpu, word, value, |irq, set| irq.active |= set),
            0x380..=0x3fc => self.write_bank(cpu, word, value, |irq, set| irq.active &= !set),
            0x400..=0x7fc | 0x800..=0xbfc => {
                let first = (offset & 0x3ff) as usize;
                for byte in 0..4 {
                    if let Some(irq) = self.interrupt_mut(cpu, first + byte) {
                        let value = (value >> (8 * byte)) as u8;
                        if offset < 0x800 {
                            irq.priority = value;
                        } else if first + byte >= 32 {
                            irq.targets = value;
                        }
                    }
                }
            }
            0xc00..=0xcfc => {
                let first = ((offset - 0xc00) as usize / 4) * 16;
                for field in 0..16 {
                    if first + field >= 16
                        && let Some(irq) = self.interrupt_mut(cpu, first + field)
                    {
                        irq.edge = value & (1 << (2 * field + 1)) != 0;
                    }
                }
            }
            _ => return false,
        }
        true
    }

    pub(crate) fn read_distributor(&self, cpu: usize, offset: u32) -> u32 {
        match offset {
            0 => self.distributor,
            4 => {
                (self.shared.len().div_ceil(32).min(31) as u32)
                    | (self.cpus.len().saturating_sub(1) as u32) << 5
            }
            8 => 0x43b,
            0xfe8 if self.version == Version::V2 => 0x20,
            0xffe8 if self.version == Version::V3 => 0x30,
            0x6100..=0x7fdc if self.version == Version::V3 => {
                let route = (offset - 0x6100) as usize / 8;
                self.shared.get(route).map_or(0, |interrupt| {
                    (interrupt.route >> ((offset & 4) * 8)) as u32
                })
            }
            _ => self.read_register(cpu, offset).unwrap_or(0),
        }
    }

    pub(crate) fn write_distributor(&mut self, cpu: usize, offset: u32, value: u32) {
        match offset {
            0 => self.distributor = value & if self.version == Version::V3 { 0x37 } else { 3 },
            0xf00 if self.version == Version::V2 => {
                let id = (value & 15) as usize;
                let filter = (value >> 24) & 3;
                for (index, interface) in self.cpus.iter_mut().enumerate() {
                    if match filter {
                        0 => (value >> 16) & (1 << index) != 0,
                        1 => index != cpu,
                        2 => index == cpu,
                        _ => false,
                    } {
                        interface.private[id].pending = true;
                    }
                }
            }
            0x6100..=0x7fdc if self.version == Version::V3 => {
                let route = (offset - 0x6100) as usize / 8;
                if let Some(interrupt) = self.shared.get_mut(route) {
                    let shift = (offset & 4) * 8;
                    interrupt.route = (interrupt.route & !(u64::from(u32::MAX) << shift))
                        | (u64::from(value) << shift);
                }
            }
            _ => {
                self.write_register(cpu, offset, value);
            }
        }
    }

    pub(crate) fn read_cpu_interface(&mut self, cpu: usize, offset: u32) -> u32 {
        let Some(interface) = self.cpus.get(cpu) else {
            return 0;
        };
        match offset {
            0 => u32::from(interface.enabled_groups),
            4 => u32::from(interface.priority_mask),
            8 => u32::from(interface.binary_point[0]),
            0x0c => u32::from(self.acknowledge(cpu, None)),
            0x14 => interface
                .active
                .last()
                .map_or(0xff, |(_, priority)| u32::from(*priority)),
            0x18 => u32::from(self.highest_pending(cpu, None)),
            0x1c => u32::from(interface.binary_point[1]),
            0x20 => u32::from(self.acknowledge(cpu, Some(1))),
            0x28 => u32::from(self.highest_pending(cpu, Some(1))),
            _ => 0,
        }
    }

    pub(crate) fn write_cpu_interface(&mut self, cpu: usize, offset: u32, value: u32) {
        match offset {
            0 => self.set_cpu_groups(cpu, value as u8),
            4 => self.set_priority_mask(cpu, value as u8),
            8 | 0x1c if cpu < self.cpus.len() => {
                self.cpus[cpu].binary_point[usize::from(offset != 8)] = value as u8 & 7
            }
            0x10 | 0x24 => self.end_interrupt(cpu, (value & 0x3ff) as u16),
            _ => {}
        }
    }

    pub(crate) fn read_redistributor(&self, cpu: usize, offset: u32) -> u32 {
        let Some(interface) = self.cpus.get(cpu) else {
            return 0;
        };
        match offset {
            4 => 0x43b,
            8 => (cpu as u32) << 8 | (u32::from(cpu + 1 == self.cpus.len()) << 4),
            0x0c => interface.affinity,
            0x14 => {
                if interface.processor_sleep {
                    6
                } else {
                    0
                }
            }
            0xffe8 => 0x30,
            0x1_0080
            | 0x1_0100
            | 0x1_0180
            | 0x1_0200
            | 0x1_0280
            | 0x1_0300
            | 0x1_0380
            | 0x1_0400..=0x1_041c
            | 0x1_0c00
            | 0x1_0c04 => self.read_register(cpu, offset - 0x1_0000).unwrap_or(0),
            _ => 0,
        }
    }

    pub(crate) fn write_redistributor(&mut self, cpu: usize, offset: u32, value: u32) {
        match offset {
            0x14 if cpu < self.cpus.len() => self.cpus[cpu].processor_sleep = value & 2 != 0,
            0x1_0080
            | 0x1_0100
            | 0x1_0180
            | 0x1_0200
            | 0x1_0280
            | 0x1_0300
            | 0x1_0380
            | 0x1_0400..=0x1_041c
            | 0x1_0c00
            | 0x1_0c04 => {
                self.write_register(cpu, offset - 0x1_0000, value);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distributor_routes_spi_to_selected_cpu_and_reasserts_level() {
        let mut gic = Gic::new(Version::V2, 64, &[0, 1]).unwrap();
        gic.write_distributor(0, 0, 3);
        gic.write_distributor(0, 0x104, 1 << 5);
        gic.write_distributor(0, 0x824, 2 << 8);
        gic.write_distributor(0, 0x424, 0x20 << 8);
        for cpu in 0..2 {
            gic.write_cpu_interface(cpu, 0, 3);
            gic.write_cpu_interface(cpu, 4, 0xff);
        }
        gic.set_shared(5, true);
        assert_eq!(gic.read_cpu_interface(0, 0x0c), 1023);
        assert_eq!(gic.read_cpu_interface(1, 0x0c), 37);
        assert_eq!(gic.read_cpu_interface(1, 0x18), 1023);
        gic.write_cpu_interface(1, 0x10, 37);
        assert_eq!(gic.read_cpu_interface(1, 0x18), 37);
        gic.set_shared(5, false);
        assert_eq!(gic.read_cpu_interface(1, 0x18), 1023);
    }

    #[test]
    fn redistributor_private_interrupts_stay_in_their_cpu_bank() {
        let mut gic = Gic::new(Version::V3, 64, &[0, 1]).unwrap();
        gic.write_distributor(0, 0, 3);
        gic.write_redistributor(1, 0x1_0080, 1 << 27);
        gic.write_redistributor(1, 0x1_0100, 1 << 27);
        gic.set_priority_mask(1, 0x80);
        gic.set_cpu_groups(1, 3);
        gic.set_private(1, 27, true);
        assert_eq!(gic.read_redistributor(0, 0x1_0100), 0xffff);
        assert_eq!(gic.acknowledge(1, Some(0)), 1023);
        assert_eq!(gic.acknowledge(1, Some(1)), 27);
        gic.end_interrupt(1, 27);
        assert_eq!(gic.highest_pending(1, Some(1)), 27);
    }
}
