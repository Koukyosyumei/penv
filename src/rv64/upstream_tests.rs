//! RV64 cases ported from Renode's riscv-priv.robot and riscv-amocas.robot.
use super::*;

const A0: usize = 10;
const A1: usize = 11;
const A2: usize = 12;
const A3: usize = 13;
const A4: usize = 14;
const A5: usize = 15;
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
    cpu.privilege.mode = Mode::User;
    assert_illegal(&mut cpu, &mut bus, 0x1200_0073); // sfence.vma zero, zero
}

fn assert_amocas(width: usize, expected: u128, replacement: u128, swaps: bool) {
    assert_amocas_at(
        u64::from(RAM_BASE) + 0x1000,
        width,
        expected,
        replacement,
        swaps,
    );
}

fn assert_amocas_at(address: u64, width: usize, expected: u128, replacement: u128, swaps: bool) {
    const ORIGINAL: u128 = 0x2bad_c000_10ff_b0ba_1afe_dead_beef_d00d;
    let mut cpu = Rv64::new(Isa::parse("rv64gc_zacas").unwrap());
    cpu.allow_unaligned = true;
    let mut bus = Bus::new(8192).unwrap();
    if address >= u64::from(RAM_BASE) + 8192 {
        bus.map_ram64(address & !0xfff, 8192).unwrap();
    }
    bus.memory64(address, 16)
        .unwrap()
        .copy_from_slice(&ORIGINAL.to_le_bytes());
    cpu.registers[A0] = expected as u64;
    cpu.registers[A1] = (expected >> 64) as u64;
    cpu.registers[A3] = address;
    cpu.registers[S2] = replacement as u64;
    cpu.registers[S2 + 1] = (replacement >> 64) as u64;

    run_straight_line(&mut cpu, &mut bus, &[amocas(width, A0, S2)]);

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

fn assert_illegal(cpu: &mut Rv64, bus: &mut Bus, instruction: u32) {
    let pc = cpu.pc;
    let vector = u64::from(RAM_BASE) + 0x100;
    cpu.privilege.set_vector(vector);
    load_program(bus, &[instruction]);
    assert!(matches!(
        cpu.step(bus, 0, 1, &mut Vec::new()).unwrap(),
        Step::Trap
    ));
    assert_eq!(cpu.pc, vector);
    assert_eq!(cpu.privilege.read(0x342, cpu.misa(), !3), Some(2));
    assert_eq!(cpu.privilege.read(0x341, cpu.misa(), !3), Some(pc));
}

#[test]
fn traps_on_counter_access_from_user_mode() {
    for csr in [MCYCLE, 0xc02] {
        let mut cpu = Rv64::new(Isa::IM);
        let mut bus = Bus::new(4096).unwrap();
        assert!(cpu.privilege.write(0x306, 0, cpu.misa()));
        assert!(cpu.privilege.write(0x106, 7, cpu.misa()));
        cpu.privilege.mode = Mode::User;
        assert_illegal(&mut cpu, &mut bus, read_csr(csr, A0));
    }
}

#[test]
fn sret_obeys_tsr_in_supervisor_mode() {
    for tsr in [0, 1 << 22] {
        let mut cpu = Rv64::new(Isa::IM);
        let mut bus = Bus::new(4096).unwrap();
        assert!(cpu.privilege.write(0x141, 0xbad0, cpu.misa()));
        assert!(cpu.privilege.write(0x300, tsr, cpu.misa()));
        cpu.privilege.mode = Mode::Supervisor;
        const SRET: u32 = 0x1020_0073;
        if tsr != 0 {
            assert_illegal(&mut cpu, &mut bus, SRET);
        } else {
            run_straight_line(&mut cpu, &mut bus, &[SRET]);
            assert_eq!(cpu.pc, 0xbad0);
            assert!(cpu.privilege.mode == Mode::User);
        }
    }
}

#[test]
fn supervisor_satp_and_sfence_obey_tvm() {
    const SATP: u32 = 0x180;
    const SFENCE: u32 = 0x1200_0073;
    let read = read_csr(SATP, A0);
    let write = write_csr(SATP, A0);
    for instruction in [read, write, SFENCE] {
        for tvm in [0, 1 << 20] {
            let mut cpu = Rv64::new(Isa::IM);
            let mut bus = Bus::new(4096).unwrap();
            assert!(cpu.privilege.write(0x300, tvm, cpu.misa()));
            cpu.privilege.mode = Mode::Supervisor;
            cpu.registers[A0] = 0x0000_ffff_0000_0000;
            if tvm != 0 {
                assert_illegal(&mut cpu, &mut bus, instruction);
                assert_eq!(cpu.privilege.satp, 0);
            } else {
                run_straight_line(&mut cpu, &mut bus, &[instruction]);
                if instruction == read {
                    assert_eq!(cpu.registers[A0], 0);
                } else if instruction == write {
                    assert_eq!(cpu.privilege.satp, cpu.registers[A0]);
                }
            }
        }
    }
}

fn amocas(width: usize, rd: usize, rs2: usize) -> u32 {
    (0b00101 << 27)
        | ((rs2 as u32) << 20)
        | ((A3 as u32) << 15)
        | (width.trailing_zeros() << 12)
        | ((rd as u32) << 7)
        | 0x2f
}

const AMOCAS_OLD_LOW: u64 = 0x1afe_dead_beef_d00d;
const AMOCAS_OLD_HIGH: u64 = 0x2bad_c000_10ff_b0ba;
const AMOCAS_NEW_LOW: u64 = 0x1eef_f00d_babe_dead;
const AMOCAS_NEW_HIGH: u64 = 0x216b_00b5_d0d0_caca;

#[test]
fn amocas_q_zero_source_ignores_x1() {
    let mut cpu = Rv64::new(Isa::parse("rv64gc_zacas").unwrap());
    let mut bus = Bus::new(8192).unwrap();
    let address = u64::from(RAM_BASE) + 0x1000;
    cpu.registers[A3] = address;
    cpu.registers[A0] = AMOCAS_OLD_LOW;
    cpu.registers[A1] = AMOCAS_OLD_HIGH;
    cpu.registers[1] = 0x1234;
    let old = (u128::from(AMOCAS_OLD_HIGH) << 64) | u128::from(AMOCAS_OLD_LOW);
    bus.memory64(address, 16)
        .unwrap()
        .copy_from_slice(&old.to_le_bytes());
    run_straight_line(&mut cpu, &mut bus, &[amocas(16, A0, 0)]);
    assert_eq!(bus.ram64(address, 16).unwrap(), [0; 16]);
    assert_eq!(cpu.registers[A0], AMOCAS_OLD_LOW);
    assert_eq!(cpu.registers[A1], AMOCAS_OLD_HIGH);
    assert_eq!(cpu.registers[1], 0x1234);
    assert_eq!(cpu.registers[A3], address);
}

#[test]
fn amocas_q_zero_destination_does_not_write_x1() {
    let mut cpu = Rv64::new(Isa::parse("rv64gc_zacas").unwrap());
    let mut bus = Bus::new(8192).unwrap();
    let address = u64::from(RAM_BASE) + 0x1000;
    cpu.registers[A3] = address;
    cpu.registers[S2] = AMOCAS_NEW_LOW;
    cpu.registers[S2 + 1] = AMOCAS_NEW_HIGH;
    cpu.registers[1] = 0x1234;
    run_straight_line(&mut cpu, &mut bus, &[amocas(16, 0, S2)]);
    let new = (u128::from(AMOCAS_NEW_HIGH) << 64) | u128::from(AMOCAS_NEW_LOW);
    assert_eq!(bus.ram64(address, 16).unwrap(), new.to_le_bytes());
    assert_eq!(cpu.registers[S2], AMOCAS_NEW_LOW);
    assert_eq!(cpu.registers[S2 + 1], AMOCAS_NEW_HIGH);
    assert_eq!((cpu.registers[0], cpu.registers[1]), (0, 0x1234));
    assert_eq!(cpu.registers[A3], address);
}

#[test]
fn amocas_q_traps_on_odd_register_pairs() {
    for (rd, rs2) in [(A0 + 1, S2), (A0, S2 + 1), (A0 + 1, S2 + 1)] {
        let mut cpu = Rv64::new(Isa::parse("rv64gc_zacas").unwrap());
        let mut bus = Bus::new(8192).unwrap();
        let address = u64::from(RAM_BASE) + 0x1000;
        let old = (u128::from(AMOCAS_OLD_HIGH) << 64) | u128::from(AMOCAS_OLD_LOW);
        bus.memory64(address, 16)
            .unwrap()
            .copy_from_slice(&old.to_le_bytes());
        cpu.registers[A3] = address;
        cpu.registers[rd] = AMOCAS_OLD_LOW;
        cpu.registers[rd + 1] = AMOCAS_OLD_HIGH;
        cpu.registers[rs2] = AMOCAS_NEW_LOW;
        cpu.registers[rs2 + 1] = AMOCAS_NEW_HIGH;
        cpu.privilege.set_vector(0x1010);
        let opcode = amocas(16, rd, rs2);
        load_program(&mut bus, &[opcode]);
        assert!(matches!(
            cpu.step(&mut bus, 0, 1, &mut Vec::new()).unwrap(),
            Step::Trap
        ));
        assert_eq!(cpu.pc, 0x1010);
        assert_eq!(cpu.privilege.read(0x342, cpu.misa(), !3), Some(2));
        assert_eq!(
            cpu.privilege.read(0x343, cpu.misa(), !3),
            Some(u64::from(opcode))
        );
        assert_eq!(
            cpu.privilege.read(0x341, cpu.misa(), !3),
            Some(u64::from(RAM_BASE))
        );
        assert_eq!(bus.ram64(address, 16).unwrap(), old.to_le_bytes());
        assert_eq!(cpu.registers[rd], AMOCAS_OLD_LOW);
        assert_eq!(cpu.registers[rd + 1], AMOCAS_OLD_HIGH);
        assert_eq!(cpu.registers[rs2], AMOCAS_NEW_LOW);
        assert_eq!(cpu.registers[rs2 + 1], AMOCAS_NEW_HIGH);
        assert_eq!(cpu.registers[A3], address);
    }
}

#[test]
fn amocas_page_spanning_ram_matches_and_mismatches() {
    assert_amocas_matches_and_mismatches(u64::from(RAM_BASE) + 0x4000_0000 - 1, 4);
}

#[test]
fn amocas_array_memory_matches_and_mismatches() {
    assert_amocas_matches_and_mismatches(0x1_0000_1000, 8);
}

fn assert_amocas_matches_and_mismatches(address: u64, minimum_width: usize) {
    for (width, expected, wrong, replacement) in [
        (4, 0xbeef_d00d, 0x1234, 0xbeef_babe),
        (
            8,
            0x1afe_dead_beef_d00d,
            0x0001_2345_6789_1011,
            0xbeef_f00d_babe_dead,
        ),
        (
            16,
            0x2bad_c000_10ff_b0ba_1afe_dead_beef_d00d,
            0x1234_5678_9101_1121_3141_5161_7181_9202,
            0x216b_00b5_d0d0_caca_1eef_f00d_babe_dead,
        ),
    ] {
        if width >= minimum_width {
            assert_amocas_at(address, width, expected, replacement, true);
            assert_amocas_at(address, width, wrong, replacement, false);
        }
    }
}

#[test]
fn unaligned_amocas_fault_leaves_the_first_page_unchanged() {
    let address = u64::from(RAM_BASE) + 4095;
    let mut cpu = Rv64::new(Isa::parse("rv64gc_zacas").unwrap());
    cpu.allow_unaligned = true;
    cpu.privilege.set_vector(u64::from(RAM_BASE) + 0x100);
    cpu.registers[A3] = address;
    cpu.registers[A0] = 0x42;
    cpu.registers[S2] = 0x53;
    let mut bus = Bus::new(4096).unwrap();
    bus.memory64(address, 1).unwrap()[0] = 0x42;
    load_program(&mut bus, &[amocas(4, A0, S2)]);
    assert!(matches!(
        cpu.step(&mut bus, 0, 1, &mut Vec::new()).unwrap(),
        Step::Trap
    ));
    assert_eq!(cpu.privilege.read(0x342, cpu.misa(), !3), Some(7));
    assert_eq!(cpu.privilege.read(0x343, cpu.misa(), !3), Some(address + 1));
    assert_eq!(bus.ram64(address, 1).unwrap(), [0x42]);
    assert_eq!(cpu.registers[A0], 0x42);
}

#[test]
fn upstream_test_watcher_decodes_only_odd_writes_at_base() {
    let address = u64::from(RAM_BASE) + 4096;
    let mut bus = Bus::new(8192).unwrap();
    bus.memory64(address, 8).unwrap().fill(0xff);
    bus.watch_robot_tohost(address).unwrap();
    let cpu = Rv64::new(Isa::IM);
    let mut output = Vec::new();
    assert_eq!(cpu.load(&mut bus, address, 8).unwrap(), 0);
    assert!(
        cpu.store(&mut bus, address, 8, 0x0000_0002_0000_0000, &mut output)
            .is_ok()
    );
    assert!(cpu.store(&mut bus, address + 1, 1, 1, &mut output).is_ok());
    assert_eq!(bus.rv64_test_exit, None);
    assert!(cpu.store(&mut bus, address, 2, 7, &mut output).is_ok());
    assert_eq!(bus.rv64_test_exit, Some(0x1_0000_0003));
    assert!(cpu.store(&mut bus, address, 8, 1, &mut output).is_ok());
    assert_eq!(bus.rv64_test_exit, Some(0x1_0000_0003));
    assert_eq!(bus.ram64(address, 8).unwrap(), [0xff; 8]);
}

fn czero(funct3: u32, rd: usize, rs1: usize, rs2: usize) -> u32 {
    (7 << 25)
        | ((rs2 as u32) << 20)
        | ((rs1 as u32) << 15)
        | (funct3 << 12)
        | ((rd as u32) << 7)
        | 0x33
}

#[test]
fn zicond_robot_rv64_selectors_and_destination_aliases() {
    for (funct3, destination, value, selector, result) in [
        (5, A2, 0x1234, 1, 0x1234),
        (5, A2, 0x1234, 0x1_0000_0000, 0x1234),
        (5, A2, 0x1234, 0, 0),
        (7, A2, 0x1234, 0, 0x1234),
        (7, A2, 0x1234, 1, 0),
        (7, A2, 0x1234, 0x1_0000_0000, 0),
        (5, A0, 0xcafe, 1, 0xcafe),
        (5, A0, 0xcafe, 0, 0),
        (7, A0, 0xcafe, 0, 0xcafe),
        (7, A0, 0xcafe, 1, 0),
        (5, A2, 0xdead_beef_cafe_babe, 1, 0xdead_beef_cafe_babe),
    ] {
        let mut cpu = Rv64::new(Isa::parse("rv64gc_zicond").unwrap());
        let mut bus = Bus::new(4096).unwrap();
        cpu.registers[A0] = value;
        cpu.registers[A1] = selector;
        run_straight_line(&mut cpu, &mut bus, &[czero(funct3, destination, A0, A1)]);
        assert_eq!(cpu.registers[destination], result);
    }
}

#[test]
fn zicond_robot_traps_when_extension_disabled() {
    for funct3 in [5, 7] {
        let mut cpu = Rv64::new(Isa::parse("rv64gc").unwrap());
        let mut bus = Bus::new(4096).unwrap();
        cpu.privilege.set_vector(0x1010);
        load_program(&mut bus, &[czero(funct3, A0, A1, A2)]);
        assert!(matches!(
            cpu.step(&mut bus, 0, 1, &mut Vec::new()).unwrap(),
            Step::Trap
        ));
        assert_eq!(cpu.pc, 0x1010);
        assert_eq!(cpu.privilege.read(0x342, cpu.misa(), !3), Some(2));
        assert_eq!(
            cpu.privilege.read(0x341, cpu.misa(), !3),
            Some(u64::from(RAM_BASE))
        );
    }
}

#[test]
fn zbkb_robot_rv64_pack_and_brev8() {
    for (word, lhs, rhs, expected) in [
        (
            0x08b5_4633,
            0xffff_ffff_bad0_cafe,
            0xffff_ffff_dead_beef,
            0xdead_beef_bad0_cafe,
        ),
        (0x08b5_7633, 0xffff_ffcd, 0xffff_ffab, 0xabcd),
        (
            0x08b5_463b,
            0xffff_ffff_ffff_beef,
            0xffff_ffff_ffff_dead,
            0xffff_ffff_dead_beef,
        ),
        (0x6875_5613, u64::MAX, 0, u64::MAX),
        (0x6875_5613, 0xdead_beaf_bad0_cafe, 0, 0x7bb5_7df5_5d0b_537f),
    ] {
        let mut cpu = Rv64::new(Isa::parse("rv64gc_zbkb").unwrap());
        let mut bus = Bus::new(4096).unwrap();
        cpu.registers[A0] = lhs;
        cpu.registers[A1] = rhs;
        run_straight_line(&mut cpu, &mut bus, &[word]);
        assert_eq!(cpu.registers[A2], expected);
    }
}

#[test]
fn mret_robot_rv64_privilege_levels() {
    use crate::rv64_priv::Levels;
    for (levels, lowest_mpp) in [(Levels::MachineUser, 0), (Levels::Machine, 3)] {
        let mut cpu = Rv64::new(Isa::parse("rv64gc").unwrap());
        cpu.privilege.configure_levels(levels);
        let mut bus = Bus::new(4096).unwrap();
        bus.map_ram64(0x1000, 0x40000).unwrap();
        cpu.pc = 0x2000;
        cpu.privilege.set_vector(0x4000);
        bus.memory64(0x2000, 4)
            .unwrap()
            .copy_from_slice(&0xc000_1073u32.to_le_bytes());
        bus.memory64(0x4000, 4)
            .unwrap()
            .copy_from_slice(&0x3020_0073u32.to_le_bytes());
        assert!(matches!(
            cpu.step(&mut bus, 0, 1, &mut Vec::new()).unwrap(),
            Step::Trap
        ));
        assert_eq!(cpu.pc, 0x4000);
        assert_eq!(cpu.privilege.status >> 11 & 3, 3);
        assert!(matches!(
            cpu.step(&mut bus, 0, 2, &mut Vec::new()).unwrap(),
            Step::Retired
        ));
        assert_eq!(cpu.pc, 0x2000);
        assert_eq!(cpu.privilege.status >> 11 & 3, lowest_mpp);
    }
}

#[test]
fn amoadd_robot_single_and_page_spanning_ram() {
    const ORIGINAL: u64 = 0xffff_ffff_8000_0000;
    for address in [0x8000_0100, 0x8000_0fff] {
        const INCREMENT_X: u64 = 0xffff_ffff_ffff_f800;
        for (width, first, second, second_increment) in [
            (4usize, 0x7fff_f800, 0xffff_f800, ORIGINAL),
            (8, 0xffff_ffff_7fff_f800, 0xffff_ffff_7fff_f000, INCREMENT_X),
        ] {
            let mut cpu = Rv64::new(Isa::parse("rv64gc_zicsr_zifencei").unwrap());
            cpu.allow_unaligned = true;
            let mut bus = Bus::new(8192).unwrap();
            bus.memory64(address, width)
                .unwrap()
                .copy_from_slice(&ORIGINAL.to_le_bytes()[..width]);
            let instruction =
                (11 << 20) | (10 << 15) | (width.trailing_zeros() << 12) | (12 << 7) | 0x2f;
            load_program(&mut bus, &[instruction; 2]);
            cpu.registers[A0] = address;
            for (old, increment, new) in [
                (ORIGINAL, INCREMENT_X, first),
                (first, second_increment, second),
            ] {
                cpu.registers[A1] = increment;
                assert!(matches!(
                    cpu.step(&mut bus, 1, 1, &mut Vec::new()).unwrap(),
                    Step::Retired
                ));
                assert_eq!(cpu.registers[A2], old);
                assert_eq!(cpu.registers[A1], increment);
                assert_eq!(
                    bus.ram64(address, width).unwrap(),
                    &new.to_le_bytes()[..width]
                );
            }
        }
    }
}

#[test]
fn minstret_explicit_write_replaces_the_instruction_increment() {
    let mut cpu = Rv64::new(Isa::IM);
    let mut bus = Bus::new(4096).unwrap();
    cpu.registers[A0] = u64::MAX;
    run_straight_line(
        &mut cpu,
        &mut bus,
        &[
            write_csr(MINSTRET, A0),
            read_csr(MINSTRET, A1),
            read_csr(MINSTRET, A2),
        ],
    );
    assert_eq!(cpu.registers[A1], u64::MAX);
    assert_eq!(cpu.registers[A2], 0);
}

#[test]
fn zfh_boxes_half_values_and_honors_rounding_and_nan_rules() {
    use crate::rv64_float::{Format, Rounding};
    let half = Format::Half;
    let zero = half.decode(half.boxed(0));
    let smallest = half.decode(half.boxed(1));
    let one = half.decode(half.boxed(0x3c00));
    let result = half.add(one, smallest, Rounding::Up);
    assert_eq!((result.bits, result.flags), (0x3c01, 1));
    assert_eq!(half.add(one, smallest, Rounding::NearestEven).bits, 0x3c00);
    assert_eq!(
        half.add(half.decode(0x3c00), zero, Rounding::NearestEven)
            .bits,
        0x7e00
    );
    assert_eq!(half.boxed(0x8000), 0xffff_ffff_ffff_8000);
    assert!(Isa::parse("rv64i_zfh").is_err());
}

#[test]
fn cbo_zero_clears_only_the_selected_block_and_checks_environment_bits() {
    const OPCODE: u32 = 0x0045_200f; // cbo.zero (a0)
    let base = u64::from(RAM_BASE) + 0x100;
    for mode in [Mode::Machine, Mode::Supervisor, Mode::User] {
        let mut cpu = Rv64::new(Isa::parse("rv64i_zicboz").unwrap());
        let mut bus = Bus::new(4096).unwrap();
        cpu.registers[A0] = base + 17;
        cpu.privilege.mode = mode;
        if mode != Mode::Machine {
            assert_illegal(&mut cpu, &mut bus, OPCODE);
            cpu.pc = u64::from(RAM_BASE);
            assert!(cpu.privilege.write(0x30a, 1 << 7, cpu.misa()));
            if mode == Mode::User {
                cpu.privilege.mode = mode;
                assert_illegal(&mut cpu, &mut bus, OPCODE);
                cpu.pc = u64::from(RAM_BASE);
                assert!(cpu.privilege.write(0x10a, 1 << 7, cpu.misa()));
            }
        }
        cpu.privilege.mode = mode;
        bus.memory64(base - 1, 66).unwrap().fill(0xff);
        run_straight_line(&mut cpu, &mut bus, &[OPCODE]);
        assert_eq!(bus.ram64(base, 64).unwrap(), [0; 64]);
        assert_eq!(bus.ram64(base - 1, 1).unwrap(), [0xff]);
        assert_eq!(bus.ram64(base + 64, 1).unwrap(), [0xff]);
    }
}

#[test]
fn fma_rounds_to_min_normal_but_still_raises_underflow() {
    use crate::rv64_float::{Format, Rounding};
    let f = Format::Single;
    let decode = |bits| f.decode(f.boxed(bits));
    let result = f.fused(
        decode(0x0080_0000),
        decode(0x32c1_1a50),
        decode(0x007f_ffff),
        Rounding::Up,
    );
    assert_eq!((result.bits, result.flags), (0x0080_0000, 3));

    let result = f.convert(Format::Double.decode(0x380f_ffff_ffff_ffff), Rounding::Up);
    assert_eq!((result.bits, result.flags), (0x0080_0000, 1));
}

#[test]
fn smepmp_execution_and_data_access_trap_with_physical_protection_causes() {
    use crate::rv64_priv::Pmp;
    let base = u64::from(RAM_BASE);
    for (instruction, expected, fault_address) in [
        (0x0000_006fu32, 1, base + 0x1000), // fetch outside the executable rule
        (0x0005_3303, 5, base + 0x1000),    // ld t1, 0(a0)
        (0x0005_3023, 7, base + 0x1000),    // sd zero, 0(a0)
    ] {
        let mut cpu = Rv64::new(Isa::parse("rv64i_smepmp").unwrap());
        let mut bus = Bus::new(8192).unwrap();
        let mut pmp = Pmp::new(16, 54);
        pmp.enable_smepmp();
        cpu.privilege.pmp = Some(pmp);
        cpu.privilege.set_vector(base + 0x100);
        cpu.registers[A0] = fault_address;
        let misa = cpu.misa();
        for (csr, value) in [
            (0x747, 4),
            (0x3b0, (base >> 2) | 0x1ff),
            (0x3a0, 0x9d),
            (0x747, 3),
        ] {
            assert!(cpu.privilege.write(csr, value, misa));
        }
        if expected == 1 {
            cpu.pc = fault_address;
        }
        load_program(&mut bus, &[instruction]);
        let fault_pc = cpu.pc;
        assert!(matches!(
            cpu.step(&mut bus, 0, 1, &mut Vec::new()).unwrap(),
            Step::Trap
        ));
        assert_eq!(cpu.pc, base + 0x100);
        assert_eq!(cpu.privilege.read(0x342, misa, !3), Some(expected));
        assert_eq!(cpu.privilege.read(0x343, misa, !3), Some(fault_address));
        assert_eq!(cpu.privilege.read(0x341, misa, !3), Some(fault_pc));
    }
}

#[test]
fn pmp_robot_illegal_rv64_csr_indices_still_trap_with_smepmp() {
    // pmp.robot uses Priv1_10 and checks Renode's warning for each instruction.
    // Here check the architectural trap using the very same opcodes and count.
    for opcode in [0x3a12_9073, 0x3da0_5073] {
        let mut cpu = Rv64::new(Isa::parse("rv64gc_smepmp").unwrap());
        let mut pmp = crate::rv64_priv::Pmp::new(16, 54);
        pmp.enable_smepmp();
        cpu.privilege.pmp = Some(pmp);
        let mut bus = Bus::new(4096).unwrap();
        assert_illegal(&mut cpu, &mut bus, opcode);
        assert_eq!(
            cpu.privilege.read(0x343, cpu.misa(), !3),
            Some(u64::from(opcode))
        );
    }
}

#[test]
fn rv64_robot_isa_string_accepts_mixed_case() {
    // The fixed Robot case also constructs an RV32 CPU in a separate keyword.
    let isa = Isa::parse("RV64IMAFDC_Zicsr_Zifencei").unwrap();
    assert!(isa.multiply && isa.atomic && isa.single && isa.double && isa.compressed);
    assert!(Isa::parse("RV64GC_Invalid_ISA").is_err());
}

#[test]
fn vector_control_csrs_gate_on_vs_and_share_status() {
    let mut cpu = Rv64::new(Isa::parse("rv64gcv_zba_zbb_zbc_zbs_zfh_zacas_smepmp").unwrap());
    let mut bus = Bus::new(4096).unwrap();
    let write = Instruction(write_csr(0x00a, A0));
    assert_eq!(cpu.csr(write, u64::MAX, 0), None);
    assert_eq!(cpu.misa() & (1 << 21), 1 << 21);
    cpu.registers[A0] = 1 << 9;
    cpu.registers[A1] = u64::MAX;
    run_straight_line(
        &mut cpu,
        &mut bus,
        &[
            write_csr(0x300, A0),
            write_csr(0x00a, A1),
            read_csr(0x00f, A2),
            write_csr(0x009, A1),
            read_csr(0x00f, A3),
            write_csr(0x008, A1),
            read_csr(0x008, A4),
        ],
    );
    assert_eq!(
        [cpu.registers[A2], cpu.registers[A3], cpu.registers[A4]],
        [6, 7, 511]
    );
    assert_eq!(cpu.privilege.read(0xc22, cpu.misa(), !1), Some(64));
    assert_eq!(cpu.privilege.read(0x100, cpu.misa(), !1).unwrap() >> 63, 1);
    assert_eq!(cpu.csr(Instruction(write_csr(0xc22, A0)), 0, 0), None);
    let mut scalar = Rv64::new(Isa::parse("rv64gc").unwrap());
    assert!(scalar.privilege.write(0x300, 3 << 9, scalar.misa()));
    assert_eq!(scalar.privilege.status & (3 << 9), 0);
    assert_eq!(scalar.csr(write, 0, 0), None);
}

#[test]
fn vector_length_instructions_handle_avl_and_invalid_types() {
    let mut cpu = Rv64::new(Isa::parse("rv64gc_v").unwrap());
    let mut bus = Bus::new(4096).unwrap();
    let immediate =
        |bits: u32, avl: u32, rd: usize| bits << 20 | avl << 15 | 7 << 12 | (rd as u32) << 7 | 0x57;
    const E16_M2: u32 = 0x09;
    const E64_MF2: u32 = 0x1f;
    assert!(cpu.privilege.write(0x300, 1 << 9, cpu.misa()));
    cpu.registers[A0] = E16_M2.into();
    run_straight_line(
        &mut cpu,
        &mut bus,
        &[
            immediate(0xc00 | E16_M2, 7, A1), // vsetivli
            immediate(E16_M2, 0, 0),          // preserve vl
            read_csr(0xc20, A2),
            immediate(E16_M2, 0, A3), // use VLMAX
            0x8000_7057 | (A0 as u32) << 20 | (A1 as u32) << 15 | (A4 as u32) << 7,
            immediate(E64_MF2, 0, A5),
            read_csr(0xc21, A0),
        ],
    );
    assert_eq!(
        [
            cpu.registers[A1],
            cpu.registers[A2],
            cpu.registers[A3],
            cpu.registers[A4],
            cpu.registers[A5]
        ],
        [7, 7, 64, 7, 0]
    );
    assert_eq!(cpu.registers[A0], 1 << 63);
}

#[test]
fn vector_signed_saturation_sets_sticky_vxsat() {
    let mut cpu = Rv64::new(Isa::parse("rv64gcv").unwrap());
    let mut bus = Bus::new(4096).unwrap();
    cpu.registers[A0] = 127;
    cpu.registers[A1] = 127;
    assert!(cpu.privilege.write(0x300, 1 << 9, cpu.misa()));
    run_straight_line(
        &mut cpu,
        &mut bus,
        &[
            0x0c00_72d7, // vsetvli t0, zero, e8, m1, ta, ma
            0x5e05_4057, // vmv.v.x v0, a0
            0x5e05_c0d7, // vmv.v.x v1, a1
            0x8600_8157, // vsadd.vv v2, v0, v1
            read_csr(0x00f, A2),
            0x5e00_4057, // vmv.v.x v0, zero
            0x8600_8157, // vsadd.vv v2, v0, v1 (no new saturation)
            read_csr(0x009, A3),
        ],
    );
    assert_eq!((cpu.registers[A2], cpu.registers[A3]), (1, 1));
}

#[test]
fn renode_lrsc_preserves_same_hart_reservation_across_store_and_width_change() {
    for compatible in [false, true] {
        let mut cpu = Rv64::new(Isa::parse("rv64ia").unwrap());
        let mut bus = Bus::new(4096).unwrap();
        bus.renode_lrsc = compatible;
        cpu.registers[A0] = u64::from(RAM_BASE) + 0x100;
        cpu.registers[A1] = 0x0102_0304_0506_0708;
        let lr = 2 << 27 | (A0 as u32) << 15 | 2 << 12 | (A2 as u32) << 7 | 0x2f;
        let sc =
            3 << 27 | (A1 as u32) << 20 | (A0 as u32) << 15 | 3 << 12 | (A3 as u32) << 7 | 0x2f;
        run_straight_line(&mut cpu, &mut bus, &[lr, 0x0005_01a3, sc]); // sb zero, 3(a0)
        assert_eq!(cpu.registers[A3], u64::from(!compatible));
        let expected = if compatible { cpu.registers[A1] } else { 0 };
        assert_eq!(ram_read(&bus, None, cpu.registers[A0], 8), Some(expected));
    }
}

#[test]
fn renode_unmapped_data_does_not_relax_other_access_checks() {
    let mut cpu = Rv64::new(Isa::parse("rv64i").unwrap());
    let mut bus = Bus::new(4096).unwrap();
    assert_eq!(cpu.load(&mut bus, 0x2000, 8).unwrap_err().cause, 5);
    bus.renode_unmapped_data = true;
    assert_eq!(cpu.load(&mut bus, 0x2000, 8).unwrap(), 0);
    cpu.store(&mut bus, 0x2000, 8, u64::MAX, &mut Vec::new())
        .unwrap();
    assert_eq!(cpu.load(&mut bus, 0x2000, 8).unwrap(), 0);
    assert_eq!(cpu.load(&mut bus, 0x2001, 8).unwrap_err().cause, 4);
    cpu.pc = 0x2000;
    assert_eq!(cpu.fetch(&mut bus).unwrap_err().cause, 1);
}

#[test]
fn offset_ram_window_keeps_backing_and_cpu_context() {
    let mut bus = Bus::new(4096).unwrap();
    let memory = bus.map_ram_backing64(0x9000_0000, 0x5000, 0x5000).unwrap();
    bus.map_alias64(memory, 0x9100_0000, 0x1000, 0x4000, 1, false)
        .unwrap();
    let cpu = Rv64::new(Isa::parse("rv64g").unwrap());
    for (width, test_word) in [
        (8, 0xfeed_face_cafe_beefu64),
        (4, 0xdead_beef),
        (2, 0xface),
        (1, 0xbe),
    ] {
        let offset = 0x9000_4000;
        let alias = 0x9100_0000;
        bus.memory64(offset, width)
            .unwrap()
            .copy_from_slice(&test_word.to_le_bytes()[..width]);
        assert!(bus.ram64(alias, width).is_none());
        assert_eq!(cpu.load(&mut bus, alias, width).unwrap(), test_word);
        bus.memory64(offset, width).unwrap().fill(0);
        cpu.store(&mut bus, alias, width, test_word, &mut Vec::new())
            .unwrap();
        assert_eq!(
            bus.ram64(offset, width).unwrap(),
            &test_word.to_le_bytes()[..width]
        );
    }
    let shadowed = bus.map_ram_backing64(0x9300_0000, 0x1000, 0x1000).unwrap();
    assert_ne!(memory, shadowed);
    bus.map_alias64(memory, 0x9300_0000, 0x1000, 0x4000, 1, false)
        .unwrap();
    assert_eq!(bus.ram64(0x9300_0000, 1), Some(&[0][..]));
    assert_eq!(bus.cpu_ram64(0, 0x9300_0000, 1), Some(&[0xbe][..]));
    assert!(bus.cpu_ram64(0, 0x9100_1000, 1).is_none());
    assert!(
        bus.map_alias64(memory, 0x9200_0000, 0x1001, 0x4000, 0, false)
            .is_err()
    );
}

#[test]
fn per_hart_ram_windows_have_independent_backings() {
    let mut bus = Bus::new(4096).unwrap();
    let first = bus.map_ram_backing64(0x2010000, 4096, 4096).unwrap();
    let second = bus.map_ram_backing64(0x2020000, 4096, 4096).unwrap();
    let address = 0x3000000;
    bus.map_alias64(first, address, 4096, 0, 1, false).unwrap();
    bus.map_alias64(second, address, 4096, 0, (1 << 1) | (1 << 2), false)
        .unwrap();
    assert!(
        bus.map_alias64(second, address, 4096, 0, 1 | (1 << 3), false)
            .is_err()
    );
    bus.cpu_memory64(0, address, 1).unwrap()[0] = 0xb0;
    bus.cpu_memory64(1, address, 1).unwrap()[0] = 0xba;
    assert_eq!(bus.cpu_ram64(2, address, 1).unwrap(), &[0xba]);
    assert_eq!(bus.ram64(0x2010000, 1).unwrap(), &[0xb0]);
    assert_eq!(bus.ram64(0x2020000, 1).unwrap(), &[0xba]);
    assert!(bus.cpu_ram64(3, address, 1).is_none());
    assert!(bus.ram64(address, 1).is_none());
}

#[test]
fn renode_ram_move_targets_only_exclusive_cpu_registrations() {
    let mut bus = Bus::new(4096).unwrap();
    let one = bus.map_ram_backing64(0x2010000, 4096, 4096).unwrap();
    let two = bus.map_ram_backing64(0x2020000, 4096, 4096).unwrap();
    let old = 0x3000000;
    let new = 0xaaa0000;
    bus.map_alias64(one, old, 4096, 0, 1, false).unwrap();
    bus.map_alias64(two, old, 4096, 0, 2, true).unwrap();
    bus.cpu_memory64(0, old, 1).unwrap()[0] = 0x12;
    bus.cpu_memory64(1, old, 1).unwrap()[0] = 0x34;
    assert!(bus.move_cpu_ram_alias(0, 0x2010000, new).is_err());
    assert!(bus.move_cpu_ram_alias(1, old, new).is_err());
    assert!(bus.move_cpu_ram_alias(0, old, u64::MAX).is_err());
    assert_eq!(bus.cpu_ram64(0, old, 1), Some(&[0x12][..]));
    bus.move_cpu_ram_alias(0, old, new).unwrap();
    assert_eq!(bus.cpu_ram64(0, old, 1), None);
    assert_eq!(bus.cpu_ram64(1, old, 1), Some(&[0x34][..]));
    assert_eq!(bus.cpu_ram64(0, new, 1), Some(&[0x12][..]));
    bus.cpu_memory64(0, new, 1).unwrap()[0] = 0x56;
    assert_eq!(bus.ram64(0x2010000, 1), Some(&[0x56][..]));
}
