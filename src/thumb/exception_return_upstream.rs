//! Applicable Cortex-M cases from Renode arm-cortex-m-exception-return.robot.

use super::*;

fn check_nmi_branch(instruction: u16, expected_pc: u32, active: u8) {
    let mut bus = Bus::default();
    bus.map_ram(0, 0x1000).unwrap();
    bus.map_nvic(0xc0, None, true).unwrap();
    bus.map_systick().unwrap();
    for (address, value) in [(0, 0x800u32), (4, 0x21), (8, 0x101)] {
        bus.memory(address, 4)
            .unwrap()
            .copy_from_slice(&value.to_le_bytes());
    }
    bus.memory(0x20, 2).unwrap().copy_from_slice(&[0xfe, 0xe7]); // b .
    bus.memory(0x100, 2)
        .unwrap()
        .copy_from_slice(&0xbf00u16.to_le_bytes());
    bus.memory(0x102, 2)
        .unwrap()
        .copy_from_slice(&instruction.to_le_bytes());

    let mut cpu = Thumb::new(Some(0));
    cpu.thumb2 = true;
    let mut registers = [0; 32];
    let (sp, mut pc) = cpu.reset(&bus).unwrap().unwrap();
    registers[13] = sp;
    let mut output = std::io::sink();
    cpu.step(&mut pc, &mut registers, &mut bus, &mut output)
        .unwrap();
    assert_eq!(pc, 0x20);

    bus.write(0xe000_ed04, 4, 1 << 31, &mut output).unwrap();
    assert!(
        cpu.service_interrupts(&mut pc, &mut registers, &mut bus)
            .unwrap()
    );
    cpu.step(&mut pc, &mut registers, &mut bus, &mut output)
        .unwrap();
    assert_eq!(pc, 0x102);
    cpu.step(&mut pc, &mut registers, &mut bus, &mut output)
        .unwrap();
    assert_eq!(pc, expected_pc);
    assert_eq!(cpu.active_exception, active);
}

#[test]
fn should_return_from_exception_when_using_bx_lr() {
    check_nmi_branch(0x4770, 0x20, 0);
}

#[test]
fn upstream_blx_lr_does_not_return_from_nmi() {
    // Cortex-M4 enters from Thread/MSP with LR=0xfffffff9.
    check_nmi_branch(0x47f0, 0xffff_fff8, NMI);
}

#[test]
fn upstream_bx_exc_return_like_address_does_not_return_from_thread() {
    let mut bus = Bus::default();
    bus.map_ram(0, 0x1000).unwrap();
    bus.map_nvic(0xc0, None, true).unwrap();
    let mut out = std::io::sink();
    for (address, value) in [(0, 0x800), (4, 0x21)] {
        bus.write(address, 4, value, &mut out).unwrap();
    }
    // mov r0, 0xffffffb8; bx r0
    for (address, opcode) in [(0x20, 0xf06f), (0x22, 0x0047), (0x24, 0x4700)] {
        bus.write(address, 2, opcode, &mut out).unwrap();
    }
    let mut cpu = Thumb::new(Some(0));
    cpu.thumb2 = true;
    let (sp, mut pc) = cpu.reset(&bus).unwrap().unwrap();
    let mut registers = [0; 32];
    registers[13] = sp;
    for _ in 0..2 {
        assert_eq!(
            cpu.step(&mut pc, &mut registers, &mut bus, &mut out)
                .unwrap(),
            ThumbStep::Retired
        );
    }
    assert_eq!(pc, 0xffff_ffb8);
    assert!(cpu.invalid_thumb_state);
    assert_eq!(cpu.active_exception, 0);
    assert_eq!(registers[13], 0x800);
}

#[test]
fn should_not_return_from_insecure_mode_when_not_using_trustzone() {
    let mut bus = Bus::default();
    bus.map_ram(0, 0x1000).unwrap();
    bus.map_nvic(0xc0, None, true).unwrap();
    for (address, value) in [(0, 0x800u32), (4, 0x21)] {
        bus.memory(address, 4)
            .unwrap()
            .copy_from_slice(&value.to_le_bytes());
    }
    bus.memory(0x20, 6)
        .unwrap()
        .copy_from_slice(&[0x6f, 0xf0, 0x80, 0x70, 0x00, 0x47]); // mov r0, 0xfeffffff; bx r0
    let mut cpu = Thumb::new(Some(0));
    cpu.thumb2 = true;
    let (sp, mut pc) = cpu.reset(&bus).unwrap().unwrap();
    let mut registers = [0; 32];
    registers[13] = sp;
    for _ in 0..2 {
        cpu.step(&mut pc, &mut registers, &mut bus, &mut std::io::sink())
            .unwrap();
    }
    assert_eq!(pc, 0xfeff_fffe);
}
