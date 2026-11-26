//! AArch32 integer execution and interworking for ARMv7-A and ARMv7-R.
mod pmsa;
mod snapshot;

use std::io::Write;

use crate::{
    bus::Bus,
    machine::{Step, Stop},
    thumb::Thumb,
};

const N: u32 = 1 << 31;
const Z: u32 = 1 << 30;
const C: u32 = 1 << 29;
const V: u32 = 1 << 28;
const T: u32 = 1 << 5;
const F: u32 = 1 << 6;
const I: u32 = 1 << 7;
const A: u32 = 1 << 8;
const FLAGS: u32 = N | Z | C | V;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Sleep {
    Awake,
    Interrupt,
    Event,
}

#[derive(Clone, Copy)]
pub(crate) enum Profile {
    Application,
    RealTime,
    RealTimeV8,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arm_gic::{Gic, Version, Window, WindowKind};

    #[test]
    fn arm_branch_exchange_does_not_decode_as_status_register_write() {
        let mut cpu = Cpu::new(Profile::RealTimeV8);
        let mut bus = Bus::default();
        bus.map_ram(0, 0x1000).unwrap();
        let mut output = Vec::new();
        bus.write(0, 4, 0x012f_ff1e, &mut output).unwrap(); // BXEQ lr
        let mut registers = [0; 32];
        registers[14] = 0x201;
        let mut pc = 0;
        cpu.step(&mut pc, &mut registers, &mut bus, 0, &mut output)
            .unwrap();
        assert_eq!(pc, 0x200);
        assert_eq!(cpu.cpsr() & (T | 31), T | 0x1a);
    }

    #[test]
    fn arm_system_mode_can_restore_an_exception_frame() {
        let mut cpu = Cpu::new(Profile::RealTimeV8);
        let mut bus = Bus::default();
        bus.map_ram(0, 0x1000).unwrap();
        let mut output = Vec::new();
        bus.write(0, 4, 0xf8bd_0a00, &mut output).unwrap(); // RFEIA sp!
        bus.write(0x100, 4, 0x200, &mut output).unwrap();
        bus.write(0x104, 4, 0x10 | T, &mut output).unwrap();
        let mut registers = [0; 32];
        cpu.set_cpsr(0x1f, &mut registers).unwrap();
        registers[13] = 0x100;
        let mut pc = 0;
        cpu.step(&mut pc, &mut registers, &mut bus, 0, &mut output)
            .unwrap();
        assert_eq!((pc, cpu.cpsr(), registers[13]), (0x200, 0x10 | T, 0x108));
    }

    #[test]
    fn arm_hyp_and_system_modes_share_lr_but_not_sp() {
        let mut cpu = Cpu::new(Profile::RealTimeV8);
        let mut registers = [0; 32];
        registers[13] = 0x100;
        registers[14] = 0x200;
        cpu.set_cpsr(0x1f, &mut registers).unwrap();
        assert_eq!((registers[13], registers[14]), (0, 0x200));
        registers[13] = 0x300;
        registers[14] = 0x400;
        cpu.set_cpsr(0x13, &mut registers).unwrap();
        assert_eq!((registers[13], registers[14]), (0, 0));
        cpu.set_cpsr(0x1a, &mut registers).unwrap();
        assert_eq!((registers[13], registers[14]), (0x100, 0x400));
        cpu.set_cpsr(0x1f, &mut registers).unwrap();
        assert_eq!((registers[13], registers[14]), (0x300, 0x400));
    }

    #[test]
    fn arm_generic_timer_distinguishes_count_offset_compare_and_mask() {
        let mut timer = GenericTimer::new(100_000_000, 100_000_000, [Some(30), Some(27), Some(26)]);
        assert_eq!(timer.read32(0x000e_0000, 0), Some(100_000_000));
        assert!(timer.write64(4, 10));
        assert_eq!(timer.read64(0, 200), Some(20));
        assert_eq!(timer.read64(1, 200), Some(10));
        assert!(timer.write32(0x000e_0003, 200, 5));
        assert!(timer.write32(0x000e_0023, 200, 1));
        assert_eq!(timer.read64(3, 200), Some(15));
        assert_eq!(timer.read32(0x000e_0023, 240), Some(1));
        assert_eq!(timer.read32(0x000e_0023, 250), Some(5));
        assert!(timer.write32(0x000e_0023, 250, 3));
        assert_eq!(timer.read32(0x000e_0023, 250), Some(7));
    }

    #[test]
    fn arm_doubleword_store_uses_word_alignment_in_hyp_mode() {
        let mut cpu = Cpu::new(Profile::RealTimeV8);
        cpu.hsctlr = (1 << 17) | 3;
        let mut bus = Bus::default();
        bus.map_ram(0, 0x1000).unwrap();
        let mut output = Vec::new();
        bus.write(0, 4, 0xe16d_41fc, &mut output).unwrap(); // STRD r4, r5, [sp, #-28]!
        let mut registers = [0; 32];
        registers[4] = 0x1020_3040;
        registers[5] = 0x5060_7080;
        registers[13] = 0x838;
        let mut pc = 0;
        cpu.step(&mut pc, &mut registers, &mut bus, 0, &mut output).unwrap();
        assert_eq!((pc, registers[13]), (4, 0x81c));
        assert_eq!(bus.read::<4>(0x81c).unwrap(), 0x1020_3040);
        assert_eq!(bus.read::<4>(0x820).unwrap(), 0x5060_7080);
        registers[13] = 0x83a;
        pc = 0;
        cpu.step(&mut pc, &mut registers, &mut bus, 0, &mut output).unwrap();
        assert_eq!((pc, registers[13], cpu.fault_address[2]), (0x10, 0x83a, 0x81e));
    }

    #[test]
    fn arm_sleep_fast_forward_leaves_the_timer_boundary_to_the_scheduler() {
        let mut cpu = Cpu::new(Profile::RealTimeV8);
        let mut bus = Bus::default();
        bus.set_step_hz(100_000_000);
        let mut timer = GenericTimer::new(10_000_000, 10_000_000, [Some(30), None, None]);
        timer.channels[0].compare = 3;
        timer.channels[0].control = 1;
        cpu.timer = Some(timer);
        assert_eq!(cpu.quiet_steps(&bus, 0, 0, 100), 0);
        cpu.sleep = Sleep::Interrupt;
        assert_eq!(cpu.quiet_steps(&bus, 0, 0, 100), 20);
        assert_eq!(cpu.quiet_steps(&bus, 0, 299, 100), 0);
        assert_eq!(cpu.quiet_steps(&bus, 0, 300, 100), 0);
        cpu.timer.as_mut().unwrap().channels[0].control = 3;
        assert_eq!(cpu.quiet_steps(&bus, 0, 300, 100), 100);
        cpu.sleep = Sleep::Event;
        bus.arm_events = 1;
        assert_eq!(cpu.quiet_steps(&bus, 0, 300, 100), 0);
    }

    #[test]
    fn arm_gic_irq_uses_vector_base_and_banked_return_address() {
        let mut cpu = Cpu::new(Profile::Application);
        let mut bus = Bus::default();
        bus.map_ram(0, 0x1000).unwrap();
        let mut output = Vec::new();
        let mut registers = [0; 32];
        let mut pc = 0;
        bus.map_arm_gic(
            Gic::new(Version::V2, 32, &[0]).unwrap(),
            vec![
                Window {
                    base: 0x1000,
                    size: 0x1000,
                    kind: WindowKind::Distributor,
                },
                Window {
                    base: 0x2000,
                    size: 0x1000,
                    kind: WindowKind::CpuInterface,
                },
            ],
        )
        .unwrap();
        for (address, value) in [
            (0x0000, 0xee0c_0f10), // MCR p15, 0, r0, c12, c0, 0
            (0x0004, 0xf108_0080), // CPSIE i
            (0x0118, 0xe25e_f004), // SUBS pc, lr, #4
            (0x1000, 1),
            (0x1104, 1 << 5),
            (0x2000, 1),
            (0x2004, 0xff),
        ] {
            bus.write(address, 4, value, &mut output).unwrap();
        }
        registers[0] = 0x100;
        registers[14] = 0x444;
        bus.arm_gic.as_mut().unwrap().set_shared(5, true);
        assert!(matches!(
            cpu.step(&mut pc, &mut registers, &mut bus, 0, &mut output)
                .unwrap(),
            Step::Retired
        ));
        assert!(matches!(
            cpu.step(&mut pc, &mut registers, &mut bus, 0, &mut output)
                .unwrap(),
            Step::Retired
        ));
        assert!(matches!(
            cpu.step(&mut pc, &mut registers, &mut bus, 0, &mut output)
                .unwrap(),
            Step::AsynchronousException
        ));
        assert_eq!((pc, cpu.cpsr() & 31, registers[14]), (0x118, 0x12, 12));
        assert_eq!(bus.read::<4>(0x200c).unwrap(), 37);
        cpu.step(&mut pc, &mut registers, &mut bus, 0, &mut output)
            .unwrap();
        assert_eq!((pc, cpu.cpsr() & 31, registers[14]), (8, 0x13, 0x444));
    }
}

impl Profile {
    pub(crate) fn triple(self) -> &'static str {
        match self {
            Self::Application => "armv7a-none-eabi",
            Self::RealTime => "armv7r-none-eabi",
            Self::RealTimeV8 => "armv8r-none-eabi",
        }
    }
}

#[derive(Clone, Copy, Default)]
struct Bank {
    sp: u32,
    lr: u32,
    spsr: u32,
}

#[derive(Clone, Copy, Default)]
struct TimerChannel {
    compare: u64,
    control: u32,
    interrupt: Option<usize>,
}

impl TimerChannel {
    fn expired(&self, count: u64) -> bool {
        count >= self.compare
    }
}

#[derive(Clone)]
pub(crate) struct GenericTimer {
    frequency: u32,
    counter_frequency_register: u32,
    virtual_offset: u64,
    channels: [TimerChannel; 3],
}

impl GenericTimer {
    pub(crate) fn new(
        frequency: u32,
        counter_frequency_register: u32,
        interrupts: [Option<usize>; 3],
    ) -> Self {
        Self {
            frequency,
            counter_frequency_register,
            virtual_offset: 0,
            channels: interrupts.map(|interrupt| TimerChannel {
                interrupt,
                ..TimerChannel::default()
            }),
        }
    }

    fn physical_count(&self, ns: u128) -> u64 {
        (ns.saturating_mul(u128::from(self.frequency)) / 1_000_000_000) as u64
    }

    fn count(&self, ns: u128, channel: usize) -> u64 {
        let physical = self.physical_count(ns);
        if channel == 1 {
            physical.wrapping_sub(self.virtual_offset)
        } else {
            physical
        }
    }

    fn read32(&self, key: u32, ns: u128) -> Option<u32> {
        if key == 0x000e_0000 {
            return Some(self.counter_frequency_register);
        }
        let (index, control) = Self::decode32(key)?;
        let channel = &self.channels[index];
        let count = self.count(ns, index);
        Some(if control {
            channel.control | (u32::from(channel.control & 1 != 0 && channel.expired(count)) << 2)
        } else {
            channel.compare.wrapping_sub(count) as u32
        })
    }

    fn write32(&mut self, key: u32, ns: u128, value: u32) -> bool {
        if key == 0x000e_0000 {
            self.counter_frequency_register = value;
            return true;
        }
        let Some((index, control)) = Self::decode32(key) else {
            return false;
        };
        let count = self.count(ns, index);
        let channel = &mut self.channels[index];
        if control {
            channel.control = value & 3;
        } else {
            channel.compare = count.wrapping_add_signed(i64::from(value as i32));
        }
        true
    }

    fn decode32(key: u32) -> Option<(usize, bool)> {
        let channel = match key & !0x20 {
            0x000e_0002 => 0,
            0x000e_0003 => 1,
            0x008e_0002 => 2,
            _ => return None,
        };
        Some((channel, key & 0x20 != 0))
    }

    fn read64(&self, operation: u32, ns: u128) -> Option<u64> {
        Some(match operation {
            0 | 1 => self.count(ns, operation as usize),
            2 | 3 => self.channels[(operation - 2) as usize].compare,
            4 => self.virtual_offset,
            6 => self.channels[2].compare,
            _ => return None,
        })
    }

    fn write64(&mut self, operation: u32, value: u64) -> bool {
        match operation {
            2 | 3 => self.channels[(operation - 2) as usize].compare = value,
            4 => self.virtual_offset = value,
            6 => self.channels[2].compare = value,
            _ => return false,
        }
        true
    }
}

#[derive(Clone)]
pub(crate) struct Cpu {
    pub(crate) profile: Profile,
    cpsr: u32,
    thumb: Thumb,
    banks: [Bank; 8],
    shared_high: [u32; 5],
    fiq_high: [u32; 5],
    sctlr: u32,
    vbar: u32,
    hsctlr: u32,
    hactlr: u32,
    hvbar: u32,
    hcr: u32,
    cpacr: u32,
    thread_ids: [u32; 4],
    fpexc: u32,
    vfp_high_words: [u32; 32],
    affinity: u32,
    gic_system_registers: bool,
    elr_hyp: u32,
    pmsa: Option<[pmsa::Pmsa; 2]>,
    interrupt_inputs: [bool; 2],
    sleep: Sleep,
    fault_status: [u32; 3],
    fault_address: [u32; 4],
    debug_status: u32,
    pub(crate) timer: Option<GenericTimer>,
}

impl Cpu {
    pub(crate) fn new(profile: Profile) -> Self {
        let mut thumb = Thumb::new(None);
        thumb.thumb2 = true;
        thumb.arm_interworking = true;
        Self {
            profile,
            cpsr: if matches!(profile, Profile::RealTimeV8) {
                Z | 0x1da
            } else {
                0x1d3
            },
            thumb,
            banks: [Bank::default(); 8],
            shared_high: [0; 5],
            fiq_high: [0; 5],
            sctlr: 0,
            vbar: 0,
            hsctlr: 0,
            hactlr: 0,
            hvbar: 0,
            hcr: if matches!(profile, Profile::RealTimeV8) { 2 } else { 0 },
            cpacr: 0,
            thread_ids: [0; 4],
            fpexc: 0x700,
            vfp_high_words: [0; 32],
            affinity: 0,
            gic_system_registers: false,
            elr_hyp: 0,
            pmsa: matches!(profile, Profile::RealTimeV8)
                .then(|| [pmsa::Pmsa::new(16), pmsa::Pmsa::new(16)]),
            interrupt_inputs: [false; 2],
            sleep: Sleep::Awake,
            fault_status: [0; 3],
            fault_address: [0; 4],
            debug_status: 0,
            timer: None,
        }
    }

    pub(crate) fn sleeping(&self) -> bool {
        self.sleep != Sleep::Awake
    }

    pub(crate) fn quiet_steps(&self, bus: &Bus, slot: usize, now: u128, limit: u64) -> u64 {
        if !self.is_v8r()
            || !self.sleeping()
            || self.sleep == Sleep::Event && bus.arm_events & (1 << slot) != 0
            || self.interrupt_inputs.into_iter().any(|asserted| asserted)
            || bus.arm_gic.as_ref().is_some_and(|gic| gic.cpu_inputs(slot) != (false, false))
        {
            return 0;
        }
        let Some(timer) = &self.timer else { return limit };
        let rate = u128::from(bus.monitor_step_hz());
        timer.channels.iter().enumerate()
            .filter(|(_, channel)| channel.control == 1 && channel.interrupt.is_some())
            .fold(limit, |quiet, (index, channel)| {
                // Counting is discrete. Reserve a full counter tick before the
                // compare point so fractional timer and scheduler phases cannot
                // skip the first instruction at which the IRQ is visible.
                let ticks = channel.compare.saturating_sub(timer.count(now, index)).saturating_sub(1);
                let steps = u128::from(ticks) * rate / u128::from(timer.frequency);
                quiet.min(u64::try_from(steps).unwrap_or(u64::MAX))
            })
    }

    pub(crate) fn cpsr(&self) -> u32 {
        self.cpsr
    }

    pub(crate) fn is_v8r(&self) -> bool {
        matches!(self.profile, Profile::RealTimeV8)
    }

    pub(crate) fn set_gic_system_registers(&mut self, enabled: bool) {
        self.gic_system_registers = enabled;
    }

    pub(crate) fn set_affinity(&mut self, affinity: u32) {
        self.affinity = affinity;
    }

    pub(crate) fn drive_interrupt(&mut self, input: u32, asserted: bool) -> Result<(), String> {
        let level = self
            .interrupt_inputs
            .get_mut(input as usize)
            .ok_or("expected CPU interrupt input 0 (IRQ) or 1 (FIQ)")?;
        *level = asserted;
        Ok(())
    }

    pub(crate) fn image_entry(&mut self, address: u32) -> u32 {
        self.exchange(address)
    }

    pub(crate) fn validate_entry(
        &self,
        bus: &mut Bus,
        slot: usize,
        address: u32,
    ) -> Result<(), String> {
        self.load(bus, slot, address, if self.cpsr & T != 0 { 2 } else { 4 })
            .map(|_| ())
    }

    pub(crate) fn configure_pmsa(
        &mut self,
        regions: usize,
        hypervisor_regions: usize,
    ) -> Result<(), String> {
        if regions > 32 || hypervisor_regions > 32 {
            return Err("Cortex-R52 supports at most 32 region selectors per MPU".into());
        }
        self.pmsa = Some([
            pmsa::Pmsa::new(regions),
            pmsa::Pmsa::new(hypervisor_regions),
        ]);
        Ok(())
    }

    pub(crate) fn monitor_exception_level(&self) -> &'static str {
        match self.cpsr & 31 {
            0x10 => "EL0_UserMode",
            0x1a => "EL2_HypervisorMode",
            0x16 => "EL3_MonitorMode",
            _ => "EL1_SystemMode",
        }
    }

    pub(crate) fn set_monitor_exception_level(
        &mut self,
        level: &str,
        registers: &mut [u32; 32],
    ) -> Result<(), String> {
        let mode = match level {
            "EL0_UserMode" => 0x10,
            "EL1_SystemMode" => 0x1f,
            "EL2_HypervisorMode" => 0x1a,
            _ => return Err(format!("unsupported Cortex-R52 exception level {level:?}")),
        };
        self.set_cpsr((self.cpsr & !31) | mode, registers)
    }

    pub(crate) fn access_tcm_register(
        bus: &mut Bus,
        slot: usize,
        key: u32,
        value: Option<u32>,
    ) -> Option<Option<u32>> {
        let reply = match key {
            0x0000_0040 if value.is_none() => return Some(Some(bus.arm_tcm_type(slot))),
            0x0009_0001 | 0x0009_0021 | 0x0009_0041 => {
                let index = ((key >> 5) & 3) as usize;
                bus.arm_tcm_register(slot, index, value).unwrap_or(0)
            }
            _ => return None,
        };
        Some(value.is_none().then_some(reply))
    }

    pub(crate) fn monitor_system_register(
        &mut self,
        name: &str,
        value: Option<u64>,
    ) -> Result<Option<u64>, String> {
        let canonical = name.to_ascii_lowercase();
        let value = value
            .map(u32::try_from)
            .transpose()
            .map_err(|_| "system register value exceeds 32 bits")?;
        if let Some((bank, register)) = pmsa::Register::named(&canonical)
            && let Some(pmsa) = self.pmsa.as_mut()
        {
            return pmsa[bank]
                .access(register, value)
                .map(|value| value.map(u64::from))
                .ok_or_else(|| format!("invalid access to {name:?}"));
        }
        let key = match canonical.as_str() {
            "dbgdscrext" if self.is_v8r() => {
                if let Some(value) = value {
                    self.debug_status = value;
                    return Ok(None);
                }
                return Ok(Some(u64::from(self.debug_status)));
            }
            "sctlr" => 0x0001_0000,
            "vbar" => 0x000c_0000,
            "hsctlr" => 0x0081_0000,
            "hactlr" => 0x0081_0020,
            "hvbar" => 0x008c_0000,
            "hcr" => 0x0081_0001,
            "cpacr" => 0x0001_0040,
            "dfsr" => 0x0005_0000,
            "ifsr" => 0x0005_0020,
            "hsr" => 0x0085_0002,
            "dfar" => 0x0006_0000,
            "ctr" => 0x0000_0020,
            "clidr" => 0x0020_0020,
            "revidr" => 0x0000_00c0,
            "id_pfr0" => 0x0000_0001,
            "id_pfr1" => 0x0000_0021,
            "id_mmfr0" => 0x0000_0081,
            "mpidr" => 0x0000_00a0,
            "tpidrurw" => 0x000d_0040,
            "tpidruro" => 0x000d_0060,
            "tpidrprw" => 0x000d_0080,
            "htpidr" => 0x008d_0040,
            "ifar" => 0x0006_0040,
            "hdfar" => 0x0086_0000,
            "hifar" => 0x0086_0040,
            "rvbar" if self.is_v8r() => {
                return if value.is_none() {
                    Ok(Some(0))
                } else {
                    Err("RVBAR is read-only".into())
                };
            }
            _ => return Err(format!("unknown AArch32 system register {name:?}")),
        };
        self.access_cp15(key, value)
            .map(|reply| reply.map(u64::from))
            .ok_or_else(|| format!("unknown AArch32 system register {name:?}"))
    }

    fn access_cp15(&mut self, key: u32, value: Option<u32>) -> Option<Option<u32>> {
        if let Some((bank, register)) = pmsa::Register::decode(key)
            && let Some(pmsa) = self.pmsa.as_mut()
        {
            return pmsa[bank].access(register, value);
        }
        let v8r = self.is_v8r();
        if value.is_some()
            && matches!(
                key,
                0x0007_0026
                    | 0x0007_002a
                    | 0x0007_002e
                    | 0x0007_0005
                    | 0x0007_0025
                    | 0x0007_00aa
                    | 0x0007_008a
                    | 0x0007_00ba
            )
        {
            // The interpreter reads and writes coherent backing memory directly.
            return Some(None);
        }
        if v8r {
            let read_only = match key {
                0x0000_0000 => Some(0x411f_d132),
                0x0000_0020 => Some(0x8144_c004),
                0x0020_0020 => Some(0x0900_0003),
                0x0000_00c0 => Some(0),
                0x0000_0001 => Some(0x0000_0131),
                0x0000_0021 => Some(0x0011_1001 | (u32::from(self.gic_system_registers) << 28)),
                0x0000_0081 => Some(0x0021_1040),
                0x0000_00a0 => Some(0x8000_0000 | self.affinity),
                _ => None,
            };
            if let Some(reply) = read_only {
                return value.is_none().then_some(Some(reply));
            }
        }
        let (register, mask) = match key {
            0x0001_0000 => (&mut self.sctlr, u32::MAX),
            0x000c_0000 => (&mut self.vbar, !31),
            0x000d_0040 => (&mut self.thread_ids[0], u32::MAX),
            0x000d_0060 => (&mut self.thread_ids[1], u32::MAX),
            0x000d_0080 => (&mut self.thread_ids[2], u32::MAX),
            0x008d_0040 if v8r => (&mut self.thread_ids[3], u32::MAX),
            0x0001_0040 => (&mut self.cpacr, 0xf0_0000),
            0x0081_0000 if v8r => (&mut self.hsctlr, u32::MAX),
            0x0081_0020 if v8r => (&mut self.hactlr, u32::MAX),
            // Renode retains word-aligned values in its AArch32 HVBAR model.
            0x008c_0000 if v8r => (&mut self.hvbar, !3),
            0x0081_0001 if v8r => (&mut self.hcr, u32::MAX),
            0x0005_0000 if v8r => (&mut self.fault_status[0], u32::MAX),
            0x0005_0020 if v8r => (&mut self.fault_status[1], u32::MAX),
            0x0085_0002 if v8r => (&mut self.fault_status[2], u32::MAX),
            0x0006_0000 if v8r => (&mut self.fault_address[0], u32::MAX),
            0x0006_0040 if v8r => (&mut self.fault_address[1], u32::MAX),
            0x0086_0000 if v8r => (&mut self.fault_address[2], u32::MAX),
            0x0086_0040 if v8r => (&mut self.fault_address[3], u32::MAX),
            _ => return None,
        };
        if let Some(value) = value {
            *register = value & mask;
            Some(None)
        } else {
            Some(Some(*register))
        }
    }

    fn bank_for(status: u32) -> Option<usize> {
        Some(match status & 31 {
            0x10 | 0x1f => 0,
            0x11 => 1,
            0x12 => 2,
            0x13 => 3,
            0x16 => 4,
            0x17 => 5,
            0x1a => 6,
            0x1b => 7,
            _ => return None,
        })
    }

    fn link_register_bank(bank: usize) -> usize {
        if bank == 6 { 0 } else { bank }
    }

    pub(crate) fn set_cpsr(&mut self, value: u32, registers: &mut [u32; 32]) -> Result<(), String> {
        let old = Self::bank_for(self.cpsr).expect("current CPSR has a valid mode");
        let new = Self::bank_for(value).ok_or("invalid AArch32 mode")?;
        if old != new {
            self.banks[old].sp = registers[13];
            self.banks[Self::link_register_bank(old)].lr = registers[14];
            registers[13] = self.banks[new].sp;
            registers[14] = self.banks[Self::link_register_bank(new)].lr;
            if old == 1 {
                self.fiq_high.copy_from_slice(&registers[8..13]);
                registers[8..13].copy_from_slice(&self.shared_high);
            } else if new == 1 {
                self.shared_high.copy_from_slice(&registers[8..13]);
                registers[8..13].copy_from_slice(&self.fiq_high);
            }
        }
        self.cpsr = value;
        Ok(())
    }

    fn undefined(&mut self, return_address: u32, registers: &mut [u32; 32]) -> u32 {
        self.exception(0x1b, 4, return_address, registers)
    }

    fn check_protection(
        &mut self,
        address: u32,
        width: usize,
        access: pmsa::Access,
        pc: u32,
        registers: &mut [u32; 32],
    ) -> Option<u32> {
        let banks = self.pmsa.as_ref()?;
        let hyp_mode = self.cpsr & 31 == 0x1a;
        let user = self.cpsr & 31 == 0x10;
        let stage1 = !hyp_mode && self.sctlr & 1 != 0;
        let stage2 = if hyp_mode {
            self.hsctlr & 1 != 0
        } else {
            self.hcr & 1 != 0
        };
        let check = |bank: usize, user: bool, background: bool, control| {
            if access != pmsa::Access::Fetch
                && control & 2 != 0
                && !address.is_multiple_of(width as u32)
            {
                return Err(pmsa::Fault::Alignment);
            }
            banks[bank].check(address, width as u32, access, user, background, control)
        };
        let fault = if stage1
            && let Err(fault) = check(0, user, !user && self.sctlr & (1 << 17) != 0, self.sctlr)
        {
            Some((false, fault))
        } else if stage2
            && let Err(fault) = check(
                1,
                !hyp_mode,
                hyp_mode && self.hsctlr & (1 << 17) != 0,
                self.hsctlr,
            )
        {
            Some((true, fault))
        } else {
            None
        };
        let (hyp, reason) = fault?;
        Some(self.memory_fault(address, access, pc, hyp, reason, registers))
    }

    fn memory_fault(
        &mut self,
        address: u32,
        access: pmsa::Access,
        pc: u32,
        hyp: bool,
        reason: pmsa::Fault,
        registers: &mut [u32; 32],
    ) -> u32 {
        let hyp_mode = self.is_v8r() && self.cpsr & 31 == 0x1a;
        let instruction = access == pmsa::Access::Fetch;
        let write = access == pmsa::Access::Write;
        let offset = if instruction { 0x0c } else { 0x10 };
        if hyp {
            let ec = if instruction { 0x20 } else { 0x24 } | u32::from(hyp_mode);
            self.fault_status[2] = (ec << 26)
                | (u32::from(self.cpsr & T == 0) << 25)
                | (u32::from(write) << 6)
                | reason as u32;
            self.fault_address[2 + usize::from(instruction)] = address;
            let vector = if hyp_mode { offset } else { 0x14 };
            self.exception(0x1a, vector, pc, registers)
        } else {
            let index = usize::from(instruction);
            self.fault_status[index] = (1 << 9) | (u32::from(write) << 11) | reason as u32;
            self.fault_address[index] = address;
            self.exception(
                0x17,
                offset,
                pc.wrapping_add(if instruction { 4 } else { 8 }),
                registers,
            )
        }
    }

    fn exception(
        &mut self,
        mode: u32,
        offset: u32,
        return_address: u32,
        registers: &mut [u32; 32],
    ) -> u32 {
        let status = self.cpsr;
        let hypervisor = self.is_v8r()
            && (status & 31 == 0x1a
                || mode == 0x1a
                || offset == 0x18 && self.hcr & (1 << 4) != 0
                || offset == 0x1c && self.hcr & (1 << 3) != 0);
        let control = if hypervisor { self.hsctlr } else { self.sctlr };
        let mode = if hypervisor { 0x1a } else { mode };
        let thumb_exceptions = (control >> 25) & T;
        let exception_masks = if hypervisor {
            A | I | F
        } else if offset == 0x1c {
            I | F
        } else {
            I
        };
        let updated = (status & !(31 | T | (0b11 << 25) | (0x3f << 10)))
            | mode
            | exception_masks
            | thumb_exceptions;
        self.set_cpsr(updated, registers)
            .expect("valid exception mode");
        self.banks[Self::bank_for(updated).unwrap()].spsr = status;
        if hypervisor {
            self.elr_hyp = if matches!(offset, 0x18 | 0x1c) {
                return_address.wrapping_sub(4)
            } else {
                return_address
            };
        } else {
            registers[14] = return_address;
        }
        let base = if hypervisor { self.hvbar } else { self.vbar };
        let high_vectors = if self.sctlr & (1 << 13) != 0 {
            0xffff_0000
        } else {
            0
        };
        base.wrapping_add(high_vectors).wrapping_add(offset)
    }

    fn banked_register<'a>(
        &'a mut self,
        instruction: u32,
        registers: &'a mut [u32; 32],
    ) -> Option<&'a mut u32> {
        let current = Self::bank_for(self.cpsr)?;
        if self.cpsr & 31 == 0x10 {
            return None;
        }
        let sysm = (((instruction >> 4) & 16) | ((instruction >> 16) & 15)) as usize;
        if instruction & (1 << 22) != 0 {
            let bank = match sysm {
                14 => 1,
                16 => 2,
                18 => 3,
                20 => 5,
                22 => 7,
                30 if current == 6 => 6,
                _ => return None,
            };
            return Some(&mut self.banks[bank].spsr);
        }
        let (bank, index) = match sysm {
            0..=4 if current == 1 => return Some(&mut self.shared_high[sysm]),
            0..=4 => return Some(&mut registers[8 + sysm]),
            5..=6 => (0, 8 + sysm),
            8..=12 if current != 1 => return Some(&mut self.fiq_high[sysm - 8]),
            8..=14 => (1, sysm),
            16..=23 => ([2, 3, 5, 7][(sysm - 16) / 2], 14 - (sysm & 1)),
            30 if current == 6 => return Some(&mut self.elr_hyp),
            31 if current == 6 => (6, 13),
            _ => return None,
        };
        let current_register_bank = if index == 14 {
            Self::link_register_bank(current)
        } else {
            current
        };
        if bank == current_register_bank {
            Some(&mut registers[index])
        } else if index == 13 {
            Some(&mut self.banks[bank].sp)
        } else {
            Some(&mut self.banks[bank].lr)
        }
    }

    fn user_register<'a>(&'a mut self, registers: &'a mut [u32; 32], index: usize) -> &'a mut u32 {
        let bank = Self::bank_for(self.cpsr).unwrap();
        match index {
            8..=12 if bank == 1 => &mut self.shared_high[index - 8],
            13 if bank != 0 => &mut self.banks[0].sp,
            14 if Self::link_register_bank(bank) != 0 => &mut self.banks[0].lr,
            _ => &mut registers[index],
        }
    }

    fn exception_return(
        &mut self,
        opcode: u32,
        low: u32,
        pc: u32,
        registers: &mut [u32; 32],
        bus: &mut Bus,
        slot: usize,
        output: &mut impl Write,
    ) -> Result<Option<u32>, String> {
        if !matches!(opcode & 0xffc0, 0xe800 | 0xe980) || low & 0xffe0 != 0xc000 {
            return Ok(None);
        }
        let load = opcode & 0x10 != 0;
        if !load && opcode & 15 != 13 || load && low != 0xc000 {
            return Ok(None);
        }
        let increment = opcode & 0x100 != 0;
        let encoded = (u32::from(!increment) << 24)
            | (u32::from(increment) << 23)
            | (u32::from(opcode & 0x20 != 0) << 21)
            | (u32::from(load) << 20)
            | ((opcode & 15) << 16)
            | (low & 31);
        self.arm_exception_frame(encoded, pc.wrapping_add(4), registers, bus, slot, output)
            .map(Some)
    }

    fn arm_exception_frame(
        &mut self,
        instruction: u32,
        next: u32,
        registers: &mut [u32; 32],
        bus: &mut Bus,
        slot: usize,
        output: &mut impl Write,
    ) -> Result<u32, String> {
        let current = Self::bank_for(self.cpsr).unwrap();
        let load = instruction & (1 << 20) != 0;
        if self.cpsr & 31 == 0x10 || current == 6 || !load && current == 0 {
            return Ok(self.undefined(next, registers));
        }
        let before = instruction & (1 << 24) != 0;
        let increment = instruction & (1 << 23) != 0;
        let writeback = instruction & (1 << 21) != 0;
        let (base, bank, rn) = if load {
            let rn = ((instruction >> 16) & 15) as usize;
            if rn == 15 {
                return Ok(self.undefined(next, registers));
            }
            (registers[rn], current, rn)
        } else {
            let Some(bank) = Self::bank_for(instruction) else {
                return Ok(self.undefined(next, registers));
            };
            (
                if bank == current {
                    registers[13]
                } else {
                    self.banks[bank].sp
                },
                bank,
                13,
            )
        };
        let updated = if increment {
            base.wrapping_add(8)
        } else {
            base.wrapping_sub(8)
        };
        let low = if increment { base } else { updated };
        let address = if before == increment {
            low.wrapping_add(4)
        } else {
            low
        };
        let access = if load {
            pmsa::Access::Read
        } else {
            pmsa::Access::Write
        };
        for target in [address, address.wrapping_add(4)] {
            if let Some(vector) =
                self.check_protection(target, 4, access, next.wrapping_sub(4), registers)
            {
                return Ok(vector);
            }
        }
        if load {
            let pc = self.load(bus, slot, address, 4)?;
            let status = self.load(bus, slot, address.wrapping_add(4), 4)?;
            if Self::bank_for(status).is_none() {
                return Ok(self.undefined(next, registers));
            }
            if writeback {
                registers[rn] = updated;
            }
            self.set_cpsr(status, registers)?;
            Ok(pc & if status & T == 0 { !3 } else { !1 })
        } else {
            self.store(bus, slot, address, 4, registers[14], output)?;
            self.store(
                bus,
                slot,
                address.wrapping_add(4),
                4,
                self.banks[current].spsr,
                output,
            )?;
            if writeback {
                if bank == current {
                    registers[13] = updated;
                } else {
                    self.banks[bank].sp = updated;
                }
            }
            Ok(next)
        }
    }

    fn reg(registers: &[u32; 32], index: usize, address: u32) -> u32 {
        if index == 15 {
            address.wrapping_add(8)
        } else {
            registers[index]
        }
    }

    fn exchange(&mut self, address: u32) -> u32 {
        self.cpsr = (self.cpsr & !T) | ((address & 1) << 5);
        address & if self.cpsr & T != 0 { !1 } else { !3 }
    }

    fn condition(&self, condition: u32) -> bool {
        let n = self.cpsr & N != 0;
        let z = self.cpsr & Z != 0;
        let c = self.cpsr & C != 0;
        let v = self.cpsr & V != 0;
        let passes = match condition >> 1 {
            0 => z,
            1 => c,
            2 => n,
            3 => v,
            4 => c && !z,
            5 => n == v,
            6 => n == v && !z,
            _ => true,
        };
        passes ^ (condition & 1 != 0)
    }

    fn shift(&self, value: u32, kind: u32, amount: u32, immediate: bool) -> (u32, bool) {
        let carry = self.cpsr & C != 0;
        if amount == 0 && (!immediate || kind == 0) {
            return (value, carry);
        }
        match (kind, amount) {
            (0, 1..=32) => (
                value.checked_shl(amount).unwrap_or(0),
                value & (1 << (32 - amount)) != 0,
            ),
            (0, _) => (0, false),
            (1, 0 | 32) => (0, value & N != 0),
            (1, 1..=31) => (value >> amount, value & (1 << (amount - 1)) != 0),
            (1, _) => (0, false),
            (2, 0 | 32..) => (((value as i32) >> 31) as u32, value & N != 0),
            (2, amount) => (
                ((value as i32) >> amount) as u32,
                value & (1 << (amount - 1)) != 0,
            ),
            (3, 0) => (value >> 1 | (u32::from(carry) << 31), value & 1 != 0),
            (3, amount) => {
                let shifted = value.rotate_right(amount);
                (shifted, shifted & N != 0)
            }
            _ => unreachable!(),
        }
    }

    fn operand(&self, instruction: u32, registers: &[u32; 32], pc: u32) -> (u32, bool) {
        if instruction & (1 << 25) != 0 {
            let rotation = ((instruction >> 8) & 15) * 2;
            let shifted = (instruction & 255).rotate_right(rotation);
            (
                shifted,
                if rotation == 0 {
                    self.cpsr & C != 0
                } else {
                    shifted & N != 0
                },
            )
        } else {
            let value = Self::reg(registers, (instruction & 15) as usize, pc);
            let immediate = instruction & 0x10 == 0;
            let amount = if immediate {
                (instruction >> 7) & 31
            } else {
                Self::reg(registers, ((instruction >> 8) & 15) as usize, pc) & 255
            };
            self.shift(value, (instruction >> 5) & 3, amount, immediate)
        }
    }

    fn arithmetic(left: u32, right: u32, carry: bool) -> (u32, bool, bool) {
        let sum = u64::from(left) + u64::from(right) + u64::from(carry);
        let signed = i64::from(left as i32) + i64::from(right as i32) + i64::from(carry);
        (sum as u32, sum >> 32 != 0, i32::try_from(signed).is_err())
    }

    fn process(&mut self, instruction: u32, registers: &mut [u32; 32], pc: u32) -> Option<u32> {
        let opcode = (instruction >> 21) & 15;
        let ra = Self::reg(registers, ((instruction >> 16) & 15) as usize, pc);
        let rd = ((instruction >> 12) & 15) as usize;
        let (rb, shift_carry) = self.operand(instruction, registers, pc);
        let carry = self.cpsr & C != 0;
        let old_overflow = self.cpsr & V != 0;
        let logic = |value| (value, shift_carry, old_overflow);
        let (value, carry, overflow) = match opcode {
            0 | 8 => logic(ra & rb),
            1 | 9 => logic(ra ^ rb),
            2 | 10 => Self::arithmetic(ra, !rb, true),
            3 => Self::arithmetic(rb, !ra, true),
            4 | 11 => Self::arithmetic(ra, rb, false),
            5 => Self::arithmetic(ra, rb, carry),
            6 => Self::arithmetic(ra, !rb, carry),
            7 => Self::arithmetic(rb, !ra, carry),
            12 => logic(ra | rb),
            13 => logic(rb),
            14 => logic(ra & !rb),
            _ => logic(!rb),
        };
        let test = matches!(opcode, 8..=11);
        if !test && rd == 15 && instruction & (1 << 20) != 0 {
            let bank = Self::bank_for(self.cpsr).unwrap();
            if bank != 0 && self.set_cpsr(self.banks[bank].spsr, registers).is_ok() {
                return Some(value & if self.cpsr & T != 0 { !1 } else { !3 });
            }
            return Some(self.undefined(pc.wrapping_add(4), registers));
        }
        if test || instruction & (1 << 20) != 0 {
            self.cpsr = (self.cpsr & !FLAGS)
                | (value & N)
                | (u32::from(value == 0) * Z)
                | (u32::from(carry) * C)
                | (u32::from(overflow) * V);
        }
        if test {
            None
        } else if rd == 15 {
            Some(self.exchange(value))
        } else {
            registers[rd] = value;
            None
        }
    }

    fn load(&self, bus: &mut Bus, slot: usize, address: u32, width: usize) -> Result<u32, String> {
        if self.is_v8r() && !bus.arm_tcm_permits(slot, address, self.cpsr & 31 == 0x1a) {
            crate::monitor_log::record(
                2,
                &format!(
                    "sysbus: Tried to read {width} bytes at 0x{address:X}, with incorrect permissions, returning 0."
                ),
            );
            return Ok(0);
        }
        if let Some(bytes) = bus.cpu_ram64(slot, u64::from(address), width) {
            let mut word = [0; 4];
            word[..width].copy_from_slice(bytes);
            return Ok(u32::from_le_bytes(word));
        }
        match width {
            1 => bus.read::<1>(address),
            2 => bus.read::<2>(address),
            _ => bus.read::<4>(address),
        }
        .map_err(|error| error.to_string())
    }

    fn store(
        &self,
        bus: &mut Bus,
        slot: usize,
        address: u32,
        width: usize,
        value: u32,
        output: &mut impl Write,
    ) -> Result<(), String> {
        if self.is_v8r() && !bus.arm_tcm_permits(slot, address, self.cpsr & 31 == 0x1a) {
            crate::monitor_log::record(
                2,
                &format!(
                    "sysbus: Tried to write {width} bytes at 0x{address:X}, with incorrect permissions."
                ),
            );
            return Ok(());
        }
        if let Some(bytes) = bus.cpu_memory64(slot, u64::from(address), width) {
            bytes.copy_from_slice(&value.to_le_bytes()[..width]);
            bus.observe_memory_store(u64::from(address), width, u64::from(value));
            return Ok(());
        }
        bus.write(address, width, value, output)
            .map_err(|error| error.to_string())
    }

    fn vfp_memory_transfer(
        &mut self,
        instruction: u32,
        pc: u32,
        registers: &mut [u32; 32],
        bus: &mut Bus,
        slot: usize,
        output: &mut impl Write,
    ) -> Result<Option<u32>, String> {
        let rn = ((instruction >> 16) & 15) as usize;
        let vd = ((instruction >> 12) & 15) as usize;
        let extension = ((instruction >> 22) & 1) as usize;
        let double = instruction & (1 << 8) != 0;
        let first = if double {
            (16 * extension + vd) * 2
        } else {
            vd * 2 + extension
        };
        let immediate = instruction & 255;
        let bytes = immediate * 4;
        let up = instruction & (1 << 23) != 0;
        let pre = instruction & (1 << 24) != 0;
        let writeback = instruction & (1 << 21) != 0;
        let load = instruction & (1 << 20) != 0;
        let base = Self::reg(registers, rn, pc);
        let updated = if up {
            base.wrapping_add(bytes)
        } else {
            base.wrapping_sub(bytes)
        };
        let (count, mut address) = if pre && !writeback {
            (1 + usize::from(double), updated)
        } else if rn != 15
            && ((!pre && up) || (pre && !up && writeback))
            && (!double || immediate.is_multiple_of(2))
        {
            (immediate as usize, if up { base } else { updated })
        } else {
            return Ok(Some(self.undefined(pc.wrapping_add(4), registers)));
        };
        if count == 0 || first + count > if double { 64 } else { 32 } {
            return Ok(Some(self.undefined(pc.wrapping_add(4), registers)));
        }
        let access = if load {
            pmsa::Access::Read
        } else {
            pmsa::Access::Write
        };
        for index in first..first + count {
            if let Some(vector) = self.check_protection(address, 4, access, pc, registers) {
                return Ok(Some(vector));
            }
            if load {
                let value = self.load(bus, slot, address, 4)?;
                if index < 32 {
                    self.thumb.vfp_words_mut()[index] = value;
                } else {
                    self.vfp_high_words[index - 32] = value;
                }
            } else {
                let value = if index < 32 {
                    self.thumb.vfp_words_mut()[index]
                } else {
                    self.vfp_high_words[index - 32]
                };
                self.store(bus, slot, address, 4, value, output)?;
            }
            address = address.wrapping_add(4);
        }
        if writeback {
            registers[rn] = updated;
        }
        Ok(None)
    }

    pub(crate) fn step(
        &mut self,
        pc: &mut u32,
        registers: &mut [u32; 32],
        bus: &mut Bus,
        slot: usize,
        output: &mut impl Write,
    ) -> Result<Step, String> {
        bus.refresh_arm_gic_inputs();
        let timer_now = self
            .timer
            .as_ref()
            .map(|_| bus.monitor_time_ns(bus.uart_clock_step));
        if let (Some(timer), Some(now), Some(gic)) = (&self.timer, timer_now, &mut bus.arm_gic) {
            for (index, channel) in timer.channels.iter().enumerate() {
                if let Some(interrupt) = channel.interrupt {
                    gic.set_private(
                        slot,
                        interrupt,
                        channel.control == 1 && channel.expired(timer.count(now, index)),
                    );
                }
            }
        }
        let (gic_irq, gic_fiq) = bus
            .arm_gic
            .as_ref()
            .map_or((false, false), |gic| gic.cpu_inputs(slot));
        let fiq = self.interrupt_inputs[1] || gic_fiq;
        let irq = self.interrupt_inputs[0] || gic_irq;
        if self.sleep != Sleep::Awake {
            let event = self.sleep == Sleep::Event && bus.arm_events & (1 << slot) != 0;
            if !event && !fiq && !irq {
                return Ok(Step::Trap);
            }
            if event {
                bus.arm_events &= !(1 << slot);
            }
            self.sleep = Sleep::Awake;
        }
        if self.cpsr & F == 0 && fiq {
            *pc = self.exception(0x11, 0x1c, pc.wrapping_add(4), registers);
            return Ok(Step::AsynchronousException);
        }
        if self.cpsr & I == 0 && irq {
            *pc = self.exception(0x12, 0x18, pc.wrapping_add(4), registers);
            return Ok(Step::AsynchronousException);
        }
        if self.cpsr & T != 0 {
            if let Some(vector) = self.check_protection(*pc, 2, pmsa::Access::Fetch, *pc, registers)
            {
                *pc = vector;
                return Ok(Step::Trap);
            }
            let raw = self.load(bus, slot, *pc, 2)?;
            if matches!(raw & 0xffc0, 0xe800 | 0xe980) {
                if let Some(vector) = self.check_protection(
                    pc.wrapping_add(2),
                    2,
                    pmsa::Access::Fetch,
                    *pc,
                    registers,
                ) {
                    *pc = vector;
                    return Ok(Step::Trap);
                }
                let low = self.load(bus, slot, pc.wrapping_add(2), 2)?;
                if let Some(next) =
                    self.exception_return(raw, low, *pc, registers, bus, slot, output)?
                {
                    *pc = next;
                    return Ok(Step::Retired);
                }
            }
            self.thumb.set_apsr(self.cpsr);
            self.thumb.arm_branch_taken = false;
            let hypervisor = self.cpsr & 31 == 0x1a;
            let control = if hypervisor { self.hsctlr } else { self.sctlr };
            self.thumb.arm_unaligned_context = (control & 2 == 0).then_some((slot, hypervisor));
            let retired = self.thumb.step(pc, registers, bus, output)?;
            self.cpsr = (self.cpsr & !FLAGS) | (self.thumb.apsr() & FLAGS);
            if self.thumb.arm_branch_taken {
                self.cpsr &= !T;
            }
            return Ok(if retired {
                Step::Retired
            } else {
                Step::Stop(Stop::Ebreak)
            });
        }
        if let Some(vector) = self.check_protection(*pc, 4, pmsa::Access::Fetch, *pc, registers) {
            *pc = vector;
            return Ok(Step::Trap);
        }
        let instruction = self.load(bus, slot, *pc, 4)?;
        let address = *pc;
        let mut next = address.wrapping_add(4);
        let condition = instruction >> 28;
        if instruction & 0xfe00_0000 == 0xfa00_0000 {
            let offset = (((instruction << 8) as i32 >> 6) as u32) | ((instruction >> 23) & 2);
            registers[14] = next;
            next = self.exchange(address.wrapping_add(8).wrapping_add(offset) | 1);
        } else if condition == 15 {
            if instruction & 0xfe5f_ffe0 == 0xf84d_0500 || instruction & 0xfe50_ffff == 0xf810_0a00
            {
                next = self.arm_exception_frame(instruction, next, registers, bus, slot, output)?;
            } else if instruction == 0xf57f_f01f {
                bus.take_reservation64(slot);
            } else if instruction & 0xfff1_fe20 == 0xf100_0000 {
                if self.cpsr & 31 != 0x10 {
                    let mut status = self.cpsr;
                    match (instruction >> 18) & 3 {
                        2 => status &= !(instruction & 0x1c0),
                        3 => status |= instruction & 0x1c0,
                        _ => {}
                    }
                    if instruction & (1 << 17) != 0 {
                        status = (status & !31) | (instruction & 31);
                    }
                    self.set_cpsr(status, registers)?;
                }
            } else if !matches!(
                instruction & 0xffff_fff0,
                0xf57f_f040 | 0xf57f_f050 | 0xf57f_f060
            ) {
                return Err(format!(
                    "unsupported unconditional A32 instruction {instruction:#010x} at {address:#010x}"
                ));
            }
        } else if self.condition(condition) {
            let rn = ((instruction >> 16) & 15) as usize;
            let rd = ((instruction >> 12) & 15) as usize;
            match instruction {
                _ if instruction & 0x0fff_fff0 == 0x0320_f000 => match instruction & 15 {
                    2 if bus.arm_events & (1 << slot) != 0 => bus.arm_events &= !(1 << slot),
                    2 => self.sleep = Sleep::Event,
                    3 => self.sleep = Sleep::Interrupt,
                    4 => bus.arm_events = u64::MAX,
                    5 if self.is_v8r() => bus.arm_events |= 1 << slot,
                    _ => {}
                },
                _ if matches!(instruction & 0x0ff0_f0f0, 0x0710_f010 | 0x0730_f010) => {
                    let lhs = (instruction & 15) as usize;
                    let rhs = ((instruction >> 8) & 15) as usize;
                    if [rn, lhs, rhs].contains(&15) {
                        next = self.undefined(next, registers);
                    } else if registers[rhs] == 0 {
                        registers[rn] = 0;
                    } else if instruction & (1 << 21) != 0 {
                        registers[rn] = registers[lhs] / registers[rhs];
                    } else {
                        registers[rn] =
                            (registers[lhs] as i32).wrapping_div(registers[rhs] as i32) as u32;
                    }
                }
                _ if instruction & 0x0f80_0cf0 == 0x0180_0c90 && instruction & 0xf00 != 0xd00 => {
                    let load = instruction & (1 << 20) != 0;
                    let exclusive = instruction & (1 << 9) != 0;
                    let source = (instruction & 15) as usize;
                    let width = [4, 8, 1, 2][((instruction >> 21) & 3) as usize];
                    let first = if load { rd } else { source };
                    if rn == 15
                        || (load || exclusive) && rd == 15
                        || (load != (source == 15))
                        || !exclusive && (width == 8 || !load && rd != 15)
                        || width == 8 && (first & 1 != 0 || first > 12)
                    {
                        *pc = self.undefined(next, registers);
                        return Ok(Step::Retired);
                    }
                    let target = registers[rn];
                    let access = if load {
                        pmsa::Access::Read
                    } else {
                        pmsa::Access::Write
                    };
                    if let Some(vector) =
                        self.check_protection(target, width, access, address, registers)
                    {
                        *pc = vector;
                        return Ok(Step::Trap);
                    }
                    if !target.is_multiple_of(width as u32) {
                        return Err(format!("unaligned ARM exclusive access at {target:#010x}"));
                    }
                    let physical = u64::from(target);
                    if load {
                        registers[rd] = self.load(bus, slot, target, width.min(4))?;
                        if width == 8 {
                            registers[rd + 1] = self.load(bus, slot, target.wrapping_add(4), 4)?;
                        }
                        if exclusive {
                            bus.reserve64(slot, physical, physical, width);
                        }
                    } else if !exclusive {
                        self.store(bus, slot, target, width, registers[source], output)?;
                    } else {
                        let reservation = bus.take_reservation64(slot);
                        let succeeded =
                            bus.matches_reservation64(slot, reservation, physical, physical, width);
                        if succeeded {
                            self.store(bus, slot, target, width.min(4), registers[source], output)?;
                            if width == 8 {
                                self.store(
                                    bus,
                                    slot,
                                    target.wrapping_add(4),
                                    4,
                                    registers[source + 1],
                                    output,
                                )?;
                            }
                        }
                        registers[rd] = u32::from(!succeeded);
                    }
                }
                _ if matches!(
                    instruction & 0x0fe0_0070,
                    0x07a0_0050 | 0x07e0_0050 | 0x07c0_0010
                ) && rd != 15 =>
                {
                    let source = (instruction & 15) as usize;
                    let lsb = (instruction >> 7) & 31;
                    let width_field = (instruction >> 16) & 31;
                    let insert = instruction & 0x40 == 0;
                    if insert && width_field < lsb || !insert && width_field + lsb >= 32 {
                        next = self.undefined(next, registers);
                    } else if insert {
                        let width = width_field - lsb + 1;
                        let mask = (u32::MAX >> (32 - width)) << lsb;
                        let field = if source == 15 {
                            0
                        } else {
                            registers[source] << lsb
                        };
                        registers[rd] = (registers[rd] & !mask) | (field & mask);
                    } else if source == 15 {
                        next = self.undefined(next, registers);
                    } else {
                        let width = width_field + 1;
                        let field = registers[source] << (32 - lsb - width);
                        registers[rd] = if instruction & (1 << 22) != 0 {
                            field >> (32 - width)
                        } else {
                            ((field as i32) >> (32 - width)) as u32
                        };
                    }
                }
                _ if instruction & 0x0fff_0ff0 == 0x016f_0f10 && rd != 15 => {
                    registers[rd] = registers[(instruction & 15) as usize].leading_zeros();
                }
                _ if matches!(
                    instruction & 0x0fff_0ff0,
                    0x06bf_0f30 | 0x06bf_0fb0 | 0x06ff_0f30 | 0x06ff_0fb0
                ) =>
                {
                    let source = (instruction & 15) as usize;
                    if rd == 15 || source == 15 {
                        next = self.undefined(next, registers);
                    } else {
                        let value = registers[source];
                        registers[rd] = match instruction & 0x0040_0080 {
                            0 => value.swap_bytes(),
                            0x80 => (value & 0x00ff_00ff) << 8 | (value & 0xff00_ff00) >> 8,
                            0x0040_0000 => value.reverse_bits(),
                            _ => (value as u16).swap_bytes() as i16 as i32 as u32,
                        };
                    }
                }
                _ if instruction & 0x0ff0_00f0 == 0x0060_0090 => {
                    let lhs = (instruction & 15) as usize;
                    let rhs = ((instruction >> 8) & 15) as usize;
                    if [lhs, rhs, rn, rd].contains(&15) {
                        next = self.undefined(next, registers);
                    } else {
                        registers[rn] =
                            registers[rd].wrapping_sub(registers[lhs].wrapping_mul(registers[rhs]));
                    }
                }
                _ if instruction & 0x0fc0_00f0 == 0x0000_0090 => {
                    let lhs = (instruction & 15) as usize;
                    let rhs = ((instruction >> 8) & 15) as usize;
                    if [lhs, rhs, rn, rd].contains(&15) {
                        next = self.undefined(next, registers);
                    } else {
                        let mut result = registers[lhs].wrapping_mul(registers[rhs]);
                        if instruction & (1 << 21) != 0 {
                            result = result.wrapping_add(registers[rd]);
                        }
                        registers[rn] = result;
                        if instruction & (1 << 20) != 0 {
                            self.cpsr = (self.cpsr & !(N | Z))
                                | (result & N)
                                | (u32::from(result == 0) * Z);
                        }
                    }
                }
                _ if instruction & 0x0f80_00f0 == 0x0080_0090 => {
                    let lhs = (instruction & 15) as usize;
                    let rhs = ((instruction >> 8) & 15) as usize;
                    if [lhs, rhs, rd, rn].contains(&15) || rn == rd {
                        next = self.undefined(next, registers);
                    } else {
                        let signed = instruction & (1 << 22) != 0;
                        let mut product = if signed {
                            (i64::from(registers[lhs] as i32) * i64::from(registers[rhs] as i32))
                                as u64
                        } else {
                            u64::from(registers[lhs]) * u64::from(registers[rhs])
                        };
                        if instruction & (1 << 21) != 0 {
                            product = product.wrapping_add(
                                (u64::from(registers[rn]) << 32) | u64::from(registers[rd]),
                            );
                        }
                        registers[rd] = product as u32;
                        registers[rn] = (product >> 32) as u32;
                        if instruction & (1 << 20) != 0 {
                            self.cpsr = (self.cpsr & !(N | Z))
                                | (registers[rn] & N)
                                | (u32::from(product == 0) * Z);
                        }
                    }
                }
                _ if instruction & 0x0f00_0000 == 0x0f00_0000 => {
                    if self.is_v8r() && self.cpsr & 31 == 0x1a {
                        self.fault_status[2] = (0x11 << 26) | (1 << 25) | (instruction & 0xffff);
                    }
                    next = self.exception(0x13, 8, next, registers);
                }
                _ if self.is_v8r() && instruction & 0x0ff0_00f0 == 0x0140_0070 => {
                    if self.cpsr & 31 == 0x10 || self.hcr & (1 << 29) != 0 {
                        next = self.undefined(next, registers);
                    } else {
                        let immediate = ((instruction >> 4) & 0xfff0) | (instruction & 15);
                        self.fault_status[2] = (0x12 << 26) | (1 << 25) | immediate;
                        next = self.exception(0x1a, 8, next, registers);
                    }
                }
                _ if self.is_v8r() && instruction & 0x0ff0_00f0 == 0x0120_0070 => {
                    let immediate = ((instruction >> 4) & 0xfff0) | (instruction & 15);
                    self.debug_status = (self.debug_status & !0x3c) | 0x0c;
                    if self.cpsr & 31 == 0x1a {
                        self.fault_status[2] = (0x38 << 26) | (1 << 25) | immediate;
                        next = self.exception(0x1a, 0x0c, address, registers);
                    } else {
                        next = self.exception(0x17, 0x0c, next, registers);
                    }
                }
                _ if instruction & 0x0ff0_00f0 == 0x07f0_00f0
                    || instruction & 0x0f00_0f10 == 0x0e00_0e00 =>
                {
                    // UDF and the unimplemented CP14 CDP encoding take Undefined.
                    next = self.undefined(next, registers);
                }
                0xe160_006e if self.is_v8r() && self.cpsr & 31 == 0x1a => {
                    let status = self.banks[6].spsr;
                    self.set_cpsr(status, registers)?;
                    next = self.elr_hyp & if status & T != 0 { !1 } else { !3 };
                }
                _ if self.is_v8r() && instruction & 0x0fb0_0eff == 0x0100_0200 => {
                    if rd != 15
                        && let Some(value) = self
                            .banked_register(instruction, registers)
                            .map(|source| *source)
                    {
                        registers[rd] = value;
                    } else {
                        next = self.undefined(address.wrapping_add(4), registers);
                    }
                }
                _ if self.is_v8r() && instruction & 0x0fb0_fef0 == 0x0120_f200 => {
                    let source = (instruction & 15) as usize;
                    let value = registers[source];
                    if source != 15
                        && let Some(register) = self.banked_register(instruction, registers)
                    {
                        *register = value;
                    } else {
                        next = self.undefined(address.wrapping_add(4), registers);
                    }
                }
                _ if instruction & 0x0fe0_0f0f == 0x0c40_0f0e => {
                    let operation = (instruction >> 4) & 15;
                    let handled = if self.cpsr & 31 == 0x10 || rd == 15 || rn == 15 || rd == rn {
                        false
                    } else if let (Some(timer), Some(now)) = (&mut self.timer, timer_now) {
                        if instruction & (1 << 20) != 0 {
                            if let Some(value) = timer.read64(operation, now) {
                                registers[rd] = value as u32;
                                registers[rn] = (value >> 32) as u32;
                                true
                            } else {
                                false
                            }
                        } else {
                            let value = u64::from(registers[rd]) | (u64::from(registers[rn]) << 32);
                            timer.write64(operation, value)
                        }
                    } else {
                        false
                    };
                    if !handled {
                        next = self.undefined(address.wrapping_add(4), registers);
                    }
                }
                _ if instruction & 0x0ff0_0f0f == 0x0c40_0f0c => {
                    let operation = (instruction >> 4) & 15;
                    let valid = self.cpsr & 31 != 0x10 && rd != 15 && rn != 15;
                    let value = u64::from(registers[rd]) | (u64::from(registers[rn]) << 32);
                    if !valid
                        || !bus
                            .arm_gic
                            .as_mut()
                            .is_some_and(|gic| gic.send_aarch32_sgi(slot, operation, value))
                    {
                        next = self.undefined(next, registers);
                    }
                }
                _ if self.is_v8r()
                    && instruction & 0x0fe0_0fff == 0x0ee0_0a10
                    && matches!(rn, 0 | 5..=8) =>
                {
                    if self.cpsr & 31 == 0x10 || rd == 15 {
                        next = self.undefined(next, registers);
                    } else if instruction & (1 << 20) != 0 {
                        registers[rd] = match rn {
                            0 => 0x4103_4023,
                            5 => 0x0000_0043,
                            6 => 0x1211_1111,
                            7 => 0x1011_0222,
                            _ => self.fpexc,
                        };
                    } else if rn == 8 {
                        self.fpexc = (registers[rd] & 0xc000_0000) | 0x700;
                    } else {
                        next = self.undefined(next, registers);
                    }
                }
                _ if self.is_v8r()
                    && Thumb::is_fpu_instruction(
                        (instruction >> 16) & 0x0fff | 0xe000,
                        instruction & 0xffff,
                    ) =>
                {
                    let high = (instruction >> 16) & 0x0fff | 0xe000;
                    let user = self.cpsr & 31 == 0x10;
                    let enabled = self.fpexc & (1 << 30) != 0
                        && (self.cpsr & 31 == 0x1a
                            || (self.cpacr >> 20) & 0xf == 15
                            || !user && (self.cpacr >> 20) & 0xf == 5);
                    if !enabled {
                        next = self.undefined(next, registers);
                    } else if high & 0xfe00 == 0xec00 && high & 0xffe0 != 0xec40 {
                        if let Some(vector) = self.vfp_memory_transfer(
                            instruction,
                            address,
                            registers,
                            bus,
                            slot,
                            output,
                        )? {
                            next = vector;
                        }
                    } else if {
                        self.thumb.set_apsr(self.cpsr);
                        self.thumb.execute_fpu(
                            high,
                            instruction & 0xffff,
                            address.wrapping_add(8),
                            registers,
                            bus,
                            output,
                        )?
                    } {
                        self.cpsr = (self.cpsr & !FLAGS) | (self.thumb.apsr() & FLAGS);
                    } else {
                        return Err(format!(
                            "unsupported A32 VFP instruction {instruction:#010x} at {address:#010x}"
                        ));
                    }
                }
                _ if instruction & 0x0f00_0f10 == 0x0e00_0f10 => {
                    let key = instruction & 0x00ef_00ef;
                    let write = instruction & (1 << 20) == 0;
                    let user_thread_id_access = key == 0x000d_0040
                        || key == 0x000d_0060 && !write;
                    let permitted = rd != 15
                        && (self.cpsr & 31 != 0x10 || user_thread_id_access)
                        && (key & (1 << 23) == 0 || self.cpsr & 31 == 0x1a);
                    if permitted
                        && let Some(reply) = self
                            .access_cp15(key, write.then_some(registers[rd]))
                            .or_else(|| {
                                self.is_v8r()
                                    .then(|| {
                                        Self::access_tcm_register(
                                            bus,
                                            slot,
                                            key,
                                            write.then_some(registers[rd]),
                                        )
                                    })
                                    .flatten()
                            })
                            .or_else(|| {
                                bus.arm_gic.as_mut()?.access_aarch32(
                                    slot,
                                    key,
                                    write.then_some(registers[rd]),
                                )
                            })
                    {
                        if let Some(value) = reply {
                            registers[rd] = value;
                        }
                    } else if !self
                        .timer
                        .as_mut()
                        .zip(timer_now)
                        .is_some_and(|(timer, now)| {
                            if !permitted {
                                return false;
                            }
                            if instruction & (1 << 20) == 0 {
                                timer.write32(key, now, registers[rd])
                            } else if let Some(value) = timer.read32(key, now) {
                                registers[rd] = value;
                                true
                            } else {
                                false
                            }
                        })
                    {
                        next = self.undefined(address.wrapping_add(4), registers);
                    }
                }
                _ if instruction & 0x0fbf_0fff == 0x010f_0000 && rd != 15 => {
                    registers[rd] = if instruction & (1 << 22) == 0 {
                        self.cpsr
                    } else {
                        self.banks[Self::bank_for(self.cpsr).unwrap()].spsr
                    };
                }
                _ if instruction & 0x0fb0_fff0 == 0x0120_f000
                    || instruction & 0x0fb0_f000 == 0x0320_f000 =>
                {
                    let (value, _) = self.operand(instruction, registers, address);
                    let mut fields = 0;
                    for field in 0..4 {
                        if instruction & (1 << (16 + field)) != 0 {
                            fields |= 0xff << (field * 8);
                        }
                    }
                    if instruction & (1 << 22) == 0 {
                        if self.cpsr & 31 == 0x10 {
                            fields &= 0xf80f_0000;
                        }
                        let mut updated = (self.cpsr & !fields) | (value & fields);
                        if self.is_v8r() && (self.cpsr & 31 == 0x1a) != (updated & 31 == 0x1a) {
                            updated = (updated & !31) | (self.cpsr & 31) | (1 << 20);
                        }
                        if self.set_cpsr(updated, registers).is_err() {
                            next = self.undefined(address.wrapping_add(4), registers);
                        }
                    } else {
                        let bank = Self::bank_for(self.cpsr).unwrap();
                        self.banks[bank].spsr =
                            (self.banks[bank].spsr & !fields) | (value & fields);
                    }
                }
                _ if matches!(instruction & 0x0fff_fff0, 0x012f_ff10 | 0x012f_ff30) => {
                    let target = Self::reg(registers, (instruction & 15) as usize, address);
                    if instruction & 0x20 != 0 {
                        registers[14] = next;
                    }
                    next = self.exchange(target);
                }
                _ if instruction & 0x0e00_0000 == 0x0a00_0000 => {
                    if instruction & (1 << 24) != 0 {
                        registers[14] = next;
                    }
                    next = address
                        .wrapping_add(8)
                        .wrapping_add(((instruction << 8) as i32 >> 6) as u32);
                }
                _ if matches!(instruction & 0x0ff0_0000, 0x0300_0000 | 0x0340_0000) && rd != 15 => {
                    let immediate = ((instruction >> 4) & 0xf000) | (instruction & 0xfff);
                    registers[rd] = if instruction & (1 << 22) == 0 {
                        immediate
                    } else {
                        (registers[rd] & 0xffff) | (immediate << 16)
                    };
                }
                _ if matches!(
                    instruction & 0x0ff0_03f0,
                    0x06a0_0070 | 0x06b0_0070 | 0x06e0_0070 | 0x06f0_0070
                ) && rd != 15 =>
                {
                    let source = (instruction & 15) as usize;
                    let value = Self::reg(registers, source, address)
                        .rotate_right(((instruction >> 10) & 3) * 8);
                    let extended = match (instruction >> 20) & 7 {
                        2 => value as i8 as i32 as u32,
                        3 => value as i16 as i32 as u32,
                        6 => value & 0xff,
                        _ => value & 0xffff,
                    };
                    registers[rd] = if rn == 15 {
                        extended
                    } else {
                        registers[rn].wrapping_add(extended)
                    };
                }
                _ if instruction & 0x0e00_0000 == 0x0800_0000 => {
                    let list = instruction as u16;
                    if list == 0 || rn == 15 {
                        *pc = self.undefined(next, registers);
                        return Ok(Step::Retired);
                    }
                    let load = instruction & (1 << 20) != 0;
                    let pre = instruction & (1 << 24) != 0;
                    let increment = instruction & (1 << 23) != 0;
                    let user = instruction & (1 << 22) != 0;
                    let return_from_exception = load && user && list & (1 << 15) != 0;
                    let user_bank = user && !return_from_exception;
                    let base = registers[rn];
                    let bytes = 4 * list.count_ones();
                    let updated = if increment {
                        base.wrapping_add(bytes)
                    } else {
                        base.wrapping_sub(bytes)
                    };
                    let low = if increment { base } else { updated };
                    let mut target = if pre == increment {
                        low.wrapping_add(4)
                    } else {
                        low
                    };
                    let access = if load {
                        pmsa::Access::Read
                    } else {
                        pmsa::Access::Write
                    };
                    let mut loaded_pc = None;
                    for index in 0..16 {
                        if list & (1 << index) == 0 {
                            continue;
                        }
                        if let Some(vector) =
                            self.check_protection(target, 4, access, address, registers)
                        {
                            *pc = vector;
                            return Ok(Step::Trap);
                        }
                        if load {
                            let value = self.load(bus, slot, target, 4)?;
                            if index == 15 {
                                loaded_pc = Some(value);
                            } else if user_bank {
                                *self.user_register(registers, index) = value;
                            } else {
                                registers[index] = value;
                            }
                        } else {
                            let value = if user_bank && index != 15 {
                                *self.user_register(registers, index)
                            } else {
                                Self::reg(registers, index, address)
                            };
                            self.store(bus, slot, target, 4, value, output)?;
                        }
                        target = target.wrapping_add(4);
                    }
                    if instruction & (1 << 21) != 0 && (!load || list & (1 << rn) == 0) {
                        registers[rn] = updated;
                    }
                    if let Some(value) = loaded_pc {
                        if return_from_exception {
                            let bank = Self::bank_for(self.cpsr).unwrap();
                            next = if bank != 0
                                && self.set_cpsr(self.banks[bank].spsr, registers).is_ok()
                            {
                                value & if self.cpsr & T != 0 { !1 } else { !3 }
                            } else {
                                self.undefined(next, registers)
                            };
                        } else {
                            next = self.exchange(value);
                        }
                    }
                }
                _ if instruction & 0x0e00_0090 == 0x0000_0090 && instruction & 0x60 != 0 => {
                    let kind = (instruction >> 5) & 3;
                    let load_bit = instruction & (1 << 20) != 0;
                    let pair = !load_bit && kind >= 2;
                    let load = load_bit || pair && kind == 2;
                    if rd == 15 || pair && (rd & 1 != 0 || rd > 12) {
                        *pc = self.undefined(next, registers);
                        return Ok(Step::Retired);
                    }
                    let base = Self::reg(registers, rn, address);
                    let offset = if instruction & (1 << 22) != 0 {
                        ((instruction >> 4) & 0xf0) | (instruction & 15)
                    } else {
                        Self::reg(registers, (instruction & 15) as usize, address)
                    };
                    let updated = if instruction & (1 << 23) != 0 {
                        base.wrapping_add(offset)
                    } else {
                        base.wrapping_sub(offset)
                    };
                    let pre = instruction & (1 << 24) != 0;
                    let target = if pre { updated } else { base };
                    let width = if pair { 4 } else if kind == 2 { 1 } else { 2 };
                    let access = if load { pmsa::Access::Read } else { pmsa::Access::Write };
                    if pair && !target.is_multiple_of(4) {
                        let hyp = self.is_v8r() && self.cpsr & 31 == 0x1a;
                        *pc = self.memory_fault(target, access, address, hyp, pmsa::Fault::Alignment, registers);
                        return Ok(Step::Trap);
                    }
                    let count = if pair { 2 } else { 1 };
                    for index in 0..count {
                        let transfer = target.wrapping_add(index as u32 * 4);
                        if let Some(vector) =
                            self.check_protection(transfer, width, access, address, registers)
                        {
                            *pc = vector;
                            return Ok(Step::Trap);
                        }
                        if load {
                            let value = self.load(bus, slot, transfer, width)?;
                            registers[rd + index] = match (pair, kind) {
                                (false, 2) => value as i8 as i32 as u32,
                                (false, 3) => value as i16 as i32 as u32,
                                _ => value,
                            };
                        } else {
                            self.store(bus, slot, transfer, width, registers[rd + index], output)?;
                        }
                    }
                    if rn != 15 && (!pre || instruction & (1 << 21) != 0) {
                        registers[rn] = updated;
                    }
                }
                _ if instruction & 0x0c00_0000 == 0x0400_0000
                    && (instruction & (1 << 25) == 0 || instruction & 0x10 == 0) =>
                {
                    let base = Self::reg(registers, rn, address);
                    let offset = if instruction & (1 << 25) == 0 {
                        instruction & 0xfff
                    } else {
                        self.shift(
                            Self::reg(registers, (instruction & 15) as usize, address),
                            (instruction >> 5) & 3,
                            (instruction >> 7) & 31,
                            true,
                        )
                        .0
                    };
                    let updated = if instruction & (1 << 23) != 0 {
                        base.wrapping_add(offset)
                    } else {
                        base.wrapping_sub(offset)
                    };
                    let pre = instruction & (1 << 24) != 0;
                    let target = if pre { updated } else { base };
                    let width = if instruction & (1 << 22) != 0 { 1 } else { 4 };
                    let access = if instruction & (1 << 20) != 0 {
                        pmsa::Access::Read
                    } else {
                        pmsa::Access::Write
                    };
                    if let Some(vector) =
                        self.check_protection(target, width, access, address, registers)
                    {
                        *pc = vector;
                        return Ok(Step::Trap);
                    }
                    if instruction & (1 << 20) != 0 {
                        let value = self.load(bus, slot, target, width)?;
                        if rd == 15 {
                            next = self.exchange(value);
                        } else {
                            registers[rd] = value;
                        }
                    } else {
                        self.store(
                            bus,
                            slot,
                            target,
                            width,
                            Self::reg(registers, rd, address),
                            output,
                        )?;
                    }
                    if rn != 15 && (!pre || instruction & (1 << 21) != 0) {
                        registers[rn] = updated;
                    }
                }
                _ if instruction & 0x0c00_0000 == 0
                    && (instruction & (1 << 25) != 0 || instruction & 0x90 != 0x90) =>
                {
                    if let Some(branch) = self.process(instruction, registers, address) {
                        next = branch;
                    }
                }
                _ => {
                    return Err(format!(
                        "unsupported A32 instruction {instruction:#010x} at {address:#010x}"
                    ));
                }
            }
        }
        *pc = next;
        Ok(Step::Retired)
    }
}
