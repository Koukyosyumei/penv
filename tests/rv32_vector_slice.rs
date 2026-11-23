//! Standalone donor tests. No machine, CSR dispatch or REPL registration required.
#[path = "../src/rv32_vector.rs"]
mod rv32_vector;
#[path = "../src/vector_bootstrap.rs"]
mod vector_bootstrap;

use rv32_vector::{Rv32Vector, TransferError};

const VLE8_V12_A0: u32 = 0x0205_0607;
const VSE8_V12_A0: u32 = 0x0205_0627;

fn configure(cpu: &mut Rv32Vector, length: u32, sew: u32) {
    // vsetivli t0, length, sew, m1, tu, mu, as in Configure Vector.
    let opcode = 0xc000_72d7 | (sew << 23) | (length << 15);
    assert_eq!(cpu.csrs.configure(opcode, 0, 0), Some(length));
}

#[test]
fn act_v_enable_vlenb_and_robot_vlen() {
    let act = Rv32Vector::new(1024).unwrap();
    assert_eq!(act.csrs.read(0xc22), Some(128));
    let mut robot = Rv32Vector::new(128).unwrap();
    assert_eq!(robot.csrs.read(0xc22), Some(16));
    assert!(Rv32Vector::new(192).is_none());
    configure(&mut robot, 3, 0);
    assert!(robot.csrs.write(0x008, 0xffff));
    assert_eq!(robot.csrs.read(0x008), Some(127));
    configure(&mut robot, 3, 0);
    assert_eq!(robot.csrs.read(0x008), Some(0));
}

#[test]
fn andes_fixture_standard_vector_setup_and_readback() {
    // Set Vector Register and Vector Should Contain in riscv-andestar-v5.robot.
    for (sew, funct3, width, values) in [
        (0, 0, 1, [0xaa, 0xaa, 0xaa]),
        (1, 5, 2, [0x3f80, 0x8000, 0x7f81]),
        (2, 6, 4, [0x3f808000, 0x7f800001, 0x12345678]),
    ] {
        let mut cpu = Rv32Vector::new(128).unwrap();
        configure(&mut cpu, 3, sew);
        cpu.transfer(VLE8_V12_A0 | (funct3 << 12), 0x4000, |addr, size, value| {
            assert_eq!((size, value), (width, None));
            Ok::<_, ()>(values[(addr as usize - 0x4000) / width])
        })
        .unwrap();
        let mut output = Vec::new();
        cpu.transfer(VSE8_V12_A0 | (funct3 << 12), 0x5000, |addr, size, value| {
            assert_eq!(size, width);
            assert_eq!(addr, 0x5000 + (output.len() * width) as u32);
            output.push(value.unwrap());
            Ok::<_, ()>(0)
        })
        .unwrap();
        assert_eq!(output, values);
    }
}

#[test]
fn memory_fault_keeps_vstart_and_completed_elements() {
    let mut cpu = Rv32Vector::new(128).unwrap();
    configure(&mut cpu, 3, 0);
    let fault = cpu.transfer(VLE8_V12_A0, 0x4000, |addr, _, _| {
        if addr == 0x4001 { Err(addr) } else { Ok(0xaa) }
    });
    assert_eq!(fault, Err(TransferError::Memory(0x4001)));
    assert_eq!(cpu.csrs.read(0x008), Some(1));
    cpu.transfer(VLE8_V12_A0, 0x4000, |addr, _, _| {
        assert_ne!(addr, 0x4000);
        Ok::<_, ()>(addr & 0xff)
    })
    .unwrap();
    assert_eq!(cpu.csrs.read(0x008), Some(0));
    let mut output = Vec::new();
    cpu.transfer(VSE8_V12_A0, 0x5000, |_, _, value| {
        output.push(value.unwrap());
        Ok::<_, ()>(0)
    })
    .unwrap();
    assert_eq!(output, [0xaa, 1, 2]);
}

#[test]
fn masked_lanes_leave_memory_untouched_and_special_encodings_trap() {
    let mut cpu = Rv32Vector::new(128).unwrap();
    configure(&mut cpu, 3, 0);
    cpu.transfer(VLE8_V12_A0 & !(31 << 7), 0, |_, _, _| Ok::<_, ()>(0b101))
        .unwrap();
    let mut addresses = Vec::new();
    cpu.transfer(VSE8_V12_A0 & !(1 << 25), 0x5000, |addr, _, _| {
        addresses.push(addr);
        Ok::<_, ()>(0)
    })
    .unwrap();
    assert_eq!(addresses, [0x5000, 0x5002]);
    for opcode in [
        VLE8_V12_A0 | (1 << 28),
        VLE8_V12_A0 & !((31 << 7) | (1 << 25)),
    ] {
        assert_eq!(
            cpu.transfer(opcode, 0, |_, _, _| -> Result<u32, ()> {
                panic!("illegal instruction attempted a memory access")
            }),
            Err(TransferError::Illegal)
        );
    }
}

// PackedInt4.cs uses funct7 4/5 for masked/unmasked and rs2=2/3.
const VLN8_V12_A0: u32 = 0x0a25_465b;
const VLNU8_V12_A0: u32 = 0x0a35_465b;

fn int4_fixture(opcode: u32, packed: &[u8], start: u32, vl: u32, expected: &[u8]) {
    let mut cpu = Rv32Vector::new(128).unwrap();
    configure(&mut cpu, expected.len() as u32, 0);
    cpu.transfer(VLE8_V12_A0, 0x4000, |_, _, _| Ok::<_, ()>(0xaa))
        .unwrap();
    if opcode & (1 << 25) == 0 {
        cpu.transfer(VLE8_V12_A0 & !(31 << 7), 0x4000, |_, _, _| Ok::<_, ()>(1))
            .unwrap();
    }
    configure(&mut cpu, vl, 0);
    assert!(cpu.csrs.write(0x008, start));
    cpu.andes_int4_load(opcode, 0x4000, |addr| {
        Ok::<_, ()>(packed[(addr - 0x4000) as usize])
    })
    .unwrap();
    assert_eq!(cpu.csrs.read(0x008), Some(0));
    configure(&mut cpu, expected.len() as u32, 0);
    let mut output = Vec::new();
    cpu.transfer(VSE8_V12_A0, 0x5000, |_, _, value| {
        output.push(value.unwrap() as u8);
        Ok::<_, ()>(0)
    })
    .unwrap();
    assert_eq!(output, expected);
}

#[test]
fn should_restart_int4_byte_loads_at_an_odd_element() {
    int4_fixture(VLN8_V12_A0, &[0x81, 0x7f], 1, 3, &[0xaa, 0xf8, 0xff, 0xaa]);
}

#[test]
fn should_execute_vln8_v() {
    for (packed, expected) in [(0xf8, [0xf8, 0xff]), (0x70, [0, 7]), (0x91, [1, 0xf9])] {
        int4_fixture(VLN8_V12_A0, &[packed], 0, 2, &expected);
    }
    int4_fixture(VLN8_V12_A0 & !(1 << 25), &[0xf8], 0, 2, &[0xf8, 0xaa]);
}

#[test]
fn should_execute_vlnu8_v() {
    for (packed, expected) in [(0xf8, [8, 15]), (0x70, [0, 7]), (0x91, [1, 9])] {
        int4_fixture(VLNU8_V12_A0, &[packed], 0, 2, &expected);
    }
    int4_fixture(VLNU8_V12_A0 & !(1 << 25), &[0xf8], 0, 2, &[8, 0xaa]);
}
