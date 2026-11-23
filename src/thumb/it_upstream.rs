//! Instruction and register assertions from Renode tests/unit-tests/it-status.robot.
//! The halfwords are assembled from CONDITION_ASM, ITSTATE_ASM and ITSTATE_INSIDE_ASM.

use super::*;

const CONDITIONS: &[u16] = &[
    0xf04f, 0x067f, 0x2e81, 0x2e7f, 0x2e7e, 0xf04f, 0x0101, 0xea4f, 0x72c1, 0x1892,
];
const ITSTATE: &[u16] = &[
    0xf04f, 0x0006, 0xf000, 0xf804, 0xf04f, 0x0007, 0xf000, 0xf800, 0x2806, 0xbf05, 0x2101, 0x2202,
    0x2303, 0x2404, 0xbfcd, 0x2105, 0x2206, 0x2307, 0x2408, 0xf04f, 0x0100, 0xf04f, 0x0200, 0xf04f,
    0x0300, 0xf04f, 0x0400, 0x4770,
];
const INSIDE: &[u16] = &[
    0xf04f, 0x002a, 0x4280, 0xbf05, 0x2843, 0x2202, 0x282a, 0x2404,
];

struct Fixture {
    cpu: Thumb,
    r: [u32; 32],
    pc: u32,
    bus: Bus,
}

impl Fixture {
    fn new(instructions: &[u16]) -> Self {
        let mut bus = Bus::default();
        bus.map_ram(0, 0x1000).unwrap();
        bus.map_nvic(0xc0, None, false).unwrap();
        let bytes = bus.memory(0x100, instructions.len() * 2).unwrap();
        for (slot, instruction) in bytes.chunks_exact_mut(2).zip(instructions) {
            slot.copy_from_slice(&instruction.to_le_bytes());
        }
        let mut registers = [0; 32];
        registers[13] = 0x800;
        let mut cpu = Thumb::new(Some(0));
        cpu.thumb2 = true;
        Self {
            cpu,
            r: registers,
            pc: 0x100,
            bus,
        }
    }

    fn step(&mut self, count: usize) {
        for _ in 0..count {
            self.cpu
                .step(
                    &mut self.pc,
                    &mut self.r,
                    &mut self.bus,
                    &mut std::io::sink(),
                )
                .unwrap();
        }
    }
}

#[test]
fn should_evaluate_condition_codes_properly() {
    let mut m = Fixture::new(CONDITIONS);
    let cases = [
        (
            2,
            0x81000000,
            [false, false, true, false, false, false, false, true],
        ),
        (
            1,
            0x61000000,
            [true, true, false, false, false, true, false, true],
        ),
        (
            1,
            0x21000000,
            [false, true, false, false, true, true, true, true],
        ),
        (
            3,
            0x71000000,
            [true, true, false, true, false, false, false, true],
        ),
    ];
    for (steps, cpsr, conditions) in cases {
        m.step(steps);
        assert_eq!(m.cpu.apsr() | (1 << 24), cpsr);
        for (code, expected) in (0..=14).step_by(2).zip(conditions) {
            assert_eq!(m.cpu.condition(code), expected, "condition {code}");
        }
    }
}

#[test]
fn should_have_correct_condition_code() {
    let mut m = Fixture::new(ITSTATE);
    m.step(9);
    assert_eq!(m.cpu.itstate, 0xcd);
}

#[test]
fn should_shift_state_bits_after_every_it_block_instruction() {
    let mut m = Fixture::new(ITSTATE);
    m.step(4);
    for (state, expected) in [(5, true), (10, true), (20, false), (8, true)] {
        assert_eq!(m.cpu.itstate, state);
        assert_eq!(m.cpu.condition(u32::from(state >> 4)), expected);
        m.step(1);
    }
    assert_eq!(m.cpu.itstate, 0);
}

#[test]
fn should_execute_only_then_instructions() {
    let mut m = Fixture::new(ITSTATE);
    for (steps, registers) in [
        (8, [1, 2, 0, 4]),
        (5, [1, 6, 7, 4]),
        (13, [0, 0, 3, 0]),
        (5, [5, 0, 3, 8]),
    ] {
        m.step(steps);
        assert_eq!(m.r[1..5], registers);
    }
}

#[test]
fn should_allow_condition_flag_change_from_inside_it_block() {
    let mut m = Fixture::new(INSIDE);
    m.step(3);
    for expected in [true, false, false, true] {
        assert_eq!(m.cpu.condition(0), expected);
        m.step(1);
    }
    assert_eq!(m.r[..5], [42, 0, 0, 0, 4]);
}

#[test]
fn should_survive_interrupt() {
    let mut m = Fixture::new(ITSTATE);
    m.bus.map_systick().unwrap();
    m.bus
        .memory(8, 4)
        .unwrap()
        .copy_from_slice(&0x201u32.to_le_bytes());
    m.bus
        .memory(0x200, 4)
        .unwrap()
        .copy_from_slice(&[0x00, 0xbf, 0x70, 0x47]); // ITSTATE_IRQ_ASM
    m.step(4);

    m.bus
        .write(0xe000_ed04, 4, 1 << 31, &mut std::io::sink())
        .unwrap();
    assert!(
        m.cpu
            .service_interrupts(&mut m.pc, &mut m.r, &mut m.bus)
            .unwrap()
    );
    m.step(1);
    assert_eq!(m.pc, 0x202);
    assert_eq!(m.cpu.itstate, 0);
    m.step(1);
    assert_eq!(m.cpu.itstate, 5);
    m.step(4);
    assert_eq!(m.r[1..5], [1, 2, 0, 4]);
}
