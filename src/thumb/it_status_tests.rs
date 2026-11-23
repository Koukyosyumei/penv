use std::io::sink;

use super::{Instruction, Thumb};
use crate::bus::Bus;

// Cortex-M4 Thumb halfwords assembled from the named variables in the pinned
// Renode it-status.robot. Each program is loaded at the original address 0x100.
const CONDITION_ASM: &[u16] = &[
    0xf04f, 0x067f, 0x2e81, 0x2e7f, 0x2e7e, 0xf04f, 0x0101, 0xea4f, 0x72c1, 0x1892,
];
const ITSTATE_ASM: &[u16] = &[
    0xf04f, 0x0006, 0xf000, 0xf804, 0xf04f, 0x0007, 0xf000, 0xf800, 0x2806, 0xbf05, 0x2101, 0x2202,
    0x2303, 0x2404, 0xbfcd, 0x2105, 0x2206, 0x2307, 0x2408, 0xf04f, 0x0100, 0xf04f, 0x0200, 0xf04f,
    0x0300, 0xf04f, 0x0400, 0x4770,
];
const ITSTATE_INSIDE_ASM: &[u16] = &[
    0xf04f, 0x002a, 0x4280, 0xbf05, 0x2843, 0x2202, 0x282a, 0x2404,
];

struct Fixture {
    cpu: Thumb,
    bus: Bus,
    pc: u32,
    regs: [u32; 32],
}

impl Fixture {
    fn new(program: &[u16]) -> Self {
        let mut bus = Bus::default();
        bus.map_ram(0, 0x1000).unwrap();
        bus.map_nvic(0xff, None).unwrap();
        bus.map_systick().unwrap();
        bus.systick.as_mut().unwrap().set_frequency(72_000_000);
        bus.set_step_hz(125_000_000);
        let mut output = sink();
        for (address, value) in [(0, 0x800), (4, 0x101), (8, 0x201)] {
            bus.write(address, 4, value, &mut output).unwrap();
        }
        for (address, words) in [(0x100, program), (0x200, &[0xbf00, 0x4770][..])] {
            for (index, word) in words.iter().enumerate() {
                bus.write(address + index as u32 * 2, 2, u32::from(*word), &mut output)
                    .unwrap();
            }
        }
        let mut cpu = Thumb::new(Some(0));
        cpu.thumb2 = true;
        let mut regs = [0; 32];
        let pc = cpu.reset(&mut bus, &mut regs).unwrap();
        Self { cpu, bus, pc, regs }
    }

    fn step(&mut self, count: usize) {
        let mut output = sink();
        for _ in 0..count {
            self.cpu
                .service_interrupts(&mut self.pc, &mut self.regs, &mut self.bus)
                .unwrap();
            assert!(matches!(
                self.cpu
                    .execute_instruction(&mut self.pc, &mut self.regs, &mut self.bus, &mut output)
                    .unwrap(),
                Instruction::Retired
            ));
        }
    }
}

#[test]
fn renode_it_evaluates_condition_codes() {
    let mut fixture = Fixture::new(CONDITION_ASM);
    for (steps, xpsr, expected) in [
        (
            2,
            0x8100_0000,
            [false, false, true, false, false, false, false, true],
        ),
        (
            1,
            0x6100_0000,
            [true, true, false, false, false, true, false, true],
        ),
        (
            1,
            0x2100_0000,
            [false, true, false, false, true, true, true, true],
        ),
        (
            3,
            0x7100_0000,
            [true, true, false, true, false, false, false, true],
        ),
    ] {
        fixture.step(steps);
        assert_eq!(
            std::array::from_fn(|i| fixture.cpu.condition(i as u32 * 2)),
            expected
        );
        assert_eq!(fixture.cpu.xpsr(), xpsr);
    }
}

#[test]
fn renode_it_has_correct_condition_code() {
    let mut fixture = Fixture::new(ITSTATE_ASM);
    fixture.step(9);
    assert_eq!(fixture.cpu.it_state, 0xcd);
}

#[test]
fn renode_it_shifts_after_each_instruction() {
    let mut fixture = Fixture::new(ITSTATE_ASM);
    fixture.step(4);
    for (state, executes) in [(5, true), (0xa, true), (0x14, false), (8, true)] {
        assert_eq!(fixture.cpu.it_state, state);
        assert_eq!(fixture.cpu.it_instruction_executes(), executes);
        fixture.step(1);
    }
    assert_eq!(fixture.cpu.it_state, 0);
}

#[test]
fn renode_it_executes_only_matching_slots() {
    let mut fixture = Fixture::new(ITSTATE_ASM);
    for (steps, expected) in [
        (8, [1, 2, 0, 4]),
        (5, [1, 6, 7, 4]),
        (13, [0, 0, 3, 0]),
        (5, [5, 0, 3, 8]),
    ] {
        fixture.step(steps);
        assert_eq!(fixture.regs[1..5], expected);
    }
}

#[test]
fn renode_it_survives_nmi() {
    let mut fixture = Fixture::new(ITSTATE_ASM);
    fixture.step(4);
    fixture
        .bus
        .write(0xe000_ed04, 4, 1 << 31, &mut sink())
        .unwrap();
    fixture.step(1);
    assert_eq!(fixture.pc, 0x202);
    assert_eq!(fixture.cpu.it_state, 0);
    fixture.step(1);
    assert_eq!(fixture.cpu.it_state, 5);
    fixture.step(4);
    assert_eq!(fixture.regs[1..5], [1, 2, 0, 4]);
}

#[test]
fn renode_it_observes_flags_changed_inside_block() {
    let mut fixture = Fixture::new(ITSTATE_INSIDE_ASM);
    fixture.step(3);
    for expected in [true, false, false, true] {
        assert_eq!(fixture.cpu.condition(0), expected);
        fixture.step(1);
    }
    assert_eq!(fixture.regs[..5], [42, 0, 0, 0, 4]);
}
