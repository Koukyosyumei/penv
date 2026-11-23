use std::{
    fs,
    path::Path,
    process::{Command, Output},
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
#[ignore = "requires the upstream Renode tree and public STM32F072 Hello World ELF"]
fn upstream_stm32f072_cpuwait_defers_elf_boot_until_released() {
    let fixture = "stm32f072-cpuwait.resc";
    let held = run_upstream_stm32f072_script(fixture, "1000000");
    assert_eq!(held.status.code(), Some(2), "{held:?}");
    assert!(held.stdout.is_empty(), "{held:?}");

    let released = run_upstream_stm32f072_script(fixture, "11000000");
    assert!(released.status.success(), "{released:?}");
    assert!(
        released
            .stdout
            .ends_with(b"Hello World! stm32f072b_disco\r\n"),
        "{released:?}"
    );
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
    let completed = run_upstream_stm32f072_script("stm32f072-echo.resc", "1600000000");
    assert!(completed.status.success(), "{completed:?}");
    assert!(
        completed
            .stdout
            .ends_with(b"Start typing characters to see them echoed back\r\nEcho working?"),
        "{completed:?}"
    );
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
fn renode_resc_systick_counts_at_100_hz_for_both_write_orders() {
    let directory = std::env::temp_dir().join(format!("penv-resc-systick-{}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    fs::write(
        directory.join("board.repl"),
        r#"cpu: CPU.CortexM @ sysbus
    cpuType: "cortex-m0"
    nvic: nvic
nvic: IRQControllers.NVIC @ sysbus 0xe000e000
    systickFrequency: 48000000
    -> cpu@0
ram: Memory.MappedMemory @ sysbus 0
    size: 0x1000
"#,
    )
    .unwrap();
    let mut image = vec![0; 0x20];
    image[..4].copy_from_slice(&0x800u32.to_le_bytes());
    image[4..8].copy_from_slice(&0x21u32.to_le_bytes());
    // The idle core leaves the SysTick observations to the monitor.
    image.extend_from_slice(&0xbf30u16.to_le_bytes()); // wfi
    image.extend_from_slice(&0xe7fdu16.to_le_bytes()); // b 0x20
    fs::write(directory.join("firmware.bin"), image).unwrap();
    let path = directory.join("boot.resc");
    let run = |commands: &str, limit: &str| {
        fs::write(&path, format!(
            "mach create\nmachine LoadPlatformDescription @board.repl\nsysbus LoadBinary @firmware.bin 0\n{commands}"
        )).unwrap();
        Command::new(env!("CARGO_BIN_EXE_penv"))
            .args(["run", "--max-steps", limit, "--script"])
            .arg(&path)
            .output()
            .unwrap()
    };
    let reload = "sysbus WriteDoubleWord 0xe000e014 10";
    let current = "sysbus WriteDoubleWord 0xe000e018 0";
    for (first, second) in [(current, reload), (reload, current)] {
        let result = run(
            &format!(
                "nvic Frequency 100\nsysbus WriteDoubleWord 0xe000e010 0\n{first}\n{second}\n{}",
                concat!(
                    "sysbus WriteDoubleWord 0xe000e010 5\n",
                    "sysbus ReadDoubleWord 0xe000e018\n",
                    "emulation RunFor \"0.09\"\nsysbus ReadDoubleWord 0xe000e018\n",
                    "emulation RunFor \"0.01\"\nsysbus ReadDoubleWord 0xe000e018\n",
                    "emulation RunFor \"0.01\"\nsysbus ReadDoubleWord 0xe000e018\n",
                ),
            ),
            "11000000",
        );
        assert!(result.status.success(), "{result:?}");
        assert_eq!(
            result.stdout,
            b"0x0000000a\n0x00000001\n0x0000000a\n0x00000009\n"
        );
    }
    for (commands, diagnostic) in [
        ("nvic Frequency 0\n", "NVIC Frequency must be positive"),
        ("other Frequency 100\n", "unknown Cortex-M NVIC"),
    ] {
        let result = run(commands, "1");
        assert_eq!(result.status.code(), Some(1), "{result:?}");
        assert!(
            String::from_utf8_lossy(&result.stderr).contains(diagnostic),
            "{result:?}"
        );
    }
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn renode_resc_stm32_timer_frequency_changes_counter_rate_and_resets() {
    let directory =
        std::env::temp_dir().join(format!("penv-timer-frequency-{}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    fs::write(
        directory.join("board.repl"),
        r#"cpu: CPU.CortexM @ sysbus
    cpuType: "cortex-m0"
    nvic: nvic
nvic: IRQControllers.NVIC @ sysbus 0xe000e000
    -> cpu@0
ram: Memory.MappedMemory @ sysbus 0
    size: 0x1000
timer2: Timers.STM32_Timer @ sysbus 0x40000000
    frequency: 1000000
    initialLimit: 0xffffffff
"#,
    )
    .unwrap();
    fs::write(
        directory.join("firmware.bin"),
        [0x00, 0x08, 0, 0, 9, 0, 0, 0, 0xfe, 0xe7],
    )
    .unwrap();
    let script = directory.join("timer.resc");
    let setup = "mach create\nmachine LoadPlatformDescription @board.repl\nsysbus LoadBinary @firmware.bin 0\n";
    let run = |commands: &str| {
        fs::write(&script, format!("{setup}{commands}")).unwrap();
        Command::new(env!("CARGO_BIN_EXE_penv"))
            .args(["run", "--max-steps", "1000", "--script"])
            .arg(&script)
            .output()
            .unwrap()
    };
    let result = run(r#"sysbus WriteDoubleWord 0x40000000 1
emulation RunFor "0.00000125"
sysbus ReadDoubleWord 0x40000024
sysbus.timer2 Frequency 4000000
emulation RunFor "0.00000020"
sysbus ReadDoubleWord 0x40000024
emulation RunFor "0.00000080"
sysbus ReadDoubleWord 0x40000024
machine Reset
sysbus WriteDoubleWord 0x40000000 1
emulation RunFor "0.000001"
sysbus ReadDoubleWord 0x40000024
"#);
    assert!(result.status.success(), "{result:?}");
    assert_eq!(
        result.stdout,
        b"0x00000001\n0x00000001\n0x00000005\n0x00000001\n"
    );
    let invalid = run("timer2 Frequency 0\n");
    assert_eq!(invalid.status.code(), Some(1), "{invalid:?}");
    assert!(
        String::from_utf8_lossy(&invalid.stderr).contains("STM32 timer Frequency must be positive")
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn renode_resc_cortex_m_evaluates_condition_codes() {
    let directory =
        std::env::temp_dir().join(format!("penv-resc-condition-{}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    fs::write(
        directory.join("board.repl"),
        r#"cpu: CPU.CortexM @ sysbus
    cpuType: "cortex-m0"
ram: Memory.MappedMemory @ sysbus 0
    size: 0x1000
"#,
    )
    .unwrap();
    let mut image = vec![0; 0x100];
    image[..4].copy_from_slice(&0x800u32.to_le_bytes());
    image[4..8].copy_from_slice(&0x101u32.to_le_bytes());
    for instruction in [
        0x267fu16, // movs r6, #127
        0x2e81,    // cmp r6, #129
        0x2e7f,    // cmp r6, #127
        0x2e7e,    // cmp r6, #126
        0x2101,    // movs r1, #1
        0x07ca,    // lsls r2, r1, #31
        0x1892,    // adds r2, r2, r2
    ] {
        image.extend_from_slice(&instruction.to_le_bytes());
    }
    fs::write(directory.join("firmware.bin"), image).unwrap();

    let mut script = String::from(
        "mach create\nmachine LoadPlatformDescription @board.repl\nsysbus LoadBinary @firmware.bin 0\n",
    );
    let mut expected = String::new();
    for (step, conditions, flags) in [
        (
            2,
            [false, false, true, false, false, false, false, true],
            "0x81000000",
        ),
        (
            1,
            [true, true, false, false, false, true, false, true],
            "0x61000000",
        ),
        (
            1,
            [false, true, false, false, true, true, true, true],
            "0x21000000",
        ),
        (
            3,
            [true, true, false, true, false, false, false, true],
            "0x71000000",
        ),
    ] {
        use std::fmt::Write;
        writeln!(script, "cpu Step {step}").unwrap();
        for (code, result) in (0..=14).step_by(2).zip(conditions) {
            writeln!(script, "cpu EvaluateConditionCode {code}").unwrap();
            writeln!(expected, "{}", if result { "True" } else { "False" }).unwrap();
        }
        writeln!(script, "cpu GetRegister \"CPSR\"").unwrap();
        writeln!(expected, "{flags}").unwrap();
    }
    let path = directory.join("boot.resc");
    fs::write(&path, script).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_penv"))
        .args(["run", "--max-steps", "7", "--script"])
        .arg(&path)
        .output()
        .unwrap();
    fs::remove_dir_all(directory).unwrap();
    assert!(result.status.success(), "{result:?}");
    assert_eq!(result.stdout, expected.as_bytes());
}

#[test]
fn renode_resc_systembus_bulk_memory_commands() {
    let directory = std::env::temp_dir().join(format!("penv-resc-bulk-{}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    fs::write(
        directory.join("board.repl"),
        r#"cpu: CPU.CortexM @ sysbus
    cpuType: "cortex-m0"
ram: Memory.MappedMemory @ sysbus 0
    size: 0x10
extra: Memory.MappedMemory @ sysbus 0xc0000000
    size: 0x10
"#,
    )
    .unwrap();
    let script = directory.join("boot.resc");
    fs::write(directory.join("firmware.bin"), [0u8; 8]).unwrap();
    fs::write(
        &script,
        r#"mach create
machine LoadPlatformDescription @board.repl
sysbus LoadBinary @firmware.bin 0
sysbus WriteBytes [0x11, 0x22, 0x33, 0x44] 0x0e
sysbus ReadBytes 0x0e 4
sysbus WriteBytes [0, 1, 2, 3, 4, 5, 6, 7] 0xbffffffc
sysbus ReadBytes 0xbffffffc 8
sysbus ReadBytes 0 0
sysbus FindBytes "11 22"
sysbus FindBytes [0x04, 0x05, 0x06, 0x07] 0xc0000000 0xc0000003
sysbus FindBytes "04050607" 0xc0000000 0xc0000002
sysbus ReadByte 0xabcd1234
sysbus WriteDoubleWord 0xc0000008 0x12345678
sysbus ReadWord 0xc000000a
sysbus ReadDoubleWord 0xc0000008
sysbus UnhandledAccessBehaviour ThrowExceptionIfNotTagged
sysbus Tag <0xabcd1200, 0x100> "parent" throwException=True
sysbus Tag <0xabcd1230 0x10> "child tag" 0x142
sysbus WriteByte 0xabcd1234 0
sysbus ReadByte 0xabcd1234
"#,
    )
    .unwrap();
    let invoke = || {
        Command::new(env!("CARGO_BIN_EXE_penv"))
            .arg("run")
            .arg("--script")
            .arg(&script)
            .output()
            .unwrap()
    };
    let run = invoke();
    assert!(run.status.success(), "{run:?}");
    assert_eq!(
        run.stdout,
        concat!(
            "[0x11, 0x22, 0x00, 0x00]\n",
            "[0x00, 0x00, 0x00, 0x00, 0x04, 0x05, 0x06, 0x07]\n",
            "[]\n[0x0000000e]\n[0xc0000000]\n[]\n0x00\n0x1234\n0x12345678\n0x42\n",
        )
        .as_bytes()
    );
    let source = fs::read_to_string(&script).unwrap();
    fs::write(
        &script,
        source + "sysbus UnhandledAccessBehaviour DoNotReport\nsysbus WriteByte 0xabcd1240 0\n",
    )
    .unwrap();
    let fault = invoke();
    fs::remove_dir_all(directory).unwrap();
    assert_eq!(fault.status.code(), Some(1), "{fault:?}");
    assert!(
        String::from_utf8_lossy(&fault.stderr).contains("AddressError at 0xabcd1240"),
        "{fault:?}"
    );
}

#[test]
#[ignore = "requires the upstream Renode tree and public STM32F072 Hello World ELF"]
fn upstream_stm32f072_uart_binary_waits_for_adjacent_output() {
    let result = run_upstream_stm32f072_script("stm32f072-uart-binary.resc", "1000000");
    assert!(result.status.success(), "{result:?}");
    let output = String::from_utf8(result.stdout).unwrap();
    assert!(
        output.contains(
            "Hello World![0x48, 0x65, 0x6c, 0x6c, 0x6f, 0x20, 0x57, 0x6f, 0x72, 0x6c, 0x64, 0x21]"
        ),
        "{output:?}"
    );
    assert!(output.contains(" stm32f072b_disco\r\n[0x20, 0x73, 0x74, 0x6d, 0x33, 0x32, 0x66, 0x30, 0x37, 0x32, 0x62, 0x5f, 0x64, 0x69, 0x73, 0x63, 0x6f, 0x0d, 0x0a]"), "{output:?}");
}

#[test]
#[ignore = "requires the upstream Renode tree and public STM32F072 Hello World ELF"]
fn upstream_stm32f072_uart_tester_id_overrides_default() {
    let result = run_upstream_stm32f072_script("stm32f072-uart-testers.resc", "1000000");
    assert!(result.status.success(), "{result:?}");
    assert!(result.stdout.starts_with(b"0\n1\n"), "{result:?}");
    assert!(
        result
            .stdout
            .ends_with(b"Hello World! stm32f072b_disco\r\n"),
        "{result:?}"
    );
    let timeout = run_upstream_stm32f072_script("stm32f072-uart-tester-timeout.resc", "101000000");
    assert_eq!(timeout.status.code(), Some(1), "{timeout:?}");
    assert!(timeout.stdout.starts_with(b"0\n1\n"), "{timeout:?}");
    assert!(
        timeout
            .stdout
            .ends_with(b"Hello World! stm32f072b_disco\r\n"),
        "{timeout:?}"
    );
    assert!(
        String::from_utf8_lossy(&timeout.stderr).contains("Terminal tester failed!"),
        "{timeout:?}"
    );
}

#[test]
fn renode_resc_uart_testers_follow_identity_and_default_selection() {
    use std::fmt::Write;
    let directory = std::env::temp_dir().join(format!("penv-resc-testers-{}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    fs::write(
        directory.join("board.repl"),
        r#"cpu: CPU.CortexM @ sysbus
    cpuType: "cortex-m0"
ram: Memory.MappedMemory @ sysbus 0
    size: 0x1000
uart0: UART.STM32F7_USART @ sysbus 0x40013800
uart1: UART.STM32F7_USART @ sysbus 0x40004400
"#,
    )
    .unwrap();
    let mut image = vec![0; 0x20];
    image[..4].copy_from_slice(&0x800u32.to_le_bytes());
    image[4..8].copy_from_slice(&0x21u32.to_le_bytes());
    image.extend_from_slice(&0xbf30u16.to_le_bytes()); // wfi
    fs::write(directory.join("firmware.bin"), image).unwrap();
    let boot = "mach create\nmachine LoadPlatformDescription @board.repl\nsysbus LoadBinary @firmware.bin 0\n";
    let first = "CreateTerminalTester sysbus.uart0\n";
    let second = "CreateTerminalTester sysbus.uart1\n";
    let mut output = String::from("sysbus WriteDoubleWord 0x40013800 9\n");
    for byte in b"ZEPHYR\r\n" {
        writeln!(output, "sysbus WriteByte 0x40013828 {byte}").unwrap();
    }
    let run = |setup: &str, output: &str, selection: &str| {
        let script = directory.join("boot.resc");
        fs::write(&script, format!("{boot}{setup}{output}{selection}")).unwrap();
        Command::new(env!("CARGO_BIN_EXE_penv"))
            .args(["run", "--max-steps", "1000", "--script"])
            .arg(script)
            .output()
            .unwrap()
    };
    let both = format!("{first}{second}");
    let wait = "WaitForLineOnUart \"ZEPHYR\"";
    for (setup, selection, error) in [
        (
            "CreateTerminalTester sysbus.non_existing_uart\n",
            String::new(),
            Some("not found or of wrong type"),
        ),
        (first, wait.into(), None),
        (
            &both,
            wait.into(),
            Some("There is more than one tester available"),
        ),
        (&both, format!("{wait} testerId=0"), None),
        (&both, format!("SetDefaultTester 0\n{wait}"), None),
        (
            &both,
            format!("SetDefaultTester 1\n{wait} testerId=0"),
            None,
        ),
        (
            &both,
            format!("SetDefaultTester 1\n{wait} timeout=1"),
            Some("Terminal tester failed!"),
        ),
        (
            &both,
            format!("SetDefaultTester 1\nSetDefaultTester 0\n{wait}"),
            None,
        ),
        (
            &both,
            format!("SetDefaultTester 0\nSetDefaultTester null\n{wait}"),
            Some("There is more than one tester available"),
        ),
    ] {
        let result = run(setup, &output, &selection);
        if let Some(error) = error {
            assert_eq!(result.status.code(), Some(1), "{result:?}");
            assert!(
                String::from_utf8_lossy(&result.stderr).contains(error),
                "{result:?}"
            );
        } else {
            assert!(result.status.success(), "{result:?}");
            assert!(result.stdout.ends_with(b"ZEPHYR\r\n"), "{result:?}");
        }
    }
    let binary = "CreateTerminalTester sysbus.uart0 binaryMode=true defaultPauseEmulation=true\n";
    let stream = |data: &[u8]| {
        let mut commands = String::from("sysbus WriteDoubleWord 0x40013800 9\n");
        for byte in data {
            writeln!(commands, "sysbus WriteByte 0x40013828 {byte}").unwrap();
        }
        commands
    };
    let banner = stream(b"ZEPHYR OS\r\n****\r\nshell");
    let spanning = run(
        binary,
        &banner,
        "WaitForBytesOnUart \"2a 2a 2a 2a 0d 0a 73 68\"",
    );
    assert!(spanning.status.success(), "{spanning:?}");
    assert!(
        spanning
            .stdout
            .ends_with(b"[0x2a, 0x2a, 0x2a, 0x2a, 0x0d, 0x0a, 0x73, 0x68]\n")
    );
    let adjacent = run(
        binary,
        &banner,
        concat!(
            "WaitForBytesOnUart \"5a4550485952\"\n",
            "WaitForBytesOnUart \"20 4f 53\" matchStart=true timeout=0\n",
            "WaitForBytesOnUart \"2a2a2a2a0d0a7368\" timeout=0\n",
            "WaitForBytesOnUart \"656c6c\" matchStart=true timeout=0",
        ),
    );
    assert!(adjacent.status.success(), "{adjacent:?}");
    assert!(adjacent.stdout.ends_with(b"[0x65, 0x6c, 0x6c]\n"));
    for (setup, command, message) in [
        (
            first,
            "WaitForBytesOnUart \"59 52 20 4f 53\" timeout=1",
            "Attempt to wait for bytes on a tester configured in text mode",
        ),
        (
            binary,
            "WaitForLineOnUart \"ZEPHYR\" timeout=1",
            "Attempt to wait for text on a tester configured in binary mode",
        ),
        (
            binary,
            "WaitForBytesOnUart \"5a4550485952\"\nWaitForBytesOnUart \"52 20 4f 53\" timeout=0",
            "Terminal tester failed!",
        ),
        (
            binary,
            "WaitForBytesOnUart \"5a4550485952\"\nWaitForBytesOnUart \"4f 53\" matchStart=true timeout=0",
            "Terminal tester failed!",
        ),
    ] {
        let result = run(setup, &banner, command);
        assert_eq!(result.status.code(), Some(1), "{result:?}");
        assert!(
            String::from_utf8_lossy(&result.stderr).contains(message),
            "{result:?}"
        );
    }
    let all: Vec<u8> = (0..=255).collect();
    let hex = all
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let list = all
        .iter()
        .map(|byte| format!("0x{byte:02x}"))
        .collect::<Vec<_>>()
        .join(", ");
    let result = run(
        binary,
        &stream(&all),
        &format!("WaitForBytesOnUart \"{hex}\" matchStart=true"),
    );
    assert!(result.status.success(), "{result:?}");
    assert_eq!(
        result.stdout,
        [b"0\n", all.as_slice(), format!("[{list}]\n").as_bytes()].concat()
    );

    let expression: String = all.iter().map(|byte| format!(r"\x{byte:02x}")).collect();
    let tripled = all.repeat(3);
    let captures = run(
        binary,
        &stream(&tripled),
        &format!(
            "WaitForBytesOnUart \"{expression}\" matchStart=true treatAsRegex=true\n{}",
            concat!(
                "WaitForBytesOnUart \"\\x00.*?\\xff\" matchStart=true treatAsRegex=true\n",
                "WaitForBytesOnUart \"(\\x11.*)\\x41.*?([\\x50-\\x5f]+).*?\\xfd\" treatAsRegex=true\n",
                "WaitForBytesOnUart \"feff\" matchStart=true timeout=0",
            ),
        ),
    );
    assert!(captures.status.success(), "{captures:?}");
    let hex_list = |bytes: &[u8]| {
        let items = bytes
            .iter()
            .map(|byte| format!("0x{byte:02x}"))
            .collect::<Vec<_>>()
            .join(", ");
        format!("[{items}]\n")
    };
    let expected: String = [
        &all[..],
        &all[..],
        &all[0x11..0xfe],
        &all[0x11..0x41],
        &all[0x50..0x60],
        &all[0xfe..],
    ]
    .into_iter()
    .map(hex_list)
    .collect();
    assert_eq!(
        captures.stdout,
        [b"0\n", tripled.as_slice(), expected.as_bytes()].concat()
    );

    for (setup, data, query) in [
        (
            first,
            &b"D2\r\nEntering test loop\r\n"[..],
            "RegisterFailingUartString \"Entering.*loop\" treatAsRegex=true\nWaitForLineOnUart \"D2\"",
        ),
        (
            &format!("{second}{first}")[..],
            &b"D1\r\n"[..],
            "RegisterFailingUartString \"D1\" testerId=1\nWaitForLineOnUart \"D1\" testerId=1",
        ),
    ] {
        let received = run(setup, &stream(data), query);
        assert_eq!(received.status.code(), Some(1), "{received:?}");
        assert!(
            String::from_utf8_lossy(&received.stderr).contains("Test failing entry"),
            "{received:?}"
        );
    }
    let unregistered = run(
        first,
        &stream(b"D1\r\n"),
        concat!(
            "RegisterFailingUartString \"D1\"\n",
            "UnregisterFailingUartString \"D1\"\n",
            "WaitForLineOnUart \"D1\"",
        ),
    );
    assert!(unregistered.status.success(), "{unregistered:?}");

    fs::remove_dir_all(directory).unwrap();
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
    let timed_out = run(
        "waitForUartPrompt sysbus.usart1 \"A\" timeout=0.00000004s",
        "10",
    );
    let timed_cr = run(
        "waitForUart sysbus.usart1 \"A\" CR timeout=0.00000007s",
        "10",
    );
    let buffered_poll = run(
        "emulation RunFor \"0.00000007s\"\nwaitForUart sysbus.usart1 \"A\" CR timeout=0s",
        "10",
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
    for result in [
        selected,
        merged,
        buffered,
        independent,
        timed_cr,
        buffered_poll,
    ] {
        assert!(result.status.success(), "{result:?}");
        assert_eq!(result.stdout, b"A\r");
        assert!(String::from_utf8_lossy(&result.stderr).contains("ScriptComplete"));
    }
    for result in [prompt, merged_prompt, buffered_prompt] {
        assert!(result.status.success(), "{result:?}");
        assert_eq!(result.stdout, b"A");
        assert!(String::from_utf8_lossy(&result.stderr).contains("ScriptComplete, 5 instructions"));
    }
    assert_eq!(timed_out.status.code(), Some(1), "{timed_out:?}");
    assert!(timed_out.stdout.is_empty());
    assert!(String::from_utf8_lossy(&timed_out.stderr).contains("(Limit)"));
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
            .args(["run", "--max-steps", "1000", "--raw", "--platform"])
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
    assert_eq!(invalid.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("Cortex-M lockup at pc=0xeffffffe"));
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
