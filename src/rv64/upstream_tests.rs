//! RV64 cases ported from Renode's riscv-priv.robot and riscv-amocas.robot.
use super::*;

const A0: usize = 10;
const A1: usize = 11;
const A2: usize = 12;
const A3: usize = 13;
const S2: usize = 18;

fn load_program(bus: &mut Bus, instructions: &[u32]) {
    let code = bus.memory(RAM_BASE, instructions.len() * 4).unwrap();
    for (word, slot) in instructions.iter().zip(code.chunks_exact_mut(4)) {
        slot.copy_from_slice(&word.to_le_bytes());
    }
}

fn run_straight_line(cpu: &mut Rv64, bus: &mut Bus, instructions: &[u32]) {
    load_program(bus, instructions);
    for (index, instruction) in instructions.iter().enumerate() {
        let step = cpu
            .step(bus, 0, (index + 1) as u64, &mut Vec::new())
            .unwrap();
        assert!(
            matches!(step, Step::Retired),
            "instruction {index}: {instruction:#010x}"
        );
    }
}

const MCYCLE: u32 = 0xb00;
const MINSTRET: u32 = 0xb02;
const COUNT_INHIBIT: u32 = 0x320;
const NOP: u32 = 0x0000_0013;
const SUB_A3_A2_A1: u32 = 0x40b6_06b3;

fn read_csr(csr: u32, rd: usize) -> u32 {
    (csr << 20) | (2 << 12) | ((rd as u32) << 7) | 0x73 // csrrs rd, csr, zero
}

fn write_csr(csr: u32, rs: usize) -> u32 {
    (csr << 20) | ((rs as u32) << 15) | (1 << 12) | 0x73 // csrrw zero, csr, rs
}

fn assert_counter_delta(program: &[u32], expected: u64) {
    let mut cpu = Rv64::new(Isa::IM);
    let mut bus = Bus::new(4096).unwrap();
    run_straight_line(&mut cpu, &mut bus, program);
    assert_eq!(cpu.registers[A3], expected);
}

fn assert_counting(csr: u32) {
    assert_counter_delta(
        &[read_csr(csr, A1), NOP, read_csr(csr, A2), SUB_A3_A2_A1],
        2,
    );
}

fn assert_inhibited(csr: u32, bit: u32) {
    assert_counter_delta(
        &[
            (bit << 20) | 0x513, // addi a0, zero, bit
            write_csr(COUNT_INHIBIT, A0),
            read_csr(csr, A1),
            NOP,
            read_csr(csr, A2),
            SUB_A3_A2_A1,
        ],
        0,
    );
}

fn assert_inhibit_after_write(csr: u32, bit: u32) {
    assert_counter_delta(
        &[
            (bit << 20) | 0x513, // addi a0, zero, bit
            0x0000_0293,         // addi t0, zero, 0
            read_csr(csr, A1),
            write_csr(COUNT_INHIBIT, A0),
            NOP,
            write_csr(COUNT_INHIBIT, 5),
            read_csr(csr, A2),
            SUB_A3_A2_A1,
        ],
        2,
    );
}

#[test]
fn increments_mcycle() {
    assert_counting(MCYCLE);
}

#[test]
fn increments_minstret() {
    assert_counting(MINSTRET);
}

#[test]
fn inhibits_mcycle() {
    assert_inhibited(MCYCLE, 1);
}

#[test]
fn inhibits_minstret() {
    assert_inhibited(MINSTRET, 4);
}

#[test]
fn inhibits_mcycle_after_write_finishes() {
    assert_inhibit_after_write(MCYCLE, 1);
}

#[test]
fn inhibits_minstret_after_write_finishes() {
    assert_inhibit_after_write(MINSTRET, 4);
}

#[test]
fn traps_sfence_vma_in_user_mode() {
    let mut cpu = Rv64::new(Isa::IM);
    let mut bus = Bus::new(4096).unwrap();
    let trap_vector = u64::from(RAM_BASE) + 0x100;
    let fault_pc = cpu.pc;
    cpu.privilege.set_vector(trap_vector);
    cpu.privilege.mode = Mode::User;
    load_program(&mut bus, &[0x1200_0073]); // sfence.vma zero, zero

    let step = cpu.step(&mut bus, 0, 1, &mut Vec::new()).unwrap();
    assert!(matches!(step, Step::Trap));
    assert_eq!(cpu.pc, trap_vector);
    assert_eq!(cpu.privilege.read(0x342, cpu.misa(), !3), Some(2));
    assert_eq!(cpu.privilege.read(0x341, cpu.misa(), !3), Some(fault_pc));
}

fn assert_amocas(width: usize, expected: u128, replacement: u128, swaps: bool) {
    const ORIGINAL: u128 = 0x2bad_c000_10ff_b0ba_1afe_dead_beef_d00d;
    let address = u64::from(RAM_BASE) + 0x1000;
    let mut cpu = Rv64::new(Isa::parse("rv64gc_zacas").unwrap());
    let mut bus = Bus::new(8192).unwrap();
    bus.memory64(address, 16)
        .unwrap()
        .copy_from_slice(&ORIGINAL.to_le_bytes());
    cpu.registers[A0] = expected as u64;
    cpu.registers[A1] = (expected >> 64) as u64;
    cpu.registers[A3] = address;
    cpu.registers[S2] = replacement as u64;
    cpu.registers[S2 + 1] = (replacement >> 64) as u64;

    let size = width.trailing_zeros();
    let instruction = (0b00101 << 27)
        | ((S2 as u32) << 20)
        | ((A3 as u32) << 15)
        | (size << 12)
        | ((A0 as u32) << 7)
        | 0x2f;
    run_straight_line(&mut cpu, &mut bus, &[instruction]);

    let previous = if width == 4 {
        ORIGINAL as i32 as i64 as u64
    } else {
        ORIGINAL as u64
    };
    assert_eq!(cpu.registers[A0], previous);
    assert_eq!(
        cpu.registers[A1],
        if width == 16 {
            (ORIGINAL >> 64) as u64
        } else {
            (expected >> 64) as u64
        }
    );
    assert_eq!(cpu.registers[A3], address);
    assert_eq!(cpu.registers[S2], replacement as u64);
    assert_eq!(cpu.registers[S2 + 1], (replacement >> 64) as u64);
    let mut memory = ORIGINAL.to_le_bytes();
    if swaps {
        memory[..width].copy_from_slice(&replacement.to_le_bytes()[..width]);
    }
    assert_eq!(bus.ram64(address, 16).unwrap(), memory);
}

#[test]
fn amocas_w_changes_single_page_on_match() {
    assert_amocas(4, 0xbeef_d00d, 0xbeef_babe, true);
}

#[test]
fn amocas_w_preserves_single_page_on_mismatch() {
    assert_amocas(4, 0x1234, 0xbeef_babe, false);
}

#[test]
fn amocas_d_changes_single_page_on_match() {
    assert_amocas(8, 0x1afe_dead_beef_d00d, 0xbeef_f00d_babe_dead, true);
}

#[test]
fn amocas_d_preserves_single_page_on_mismatch() {
    assert_amocas(8, 0x0001_2345_6789_1011, 0xbeef_f00d_babe_dead, false);
}

#[test]
fn amocas_q_changes_single_page_on_match() {
    assert_amocas(
        16,
        0x2bad_c000_10ff_b0ba_1afe_dead_beef_d00d,
        0x216b_00b5_d0d0_caca_1eef_f00d_babe_dead,
        true,
    );
}

#[test]
fn amocas_q_preserves_single_page_on_mismatch() {
    assert_amocas(
        16,
        0x1234_5678_9101_1121_3141_5161_7181_9202,
        0x216b_00b5_d0d0_caca_1eef_f00d_babe_dead,
        false,
    );
}
