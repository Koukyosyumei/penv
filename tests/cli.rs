use std::{
    fs,
    io::{Read, Write},
    path::Path,
    process::{Command, Output, Stdio},
};

fn penv_command() -> Command {
    let path = std::env::var_os("PENV_BIN")
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_penv").into());
    Command::new(path)
}

fn upstream_stm32f072_command(fixture: &str, limit: &str) -> Command {
    let root = std::env::var_os("PENV_RENODE_ROOT").expect("set PENV_RENODE_ROOT");
    let assets = std::env::var_os("PENV_RENODE_ASSETS").expect("set PENV_RENODE_ASSETS");
    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(fixture);
    let mut command = penv_command();
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
        penv_command()
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

fn run_collapsed_stack_profile(
    fixture: &str,
    limit: Option<usize>,
    instructions: &[u16],
    stack: &str,
) {
    let directory = std::env::temp_dir().join(format!(
        "penv-collapsed-stack-{fixture}-{}",
        std::process::id()
    ));
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
"#,
    )
    .unwrap();
    let mut firmware = vec![0; 0x20];
    firmware[..8].copy_from_slice(&[0, 8, 0, 0, 0x21, 0, 0, 0]);
    for instruction in instructions {
        firmware.extend(instruction.to_le_bytes());
    }
    fs::write(directory.join("firmware.bin"), firmware).unwrap();
    let script = directory.join("profile.resc");
    let limit_arg = limit.map_or_else(String::new, |value| value.to_string());
    fs::write(&script, format!(
        "mach create\nmachine LoadPlatformDescription @board.repl\nsysbus LoadBinary @firmware.bin 0\ncpu EnableProfilerCollapsedStack \"trace.txt\" true {limit_arg}\nemulation RunFor \"0.001\"\n"
    )).unwrap();
    let output = penv_command()
        .args(["run", "--max-steps", "100000", "--script"])
        .arg(&script)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let profile = fs::read_to_string(directory.join("trace.txt")).unwrap();
    assert!(profile.contains(stack), "{profile}");
    if let Some(limit) = limit {
        assert!(profile.len() <= limit);
        assert_eq!(
            output.stderr,
            b"Profiler: Maximum file size exceeded, removing profiler\n"
        );
    } else {
        assert!(profile.len() > 100);
        assert!(output.stderr.is_empty(), "{output:?}");
    }
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn renode_resc_collapsed_stack_stops_at_file_limit() {
    run_collapsed_stack_profile(
        "bounded",
        Some(100),
        &[0xf000, 0xf801, 0xe7fc, 0x4770],
        "0x00000020;0x00000026",
    );
}

#[test]
fn renode_resc_collapsed_stack_is_unlimited_without_limit() {
    run_collapsed_stack_profile(
        "unlimited",
        None,
        &[0xf000, 0xf801, 0xe7fc, 0x4770],
        "0x00000020;0x00000026",
    );
}

#[test]
fn renode_resc_collapsed_stack_disables_during_chained_calls() {
    run_collapsed_stack_profile(
        "chained",
        Some(1000),
        &[0xf000, 0xf800, 0xf000, 0xf800, 0xf000, 0xf800, 0xe7f8],
        "0x00000020;0x00000024;0x00000028;0x0000002c",
    );
}

#[test]
fn renode_resc_guestless_snapshot_restores_monitor_state_in_command_order() {
    let directory = std::env::temp_dir().join(format!("penv-monitor-save-{}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    fs::write(directory.join("saved.resc"), "echo \"saved include\"\n").unwrap();
    fs::write(directory.join("changed.resc"), "echo \"changed include\"\n").unwrap();
    let script = directory.join("monitor.resc");
    fs::write(&script, r#"$name="saved value"
alias say echo
macro greet "echo 123"
$script=@saved.resc
Save "monitor state.save"
$script=@changed.resc
include $script
alias say log
macro greet "echo 456"
set name "changed"
echo $name
printenv
runMacro $greet
Load "monitor state.save"
include $script
echo $name
printenv
runMacro $greet
say "restored alias"
"#).unwrap();
    let output = penv_command().args(["run", "--script"]).arg(script).output().unwrap();
    fs::remove_dir_all(directory).unwrap();
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8(output.stdout).unwrap();
    let observations: Vec<_> = stdout.lines().filter(|line| {
        matches!(*line, "changed" | "saved value" | "456" | "123" | "restored alias" | "changed include" | "saved include")
            || line.starts_with("global.greet ") || line.starts_with("\tglobal.say = ")
    }).collect();
    assert_eq!(observations, [
        "changed include", "changed", r#"global.greet "echo 456""#, "\tglobal.say = log", "456",
        "saved include", "saved value", r#"global.greet "echo 123""#, "\tglobal.say = echo", "123", "restored alias",
    ]);
}

#[test]
fn renode_resc_snapshot_paths_use_the_current_monitor_directory() {
    let directory = std::env::temp_dir().join(format!("penv-monitor-cwd-{}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    fs::create_dir(directory.join("first")).unwrap();
    fs::create_dir(directory.join("second")).unwrap();
    let script = directory.join("snapshot.resc");
    fs::write(&script, r#"mach create
set global.CWD $ORIGIN/first
set marker "first"
Save $CWD/slot.save
set global.CWD $ORIGIN/second
set marker "second"
Save $CWD/slot.save
set marker "changed"
Load $CWD/slot.save
echo $marker
set global.CWD $ORIGIN/first
Load $CWD/slot.save
echo $marker
"#).unwrap();
    let output = penv_command().args(["run", "--script"]).arg(script).output().unwrap();
    let saved_both = ["first", "second"].iter()
        .all(|subdir| directory.join(subdir).join("slot.save").is_file());
    fs::remove_dir_all(directory).unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(saved_both);
    assert_eq!(output.stdout, b"second\nfirst\n");
}

#[test]
fn renode_resc_array_memory_save_clear_load_restores_boot_state() {
    let directory = std::env::temp_dir().join(format!("penv-array-save-{}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    fs::write(
        directory.join("board.repl"),
        r#"cpu: CPU.CortexM @ sysbus
    cpuType: "cortex-m0+"
    nvic: nvic
nvic: IRQControllers.NVIC @ sysbus 0xe000e000
    -> cpu@0
flash: Memory.ArrayMemory @ sysbus 0x08000000
    size: 0x80000
sram: Memory.ArrayMemory @ sysbus 0x20000000
    size: 0x20000
"#,
    )
    .unwrap();
    fs::write(
        directory.join("firmware.bin"),
        [0, 8, 0, 0x20, 9, 0, 0, 8, 0, 0xbf, 0xfe, 0xe7],
    )
    .unwrap();
    let script = directory.join("save.resc");
    let run = |commands: &str| {
        fs::write(&script, format!(
            "mach create\nmachine LoadPlatformDescription @board.repl\nsysbus LoadBinary @firmware.bin 0x08000000\ncpu VectorTableOffset 0x08000000\n{commands}"
        )).unwrap();
        penv_command()
            .args(["run", "--max-steps", "10", "--script"])
            .arg(&script)
            .output()
            .unwrap()
    };
    let saved = run(r#"sysbus WriteDoubleWord 0x20000000 0x12345678
Save @test.save
sysbus WriteDoubleWord 0x20000000 0x87654321
Clear
Load @test.save
mach set 0
sysbus ReadDoubleWord 0x20000000
cpu PC
cpu Step 1
cpu PC
"#);
    assert!(saved.status.success(), "{saved:?}");
    assert_eq!(saved.stdout, b"0x12345678\n0x08000008\n0x0800000a\n");
    let after_run = run("cpu Step 1\nSave @too-late.save\n");
    assert_eq!(after_run.status.code(), Some(1), "{after_run:?}");
    assert!(!directory.join("too-late.save").exists());
    fs::OpenOptions::new()
        .write(true)
        .open(directory.join("test.save"))
        .unwrap()
        .set_len(20)
        .unwrap();
    let truncated = run("Clear\nLoad @test.save\n");
    assert_eq!(truncated.status.code(), Some(1), "{truncated:?}");
    fs::remove_dir_all(directory).unwrap();
}

#[cfg(all(target_os = "linux", target_pointer_width = "64"))]
#[test]
fn renode_resc_machine_rtc_modes_share_one_creation_time() {
    let directory = std::env::temp_dir().join(format!("penv-rtc-modes-{}", std::process::id()));
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
"#,
    )
    .unwrap();
    let script = directory.join("rtc-modes.resc");
    fs::write(
        &script,
        r#"mach create
machine LoadPlatformDescription @board.repl
machine RealTimeClockDateTime
machine RealTimeClockMode HostTimeLocal
machine RealTimeClockDateTime
machine RealTimeClockMode HostTimeUTC
machine RealTimeClockDateTime
machine RealTimeClockMode Epoch
machine RealTimeClockDateTime
"#,
    )
    .unwrap();
    let result = penv_command()
        .env("TZ", "UTC0")
        .args(["run", "--script"])
        .arg(&script)
        .output()
        .unwrap();
    assert!(result.status.success(), "{result:?}");
    let stdout = String::from_utf8(result.stdout).unwrap();
    let values: Vec<_> = stdout.lines().collect();
    assert_eq!(values.len(), 4);
    assert_eq!(values[0], "1970-01-01 00:00:00");
    assert_eq!(values[1], values[2]);
    assert_ne!(values[1], values[0]);
    assert_eq!(values[3], values[0]);
    fs::remove_dir_all(directory).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn renode_resc_opens_host_uart_pty_without_booting() {
    let directory = std::env::temp_dir().join(format!("penv-resc-pty-{}", std::process::id()));
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
"#,
    )
    .unwrap();
    let link = directory.join("allocated tty");
    fs::write(&link, b"allocated file").unwrap();
    let script = directory.join("host-uart.resc");
    let run = |force: bool| {
        fs::write(&script, format!(
            "mach create\nmachine LoadPlatformDescription @board.repl\nemulation CreateUartPtyTerminal \"tmp\" \"{}\" {force}\n",
            link.display()
        )).unwrap();
        penv_command()
            .args(["run", "--script"])
            .arg(&script)
            .output()
            .unwrap()
    };
    let refused = run(false);
    assert_eq!(refused.status.code(), Some(1), "{refused:?}");
    assert_eq!(fs::read(&link).unwrap(), b"allocated file");
    let created = run(true);
    assert!(created.status.success(), "{created:?}");
    assert!(fs::symlink_metadata(&link).is_err());
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn renode_resc_global_array_memory_above_32bit_preserves_guest_writes() {
    let directory = std::env::temp_dir().join(format!("penv-high-ram-{}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    fs::write(
        directory.join("board.repl"),
        r#"cpu: CPU.CortexM @ sysbus
    cpuType: "cortex-m0"
    nvic: nvic
nvic: IRQControllers.NVIC @ sysbus 0xe000e000
    -> cpu@0
vectors: Memory.MappedMemory @ sysbus 0
    size: 8
ram: Memory.MappedMemory @ sysbus 0x40000000
    size: 0x8000
high: Memory.ArrayMemory @ sysbus 0x8_0000_0000
    size: 0x8000
first: Memory.ArrayMemory @ sysbus 1000
    size: 100
second: Memory.ArrayMemory @ sysbus 2000
    size: 100
third: Memory.ArrayMemory @ sysbus 3000
    size: 100
"#,
    )
    .unwrap();
    let mut program = Vec::new();
    for instruction in [
        0x4803_u16, 0x4904, 0x6000, 0x3004, 0x4288, 0xd3fb, 0xe7f8, 0xbf00,
    ] {
        program.extend(instruction.to_le_bytes());
    }
    program.extend(0x40000000_u32.to_le_bytes());
    program.extend(0x40007000_u32.to_le_bytes());
    fs::write(directory.join("loop.bin"), program).unwrap();
    fs::write(
        directory.join("vectors.bin"),
        [0, 0x80, 0, 0x40, 1, 0x70, 0, 0x40],
    )
    .unwrap();
    let script = directory.join("high-ram.resc");
    fs::write(
        &script,
        r#"mach create
machine LoadPlatformDescription @board.repl
sysbus LoadBinary @vectors.bin 0
sysbus LoadBinary @loop.bin 0x40007000
sysbus WriteDoubleWord 1000 0x666
sysbus WriteDoubleWord 2000 0x667
sysbus WriteDoubleWord 3000 0x668
sysbus ReadDoubleWord 1000
sysbus ReadDoubleWord 2000
sysbus ReadDoubleWord 3000
sysbus WriteDoubleWord 0x8_0000_0000 0x12345678
emulation RunFor "0.001"
sysbus ReadDoubleWord 0x40000000
sysbus ReadDoubleWord 0x40006ffc
sysbus ReadDoubleWord 0x8_0000_0000
sysbus ReadWord 0x8_0000_0002
sysbus ReadByte 0x8_0000_0003
"#,
    )
    .unwrap();
    let output = penv_command()
        .args(["run", "--max-steps", "100000", "--script"])
        .arg(script)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        output.stdout,
        b"0x00000666\n0x00000667\n0x00000668\n0x40000000\n0x40006ffc\n0x12345678\n0x1234\n0x12\n"
    );
    assert!(!String::from_utf8_lossy(&output.stderr).contains("Invalidating memory fragment"));
    // The single-registration upstream fixture uses a 1000-byte range.
    fs::write(
        directory.join("single.repl"),
        r#"cpu: CPU.CortexM @ sysbus
    cpuType: "cortex-m0"
    nvic: nvic
nvic: IRQControllers.NVIC @ sysbus 0xe000e000
    -> cpu@0
vectors: Memory.MappedMemory @ sysbus 0
    size: 8
ram: Memory.MappedMemory @ sysbus 0x40000000
    size: 0x8000
first: Memory.ArrayMemory @ sysbus 1000
    size: 1000
"#,
    )
    .unwrap();
    let single = directory.join("single.resc");
    fs::write(
        &single,
        r#"mach create
machine LoadPlatformDescription @single.repl
sysbus LoadBinary @vectors.bin 0
sysbus LoadBinary @loop.bin 0x40007000
sysbus WriteDoubleWord 1000 0x666
sysbus ReadDoubleWord 1000
"#,
    )
    .unwrap();
    let output = penv_command()
        .args(["run", "--max-steps", "1", "--script"])
        .arg(single)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(output.stdout, b"0x00000666\n");
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn renode_resc_cortex_m_hook_logs_at_current_pc() {
    let directory = std::env::temp_dir().join(format!("penv-cpu-hook-{}", std::process::id()));
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
"#,
    )
    .unwrap();
    fs::write(
        directory.join("firmware.bin"),
        [0, 8, 0, 0, 9, 0, 0, 0, 0, 0xbf, 0xfe, 0xe7],
    )
    .unwrap();
    let script = directory.join("hook.resc");
    fs::write(
        &script,
        r#"mach create
machine LoadPlatformDescription @board.repl
sysbus LoadBinary @firmware.bin 0
cpu AddHook `cpu PC` "monitor.Parse('log \"message from the cpu hook\"')"
sysbus.cpu AddHook 0x0a "monitor.Parse('log \"second PC\"')"
cpu Pause
emulation RunFor "0.00000001"
cpu Resume
emulation RunFor "0.00000002"
"#,
    )
    .unwrap();
    let output = penv_command()
        .args(["run", "--max-steps", "2", "--script"])
        .arg(script)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    assert_eq!(output.stderr, b"message from the cpu hook\nsecond PC\n");
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn renode_resc_cortex_m_hook_enables_systick_and_pauses() {
    let directory = std::env::temp_dir().join(format!("penv-systick-hook-{}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    fs::write(
        directory.join("board.repl"),
        r#"cpu: CPU.CortexM @ sysbus
    cpuType: "cortex-m0"
    nvic: nvic
nvic: IRQControllers.NVIC @ sysbus 0xe000e000
    -> cpu@0
ram: Memory.MappedMemory @ sysbus 0
    size: 0x10000
"#,
    )
    .unwrap();
    let mut firmware = [0; 0x30];
    firmware[..8].copy_from_slice(&[0, 8, 0, 0, 9, 0, 0, 0]);
    fs::write(directory.join("firmware.bin"), firmware).unwrap();
    let script = directory.join("systick-hook.resc");
    fs::write(
        &script,
        r#"mach create
machine LoadPlatformDescription @board.repl
sysbus LoadBinary @firmware.bin 0
cpu PC 0
logLevel -1 nvic
nvic WriteDoubleWord 0x14 0
cpu AddHook 0x10 "machine.SystemBus.WriteDoubleWord(0xe000e010, 0x1)"
cpu AddHook 0x20 "machine.PauseAndRequestEmulationPause()"
emulation RunFor "0.001"
cpu IsPaused
cpu PC
sysbus ReadDoubleWord 0xe000e010
emulation RunFor "0.001"
cpu PC
cpu Resume
emulation RunFor "0.00000001"
cpu PC
"#,
    )
    .unwrap();
    let output = penv_command()
        .args(["run", "--max-steps", "18", "--script"])
        .arg(script)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        output.stdout,
        b"True\n0x00000022\n0x00000005\n0x00000022\n0x00000024\n"
    );
    assert_eq!(
        output.stderr,
        b"Systick_NS enabled but it won't be started as long as the reload value is zero\nMachine paused\n"
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn renode_resc_gpio_button_fans_out_to_distinct_pins() {
    let directory = std::env::temp_dir().join(format!("penv-gpio-fanout-{}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    let repl = directory.join("board.repl");
    let platform = r#"cpu: CPU.CortexM @ sysbus
    cpuType: "cortex-m0"
    nvic: nvic
nvic: IRQControllers.NVIC @ sysbus 0xe000e000
    -> cpu@0
ram: Memory.MappedMemory @ sysbus 0
    size: 0x1000
gpioA: GPIOPort.STM32_GPIOPort @ sysbus 0x48000000
gpioB: GPIOPort.STM32_GPIOPort @ sysbus 0x48000400
button: Miscellaneous.Button @ none
    -> gpioA@2 | gpioB@3 | gpioA@2
unused: Miscellaneous.Button @ none
"#;
    fs::write(&repl, platform).unwrap();
    fs::write(
        directory.join("firmware.bin"),
        [0, 8, 0, 0, 9, 0, 0, 0, 0xfe, 0xe7],
    )
    .unwrap();
    let script = directory.join("fanout.resc");
    fs::write(
        &script,
        "mach create\nmachine LoadPlatformDescription @board.repl\nsysbus LoadBinary @firmware.bin 0\n\
         unused PressAndRelease\nbutton Press\nsysbus ReadDoubleWord 0x48000010\nsysbus ReadDoubleWord 0x48000410\n\
         button Release\nsysbus ReadDoubleWord 0x48000010\nsysbus ReadDoubleWord 0x48000410\n\
         sysbus Unregister sysbus.gpioA\nbutton Press\nsysbus ReadDoubleWord 0x48000010\nsysbus ReadDoubleWord 0x48000410\n\
         machine UnregisterFromParent sysbus.gpioB\nbutton Release\nsysbus ReadDoubleWord 0x48000410\n",
    ).unwrap();
    let run = || {
        penv_command()
            .args(["run", "--max-steps", "1", "--script"])
            .arg(&script)
            .output()
            .unwrap()
    };
    let output = run();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        output.stdout,
        b"0x00000004\n0x00000008\n0x00000000\n0x00000000\n0x00000000\n0x00000008\n0x00000000\n"
    );
    fs::write(&repl, platform.replace("gpioB@3", "gpioB@16")).unwrap();
    let invalid = run();
    assert_eq!(invalid.status.code(), Some(1), "{invalid:?}");
    assert!(
        String::from_utf8_lossy(&invalid.stderr)
            .contains("Cannot connect 16th input of gpioB; it has only 16 GPIO inputs.")
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn renode_resc_cortex_m_pause_halt_and_virtual_time() {
    let directory = std::env::temp_dir().join(format!("penv-cpu-flow-{}", std::process::id()));
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
    frequency: 100000000
    initialLimit: 0xffffffff
"#,
    )
    .unwrap();
    let mut firmware = vec![0; 0x100];
    firmware[..8].copy_from_slice(&[0, 8, 0, 0, 1, 1, 0, 0]);
    // Upstream nop; nop; nop; jump: nop; b jump.
    firmware.extend([0, 0xbf, 0, 0xbf, 0, 0xbf, 0, 0xbf, 0xfd, 0xe7]);
    fs::write(directory.join("firmware.bin"), firmware).unwrap();
    let script = directory.join("flow.resc");
    let run_and_observe = concat!(
        "emulation RunFor \"0.00000001\"\n",
        "cpu IsPaused\ncpu IsHalted\ncpu PC\nsysbus ReadDoubleWord 0x40000024\n",
    );
    let run_case = |phases: &[(&str, bool, bool, u32, u32)]| {
        let mut commands = String::from(concat!(
            "mach create\nmachine LoadPlatformDescription @board.repl\n",
            "sysbus LoadBinary @firmware.bin 0\nsysbus WriteDoubleWord 0x40000000 1\n",
            "macro reset \"\"\" sleep 0.5; log \"Machine resetting\" \"\"\"\n",
            "emulation RunFor \"0.00000002\"\n",
        ));
        let mut expected = String::new();
        for &(change, paused, halted, pc, ticks) in phases {
            use std::fmt::Write as _;
            commands.push_str(change);
            if change == "machine Reset\n" {
                commands.push_str("cpu PC\nsysbus WriteDoubleWord 0x40000000 1\n");
                expected.push_str("Machine resetting\n0x00000100\n");
            }
            commands.push_str("emulation SynchroniseQuantum\n");
            commands.push_str(run_and_observe);
            let flag = |state| if state { "True" } else { "False" };
            writeln!(
                expected,
                "{}\n{}\n0x{pc:08x}\n0x{ticks:08x}",
                flag(paused),
                flag(halted)
            )
            .unwrap();
        }
        fs::write(&script, commands).unwrap();
        let result = penv_command()
            .args(["run", "--max-steps", "10", "--script"])
            .arg(&script)
            .output()
            .unwrap();
        assert!(result.status.success(), "{result:?}");
        assert_eq!(
            String::from_utf8(result.stdout).unwrap(),
            expected,
            "{phases:?}"
        );
    };
    run_case(&[("", false, false, 0x106, 3)]);
    run_case(&[
        ("cpu Pause\n", true, false, 0x104, 2),
        ("cpu Resume\n", false, false, 0x106, 3),
    ]);
    run_case(&[
        ("cpu IsHalted true\n", false, true, 0x104, 3),
        ("cpu Resume\n", false, true, 0x104, 4),
    ]);
    for first in [
        "cpu Pause\ncpu IsHalted true\n",
        "cpu IsHalted true\ncpu Pause\n",
    ] {
        for resume_first in [true, false] {
            let (middle, last, ticks) = if resume_first {
                (
                    ("cpu Resume\n", false, true, 0x104, 3),
                    "cpu IsHalted false\n",
                    4,
                )
            } else {
                (
                    ("cpu IsHalted false\n", true, false, 0x104, 2),
                    "cpu Resume\n",
                    3,
                )
            };
            run_case(&[
                (first, true, true, 0x104, 2),
                middle,
                (last, false, false, 0x106, ticks),
            ]);
        }
    }
    for (before_reset, paused, halted, ticks) in [
        ("", false, false, 3),
        ("cpu IsHalted true\n", false, true, 3),
        ("cpu Pause\n", true, false, 2),
        ("cpu Pause\ncpu IsHalted true\n", true, true, 2),
    ] {
        run_case(&[
            (
                before_reset,
                paused,
                halted,
                if paused || halted { 0x104 } else { 0x106 },
                ticks,
            ),
            ("cpu Reset\n", true, false, 0x100, ticks),
            ("cpu Resume\n", false, false, 0x102, ticks + 1),
        ]);
        run_case(&[
            (
                before_reset,
                paused,
                halted,
                if paused || halted { 0x104 } else { 0x106 },
                ticks,
            ),
            ("machine Reset\n", false, false, 0x102, 1),
            ("", false, false, 0x104, 2),
            ("", false, false, 0x106, 3),
        ]);
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
        penv_command()
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
    let result = penv_command()
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
        penv_command()
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
        penv_command()
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
        penv_command()
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
        penv_command()
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
        let mut command = penv_command();
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
        let mut command = penv_command();
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
        penv_command()
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
    let result = penv_command()
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
        let mut command = penv_command();
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
        penv_command()
            .args(["run", "--max-steps", "1000", "--raw", "--platform"])
            .arg(&board)
            .arg(&firmware)
            .output()
            .unwrap()
    };
    let output = invoke();
    let reset = penv_command()
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
    let arm_elf = penv_command()
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
    let result = penv_command()
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
    let output = penv_command()
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
        penv_command()
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
        let output = penv_command()
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
    let result = penv_command()
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
        penv_command()
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
    let result = penv_command()
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
    let result = penv_command()
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
    let result = penv_command()
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
    let result = penv_command()
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

#[test]
fn renode_find_bytes_uses_shared_single_and_multi_machine_monitor() {
    let directory = std::env::temp_dir().join(format!("penv-find-bytes-{}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("find.repl"),
        r#"cpu: CPU.CortexM @ sysbus
    cpuType: "cortex-m0"
    nvic: nvic
nvic: IRQControllers.NVIC @ sysbus 0xe000e000
    -> cpu@0
ramA: Memory.MappedMemory @ sysbus 0
    size: 0x100000
ramB: Memory.MappedMemory @ sysbus 0x100000
    size: 0x100000
"#,
    )
    .unwrap();
    fs::write(directory.join("first.bin"), [0, 1, 2, 3]).unwrap();
    fs::write(directory.join("second.bin"), [4, 5, 6, 7]).unwrap();
    fs::write(
        directory.join("vectors.bin"),
        [0, 8, 0, 0, 9, 0, 0, 0, 0xfe, 0xe7],
    )
    .unwrap();
    let script = directory.join("find.resc");
    fs::write(
        &script,
        r#"mach create "search"
machine LoadPlatformDescription @find.repl
sysbus LoadBinary @vectors.bin 0
sysbus LoadBinary @first.bin 0xffffc
sysbus LoadBinary @second.bin 0x100000
sysbus FindBytes "00 01 02 03 04 05 06 07"
sysbus FindBytes [3, 4] 0xfffff 0x100000
sysbus FindBytes [3, 4] 0xfffff 0xfffff
emulation RunFor "0"
"#,
    )
    .unwrap();
    fs::create_dir(directory.join("uart")).unwrap();
    for simulation in [false, true] {
        let mut command = penv_command();
        command
            .args(["run", "--max-steps", "1", "--script"])
            .arg(&script);
        if simulation {
            command.arg("--uart-log-dir").arg(directory.join("uart"));
        }
        let output = command.output().unwrap();
        assert!(output.status.success(), "{output:?}");
        let stdout = String::from_utf8(output.stdout).unwrap();
        let expected = if simulation {
            "search: sysbus matches = [0x000ffffc]\nsearch: sysbus matches = [0x000fffff]\nsearch: sysbus matches = []\n"
        } else {
            "[0x000ffffc]\n[0x000fffff]\n[]\n"
        };
        assert_eq!(stdout, expected);
    }
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn renode_repl_reports_duplicate_attribute_before_overlays() {
    let directory =
        std::env::temp_dir().join(format!("penv-repl-duplicate-{}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let platform = directory.join("board.repl");
    let script = directory.join("boot.resc");
    fs::write(
        &script,
        "mach create\nmachine LoadPlatformDescription @board.repl\nsysbus LoadBinary @vectors.bin 0\nemulation RunFor \"0\"\n",
    )
    .unwrap();
    fs::write(directory.join("vectors.bin"), [0, 8, 0, 0, 9, 0, 0, 0]).unwrap();
    fs::create_dir(directory.join("uart")).unwrap();
    for (properties, line) in [
        ("\n    size: 8\n    size: 16", 4),
        (" { size: 8; size: 16 }", 2),
    ] {
        fs::write(
            &platform,
            format!(
                "\nexternal: Antmicro.Renode.UnitTests.Mocks.MockCPU{properties}\nexternal:\n    size: 32\n"
            ),
        )
        .unwrap();
        for simulation in [false, true] {
            let mut command = penv_command();
            command.args(["run", "--script"]).arg(&script);
            if simulation {
                command.arg("--uart-log-dir").arg(directory.join("uart"));
            }
            let result = command.output().unwrap();
            assert!(!result.status.success(), "{result:?}");
            let stderr = String::from_utf8(result.stderr).unwrap();
            assert!(
                stderr.contains(&format!(
                    "board.repl:{line}: external: duplicate REPL property \"size\""
                )),
                "{stderr}"
            );
        }
    }
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn renode_resc_preboot_snapshot_restores_array_memory_and_vector_table() {
    let directory = std::env::temp_dir().join(format!("penv-boot-snapshot-{}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("snapshot.repl"),
        r#"cpu: CPU.CortexM @ sysbus
    cpuType: "cortex-m0+"
    nvic: nvic
nvic: IRQControllers.NVIC @ sysbus 0xe000e000
    -> cpu@0
flash: Memory.ArrayMemory @ sysbus 0x08000000
    size: 0x80000
ram: Memory.ArrayMemory @ sysbus 0x20000000
    size: 0x20000
"#,
    )
    .unwrap();
    fs::write(
        directory.join("boot.bin"),
        [0, 8, 0, 0x20, 9, 0, 0, 8, 0xfe, 0xe7],
    )
    .unwrap();
    let script = directory.join("snapshot.resc");
    let run = |actions: &str| {
        fs::write(
            &script,
            format!("mach create\nmachine LoadPlatformDescription @snapshot.repl\nsysbus LoadBinary @boot.bin 0x08000000\ncpu VectorTableOffset 0x08000000\n{actions}"),
        ).unwrap();
        penv_command()
            .args(["run", "--max-steps", "1000", "--script"])
            .arg(&script)
            .output()
            .unwrap()
    };
    let restored = run(
        "Save @test.save\nClear\nLoad @test.save\nmach set 0\nsysbus FindBytes \"fe e7\"\nemulation RunFor \"0.000001\"\n",
    );
    assert!(restored.status.success(), "{restored:?}");
    assert_eq!(restored.stdout, b"[0x08000008]\n");

    let late = run("emulation RunFor \"0.000001\"\nSave @late.save\n");
    assert_eq!(late.status.code(), Some(1), "{late:?}");
    assert!(String::from_utf8_lossy(&late.stderr).contains("before execution"));
    assert!(!directory.join("late.save").exists());

    fs::OpenOptions::new()
        .write(true)
        .open(directory.join("test.save"))
        .unwrap()
        .set_len(20)
        .unwrap();
    let truncated = run("Clear\nLoad @test.save\n");
    assert_eq!(truncated.status.code(), Some(1), "{truncated:?}");
    assert!(String::from_utf8_lossy(&truncated.stderr).contains("test.save"));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn cli_setup_memory_preserves_signed_values_on_rv64_and_cortex_m() {
    let directory = std::env::temp_dir().join(format!("penv-memory-setup-{}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let repl = directory.join("memory.repl");
    for cpu in [
        "cpu: CPU.RiscV64 @ sysbus\n    cpuType: \"rv64gc\"\n",
        "cpu: CPU.CortexM @ sysbus\n    cpuType: \"cortex-m4\"\n    nvic: nvic\nnvic: IRQControllers.NVIC @ sysbus 0xe000e000\n    -> cpu@0\n",
    ] {
        fs::write(
            &repl,
            format!("{cpu}mem: Memory.MappedMemory @ sysbus 0\n    size: 0x40000\n"),
        )
        .unwrap();
        let run = |options: &[&str]| {
            penv_command()
                .args(["run", "--max-steps", "0", "--platform"])
                .arg(&repl)
                .args(options)
                .output()
                .unwrap()
        };
        let result = run(&[
            "--read-memory-now",
            "8:8",
            "--write-memory",
            "8:8:-1000000000000000000",
            "--read-memory-now",
            "8:8",
            "--write-memory",
            "0x10:1:-128",
            "--read-memory-now",
            "0x10:1",
            "--write-memory",
            "0x12:2:-32768",
            "--read-memory-now",
            "0x12:2",
            "--write-memory",
            "0x14:4:-2147483648",
            "--read-memory-now",
            "0x14:4",
            "--write-memory",
            "0x10:1:255",
            "--read-memory-now",
            "0x10:1",
        ]);
        assert!(result.status.success(), "{result:?}");
        assert_eq!(
            String::from_utf8(result.stderr).unwrap(),
            concat!(
                "penv: memory sysbus@0x0000000000000008:8 = 0x0000000000000000\n",
                "penv: memory sysbus@0x0000000000000008:8 = 0xf21f494c589c0000\n",
                "penv: memory sysbus@0x0000000000000010:1 = 0x0000000000000080\n",
                "penv: memory sysbus@0x0000000000000012:2 = 0x0000000000008000\n",
                "penv: memory sysbus@0x0000000000000014:4 = 0x0000000080000000\n",
                "penv: memory sysbus@0x0000000000000010:1 = 0x00000000000000ff\n",
            )
        );
        for value in [
            "8:1:-129",
            "8:1:256",
            "8:2:-32769",
            "8:4:-2147483649",
            "8:8:-9223372036854775809",
        ] {
            let result = run(&["--write-memory", value]);
            assert_eq!(result.status.code(), Some(1), "{result:?}");
            assert!(
                String::from_utf8_lossy(&result.stderr).contains("exceeds the requested width")
            );
        }
        let result = run(&["--read-memory-now", "0x40000:1"]);
        assert_eq!(result.status.code(), Some(1), "{result:?}");
        assert!(String::from_utf8_lossy(&result.stderr).contains("unmapped RAM read"));
    }
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn renode_resc_sysbus_scalar_memory_round_trips_on_rv32_and_cortex_m() {
    let directory = std::env::temp_dir().join(format!("penv-sysbus-scalar-{}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let script = directory.join("memory.resc");
    fs::write(
        &script,
        r#"mach create
machine LoadPlatformDescription @memory.repl
sysbus WriteByte 0x1 0x42
sysbus ReadByte 0x1
sysbus WriteWord 0x2 0xdead
sysbus ReadWord 0x2
sysbus WriteDoubleWord 0x4 0xdeadbeef
sysbus ReadDoubleWord 0x4
sysbus WriteQuadWord 0x8 -1000000000000000000
sysbus ReadQuadWord 0x8
sysbus WriteByte 0x20 -1
sysbus ReadByte 0x20
"#,
    )
    .unwrap();
    for cpu in [
        "cpu: CPU.RiscV32 @ sysbus\n    cpuType: \"rv32i\"\n",
        "cpu: CPU.CortexM @ sysbus\n    cpuType: \"cortex-m4\"\n    nvic: nvic\nnvic: IRQControllers.NVIC @ sysbus 0xe000e000\n    -> cpu@0\n",
    ] {
        fs::write(
            directory.join("memory.repl"),
            format!("{cpu}ram: Memory.MappedMemory @ sysbus 0\n    size: 0x40000\n"),
        )
        .unwrap();
        let result = penv_command()
            .args(["run", "--script"])
            .arg(&script)
            .output()
            .unwrap();
        assert!(result.status.success(), "{result:?}");
        assert_eq!(
            result.stdout,
            b"0x42\n0xdead\n0xdeadbeef\n0xf21f494c589c0000\n0xff\n"
        );
    }
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn renode_resc_sd_card_file_cases_use_the_named_controller() {
    let directory = std::env::temp_dir().join(format!("penv-sd-file-{}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join("board.repl"),
        r#"cpu: CPU.CortexM @ sysbus
    cpuType: "cortex-m7"
    nvic: nvic
nvic: IRQControllers.NVIC @ sysbus 0xe000e000
    -> cpu@0
ram: Memory.MappedMemory @ sysbus 0
    size: 0x4000
sdmmc: SD.STM32HSDMMC @ sysbus 0x52007000
second: SD.STM32HSDMMC @ sysbus 0x5200a000
"#,
    )
    .unwrap();
    fs::write(directory.join("card input.img"), [1, 2, 3, 4]).unwrap();
    let script = directory.join("sd.resc");
    for (file, expected) in [
        (r#""card input.img""#, None),
        (r#"@"card input.img""#, None),
        (
            r#""/doesntexist""#,
            Some("Could not find image file '/doesntexist'"),
        ),
        (r#""""#, Some("No image file provided")),
    ] {
        for network in [false, true] {
            let mut body = format!(
                "mach create \"board\"\nmachine LoadPlatformDescription @board.repl\nmachine SdCardFromFile {file} sysbus.second 0x1000 false\n"
            );
            if network {
                body.push_str("mach create \"other\"\nmachine LoadPlatformDescription @board.repl\nmachine SdCardFromFile \"card input.img\" sdmmc 0x1000\n");
            }
            fs::write(&script, body).unwrap();
            let result = penv_command()
                .args(["run", "--script"])
                .arg(&script)
                .output()
                .unwrap();
            assert_eq!(result.status.success(), expected.is_none(), "{result:?}");
            if let Some(error) = expected {
                assert!(
                    String::from_utf8_lossy(&result.stderr).contains(error),
                    "{result:?}"
                );
            }
        }
    }
    assert_eq!(
        fs::read(directory.join("card input.img")).unwrap(),
        [1, 2, 3, 4]
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn rv64_renode_elf_load_preserves_offsets_across_unmapped_ram() {
    let directory = std::env::temp_dir().join(format!("penv-elf-ranges-{}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    let elf = directory.join("ranges.elf");
    let platform = directory.join("ranges.repl");
    fs::write(
        &platform,
        r#"cpu: CPU.RiscV64 @ sysbus
    cpuType: "rv64i"
first: Memory.MappedMemory @ sysbus 0x80000000
    size: 0x10
second: Memory.MappedMemory @ sysbus 0x80000020
    size: 0x4
"#,
    )
    .unwrap();
    let mut image = [0u8; 120 + 36];
    image[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
    image[16..18].copy_from_slice(&2u16.to_le_bytes());
    image[18..20].copy_from_slice(&243u16.to_le_bytes());
    image[20..24].copy_from_slice(&1u32.to_le_bytes());
    image[24..32].copy_from_slice(&0x8000_0000u64.to_le_bytes());
    image[32..40].copy_from_slice(&64u64.to_le_bytes());
    image[52..54].copy_from_slice(&64u16.to_le_bytes());
    image[54..56].copy_from_slice(&56u16.to_le_bytes());
    image[56..58].copy_from_slice(&1u16.to_le_bytes());
    image[64..68].copy_from_slice(&1u32.to_le_bytes()); // PT_LOAD
    image[68..72].copy_from_slice(&5u32.to_le_bytes()); // executable
    image[72..80].copy_from_slice(&120u64.to_le_bytes());
    image[80..88].copy_from_slice(&0x8000_0000u64.to_le_bytes());
    image[88..96].copy_from_slice(&0x8000_0000u64.to_le_bytes());
    image[96..104].copy_from_slice(&36u64.to_le_bytes());
    image[104..112].copy_from_slice(&40u64.to_le_bytes());
    image[120..124].copy_from_slice(&0x0200_006fu32.to_le_bytes()); // j +32
    image[152..156].copy_from_slice(&0x0010_0073u32.to_le_bytes()); // ebreak
    fs::write(&elf, image).unwrap();

    let invoke = |options: &[&str]| {
        penv_command()
            .args(["run", "--platform"])
            .arg(&platform)
            .args(options)
            .arg(&elf)
            .output()
            .unwrap()
    };
    let strict = invoke(&[]);
    let compatible = invoke(&["--renode-elf-load"]);
    fs::remove_dir_all(directory).unwrap();
    assert!(!strict.status.success());
    assert!(String::from_utf8_lossy(&strict.stderr).contains("unmapped ELF64 segment"));
    assert!(compatible.status.success(), "{compatible:?}");
    let warnings = String::from_utf8_lossy(&compatible.stderr);
    assert!(warnings.contains("0x0000000080000010..0x0000000080000020"));
    assert!(warnings.contains("0x0000000080000024..0x0000000080000028"));
}

#[test]
fn ns16550_can_echo_without_an_interrupt_controller() {
    let directory =
        std::env::temp_dir().join(format!("penv-polled-ns16550-{}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    let board = directory.join("board.repl");
    let raw = directory.join("echo.bin");
    let input = directory.join("input");
    fs::write(
        &board,
        "cpu: CPU.RiscV32 @ sysbus
    cpuType: \"rv32imac_zba_zbb_zbc_zbs\"
mem: Memory.MappedMemory @ sysbus 0x80000000
    size: 0x1000
uart: UART.NS16550 @ sysbus 0x10000000
",
    )
    .unwrap();
    let program = [
        0x1000_02b7u32, // lui t0, 0x10000
        0x0002_c303,    // lbu t1, 0(t0)
        0x0062_8023,    // sb t1, 0(t0)
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
    let output = penv_command()
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
fn uart_plan_enforces_virtual_time_and_detects_unfinished_output() {
    let directory = std::env::temp_dir().join(format!("penv-uart-check-{}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    let board = directory.join("board.repl");
    let firmware = directory.join("firmware.bin");
    let plan = directory.join("uart.tsv");
    fs::write(
        &board,
        r#"mem: Memory.MappedMemory @ sysbus 0 { size: 0x1000 }
nvic: IRQControllers.NVIC @ sysbus 0xe000e000 { -> cpu@0 }
cpu: CPU.CortexM @ sysbus { cpuType: "cortex-m4"; nvic: nvic }
uart: UART.NS16550 @ sysbus 0x40000000
"#,
    )
    .unwrap();
    fs::write(
        &firmware,
        [
            0x00, 0x10, 0x00, 0x00, 0x09, 0x00, 0x00, 0x00, // MSP and reset vector.
            0x4f, 0xf0, 0x80, 0x40, // mov.w r0, #0x40000000
            0x41, 0x21, 0x01, 0x70, // Write 'A'.
            0x0a, 0x21, 0x01, 0x70, // Write the line ending.
            0x30, 0xbf, 0xfd, 0xe7, // Wait for interrupts in a loop.
        ],
    )
    .unwrap();
    let run = |commands: &str| {
        fs::write(&plan, commands).unwrap();
        penv_command()
            .args(["run", "--raw", "--platform"])
            .arg(&board)
            .arg("--uart-check")
            .arg(&plan)
            .arg(&firmware)
            .output()
            .unwrap()
    };
    let complete = run("wait\t8\tA\nsecond\t0\nidle\t1\nsecond\t1\n");
    let idle_violation = run("idle\t1\n");
    let timeout = run("wait\t8\tmissing\n");
    fs::remove_dir_all(directory).unwrap();

    assert!(complete.status.success(), "{complete:?}");
    assert_eq!(complete.stdout, b"A\n");
    assert!(!idle_violation.status.success(), "{idle_violation:?}");
    assert_eq!(idle_violation.stdout, b"A");
    assert!(
        String::from_utf8_lossy(&idle_violation.stderr)
            .contains("UART output during 1 s idle window")
    );
    assert!(!timeout.status.success(), "{timeout:?}");
    assert!(String::from_utf8_lossy(&timeout.stderr).contains("UART timeout after 8 s"));
}
