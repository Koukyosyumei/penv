use std::{
    fs,
    io::{Read, Write},
    process::{Command, Stdio},
};

#[test]
fn runs_raw_and_elf_and_reports_faults() {
    let directory = std::env::temp_dir().join(format!("penv-cli-{}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    let raw = directory.join("uart.bin");
    let elf = directory.join("uart.elf");
    let instructions: [u32; 6] = [
        0x1000_02b7, // lui t0, 0x10000
        0x0410_0313, // addi t1, zero, 65
        0x0062_8023, // sb t1, 0(t0)
        0xfff3_0313, // addi t1, t1, -1
        0xfe03_1ce3, // bne t1, zero, -8
        0x0010_0073, // ebreak
    ];
    let program: Vec<u8> = instructions
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect();
    fs::write(&raw, &program).unwrap();

    let mut header = [0; 84];
    header[..7].copy_from_slice(b"\x7fELF\x01\x01\x01");
    for (offset, value) in [(16, 2u16), (18, 243), (40, 52), (42, 32), (44, 1)] {
        header[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }
    for (offset, value) in [
        (20, 1u32),
        (24, 0x8000_0000),
        (28, 52),
        (52, 1),
        (56, 84),
        (60, 0x8000_0000),
        (64, 0x8000_0000),
        (68, program.len() as u32),
        (72, program.len() as u32 + 4),
        (76, 5),
        (80, 4),
    ] {
        header[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    fs::write(&elf, [header.as_slice(), &program].concat()).unwrap();

    let command = || {
        let mut command = Command::new(env!("CARGO_BIN_EXE_penv"));
        command.args(["run", "--ram-mib", "1"]);
        command
    };
    let raw_output = command().arg("--raw").arg(&raw).output().unwrap();
    let elf_output = command().arg("--stats").arg(&elf).output().unwrap();
    let limited = command()
        .args(["--max-steps", "3"])
        .arg(&elf)
        .output()
        .unwrap();
    for offset in [24, 60, 64] {
        header[offset..offset + 4].copy_from_slice(&0x1000u32.to_le_bytes());
    }
    fs::write(&elf, [header.as_slice(), &program].concat()).unwrap();
    let mapped = command()
        .args(["--map-ram", "0x1000:4096"])
        .arg(&elf)
        .output()
        .unwrap();
    let unmapped = command().arg(&elf).output().unwrap();
    let overlapping = command()
        .args(["--map-ram", "0x80000000:4096"])
        .arg(&elf)
        .output()
        .unwrap();
    fs::write(&elf, header).unwrap();
    let truncated = command().arg(&elf).output().unwrap();
    fs::write(&raw, u32::MAX.to_le_bytes()).unwrap();
    let illegal = command().arg("--raw").arg(&raw).output().unwrap();
    fs::remove_dir_all(&directory).unwrap();

    assert!(raw_output.status.success(), "{raw_output:?}");
    assert!(elf_output.status.success(), "{elf_output:?}");
    let expected: Vec<_> = (1..=65).rev().collect();
    assert_eq!(raw_output.stdout, expected);
    assert_eq!(elf_output.stdout, expected);
    assert!(mapped.status.success(), "{mapped:?}");
    assert_eq!(mapped.stdout, expected);
    assert_eq!(unmapped.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&unmapped.stderr).contains("unmapped RAM access"));
    assert_eq!(overlapping.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&overlapping.stderr).contains("overlapping RAM range"));
    assert!(String::from_utf8_lossy(&elf_output.stderr).contains("Ebreak, 197 instructions"));
    assert_eq!(limited.status.code(), Some(2));
    assert_eq!(limited.stdout, b"A");
    assert_eq!(truncated.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&truncated.stderr).contains("invalid ELF segment"));
    assert_eq!(illegal.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&illegal.stderr).contains("pc=0x80000000"));
}

#[test]
fn loads_platform_includes_and_uses_declared_memory() {
    let directory = std::env::temp_dir().join(format!("penv-platform-{}", std::process::id()));
    fs::create_dir_all(directory.join("platforms")).unwrap();
    let board = directory.join("platforms/board.repl");
    let devices = directory.join("cpu.repl");
    let firmware = directory.join("firmware.bin");
    fs::write(&board, "using \"cpu.repl\" // relative to platform root\n").unwrap();
    fs::write(
        &devices,
        "cpu: CPU.RiscV32 @ sysbus
    cpuType: \"rv32i\"
    timeProvider: clint
ram: Memory.MappedMemory @ sysbus 0x8000_0000
    size: 0x2000
clint: IRQControllers.CoreLevelInterruptor @ sysbus 0x0200_0000
    [0, 1] -> cpu@[3, 7]
    frequency: 10000000
",
    )
    .unwrap();
    fs::write(&firmware, 0x0010_0073u32.to_le_bytes()).unwrap();
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_penv"))
            .args(["run", "--raw", "--platform"])
            .arg(&board)
            .arg("--platform-root")
            .arg(&directory)
            .arg(&firmware)
            .output()
            .unwrap()
    };
    let success = run();
    fs::write(&firmware, 0x0220_81b3u32.to_le_bytes()).unwrap();
    let disabled_extension = run();
    fs::write(&devices, "using \"platforms/board.repl\"\n").unwrap();
    let cyclic = run();
    fs::remove_dir_all(directory).unwrap();

    assert!(success.status.success(), "{success:?}");
    assert!(
        String::from_utf8_lossy(&disabled_extension.stderr).contains("unsupported instruction")
    );
    assert!(
        String::from_utf8_lossy(&cyclic.stderr)
            .contains("recursive or excessively deep platform include")
    );
}

#[test]
fn uses_repl_uart_to_echo_input_to_stdout() {
    let directory = std::env::temp_dir().join(format!("penv-uart-{}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    let board = directory.join("board.repl");
    let raw = directory.join("echo.bin");
    let input = directory.join("input");
    fs::write(
        &board,
        "
cpu: CPU.VexRiscv @ sysbus
    cpuType: \"rv32im_zicsr_zifencei\"
ram: Memory.MappedMemory @ sysbus 0x80000000
    size: 0x1000
uart: UART.Murax_UART @ sysbus 0xf0010000
    IRQ -> cpu@11
",
    )
    .unwrap();
    let program = [
        0xf001_02b7u32, // lui t0, 0xf0010
        0x0002_a303,    // lw t1, 0(t0)
        0x0062_a023,    // sw t1, 0(t0)
        0x0010_0073,    // ebreak
    ];
    fs::write(
        &raw,
        program
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect::<Vec<_>>(),
    )
    .unwrap();
    fs::write(&input, b"Q").unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_penv"))
        .args(["run", "--raw", "--platform"])
        .arg(&board)
        .arg("--uart-input")
        .arg(&input)
        .args(["--uart-input-at", "2", "--vex-irq-mask", "0xffffffff"])
        .arg(&raw)
        .output()
        .unwrap();

    let interactive_program = [
        0xf001_02b7u32, // lui t0, 0xf0010
        0x03e0_0313,    // addi t1, zero, '>'
        0x0062_a023,    // sw t1, 0(t0)
        0x0042_a303,    // lw t1, 4(t0)
        0x2003_7313,    // andi t1, t1, 0x200
        0xfe03_0ce3,    // beq t1, zero, -8
        0x0002_a303,    // lw t1, 0(t0)
        0x0062_a023,    // sw t1, 0(t0)
        0x0010_0073,    // ebreak
    ];
    fs::write(
        &raw,
        interactive_program
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect::<Vec<_>>(),
    )
    .unwrap();
    assert!(result.status.success(), "{result:?}");
    assert_eq!(result.stdout, b"Q");
    for reset in [false, true] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_penv"));
        command.args(["run", "--raw", "--uart-stdin"]);
        if reset {
            command.arg("--uart-stdin-reset");
        }
        let mut child = command
            .arg("--platform")
            .arg(&board)
            .arg(&raw)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.as_mut().unwrap();
        let mut prompt = [0];
        stdout.read_exact(&mut prompt).unwrap();
        assert_eq!(prompt, [b'>']);
        if reset {
            stdin.write_all(b"\x1dr").unwrap();
            stdout.read_exact(&mut prompt).unwrap();
            assert_eq!(prompt, [b'>']);
        }
        stdin.write_all(b"R").unwrap();
        drop(stdin);
        let interactive = child.wait_with_output().unwrap();
        assert!(interactive.status.success(), "{interactive:?}");
        assert_eq!(interactive.stdout, b"R");
    }
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn runs_cortex_m0_from_vector_table_and_reports_invalid_thumb_state() {
    let directory = std::env::temp_dir().join(format!("penv-cortex-m-{}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    let board = directory.join("board.repl");
    let firmware = directory.join("thumb.bin");
    fs::write(
        &board,
        r#"cpu: CPU.CortexM @ sysbus
    cpuType: "cortex-m0"
    vectorTableOffset: 0x08000000
flash: Memory.MappedMemory @ sysbus 0x08000000
    size: 0x1000
sram: Memory.MappedMemory @ sysbus 0x20000000
    size: 0x1000
uart: UART.LiteX_UART @ sysbus 0x40000000
"#,
    )
    .unwrap();
    let mut image = vec![];
    image.extend_from_slice(&0x2000_1000u32.to_le_bytes());
    image.extend_from_slice(&0x0800_0009u32.to_le_bytes());
    image.extend_from_slice(&[
        0x3f, 0x20, // movs r0, #63
        0x01, 0x30, // adds r0, #1
        0x41, 0x28, // cmp r0, #65
        0xfc, 0xd1, // bne #-8
        0x01, 0x49, // ldr r1, [pc, #4]
        0x08, 0x70, // strb r0, [r1]
        0x00, 0xbe, // bkpt
        0x00, 0x00, // padding
    ]);
    image.extend_from_slice(&0x4000_0000u32.to_le_bytes());
    fs::write(&firmware, &image).unwrap();
    let invoke = || {
        Command::new(env!("CARGO_BIN_EXE_penv"))
            .args(["run", "--raw", "--platform"])
            .arg(&board)
            .arg(&firmware)
            .output()
            .unwrap()
    };
    let output = invoke();
    image[4] &= !1;
    fs::write(&firmware, image).unwrap();
    let invalid = invoke();
    let mut subroutine_image = Vec::new();
    subroutine_image.extend_from_slice(&0x2000_1000u32.to_le_bytes());
    subroutine_image.extend_from_slice(&0x0800_0009u32.to_le_bytes());
    subroutine_image.extend_from_slice(&[
        0x00, 0xf0, 0x04, 0xf8, // bl #8
        0x05, 0x49, // ldr r1, [pc, #20]
        0x08, 0x70, // strb r0, [r1]
        0x00, 0xbe, // bkpt
        0x00, 0xbf, // nop
        0x02, 0xb5, // push {r1, lr}
        0x81, 0xb0, // sub sp, #4
        0x41, 0x20, // movs r0, #65
        0x00, 0x90, // str r0, [sp]
        0x00, 0x98, // ldr r0, [sp]
        0x01, 0xb0, // add sp, #4
        0x02, 0xbd, // pop {r1, pc}
        0x00, 0x00, // padding
    ]);
    subroutine_image.extend_from_slice(&0x4000_0000u32.to_le_bytes());
    fs::write(&firmware, subroutine_image).unwrap();
    let subroutine = invoke();
    let elf_path = directory.join("thumb.elf");
    let flash = 0x0800_0000u32;
    let lma = flash + 0x80;
    let mut payload = Vec::new();
    payload.extend_from_slice(&0x2000_1000u32.to_le_bytes());
    payload.extend_from_slice(&(flash + 9).to_le_bytes());
    payload.extend_from_slice(&[
        0x02, 0x49, // ldr r1, [pc, #8] -> physical address of initialized data
        0x08, 0x68, // ldr r0, [r1]
        0x02, 0x49, // ldr r1, [pc, #8] -> UART address
        0x08, 0x70, // strb r0, [r1]
        0x00, 0xbe, // bkpt
        0x00, 0x00, // padding
    ]);
    payload.extend_from_slice(&lma.to_le_bytes());
    payload.extend_from_slice(&0x4000_0000u32.to_le_bytes());
    let mut header = [0; 116];
    header[..7].copy_from_slice(b"\x7fELF\x01\x01\x01");
    for (offset, value) in [(16, 2u16), (18, 40), (40, 52), (42, 32), (44, 2)] {
        header[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }
    let payload_size = payload.len() as u32;
    for (offset, value) in [
        (20, 1u32),
        (24, 0),
        (28, 52),
        (36, 0x0500_0200),
        (52, 1),
        (56, 116),
        (60, flash),
        (64, flash),
        (68, payload_size),
        (72, payload_size),
        (76, 5),
        (80, 4),
        (84, 1),
        (88, 116 + payload_size),
        (92, 0x2000_0000),
        (96, lma),
        (100, 4),
        (104, 0x1000),
        (108, 6),
        (112, 4),
    ] {
        header[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    fs::write(
        &elf_path,
        [header.as_slice(), &payload, b"E\0\0\0"].concat(),
    )
    .unwrap();
    let arm_elf = Command::new(env!("CARGO_BIN_EXE_penv"))
        .args(["run", "--platform"])
        .arg(&board)
        .arg(&elf_path)
        .output()
        .unwrap();
    fs::remove_dir_all(directory).unwrap();

    assert!(output.status.success(), "{output:?}");
    assert_eq!(output.stdout, b"A");
    assert!(subroutine.status.success(), "{subroutine:?}");
    assert_eq!(subroutine.stdout, b"A");
    assert!(arm_elf.status.success(), "{arm_elf:?}");
    assert_eq!(arm_elf.stdout, b"E");
    assert!(!invalid.status.success());
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("must select Thumb state"));
}

#[test]
fn cortex_m_repl_routes_uart_input_through_nvic() {
    let directory = std::env::temp_dir().join(format!("penv-nvic-{}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    let board = directory.join("board.repl");
    let firmware = directory.join("firmware.bin");
    let input = directory.join("input");
    fs::write(&input, b"I").unwrap();
    fs::write(
        &board,
        r#"nvic: IRQControllers.NVIC @ sysbus 0xe000e000 { -> cpu@0; priorityMask: 0xc0 }
cpu: CPU.CortexM @ sysbus { cpuType: "cortex-m0+"; nvic: nvic }
flash: Memory.MappedMemory @ sysbus 0 { size: 0x1000 }
sram: Memory.MappedMemory @ sysbus 0x20000000
    size: 0x1000
uart: UART.LiteX_UART @ sysbus 0x40000000
    IRQ -> nvic@3
"#,
    )
    .unwrap();
    let mut image = [0u8; 0xaa];
    for (offset, value) in [
        (0, 0x2000_1000u32),
        (4, 0x81),
        (76, 0xa1),
        (0x90, 0xe000_e100),
        (0x94, 0x4000_0000),
    ] {
        image[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    image[0x80..0x8e].copy_from_slice(&[
        0x03, 0x48, 0x08, 0x21, 0x01, 0x60, // Enable NVIC line 3.
        0x03, 0x48, 0x02, 0x21, 0x41, 0x61, // Enable UART RX events.
        0x00, 0xbe, // Stop after IRQ return.
    ]);
    image[0xa0..0xaa].copy_from_slice(&[
        0x01, 0x68, 0x01, 0x60, // Receive and echo a byte.
        0x02, 0x21, 0x01, 0x61, // Acknowledge RX event.
        0x70, 0x47, // Exception return.
    ]);
    fs::write(&firmware, image).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_penv"))
        .args(["run", "--raw", "--max-steps", "32", "--platform"])
        .arg(&board)
        .arg("--uart-input")
        .arg(&input)
        .arg(&firmware)
        .output()
        .unwrap();
    fs::remove_dir_all(directory).unwrap();
    assert!(result.status.success(), "{result:?}");
    assert_eq!(result.stdout, b"I");
}

#[test]
fn starlight_gpio_reboots_firmware_within_the_same_step_budget() {
    let directory =
        std::env::temp_dir().join(format!("penv-starlight-reset-{}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    let board = directory.join("beaglev_starlight.repl");
    let raw = directory.join("gpio-reset.bin");
    fs::write(
        &board,
        r#"cpu: CPU.RiscV64 @ sysbus
    cpuType: "rv64i"
ram: Memory.MappedMemory @ sysbus 0x80000000
    size: 0x1000
uart: UART.NS16550 @ sysbus 0x10000000
gpio: GPIOPort.SiFive_GPIO @ sysbus 0x1191000
"#,
    )
    .unwrap();
    let instructions: [u32; 9] = [
        0x1000_02b7, // lui t0, 0x10000
        0x03e0_0313, // addi t1, zero, '>'
        0x0062_8023, // sb t1, 0(t0)
        0x1191_03b7, // lui t2, 0x11910
        0x0010_0313, // addi t1, zero, 1
        0x0a63_a023, // sw t1, 0xa0(t2): select high
        0x0a03_a223, // sw zero, 0xa4(t2): enable output
        0x0a03_a023, // sw zero, 0xa0(t2): drive reset low
        0x0010_0073, // ebreak, unreachable after reset
    ];
    fs::write(
        &raw,
        instructions
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_penv"))
        .args(["run", "--raw", "--max-steps", "19", "--platform"])
        .arg(&board)
        .arg(&raw)
        .output()
        .unwrap();
    fs::remove_dir_all(directory).unwrap();

    assert_eq!(result.status.code(), Some(2), "{result:?}");
    assert_eq!(result.stdout, b">>>");
}

#[test]
fn rv64_dtb_passes_hart_ids_and_a_64_bit_pointer() {
    let directory = std::env::temp_dir().join(format!("penv-boot-dtb-{}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    let raw = directory.join("boot.bin");
    let dtb = directory.join("boot.dtb");
    let instructions: [u32; 6] = [
        0x1000_02b7, // lui t0, 0x10000
        0x0305_0313, // addi t1, a0, '0'
        0x0062_8023, // sb t1, 0(t0)
        0x0005_c303, // lbu t1, 0(a1)
        0x0062_8023, // sb t1, 0(t0)
        0x0010_0073, // ebreak
    ];
    fs::write(
        &raw,
        instructions
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let cells: [u32; 18] = [
        0xd00d_feed,
        72,
        56,
        72,
        40,
        17,
        16,
        0,
        0,
        16, // Header
        0,
        0,
        0,
        0, // Reservation terminator
        1,
        0,
        2,
        9, // Empty root node
    ];
    let mut bytes = cells
        .into_iter()
        .flat_map(u32::to_be_bytes)
        .collect::<Vec<_>>();
    fs::write(&dtb, &bytes).unwrap();
    let invoke = |address: &str| {
        Command::new(env!("CARGO_BIN_EXE_penv"))
            .args(["run", "--raw", "--rv64", "--harts", "2", "--ram-mib", "1"])
            .args(["--map-ram", "0x100000000:4096", "--dtb"])
            .arg(format!("{address}:{}", dtb.display()))
            .arg(&raw)
            .output()
            .unwrap()
    };
    let success = invoke("0x100000000");
    let unaligned = invoke("0x100000001");
    bytes[0] = 0;
    fs::write(&dtb, bytes).unwrap();
    let invalid = invoke("0x100000000");
    fs::remove_dir_all(directory).unwrap();

    assert!(success.status.success(), "{success:?}");
    assert_eq!(success.stdout, [b'0', b'1', 0xd0, 0xd0]);
    assert!(String::from_utf8_lossy(&unaligned.stderr).contains("8-byte aligned"));
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("invalid DTB magic or size"));
}

#[test]
fn polarfire_hss_can_swap_the_cache_way_enable_register() {
    let directory =
        std::env::temp_dir().join(format!("penv-polarfire-cache-{}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    let board = directory.join("polarfire.repl");
    let raw = directory.join("cache-swap.bin");
    fs::write(
        &board,
        r#"cpu: CPU.RiscV64 @ sysbus
    cpuType: "rv64ia"
ram: Memory.MappedMemory @ sysbus 0x80000000
    size: 0x1000
uart: UART.NS16550 @ sysbus 0x10000000
CacheConfig_WayEnable: Python.PythonPeripheral @ sysbus 0x02010008
    size: 0x8
    initable: true
    script: '''
if request.IsInit:
    reg = 0x0
elif request.IsRead:
    request.Value = reg
elif request.IsWrite:
    reg = request.Value
'''
"#,
    )
    .unwrap();
    let instructions: [u32; 18] = [
        0x0201_02b7, // lui t0, 0x02010
        0x0082_8293, // addi t0, t0, 8
        0x1000_03b7, // lui t2, 0x10000
        0x0410_0313, // addi t1, zero, 'A'
        0x0203_1313, // slli t1, t1, 32
        0x0862_be2f, // amoswap.d t3, t1, (t0): reset value
        0x01c3_8023, // sb t3, 0(t2)
        0x0420_0313, // addi t1, zero, 'B'
        0x0862_be2f, // amoswap.d t3, t1, (t0): old value
        0x020e_5e13, // srli t3, t3, 32
        0x01c3_8023, // sb t3, 0(t2)
        0x0002_be03, // ld t3, 0(t0): current value
        0x01c3_8023, // sb t3, 0(t2)
        0x0430_0313, // addi t1, zero, 'C'
        0x0062_b023, // sd t1, 0(t0)
        0x0862_be2f, // amoswap.d t3, t1, (t0): value from sd
        0x01c3_8023, // sb t3, 0(t2)
        0x0010_0073, // ebreak
    ];
    let program: Vec<_> = instructions
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect();
    fs::write(&raw, program).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_penv"))
        .args(["run", "--raw", "--platform"])
        .arg(&board)
        .arg(&raw)
        .output()
        .unwrap();
    fs::remove_dir_all(directory).unwrap();

    assert!(result.status.success(), "{result:?}");
    assert_eq!(result.stdout, b"\0ABC");
}
