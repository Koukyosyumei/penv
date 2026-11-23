//! Armv7-M ports of the CPACR cases from Renode arm-cortex-m-fpu.robot.

use super::*;

// NOCP_ASSEMBLY assembled at 0x10000. The vector table and code are separated
// by the upstream .align 12 padding.
const VECTORS: &[u8] = &[
    0x00, 0x10, 0x00, 0x00, 0x01, 0x10, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x3b, 0x10, 0x01, 0x00,
];
const PROGRAM: &[u8] = &[
    0x14, 0x49, 0x0a, 0x68, 0x42, 0xf4, 0x80, 0x22, 0x0a, 0x60, 0x4f, 0xf4, 0x80, 0x13, 0x40, 0xf2,
    0x00, 0x27, 0x4f, 0xf4, 0x80, 0x32, 0xd2, 0xed, 0x00, 0x7a, 0xfd, 0xee, 0xe7, 0x7a, 0xf8, 0xee,
    0xe7, 0x7a, 0xc7, 0xed, 0x00, 0x7a, 0x0c, 0x4c, 0xff, 0xe7, 0x20, 0x78, 0x04, 0xf1, 0x01, 0x04,
    0x00, 0x28, 0x01, 0xd0, 0x18, 0x70, 0xf8, 0xe7, 0x30, 0xbf, 0x08, 0x49, 0x0a, 0x68, 0x4f, 0xf0,
    0x08, 0x01, 0x07, 0x4c, 0x91, 0x42, 0xf0, 0xd0, 0x30, 0xbf, 0x4e, 0x4f, 0x43, 0x50, 0x0a, 0x00,
    0x4f, 0x4b, 0x0a, 0x00, 0x24, 0xed, 0x00, 0xe0, 0x50, 0x10, 0x01, 0x00, 0x2a, 0xed, 0x00, 0xe0,
    0x4a, 0x10, 0x01, 0x00,
];

fn nocp_fixture() -> (Bus, Thumb, [u32; 32], u32) {
    let mut bus = Bus::default();
    bus.map_ram(0, 0x40000).unwrap();
    bus.map_nvic(0xc0, None, true).unwrap();
    bus.map_trivial_uart(0x100000).unwrap();
    bus.memory(0x10000, VECTORS.len())
        .unwrap()
        .copy_from_slice(VECTORS);
    bus.memory(0x11000, PROGRAM.len())
        .unwrap()
        .copy_from_slice(PROGRAM);

    let mut cpu = Thumb::new(Some(0x10000));
    cpu.thumb2 = true;
    let (sp, pc) = cpu.reset(&bus).unwrap().unwrap();
    let mut registers = [0; 32];
    registers[13] = sp;
    (bus, cpu, registers, pc)
}

#[test]
fn should_not_raise_exception_when_fpu_is_enabled() {
    let (mut bus, mut cpu, mut registers, mut pc) = nocp_fixture();
    let mut uart = Vec::new();
    // cpu FpuEnabled true: enable CP10 and CP11 before starting the guest.
    bus.write(0xe000_ed88, 4, 0x00f0_0000, &mut uart).unwrap();
    for _ in 0..64 {
        cpu.step(&mut pc, &mut registers, &mut bus, &mut uart)
            .unwrap();
        if cpu.sleeping() {
            break;
        }
    }
    assert_eq!(uart, b"OK\n");
}

#[test]
fn should_raise_nocp_exception() {
    let (mut bus, mut cpu, mut registers, mut pc) = nocp_fixture();
    // Read the same UFSR halfword with the aligned Armv7-M PPB access width.
    bus.memory(0x1103c, 2)
        .unwrap()
        .copy_from_slice(&0x880au16.to_le_bytes());
    let mut uart = Vec::new();
    for _ in 0..64 {
        cpu.step(&mut pc, &mut registers, &mut bus, &mut uart)
            .unwrap();
        if cpu.sleeping() {
            break;
        }
    }
    assert_eq!(uart, b"NOCP\n");
    assert_eq!(cpu.active_exception, 6);
    assert_eq!(registers[13], 0xfe0);
    assert_eq!(registers[14], 0xffff_fff9);
    assert_eq!(bus.read_ram::<4>(0xff8).unwrap(), 0x11016);
    assert_eq!(bus.read::<4>(0xe000_ed28).unwrap(), 1 << 19);
    assert_eq!(bus.read::<4>(0xe000_ed2c).unwrap(), 0);
}

#[test]
fn precise_nocp_restores_psp_and_it_state_for_retry() {
    let (mut bus, mut cpu, mut registers, _) = nocp_fixture();
    let mut out = std::io::sink();
    const SHCSR: u32 = 0xe000_ed24;
    const UFSR: u32 = 0xe000_ed2a;
    const CPACR: u32 = 0xe000_ed88;
    const FAULT_PC: u32 = 0x11016;
    bus.write(SHCSR, 4, 1 << 18, &mut out).unwrap();
    bus.memory(0x1103a, 2)
        .unwrap()
        .copy_from_slice(&0x4770u16.to_le_bytes()); // bx lr
    registers[13] = 0x2000;
    cpu.system_write(9, 0x1004, &mut registers);
    cpu.system_write(20, 2, &mut registers);
    cpu.itstate = 0x08;
    cpu.zero = true;
    let mut pc = FAULT_PC;
    cpu.step(&mut pc, &mut registers, &mut bus, &mut out)
        .unwrap();
    assert_eq!(pc, 0x1103a);
    assert_eq!(registers[13], 0x2000);
    assert_eq!(registers[14], 0xffff_fffd);
    assert_eq!(cpu.psp, 0xfe0);
    assert_eq!(bus.read_ram::<4>(0xff8).unwrap(), FAULT_PC);
    assert_eq!(bus.read_ram::<4>(0xffc).unwrap(), 0x4100_0a00);
    assert!(!cpu.fp_active);
    assert_eq!(bus.read::<2>(UFSR).unwrap(), 8);
    bus.write(UFSR, 2, 8, &mut out).unwrap();
    assert_eq!(bus.read::<2>(UFSR).unwrap(), 0);
    bus.write(CPACR, 4, 0x00f0_0000, &mut out).unwrap();
    cpu.step(&mut pc, &mut registers, &mut bus, &mut out)
        .unwrap();
    assert_eq!(pc, FAULT_PC);
    assert_eq!(registers[13], 0x1004);
    assert_eq!(cpu.itstate, 0x08);
    cpu.step(&mut pc, &mut registers, &mut bus, &mut out)
        .unwrap();
    assert_eq!(pc, FAULT_PC + 4);
    assert_eq!(cpu.itstate, 0);
    assert!(cpu.fp_active);
}

#[test]
fn nocp_escalates_and_stacks_an_active_fp_context_even_with_cpacr_closed() {
    let (mut bus, mut cpu, mut registers, _) = nocp_fixture();
    let mut out = std::io::sink();
    bus.memory(0x1000c, 4)
        .unwrap()
        .copy_from_slice(&0x1103bu32.to_le_bytes());
    bus.memory(0x1103a, 2)
        .unwrap()
        .copy_from_slice(&0x4770u16.to_le_bytes());
    bus.write(0xe000_ef34, 4, 0x8000_0000, &mut out).unwrap();
    cpu.fp_active = true;
    cpu.fp[0] = 0x1234_5678;
    cpu.fpscr = 0x4000_0000;
    let mut pc = 0x11016;
    cpu.step(&mut pc, &mut registers, &mut bus, &mut out)
        .unwrap();
    assert_eq!(cpu.active_exception, 3);
    assert_eq!(pc, 0x1103a);
    assert_eq!(registers[13], 0xf98);
    assert_eq!(registers[14], 0xffff_ffe9);
    assert_eq!(bus.read_ram::<4>(0xfb0).unwrap(), 0x11016);
    assert_eq!(bus.read_ram::<4>(0xfb8).unwrap(), 0x1234_5678);
    assert_eq!(bus.read_ram::<4>(0xff8).unwrap(), 0x4000_0000);
    assert_eq!(bus.read::<4>(0xe000_ed2c).unwrap(), 1 << 30);
    bus.write(0xe000_ed2c, 4, 1 << 30, &mut out).unwrap();
    assert_eq!(bus.read::<4>(0xe000_ed2c).unwrap(), 0);
    cpu.fp[0] = 0;
    cpu.fpscr = 0;
    cpu.step(&mut pc, &mut registers, &mut bus, &mut out)
        .unwrap();
    assert_eq!((pc, registers[13]), (0x11016, 0x1000));
    assert_eq!(cpu.fp[0], 0x1234_5678);
    assert_eq!(cpu.fpscr, 0x4000_0000);
    assert!(cpu.fp_active);
}
