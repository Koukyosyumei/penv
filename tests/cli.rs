use std::{
    fs,
    io::{Read, Write},
    path::Path,
    process::{Command, Output, Stdio},
};

fn upstream_stm32f072_command(fixture: &str, limit: &str) -> Command {
    let root = std::env::var_os("PENV_RENODE_ROOT").expect("set PENV_RENODE_ROOT");
    let assets = std::env::var_os("PENV_RENODE_ASSETS").expect("set PENV_RENODE_ASSETS");
    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(fixture);
    let mut command = Command::new(env!("CARGO_BIN_EXE_penv"));
    command
        .args(["run", "--script"])
        .arg(script)
        .arg("--platform-root")
        .arg(root)
        .arg("--asset-dir")
        .arg(assets)
        .args(["--max-steps", limit]);
    command
}

fn run_upstream_stm32f072_script(fixture: &str, limit: &str) -> Output {
    upstream_stm32f072_command(fixture, limit).output().unwrap()
}

#[test]
#[ignore = "requires the upstream Renode tree and public STM32F072 Hello World images"]
fn upstream_stm32f072_hello_world_boots_elf_and_hex() {
    for fixture in ["stm32f072-hello-elf.resc", "stm32f072-hello-hex.resc"] {
        let result = run_upstream_stm32f072_script(fixture, "1000000");
        assert!(result.status.success(), "{fixture}: {result:?}");
        let uart = String::from_utf8(result.stdout).unwrap();
        assert_eq!(
            uart.lines().last(),
            Some("Hello World! stm32f072b_disco"),
            "{fixture}"
        );
    }
}

#[test]
#[ignore = "requires the upstream Renode tree and the public STM32F0 CRC ELF"]
fn upstream_stm32f072_crc_firmware_passes_840_cases() {
    let result = run_upstream_stm32f072_script("stm32f072-crc.resc", "150000000");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let uart = String::from_utf8(result.stdout).unwrap();
    assert_eq!(
        uart.lines().last(),
        Some("test result: ok. 840 passed; 0 failed")
    );
}

fn run_upstream_led_scenario(fixture: &str, limit: &str, states: &[&str]) -> String {
    let result = upstream_stm32f072_command(fixture, limit)
        .arg("--led-events")
        .output()
        .unwrap();
    assert!(result.status.success(), "{fixture}: {result:?}");
    let diagnostics = String::from_utf8(result.stderr).unwrap();
    let transitions: Vec<_> = diagnostics
        .lines()
        .filter_map(|line| line.strip_prefix("LED led "))
        .collect();
    assert_eq!(transitions, states, "{fixture}");
    String::from_utf8(result.stdout).unwrap()
}

#[test]
#[ignore = "requires the upstream Renode tree and the public STM32F072 Blinky ELF"]
fn upstream_stm32f072_blinky_pauses_while_systick_is_not_clocked() {
    run_upstream_led_scenario("stm32f072-blinky-clock.resc", "300000000", &["on", "off"]);
}

#[test]
#[ignore = "requires the upstream Renode tree and the public STM32F072 Blinky ELF"]
fn upstream_stm32f072_blinky_alternates_led_states() {
    run_upstream_led_scenario(
        "stm32f072-blinky.resc",
        "400000000",
        &["on", "off", "on", "off", "on", "off"],
    );
}

#[test]
#[ignore = "requires the upstream Renode tree and the public STM32F072 Button ELF"]
fn upstream_stm32f072_button_updates_led_and_reports_press() {
    let uart = run_upstream_led_scenario(
        "stm32f072-button.resc",
        "331000000",
        &["on", "off", "on", "off", "on", "off", "on"],
    );
    assert!(
        uart.lines().any(|line| line
            .strip_prefix("Button pressed at ")
            .is_some_and(|timestamp| timestamp.trim().parse::<u64>().is_ok())),
        "{uart}"
    );
}

#[test]
#[ignore = "requires the upstream Renode tree and the public STM32F072 ADC ELF"]
fn upstream_stm32f072_adc_waits_for_uart_between_voltage_changes() {
    let run = |limit| run_upstream_stm32f072_script("stm32f072-adc.resc", limit);

    let completed = run("120000000");
    assert!(completed.status.success(), "{completed:?}");
    let uart = String::from_utf8(completed.stdout).unwrap();
    let mut lines = uart.lines();
    assert!(lines.any(|line| line.contains("Booting Zephyr")), "{uart}");
    assert!(lines.any(|line| line == "ADC reading: 745"), "{uart}");
    assert_eq!(lines.next(), Some("ADC reading: 1489"), "{uart}");
    assert_eq!(lines.next(), None, "guest advanced past the final match");

    let timeout = run("1");
    assert_eq!(timeout.status.code(), Some(1), "{timeout:?}");
    let error = String::from_utf8(timeout.stderr).unwrap();
    assert!(
        error.contains(
            "stm32f072-adc.resc:4: UART line containing \"Booting Zephyr\" was not received on sysbus.usart1 (Limit)"
        ),
        "{error}"
    );
}

#[test]
#[ignore = "requires the upstream Renode tree and the public STM32F072 Console Echo ELF"]
fn upstream_stm32f072_console_echo_accepts_resc_uart_input() {
    let completed = run_upstream_stm32f072_script("stm32f072-echo.resc", "3000000");
    assert!(completed.status.success(), "{completed:?}");
    let uart = String::from_utf8(completed.stdout).unwrap();
    let mut lines = uart.lines();
    assert!(
        lines.any(|line| line == "Start typing characters to see them echoed back"),
        "{uart}"
    );
    assert_eq!(lines.next(), Some("Echo working?"), "{uart}");
    assert_eq!(lines.next(), None, "{uart}");
}

#[test]
#[ignore = "requires the upstream Renode tree and the public STM32F072 Console Echo ELF"]
fn upstream_stm32f072_console_echo_replies_after_uart_prompt() {
    let completed = run_upstream_stm32f072_script("stm32f072-prompt.resc", "3000000");
    assert!(completed.status.success(), "{completed:?}");
    let uart = String::from_utf8(completed.stdout).unwrap();
    assert!(
        uart.ends_with("Start typing characters to see them echoed back\r\nEcho working?\r\n"),
        "{uart}"
    );
}

#[test]
fn renode_resc_nvic_gpio_wakes_wfe_with_sevonpend() {
    let directory = std::env::temp_dir().join(format!("penv-resc-nvic-{}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    fs::write(
        directory.join("board.repl"),
        r#"cpu: CPU.CortexM @ sysbus
    cpuType: "cortex-m0"
    nvic: irqController
irqController: IRQControllers.NVIC @ sysbus 0xe000e000
    -> cpu@0
ram: Memory.MappedMemory @ sysbus 0
    size: 0x200
uart: UART.LiteX_UART @ sysbus 0x40000000
"#,
    )
    .unwrap();
    // Set SCR.SEVONPEND, execute WFE, and write W to the UART on waking.
    fs::write(
        directory.join("wfe.hex"),
        ":20000000000200000900000003481021016020BF02485721017000BE10ED00E0000000400B\n:00000001FF\n",
    )
    .unwrap();
    let script = directory.join("wfe.resc");
    let run = |gpio_command| {
        fs::write(
            &script,
            format!(
                "mach create\nmachine LoadPlatformDescription @board.repl\nsysbus LoadHEX @wfe.hex\nemulation RunFor \"0.0000001s\"\n{gpio_command}\nsysbus.irqController OnGPIO 5 false\nwaitForUartPrompt \"W\"\n"
            ),
        )
        .unwrap();
        Command::new(env!("CARGO_BIN_EXE_penv"))
            .args(["run", "--max-steps", "20", "--script"])
            .arg(&script)
            .output()
            .unwrap()
    };
    let pulse = run("sysbus.irqController OnGPIO 5 true");
    let inactive = run("sysbus.irqController OnGPIO 5 false");
    let wrong_target = run("sysbus.nvic OnGPIO 5 true");
    fs::remove_dir_all(directory).unwrap();
    assert!(pulse.status.success(), "{pulse:?}");
    assert_eq!(pulse.stdout, b"W");
    assert_eq!(inactive.status.code(), Some(1), "{inactive:?}");
    assert!(inactive.stdout.is_empty());
    assert!(String::from_utf8_lossy(&inactive.stderr).contains("(Limit)"));
    assert_eq!(wrong_target.status.code(), Some(1), "{wrong_target:?}");
    assert!(
        String::from_utf8_lossy(&wrong_target.stderr).contains("wfe.resc:5: unknown Cortex-M NVIC")
    );
}

#[test]
fn waits_for_cr_lines_and_unterminated_uart_prompts() {
    let directory = std::env::temp_dir().join(format!("penv-resc-cr-{}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    let script = directory.join("boot.resc");
    fs::write(
        directory.join("board.repl"),
        r#"cpu: CPU.CortexM @ sysbus
    cpuType: "cortex-m0"
flash: Memory.MappedMemory @ sysbus 0x08000000
    size: 0x1000
usart1: UART.STM32F7_USART @ sysbus 0x40013800
"#,
    )
    .unwrap();
    // Thumb guest enables USART1, sends A and CR, then executes BKPT.
    fs::write(
        directory.join("firmware.hex"),
        concat!(
            ":020000040800F2\n",
            ":1C0000000010002009000008034809210160412181620D21816200BE0038014040\n",
            ":00000001FF\n",
        ),
    )
    .unwrap();
    let run = |wait: &str, limit: &str| {
        fs::write(
            &script,
            format!(
                "mach create\nmachine LoadPlatformDescription @board.repl\nsysbus LoadHEX @firmware.hex\n{wait}\n"
            ),
        )
        .unwrap();
        Command::new(env!("CARGO_BIN_EXE_penv"))
            .args(["run", "--stats", "--max-steps", limit, "--script"])
            .arg(&script)
            .output()
            .unwrap()
    };
    let selected = run("waitForUart sysbus.usart1 \"A\" CR", "10");
    let prompt = run("waitForUartPrompt sysbus.usart1 \"A\"", "10");
    let merged_prompt = run("waitForUartPrompt \"A\"", "10");
    let buffered_prompt = run(
        "emulation RunFor \"0.00000005s\"\nwaitForUartPrompt sysbus.usart1 \"A\"",
        "5",
    );
    let merged = run("waitForUart \"A\" CR", "10");
    let default = run("waitForUart sysbus.usart1 \"A\"", "10");
    let backlog = "emulation RunFor \"0.00000007s\"\nwaitForUart sysbus.usart1 \"A\" CR";
    let buffered = run(backlog, "7");
    let independent = run(&format!("{backlog}\nwaitForUart \"A\" CR"), "7");
    let consumed = run(
        &format!("{backlog}\nwaitForUart sysbus.usart1 \"A\" CR"),
        "7",
    );
    fs::remove_dir_all(directory).unwrap();
    for result in [selected, merged, buffered, independent] {
        assert!(result.status.success(), "{result:?}");
        assert_eq!(result.stdout, b"A\r");
        assert!(String::from_utf8_lossy(&result.stderr).contains("ScriptComplete"));
    }
    for result in [prompt, merged_prompt, buffered_prompt] {
        assert!(result.status.success(), "{result:?}");
        assert_eq!(result.stdout, b"A");
        assert!(String::from_utf8_lossy(&result.stderr).contains("ScriptComplete, 5 instructions"));
    }
    assert_eq!(default.status.code(), Some(1), "{default:?}");
    assert!(String::from_utf8_lossy(&default.stderr).contains("(Ebreak)"));
    assert_eq!(consumed.status.code(), Some(1), "{consumed:?}");
    assert!(String::from_utf8_lossy(&consumed.stderr).contains("boot.resc:6:"));
    assert!(String::from_utf8_lossy(&consumed.stderr).contains("(Limit)"));
}

#[test]
fn boots_resc_hex_and_runs_bounded_cortex_m_intervals() {
    let directory = std::env::temp_dir().join(format!("penv-resc-hex-{}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    let script = directory.join("boot.resc");
    let firmware = directory.join("firmware.hex");
    fs::write(
        directory.join("board.repl"),
        r#"cpu: CPU.CortexM @ sysbus
    cpuType: "cortex-m0"
    vectorTableOffset: 0x08000000
flash: Memory.MappedMemory @ sysbus 0x08000000
    size: 0x1000
"#,
    )
    .unwrap();
    // The vector table is at 0x08000080 and points to a BKPT.
    fs::write(
        &firmware,
        ":020000040800F2\n:0A008000001000208900000800BEF7\n:00000001FF\n",
    )
    .unwrap();
    fs::write(
        &script,
        concat!(
            "mach create\n",
            "machine LoadPlatformDescription @board.repl\n",
            "sysbus LoadHEX @https://example.org/firmware.hex\n",
            "sysbus.cpu VectorTableOffset 0x08000080\n",
        ),
    )
    .unwrap();
    let command = || {
        let mut command = Command::new(env!("CARGO_BIN_EXE_penv"));
        command.args(["run", "--script"]).arg(&script);
        command
    };
    let asset = command()
        .arg("--asset-dir")
        .arg(&directory)
        .output()
        .unwrap();
    let override_image = command().arg(&firmware).output().unwrap();
    assert!(asset.status.success(), "{asset:?}");
    assert!(override_image.status.success(), "{override_image:?}");

    // Replace BKPT with a self-branch to exercise the complete scripted budget.
    fs::write(
        &firmware,
        ":020000040800F2\n:0A0080000010002089000008FEE7D0\n:00000001FF\n",
    )
    .unwrap();
    let boot = fs::read_to_string(&script).unwrap();
    let phased = format!("{boot}emulation RunFor \"0.00000002s\"\nemulation RunFor '0.00000003'\n");
    fs::write(&script, &phased).unwrap();
    let timed = |max: &str| {
        command()
            .args(["--stats", "--max-steps", max])
            .arg(&firmware)
            .output()
            .unwrap()
    };
    let completed = timed("5");
    let limited = timed("4");
    fs::write(&script, format!("{phased}sysbus.gpioPortA.button Press\n")).unwrap();
    let unknown_button = timed("5");
    fs::remove_dir_all(directory).unwrap();

    assert!(completed.status.success(), "{completed:?}");
    assert!(String::from_utf8_lossy(&completed.stderr).contains("ScriptComplete, 5 instructions"));
    assert_eq!(limited.status.code(), Some(2), "{limited:?}");
    assert!(String::from_utf8_lossy(&limited.stderr).contains("Limit, 4 instructions"));
    assert_eq!(unknown_button.status.code(), Some(1), "{unknown_button:?}");
    assert!(
        String::from_utf8_lossy(&unknown_button.stderr)
            .contains("boot.resc:7: unknown STM32 board button")
    );
}

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
ram: Memory.MappedMemory @ {
    sysbus 0x8000_0000;
    sysbus 0xc000_0000 // shadow
}
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

    assert!(result.status.success(), "{result:?}");
    assert_eq!(result.stdout, b"Q");
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
    let reset = Command::new(env!("CARGO_BIN_EXE_penv"))
        .args([
            "run",
            "--raw",
            "--reset-at",
            "19",
            "--reset-at",
            "10",
            "--platform",
        ])
        .arg(&board)
        .arg(&firmware)
        .output()
        .unwrap();
    assert!(reset.status.success(), "{reset:?}");
    assert_eq!(reset.stdout, b"AAA");
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
fn litex_uart_and_timer_share_shadow_registers_and_drive_vex_interrupts() {
    let directory = std::env::temp_dir().join(format!("penv-litex-{}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    let board = directory.join("board.repl");
    let raw = directory.join("firmware.bin");
    let input = directory.join("input");
    fs::write(
        &board,
        r#"cpu: CPU.VexRiscv @ sysbus
    cpuType: "rv32im"
ram: Memory.MappedMemory @ sysbus 0x80000000
    size: 0x1000
uart: UART.LiteX_UART @ { sysbus 0x60001800; sysbus 0xe0001800 }
    txFifoCapacity: 0
    -> cpu@2
timer0: Timers.LiteX_Timer_CSR32 @ { sysbus 0x60002800; sysbus 0xe0002800 }
    frequency: 100000000
    IRQ -> cpu@1
"#,
    )
    .unwrap();
    let program = [
        0x6000_22b7u32, // lui t0, 0x60002
        0x8002_8293,    // addi t0, t0, -2048 (UART)
        0x0020_0313,    // li t1, 2
        0x0062_aa23,    // sw t1, 20(t0) (enable RX event)
        0xe000_3337,    // lui t1, 0xe0003
        0x8003_0313,    // addi t1, t1, -2048 (timer shadow)
        0x0010_0393,    // li t2, 1
        0x0073_2023,    // sw t2, 0(t1) (load)
        0x0073_2e23,    // sw t2, 28(t1) (event enable)
        0x0073_2423,    // sw t2, 8(t1) (start)
        0xfc00_2e73,    // csrr t3, 0xfc0 (VexRiscv pending)
        0x0060_0e93,    // li t4, 6 (UART | timer)
        0x01de_1c63,    // bne t3, t4, +24 (skip output on mismatch)
        0x0002_a383,    // lw t2, 0(t0) (receive byte)
        0xe000_2337,    // lui t1, 0xe0002
        0x8003_0313,    // addi t1, t1, -2048 (UART shadow)
        0x0073_2023,    // sw t2, 0(t1) (transmit byte)
        0x0010_0073,    // ebreak
        0x0010_0073,    // ebreak
    ];
    fs::write(raw.as_path(), program.map(u32::to_le_bytes).as_flattened()).unwrap();
    fs::write(&input, b"Q").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_penv"))
        .args(["run", "--raw", "--platform"])
        .arg(&board)
        .arg("--uart-input")
        .arg(&input)
        .arg(&raw)
        .output()
        .unwrap();
    fs::remove_dir_all(directory).unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(output.stdout, b"Q");
}

#[test]
fn loads_resc_wrappers_with_shared_variables_and_relative_assets() {
    let directory = std::env::temp_dir().join(format!("penv-resc-wrapper-{}", std::process::id()));
    let board = directory.join("nested");
    fs::create_dir_all(&board).unwrap();
    let wrapper = directory.join("wrapper.resc");
    fs::write(
        &wrapper,
        "set bin @local\\ board.bin\ni @nested/board.resc\nrunMacro $reset\nstart\n",
    )
    .unwrap();
    fs::write(
        board.join("board.resc"),
        concat!(
            "mach create \"Board wrapper\"\n",
            "machine LoadPlatformDescription @board.repl\n",
            "$bin?=@default.bin\n",
            "macro reset\n",
            "\"\"\"\n",
            "  sysbus LoadBinary $bin 0x80000000\n",
            "  cpu PC 0x80000000\n",
            "\"\"\"\n",
        ),
    )
    .unwrap();
    fs::write(
        board.join("board.repl"),
        "cpu: CPU.RiscV32 @ sysbus\n    cpuType: \"rv32i\"\nram: Memory.MappedMemory @ sysbus 0x80000000\n    size: 0x1000\n",
    ).unwrap();
    fs::write(board.join("local board.bin"), 0x0010_0073u32.to_le_bytes()).unwrap();
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_penv"))
            .args(["run", "--script"])
            .arg(&wrapper)
            .output()
            .unwrap()
    };
    let boot = run();
    fs::write(board.join("board.resc"), "include @../wrapper.resc\n").unwrap();
    let cycle = run();
    fs::remove_dir_all(directory).unwrap();

    assert!(boot.status.success(), "{boot:?}");
    assert_eq!(cycle.status.code(), Some(1), "{cycle:?}");
    assert!(String::from_utf8_lossy(&cycle.stderr).contains("board.resc:1:"));
    assert!(
        String::from_utf8_lossy(&cycle.stderr)
            .contains("recursive or excessively deep script include")
    );
}

#[test]
fn distinguishes_rv32_act_console_packets_from_tohost_completion() {
    let directory = std::env::temp_dir().join(format!("penv-tohost-{}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    let raw = directory.join("tohost.bin");
    let program = [
        0x8000_02b7u32, // lui t0, 0x80000
        0x0802_8293,    // addi t0, t0, 128
        0x0410_0313,    // li t1, 'A' (odd low word)
        0x0062_a023,    // sw t1, 0(t0)
        0x0101_0337,    // lui t1, 0x01010 (ACT console command)
        0x0062_a223,    // sw t1, 4(t0)
        0x0010_0313,    // li t1, 1 (pass)
        0x0062_a023,    // sw t1, 0(t0)
        0x0002_a223,    // sw zero, 4(t0) (ACT completion command)
        0x0010_0073,    // ebreak
    ];
    fs::write(&raw, program.map(u32::to_le_bytes).as_flattened()).unwrap();
    for (option, limit, status, uart, marker) in [
        ("--tohost-act", "20", 0, &b"A"[..], "tohost exit code 0"),
        (
            "--tohost-act",
            "4",
            2,
            &b""[..],
            "instruction limit reached",
        ),
        ("--tohost", "20", 1, &b""[..], "tohost exit code 32"),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_penv"))
            .args(["run", "--raw", option, "0x80000080", "--max-steps", limit])
            .arg(&raw)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(status), "{output:?}");
        assert_eq!(output.stdout, uart);
        assert!(String::from_utf8_lossy(&output.stderr).contains(marker));
    }
    fs::remove_dir_all(directory).unwrap();
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
sysbus:
    init:
        SilenceRange <0x02010010, 0x02010FFF>
"#,
    )
    .unwrap();
    let instructions: [u32; 22] = [
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
        0x0201_12b7, // lui t0, 0x02011
        0x8002_8293, // addi t0, t0, -2048
        0x0862_be2f, // amoswap.d t3, t1, (t0): silenced way-mask register
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
    assert_eq!(result.stdout, b"\0ABC\0");
}

#[test]
fn polarfire_sysreg_handles_hss_access_and_register_defaults() {
    let directory =
        std::env::temp_dir().join(format!("penv-polarfire-sysreg-{}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    let board = directory.join("polarfire.repl");
    let raw = directory.join("sysreg.bin");
    fs::write(
        &board,
        r#"cpu: CPU.RiscV64 @ sysbus
    cpuType: "rv64i"
ram: Memory.MappedMemory @ sysbus 0x80000000
    size: 0x1000
uart: UART.NS16550 @ sysbus 0x10000000
TopSystemRegisters: Miscellaneous.MPFS_Sysreg @ sysbus 0x20002000
"#,
    )
    .unwrap();
    let instructions: [u32; 18] = [
        0x2000_22b7, // lui t0, 0x20002
        0x1000_0337, // lui t1, 0x10000
        0x0550_0393, // addi t2, zero, 0x55
        0x1672_aa23, // sw t2, 0x174(t0): the undefined offset used by HSS
        0x1742_ae03, // lw t3, 0x174(t0)
        0x01c3_0023, // sb t3, 0(t1)
        0x1472_a623, // sw t2, 0x14c(t0): PLL status ignores writes
        0x14c2_ae03, // lw t3, 0x14c(t0)
        0x01c3_0023, // sb t3, 0(t1)
        0x0872_a223, // sw t2, 0x84(t0): subblock clock flags
        0x0842_ae03, // lw t3, 0x84(t0)
        0x01c3_0023, // sb t3, 0(t1)
        0x0882_ae03, // lw t3, 0x88(t0): soft reset default
        0x01c3_0023, // sb t3, 0(t1)
        0x0802_a423, // sw zero, 0x88(t0): clear implemented soft reset bits
        0x0882_ae03, // lw t3, 0x88(t0)
        0x01c3_0023, // sb t3, 0(t1)
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
    let result = Command::new(env!("CARGO_BIN_EXE_penv"))
        .args(["run", "--raw", "--platform"])
        .arg(&board)
        .arg(&raw)
        .output()
        .unwrap();
    fs::remove_dir_all(directory).unwrap();

    assert!(result.status.success(), "{result:?}");
    assert_eq!(result.stdout, [0, 7, 0x55, 0xfe, 0]);
}

#[test]
fn polarfire_gem_status_responds_at_both_bus_aliases() {
    let directory = std::env::temp_dir().join(format!("penv-polarfire-gem-{}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    let board = directory.join("polarfire.repl");
    let raw = directory.join("gem-status.bin");
    fs::write(
        &board,
        r#"cpu: CPU.RiscV64 @ sysbus
    cpuType: "rv64i"
ram: Memory.MappedMemory @ sysbus 0x80000000
    size: 0x1000
uart: UART.NS16550 @ sysbus 0x10000000
mac0: Network.CadenceGEM @ { sysbus 0x20110000; sysbus 0x28110000 }
"#,
    )
    .unwrap();
    let instructions: [u32; 11] = [
        0x2011_02b7, // lui t0, 0x20110
        0x2811_0337, // lui t1, 0x28110
        0x1000_03b7, // lui t2, 0x10000
        0x0082_ae03, // lw t3, 8(t0)
        0x01c3_8023, // sb t3, 0(t2)
        0x0002_a423, // sw zero, 8(t0): read-only
        0x0082_ae03, // lw t3, 8(t0)
        0x01c3_8023, // sb t3, 0(t2)
        0x0083_2e03, // lw t3, 8(t1)
        0x01c3_8023, // sb t3, 0(t2)
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
    let result = Command::new(env!("CARGO_BIN_EXE_penv"))
        .args(["run", "--raw", "--platform"])
        .arg(&board)
        .arg(&raw)
        .output()
        .unwrap();
    fs::remove_dir_all(directory).unwrap();

    assert!(result.status.success(), "{result:?}");
    assert_eq!(result.stdout, [4, 4, 4]);
}

#[test]
fn polarfire_resc_reads_a_sector_from_an_attached_sd_image() {
    use std::io::Read;

    let directory = std::env::temp_dir().join(format!("penv-polarfire-sd-{}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    let board = directory.join("beaglev-fire.repl");
    let raw = directory.join("sd-probe.bin");
    let script = directory.join("sd-probe.resc");
    let disk = directory.join("disk.img");
    fs::write(
        &board,
        r#"cpu: CPU.RiscV64 @ sysbus
    cpuType: "rv64i"
ram: Memory.MappedMemory @ sysbus 0x80000000
    size: 0x1000
mmuart0: UART.NS16550 @ sysbus 0x10000000
mmc: SD.MPFS_SDController @ sysbus 0x20008000
"#,
    )
    .unwrap();
    let sector = if let Some(path) = std::env::var_os("PENV_FIRE_SD_IMAGE") {
        let mut sector = [0; 512];
        fs::File::open(&path)
            .unwrap()
            .read_exact(&mut sector)
            .unwrap();
        fs::copy(path, &disk).unwrap();
        sector
    } else {
        let sector = std::array::from_fn(|i| (i / 2) as u8);
        fs::write(&disk, sector).unwrap();
        sector
    };
    fs::write(
        &script,
        r#"$image?=@https://example.invalid/disk.img
mach create
machine LoadPlatformDescription @beaglev-fire.repl
showAnalyzer mmuart0
machine SdCardFromFile $image mmc 0x100000000
sysbus LoadBinary @sd-probe.bin 0x80000000
cpu PC 0x80000000
"#,
    )
    .unwrap();
    let instructions: [u32; 67] = [
        0x2000_82b7, // lui t0, 0x20008
        0x1000_03b7, // lui t2, 0x10000
        0x2242_ae03, // lw t3, 0x224(t0): the attached card is visible to legacy MMC
        0x010e_5e13, // srli t3, t3, 16
        0x01c3_8023, // sb t3, 0(t2)
        0x5000_0eb7, // lui t4, 0x50000
        0x0010_0313, // li t1, 1: select SD
        0xf06e_a023, // sw t1, -256(t4): Fire media selector
        0xf00e_ae03, // lw t3, -256(t4)
        0x01c3_8023, // sb t3, 0(t2)
        0x2242_ae03, // lw t3, 0x224(t0): SD card present
        0x010e_5e13, // srli t3, t3, 16
        0x01c3_8023, // sb t3, 0(t2)
        0x2000_0313, // li t1, 512
        0x2062_a223, // sw t1, 0x204(t0): block size
        0x0230_0313, // li t1, 0x23
        0x2262_aa23, // sw t1, 0x234(t0): status enable
        0x2262_ac23, // sw t1, 0x238(t0): signal enable
        0x2002_a623, // sw zero, 0x20c(t0): CMD0
        0x3702_0337, // lui t1, 0x37020
        0x2062_a623, // sw t1, 0x20c(t0): CMD55
        0x4000_0337, // lui t1, 0x40000: high capacity support
        0x2062_a423, // sw t1, 0x208(t0)
        0x2902_0337, // lui t1, 0x29020
        0x2062_a623, // sw t1, 0x20c(t0): ACMD41
        0x2102_ae03, // lw t3, 0x210(t0): OCR
        0x018e_5e13, // srli t3, t3, 24
        0x01c3_8023, // sb t3, 0(t2)
        0x0201_0337, // lui t1, 0x02010
        0x2062_a623, // sw t1, 0x20c(t0): CMD2
        0x0302_0337, // lui t1, 0x03020
        0x2062_a623, // sw t1, 0x20c(t0): CMD3
        0x0001_0337, // lui t1, 0x10: card address 1
        0x2062_a423, // sw t1, 0x208(t0)
        0x0901_0337, // lui t1, 0x09010
        0x2062_a623, // sw t1, 0x20c(t0): CMD9
        0x21c2_ae03, // lw t3, 0x21c(t0): CSD structure
        0x010e_5e13, // srli t3, t3, 16
        0x01c3_8023, // sb t3, 0(t2)
        0x2142_ae03, // lw t3, 0x214(t0): CSD size
        0x008e_5e13, // srli t3, t3, 8
        0x01c3_8023, // sb t3, 0(t2)
        0x0703_0337, // lui t1, 0x07030
        0x2062_a623, // sw t1, 0x20c(t0): CMD7
        0x2002_a423, // sw zero, 0x208(t0): read sector 0
        0x0100_0313, // li t1, 0x10
        0x2062_9623, // sh t1, 0x20c(t0): read direction, PIO
        0x0000_1337, // lui t1, 1
        0x1223_031b, // addiw t1, t1, 0x122
        0x2062_9723, // sh t1, 0x20e(t0): CMD17, short response and data
        0x0800_0e93, // li t4, 128
        0x2202_ae03, // loop: lw t3, 0x220(t0): data port
        0x01c3_8023, // sb t3, 0(t2)
        0x008e_5e13, // srli t3, t3, 8
        0x01c3_8023, // sb t3, 0(t2)
        0x008e_5e13, // srli t3, t3, 8
        0x01c3_8023, // sb t3, 0(t2)
        0x008e_5e13, // srli t3, t3, 8
        0x01c3_8023, // sb t3, 0(t2)
        0xfffe_8e93, // addi t4, t4, -1
        0xfc0e_9ee3, // bnez t4, loop
        0x2302_ae03, // lw t3, 0x230(t0): interrupt flags
        0x01c3_8023, // sb t3, 0(t2)
        0x23c2_a823, // sw t3, 0x230(t0): clear interrupt flags
        0x2302_ae03, // lw t3, 0x230(t0)
        0x01c3_8023, // sb t3, 0(t2)
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
    let result = Command::new(env!("CARGO_BIN_EXE_penv"))
        .args(["run", "--max-steps", "2000", "--script"])
        .arg(script)
        .arg("--asset-dir")
        .arg(&directory)
        .output()
        .unwrap();
    fs::remove_dir_all(directory).unwrap();

    assert!(result.status.success(), "{result:?}");
    assert_eq!(
        result.stdout,
        [&[0xf7, 1, 0xf7, 0xc0, 0x40, 0xff], &sector[..], &[0x23, 0]].concat()
    );
}
