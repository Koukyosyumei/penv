//! Cortex-M4 encodings and R4 assertions from Renode tests/unit-tests/arm-ge-flag.robot.

use super::*;

// Signed Byte Addition Should Set GE Flag If And Only If The Result Is Greater Or Equal To Zero (sadd8)
#[test]
fn sadd8() {
    check(
        &[
            0xf240, 0x0000, 0xf2c0, 0x0000, 0xf64f, 0x71ff, 0xf6cf, 0x71ff, 0xf640, 0x420a, 0xf6c0,
            0x420a, 0xf24f, 0x53f5, 0xf2cf, 0x53f5, 0xfa82, 0xf203, 0xfaa1, 0xf480,
        ],
        0xff00ff00,
    );
}

// Signed Word Addition Should Set GE Flag If And Only If The Result Is Greater Or Equal To Zero (sadd16)
#[test]
fn sadd16() {
    check(
        &[
            0xf240, 0x0000, 0xf2c0, 0x0000, 0xf64f, 0x71ff, 0xf6cf, 0x71ff, 0xf640, 0x2223, 0xf6c0,
            0x2225, 0xf24f, 0x1323, 0xf6cf, 0x23a2, 0xfa92, 0xf203, 0xfaa1, 0xf480,
        ],
        0xffff0000,
    );
}

// Signed Byte Subtraction Should Set GE Flag If And Only If The Result Is Greater Or Equal To Zero (ssub8)
#[test]
fn ssub8() {
    check(
        &[
            0xf240, 0x0000, 0xf2c0, 0x0000, 0xf64f, 0x71ff, 0xf6cf, 0x71ff, 0xf640, 0x220c, 0xf6c0,
            0x220c, 0xf640, 0x330b, 0xf6c0, 0x330b, 0xfac2, 0xf203, 0xfaa1, 0xf480,
        ],
        0x00ff00ff,
    );
}

// Signed Word Subtraction Should Set GE Flag If And Only If The Result Is Greater Or Equal To Zero (ssub16)
#[test]
fn ssub16() {
    check(
        &[
            0xf240, 0x0000, 0xf2c0, 0x0000, 0xf64f, 0x71ff, 0xf6cf, 0x71ff, 0xf640, 0x2225, 0xf6c0,
            0x2223, 0xf640, 0x2324, 0xf6c0, 0x2324, 0xfad2, 0xf203, 0xfaa1, 0xf480,
        ],
        0x0000ffff,
    );
}

// Unsigned Byte Subtraction Should Set GE Flag If And Only If The Result Is Greater Or Equal To Zero (usub8)
#[test]
fn usub8() {
    check(
        &[
            0xf240, 0x0000, 0xf2c0, 0x0000, 0xf64f, 0x71ff, 0xf6cf, 0x71ff, 0xf640, 0x220c, 0xf6c0,
            0x420a, 0xf640, 0x330b, 0xf6c0, 0x330b, 0xfac2, 0xf203, 0xfaa1, 0xf480,
        ],
        0xff0000ff,
    );
}

// Unsigned Word Subtraction Should Set GE Flag If And Only If The Result Is Greater Or Equal To Zero (usub16)
#[test]
fn usub16() {
    check(
        &[
            0xf240, 0x0000, 0xf2c0, 0x0000, 0xf64f, 0x71ff, 0xf6cf, 0x71ff, 0xf640, 0x2225, 0xf6c0,
            0x2223, 0xf640, 0x2324, 0xf6c0, 0x2324, 0xfad2, 0xf243, 0xfaa1, 0xf480,
        ],
        0x0000ffff,
    );
}

fn check(instructions: &[u16; 20], expected: u32) {
    let mut bus = Bus::default();
    bus.map_ram(0, 0x1000).unwrap();
    for (slot, instruction) in bus
        .memory(0, 40)
        .unwrap()
        .chunks_exact_mut(2)
        .zip(instructions)
    {
        slot.copy_from_slice(&instruction.to_le_bytes());
    }
    let mut cpu = Thumb::new(Some(0));
    cpu.thumb2 = true;
    let mut pc = 0;
    let mut registers = [0; 32];
    for _ in 0..10 {
        cpu.step(&mut pc, &mut registers, &mut bus, &mut std::io::sink())
            .unwrap();
    }
    assert_eq!(registers[4], expected);
}
