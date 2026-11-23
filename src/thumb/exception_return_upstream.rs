//! Applicable Cortex-M cases from Renode arm-cortex-m-exception-return.robot.

use super::*;

#[test]
fn should_return_from_exception_when_using_bx_lr() {
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
    bus.memory(0x100, 4)
        .unwrap()
        .copy_from_slice(&[0x00, 0xbf, 0x70, 0x47]); // nop; bx lr

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
    assert_eq!(pc, 0x20);
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
