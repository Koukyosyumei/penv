use std::io::sink;

use super::{Instruction, LOCKUP_PC, Thumb};
use crate::bus::Bus;

const CODE: u32 = 0x200;
const BUSFAULT: u32 = 0x300;
const HARDFAULT: u32 = 0x340;
const NMI: u32 = 0x380;
const IRQ: u32 = 0x3c0;
const MOCK: u32 = 0x100000;
const CFSR: u32 = 0xe000_ed28;
const HFSR: u32 = 0xe000_ed2c;
const SHCSR: u32 = 0xe000_ed24;
const STKERR: u32 = 1 << 12;
const UNSTKERR: u32 = 1 << 11;
const LSPERR: u32 = 1 << 13;
const FORCED: u32 = 1 << 30;
const HARDFAULTPENDED: u32 = 1 << 21;
const FPCCR: u32 = 0xe000_ef34;
const FPCAR: u32 = 0xe000_ef38;
const LSPACT: u32 = 1;
const HFRDY: u32 = 1 << 4;

struct Fixture {
    cpu: Thumb,
    bus: Bus,
    pc: u32,
    r: [u32; 32],
}

impl Fixture {
    fn new(offset: Option<u32>, fault_on_writes: bool) -> Self {
        let mut bus = Bus::default();
        bus.map_ram(0, 0x1000).unwrap();
        bus.map_nvic(0xf0, None).unwrap();
        bus.map_systick().unwrap();
        bus.map_faulting_memory_access(MOCK, offset, true, fault_on_writes)
            .unwrap();
        let mut fixture = Self {
            cpu: Thumb::new(Some(0)),
            bus,
            pc: CODE,
            r: [0; 32],
        };
        fixture.cpu.thumb2 = true;
        fixture.r[13] = 0x1000;
        for (number, handler) in [(2, NMI), (3, HARDFAULT), (5, BUSFAULT), (16, IRQ)] {
            fixture.write(number * 4, handler | 1);
            fixture.code(handler, &[0xe7fe]);
        }
        fixture.code(CODE, &[0xe7fe]);
        fixture
    }

    fn code(&mut self, address: u32, words: &[u16]) {
        for (index, &word) in words.iter().enumerate() {
            self.bus
                .write(address + index as u32 * 2, 2, u32::from(word), &mut sink())
                .unwrap();
        }
    }

    fn write(&mut self, address: u32, data: u32) {
        self.bus.write(address, 4, data, &mut sink()).unwrap();
    }

    fn read(&mut self, address: u32) -> u32 {
        self.bus.read::<4>(address).unwrap()
    }

    fn step(&mut self) -> Instruction {
        self.cpu
            .execute_instruction(&mut self.pc, &mut self.r, &mut self.bus, &mut sink())
            .unwrap()
    }

    fn enter(&mut self, exception: u8) {
        if exception == 2 {
            self.write(0xe000_ed04, 1 << 31);
        } else {
            assert_eq!(exception, 16);
            self.write(0xe000_e100, 1);
            self.write(0xe000_e200, 1);
        }
        assert!(
            self.cpu
                .service_interrupts(&mut self.pc, &mut self.r, &mut self.bus)
                .unwrap()
        );
    }

    fn enable_fpu(&mut self) {
        self.bus.nvic.as_mut().unwrap().enable_fpu();
        self.write(0xe000_ed88, 0x00f0_0000);
    }

    fn enter_hardfault(&mut self) {
        self.r[0] = MOCK;
        self.r[1] = 0x11;
        self.r[2] = 0xa5a5_a5a5;
        self.code(CODE, &[0x6802, 0x2122, 0xe7fe]); // ldr r2,[r0]; movs r1,#0x22; b .
        assert!(matches!(self.step(), Instruction::Fault));
        assert_eq!(self.pc, HARDFAULT);
        self.write(CFSR, u32::MAX);
        self.write(HFSR, FORCED);
    }

    fn state(&mut self, pc: u32, sp: u32, exception: u8, cfsr: u32, hfsr: u32) {
        assert_eq!(
            (self.pc, self.r[13], self.cpu.active_exception),
            (pc, sp, exception)
        );
        assert_eq!((self.read(CFSR), self.read(HFSR)), (cfsr, hfsr));
        assert_eq!(self.cpu.locked_up, pc == LOCKUP_PC);
        if pc == LOCKUP_PC {
            assert_eq!(self.read(crate::nvic::DHCSR), 1 << 19);
        }
    }
}

#[test]
fn f407_stack_fault_pends_hardfault_below_nmi() {
    let mut fixture = Fixture::new(None, true);
    fixture.r[13] = MOCK + 32;
    fixture.step();
    fixture.enter(2);
    fixture.state(NMI, MOCK, 2, STKERR, FORCED);
    assert_ne!(fixture.read(SHCSR) & HARDFAULTPENDED, 0);
    fixture.code(NMI, &[0x4770]); // bx lr
    fixture.step();
    fixture.state(HARDFAULT, MOCK, 3, STKERR, FORCED);
    assert_eq!(fixture.read(SHCSR) & HARDFAULTPENDED, 0);
}

#[test]
fn f407_stack_fault_replaces_external_irq_with_hardfault() {
    check_external_stack_fault(false, HARDFAULT, 3, FORCED);
}

#[test]
fn f407_stack_fault_prefers_busfault_at_equal_priority() {
    check_external_stack_fault(true, BUSFAULT, 5, 0);
}

fn check_external_stack_fault(busfault: bool, handler: u32, exception: u8, hfsr: u32) {
    let mut fixture = Fixture::new(None, true);
    if busfault {
        fixture.write(SHCSR, 1 << 17);
    }
    fixture.r[13] = MOCK + 32;
    fixture.step();
    fixture.enter(16);
    fixture.state(handler, MOCK, exception, STKERR, hfsr);
    assert_eq!(fixture.read(0xe000_e200) & 1, 1);
    assert_eq!(fixture.read(0xe000_e300) & 1, 0);
}

#[test]
fn f407_stack_fault_locks_nmi_preempting_hardfault() {
    let mut fixture = Fixture::new(None, true);
    fixture.code(HARDFAULT, &[0x2d00, 0xbf04, 0x2101, 0x2202, 0xe7fe]);
    fixture.enter_hardfault();
    fixture.step();
    fixture.step();
    assert_ne!(fixture.cpu.it_state, 0);
    fixture.r[13] = MOCK + 32;
    fixture.enter(2);
    fixture.state(LOCKUP_PC, MOCK, 2, STKERR, 0);
    assert_eq!(fixture.cpu.it_state, 0);
    assert!(fixture.cpu.active_exceptions.contains(2));
    assert!(fixture.cpu.active_exceptions.contains(3));
}

#[test]
fn f407_unstack_fault_locks_when_returning_to_hardfault() {
    let mut fixture = Fixture::new(None, true);
    fixture.enter_hardfault();
    // ldr r0, =0x100000; msr msp,r0; bx lr
    fixture.code(NMI, &[0x4801, 0xf380, 0x8808, 0x4770]);
    fixture.write(NMI + 8, MOCK);
    fixture.enter(2);
    for _ in 0..3 {
        fixture.step();
    }
    fixture.state(LOCKUP_PC, MOCK + 32, 3, UNSTKERR, 0);
    assert!(!fixture.cpu.active_exceptions.contains(2));
    assert!(fixture.cpu.active_exceptions.contains(3));
}

#[test]
fn f407_unstack_fault_tail_chains_through_original_frame() {
    check_unstack_tail_chain(false);
}

#[test]
fn f407_unstack_fault_tail_chain_clears_fpca() {
    check_unstack_tail_chain(true);
}

fn check_unstack_tail_chain(with_fp: bool) {
    let mut fixture = Fixture::new(None, false);
    fixture.write(SHCSR, 1 << 17);
    if with_fp {
        fixture.enable_fpu();
        fixture.code(NMI, &[0xeeb0, 0x0a40, 0x4770]); // vmov.f32 s0,s0; bx lr
    } else {
        fixture.code(NMI, &[0xbf00, 0x4770]);
    }
    fixture.r[13] = MOCK + 32;
    fixture.step();
    fixture.enter(2);
    fixture.step();
    assert_eq!(fixture.pc, NMI + if with_fp { 4 } else { 2 });
    assert_eq!(fixture.cpu.fp_active, with_fp);
    fixture.step();
    fixture.state(BUSFAULT, MOCK, 5, UNSTKERR, 0);
    assert_eq!(fixture.r[14], 0xffff_fff9);
    assert!(!fixture.cpu.fp_active);
}

#[test]
fn f407_lazy_stack_fault_pends_hardfault_when_thread_was_ready() {
    check_lazy_stack(false);
}

#[test]
fn f407_lazy_stack_fault_locks_when_hardfault_was_not_ready() {
    check_lazy_stack(true);
}

fn check_lazy_stack(from_hardfault: bool) {
    // Only the first byte of the FP frame faults. Initial data at that
    // address in the HardFault setup is reached through the upstream mock.
    let mut fixture = Fixture::new(if from_hardfault { None } else { Some(0x20) }, true);
    fixture.enable_fpu();
    if from_hardfault {
        fixture.enter_hardfault();
        fixture.bus.set_faulting_memory_offset(MOCK, Some(0x20));
    }
    fixture.code(fixture.pc, &[0xeeb0, 0x0a40, 0xe7fe]); // vmov.f32 s0,s0
    fixture.code(NMI, &[0xbf00, 0xeeb0, 0x0a40, 0xe7fe]);
    fixture.r[13] = MOCK + 0x68;
    fixture.step();
    fixture.enter(2);
    fixture.step();
    assert_eq!(fixture.pc, NMI + 2);
    assert_eq!(fixture.read(FPCAR), MOCK + 0x20);
    assert_eq!(fixture.read(FPCCR) & HFRDY != 0, !from_hardfault);
    fixture.step();
    if from_hardfault {
        fixture.state(LOCKUP_PC, MOCK, 2, LSPERR, 0);
        assert!(fixture.cpu.active_exceptions.contains(2));
        assert!(fixture.cpu.active_exceptions.contains(3));
        assert_eq!(fixture.read(SHCSR) & HARDFAULTPENDED, 0);
        assert_ne!(fixture.read(FPCCR) & LSPACT, 0);
    } else {
        fixture.state(NMI + 6, MOCK, 2, LSPERR, FORCED);
        assert_ne!(fixture.read(SHCSR) & HARDFAULTPENDED, 0);
        assert_eq!(fixture.read(FPCCR) & LSPACT, 0);
    }
}

fn instruction_time_lockup() -> Fixture {
    let mut fixture = Fixture::new(None, true);
    // The upstream handler clears HFSR.FORCED, then faults inside an IT block.
    fixture.code(
        HARDFAULT,
        &[
            0x4b04, 0x4c05, 0x601c, 0x2d00, 0xbf04, 0x6802, 0x2122, 0xe7fe,
        ],
    );
    fixture.write(HARDFAULT + 0x14, HFSR);
    fixture.write(HARDFAULT + 0x18, FORCED);
    fixture.r[0] = MOCK;
    fixture.r[1] = 0x11;
    fixture.r[2] = 0xa5a5_a5a5;
    fixture.code(CODE, &[0x6802, 0x2122, 0xe7fe]);
    for _ in 0..20 {
        fixture.step();
    }
    fixture
}

#[test]
fn renode_precise_busfault_locks_up_in_hardfault() {
    let mut fixture = instruction_time_lockup();
    fixture.state(LOCKUP_PC, 0xfe0, 3, 0x8200, 0);
    assert_eq!(fixture.r[1..3], [0x11, 0xa5a5_a5a5]);
    assert_eq!(fixture.read(0xe000_ed38), MOCK);
    assert!(fixture.cpu.active_exceptions.contains(3));
    assert!(!fixture.cpu.waiting_for_interrupt && !fixture.cpu.waiting_for_event);
    let it = fixture.cpu.it_state;
    assert_ne!(it, 0);
    for _ in 0..10 {
        assert!(!matches!(fixture.step(), Instruction::Retired));
    }
    assert_eq!((fixture.pc, fixture.cpu.it_state), (LOCKUP_PC, it));

    fixture.write(0, 0x1000);
    fixture.write(4, CODE | 1);
    fixture.cpu.reset_state();
    fixture.pc = fixture.cpu.reset(&mut fixture.bus, &mut fixture.r).unwrap();
    assert!(!fixture.cpu.locked_up);
    assert_eq!(fixture.read(crate::nvic::DHCSR), 0);
}

#[test]
fn renode_nmi_preempts_instruction_time_lockup() {
    let mut fixture = instruction_time_lockup();
    fixture.enter(2);
    fixture.step(); // NMI handler is the original self-loop.
    fixture.state(NMI, 0xfc0, 2, 0x8200, 0);
    assert_eq!(fixture.read(0xfd8), LOCKUP_PC);
    assert_eq!(fixture.read(crate::nvic::DHCSR), 0);
}

#[test]
fn renode_bfhfnmign_keeps_syndrome_and_continues_hardfault() {
    let mut fixture = Fixture::new(None, true);
    fixture.code(CODE, &[0x6802, 0x2122, 0xe7fe]);
    fixture.code(HARDFAULT, &[0x6802, 0x2122, 0xe7fe]);
    fixture.write(0xe000_ed14, 1 << 8);
    fixture.r[0] = MOCK;
    fixture.r[1] = 0x11;
    fixture.r[2] = 0xa5a5_a5a5;
    assert!(matches!(fixture.step(), Instruction::Fault));
    assert_eq!((fixture.pc, fixture.r[1]), (HARDFAULT, 0x11));

    // Complete the first retired step: exception dispatch itself did not retire.
    assert!(matches!(fixture.step(), Instruction::Retired));
    assert_eq!(fixture.r[1], 0x11);
    assert!(matches!(fixture.step(), Instruction::Retired));
    assert_eq!(fixture.r[1], 0x22);
    fixture.state(HARDFAULT + 4, 0xfe0, 3, 0x8200, FORCED);
    assert_eq!(fixture.read(0xe000_ed38), MOCK);

    // BFHFNMIGN does not suppress an instruction fetch fault.
    fixture.write(CFSR, u32::MAX);
    fixture.pc = MOCK;
    assert!(matches!(fixture.step(), Instruction::Fault));
    fixture.state(LOCKUP_PC, 0xfe0, 3, 1 << 8, FORCED);
    assert_eq!(fixture.read(0xe000_ed38), MOCK);
}
