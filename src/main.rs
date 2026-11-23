mod bus;
mod clint;
mod compressed;
mod constant_mmio;
mod crc;
mod csr;
mod dwt;
mod ethernet;
mod ethernet_switch;
mod image;
mod limit_timer;
mod litex_ethernet;
mod machine;
mod mii_phy;
mod monitor;
mod murax;
mod network_monitor;
mod network_platform;
mod ns16550;
mod nvic;
mod platform;
mod plic;
mod pmp;
mod simulation;
mod stm32_can;
mod stm32_crc;
mod stm32_ethernet;
mod stm32_exti;
mod stm32_flash;
mod stm32_iwdg;
mod stm32_uart;
mod stm32f1_i2c;
mod stm32f4_flash;
mod stm32f4_power;
mod stm32f4_rcc;
mod stm32f4_spi;
mod stm32f4_system;
mod stm32h7_adc;
mod stm32h7_adc_dma;
mod stm32h7_adc_platform;
mod stm32h7_bdma;
mod stm32h7_board_platform;
mod stm32h7_ethernet;
mod stm32h7_exti;
mod stm32h7_fdcan;
mod stm32h7_flash;
mod stm32h7_gpio;
mod stm32h7_gpio_platform;
mod stm32h7_i2c;
mod stm32h7_net_control;
mod stm32h7_qspi;
mod stm32h7_rng;
mod stm32h7_rtc;
mod stm32h7_rtc_platform;
mod stm32h7_spi;
mod stm32h7_timer;
mod sv32;
mod systick;
mod thumb;
mod uart;
mod uart_check;
mod uart_transcript;
mod zephyr_boot;

use std::{
    env,
    fs::{self, File},
    io::{self, LineWriter, Write},
    path::PathBuf,
    process::ExitCode,
    time::Instant,
};

use machine::{Machine, Stop};

const HELP: &str = "Run a bare-metal firmware image.

Usage: penv run [OPTIONS] IMAGE
       penv run --script PATH [OPTIONS] [LOCAL_ELF]
       penv peripherals export OUTPUT --platform REPL [--platform-root DIR]

Options:
  --asset-dir DIR    Resolve RESC firmware URLs to local files by filename
  --script PATH      Run a Renode RESC boot or network script
  --raw              Load a flat binary (default: ELF32)
  --ram-mib N        RAM size in MiB, 1–1024 (default: 16)
  --map-ram BASE:SIZE Add a RAM region in bytes (decimal or 0x-prefixed)
  --clint BASE       Map a CLINT for the configured harts (decimal or 0x-prefixed)
  --platform PATH    Load RAM, CPU and CLINT from a Renode REPL file
  --platform-root DIR  Root for working-directory-relative REPL includes
  --uart-event STEP:PATH  Queue file bytes at a step (repeatable)
  --uart-input PATH  Queue bytes from a file for the platform UART
  --uart-input-at N Delay UART input until execution step N
  --mtvec ADDRESS    Set the initial machine trap vector
  --vex-irq-mask N   Set the initial VexRiscv external interrupt mask
  --harts N          Start N RV32 harts at the image entry (1–8)
  --f4-repl-timing   Preserve STM32F4 REPL timer and SysTick clocks
  --uart-check PATH       Execute a timed UART expectation script
  --uart-transcript PATH  Record UART lines with their virtual step
  --uart-reply LINE FILE  Inject file bytes after the first matching UART line
  --virtual-mips N   Set Cortex-M steps per virtual microsecond (default: 1)
  --max-steps N      Stop after N execution steps or simulation rounds (default: 10000000)
  --stats            Print execution statistics to stderr
  --                 Treat remaining arguments as positional
  -h, --help         Show help
  -V, --version      Show version";

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("penv: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<ExitCode, String> {
    let mut args = env::args_os().skip(1);
    let command = args.next();
    match command.as_deref().map(|arg| arg.to_str()) {
        None | Some(Some("-h" | "--help")) => {
            println!("{HELP}");
            return Ok(ExitCode::SUCCESS);
        }
        Some(Some("-V" | "--version")) => {
            println!("penv {}", env!("CARGO_PKG_VERSION"));
            return Ok(ExitCode::SUCCESS);
        }
        Some(Some("run")) => {}
        Some(Some("peripherals")) => return export_peripherals(args),
        _ => return Err("expected 'run' or 'peripherals'; see --help".into()),
    }

    let mut raw = false;
    let mut stats = false;
    let mut positional = false;
    let mut ram_mib = None;
    let mut harts = None;
    let mut max_steps = 10_000_000;
    let mut virtual_mips = None;
    let mut f4_repl_timing = false;
    let mut transcript = None;
    let mut uart_reply = None;
    let mut uart_check = None;
    let mut boot_writes = Vec::new();
    let mut extra_ram = Vec::new();
    let mut clint = None;
    let mut mtvec = None;
    let mut vex_irq_mask = None;
    let mut platform_path = None;
    let mut script = None;
    let mut asset_dir = None;
    let mut platform_root = None;
    let mut uart_input = None;
    let mut uart_input_at = None;
    let mut uart_events = Vec::new();
    let mut path = None;
    while let Some(arg) = args.next() {
        match arg.to_str().filter(|_| !positional) {
            Some("-h" | "--help") => {
                println!("{HELP}");
                return Ok(ExitCode::SUCCESS);
            }
            Some("--") => positional = true,
            Some("--raw") => raw = true,
            Some("--stats") => stats = true,
            Some("--f4-repl-timing") => f4_repl_timing = true,
            Some("--uart-reply") => {
                let line = args
                    .next()
                    .and_then(|value| value.into_string().ok())
                    .ok_or("--uart-reply requires LINE FILE")?;
                let file = args
                    .next()
                    .map(PathBuf::from)
                    .ok_or("--uart-reply requires LINE FILE")?;
                if uart_reply.replace((line, file)).is_some() {
                    return Err("--uart-reply may only be specified once".into());
                }
            }
            Some(
                option @ ("--platform" | "--platform-root" | "--uart-input" | "--script"
                | "--asset-dir" | "--uart-transcript" | "--uart-check"),
            ) => {
                let path = args
                    .next()
                    .map(PathBuf::from)
                    .ok_or_else(|| format!("{option} requires a path"))?;
                let target = match option {
                    "--platform" => &mut platform_path,
                    "--script" => &mut script,
                    "--asset-dir" => &mut asset_dir,
                    "--uart-transcript" => &mut transcript,
                    "--uart-check" => &mut uart_check,
                    "--platform-root" => &mut platform_root,
                    _ => &mut uart_input,
                };
                if target.replace(path).is_some() {
                    return Err(format!("{option} may only be specified once"));
                }
            }
            Some("--uart-event") => {
                let value = args
                    .next()
                    .and_then(|value| value.into_string().ok())
                    .ok_or("--uart-event requires STEP:PATH")?;
                let (step, path) = value
                    .split_once(':')
                    .filter(|(_, path)| !path.is_empty())
                    .ok_or("--uart-event requires STEP:PATH")?;
                let step = step
                    .parse::<u64>()
                    .map_err(|_| "--uart-event requires a nonnegative step number")?;
                uart_events.push((step, PathBuf::from(path)));
            }
            Some("--uart-input-at") => {
                let value = args
                    .next()
                    .and_then(|v| v.into_string().ok())
                    .and_then(|v| v.parse::<u64>().ok())
                    .ok_or("--uart-input-at requires a nonnegative step number")?;
                if uart_input_at.replace(value).is_some() {
                    return Err("--uart-input-at may only be specified once".into());
                }
            }
            Some(option @ ("--mtvec" | "--vex-irq-mask")) => {
                let value = args
                    .next()
                    .and_then(|value| value.into_string().ok())
                    .ok_or_else(|| format!("{option} requires a number"))?;
                let target = if option == "--mtvec" {
                    &mut mtvec
                } else {
                    &mut vex_irq_mask
                };
                if target.replace(parse_address(&value)?).is_some() {
                    return Err(format!("{option} may only be specified once"));
                }
            }
            Some("--clint") => {
                let value = args
                    .next()
                    .and_then(|value| value.into_string().ok())
                    .ok_or("--clint requires BASE")?;
                if clint.replace(parse_address(&value)?).is_some() {
                    return Err("--clint may only be specified once".into());
                }
            }
            Some("--map-ram") => {
                let value = args
                    .next()
                    .and_then(|value| value.into_string().ok())
                    .ok_or("--map-ram requires BASE:SIZE")?;
                let (base, size) = value
                    .split_once(':')
                    .ok_or("--map-ram requires BASE:SIZE")?;
                extra_ram.push((parse_address(base)?, parse_address(size)? as usize));
            }
            Some(option @ ("--ram-mib" | "--max-steps" | "--harts" | "--virtual-mips")) => {
                let value = args
                    .next()
                    .and_then(|v| v.into_string().ok())
                    .and_then(|v| v.parse::<u64>().ok())
                    .filter(|&v| v > 0)
                    .ok_or_else(|| format!("{option} requires a positive integer"))?;
                if option == "--ram-mib" {
                    if value > 1024 {
                        return Err("--ram-mib must be between 1 and 1024".into());
                    }
                    ram_mib = Some(value as usize);
                } else if option == "--harts" {
                    if value > machine::MAX_HARTS as u64 {
                        return Err(format!(
                            "--harts must be between 1 and {}",
                            machine::MAX_HARTS
                        ));
                    }
                    harts = Some(value as usize);
                } else if option == "--virtual-mips" {
                    if value > u64::from(u32::MAX / 1_000_000) {
                        return Err("--virtual-mips is too large".into());
                    }
                    virtual_mips = Some(value as u32);
                } else {
                    max_steps = value;
                }
            }
            Some(option) if option.starts_with('-') => {
                return Err(format!("unknown option {option}"));
            }
            _ if path.is_none() => path = Some(PathBuf::from(arg)),
            _ => return Err("expected one firmware image".into()),
        }
    }

    let mut binaries = Vec::new();
    let mut boot_pcs = Vec::new();
    let mut uart_device = None;
    let mut zephyr = zephyr_boot::BootOptions::default();
    if asset_dir.is_some() && script.is_none() {
        return Err("--asset-dir requires --script".into());
    }
    if let Some(script) = script {
        if platform_path.is_some() || raw {
            return Err("--script cannot be combined with --platform or --raw".into());
        }
        let root = platform_root
            .as_deref()
            .unwrap_or(std::path::Path::new("."));
        let mut plan = network_monitor::Plan::load(&script, root)?;
        if !plan.is_single_boot() {
            if path.is_some()
                || ram_mib.is_some()
                || !extra_ram.is_empty()
                || clint.is_some()
                || harts.is_some()
                || mtvec.is_some()
                || vex_irq_mask.is_some()
                || uart_input.is_some()
                || uart_input_at.is_some()
                || !uart_events.is_empty()
                || virtual_mips.is_some()
                || f4_repl_timing
                || transcript.is_some()
                || uart_reply.is_some()
                || uart_check.is_some()
            {
                return Err(
                    "single-machine overrides cannot be combined with a simulation script".into(),
                );
            }
            return simulation::run(
                plan,
                platform_root.as_deref(),
                asset_dir.as_deref(),
                max_steps,
                stats,
            );
        }
        let boot = plan.nodes.pop().unwrap().boot;
        platform_path = boot.platform;
        virtual_mips = virtual_mips.or(boot.mips);
        boot_writes = boot.writes;
        mtvec = mtvec.or(boot.mtvec);
        vex_irq_mask = vex_irq_mask.or(boot.mask);
        if path.is_some() && boot.image.is_none() {
            return Err("a firmware override requires a LoadELF command in the script".into());
        }
        if path.is_none()
            && let Some(image) = boot.image
        {
            path = Some(monitor::asset_path(
                &image,
                &script,
                root,
                asset_dir.as_deref(),
            )?);
        }
        for (value, address) in boot.binaries {
            let path = monitor::asset_path(&value, &script, root, asset_dir.as_deref())?;
            binaries.push((path, address));
        }
        boot_pcs = boot.boot_pcs;
        uart_device = boot.uart_input;
        zephyr = boot.zephyr;
    }
    if path.is_none() && binaries.is_empty() {
        return Err("missing firmware image; see run --help".into());
    }
    let mut machine = match platform_path {
        Some(platform_path) => {
            if ram_mib.is_some() || !extra_ram.is_empty() || clint.is_some() {
                return Err(
                    "--platform defines RAM and CLINT and cannot be combined with manual mappings"
                        .into(),
                );
            }
            platform::load_with_timing(&platform_path, platform_root.as_deref(), f4_repl_timing)?
        }
        None => {
            if platform_root.is_some() {
                return Err("--platform-root requires --platform".into());
            }
            Machine::new(ram_mib.unwrap_or(16) * 1024 * 1024)?
        }
    };
    if let Some(mips) = virtual_mips {
        if !machine.is_cortex_m() {
            return Err("--virtual-mips requires Cortex-M".into());
        }
        machine.bus.set_step_hz(mips * 1_000_000);
    }
    for (base, size) in extra_ram {
        machine.bus.map_ram(base, size)?;
    }
    if let Some(base) = clint {
        machine.bus.map_clint(base, harts.unwrap_or(1))?;
    }
    if let Some(uart) = uart_device {
        machine.bus.select_uart_input(&uart);
    }
    if uart_input_at.is_some() && uart_input.is_none() {
        return Err("--uart-input-at requires --uart-input".into());
    }
    if let Some(path) = uart_input {
        uart_events.insert(0, (uart_input_at.unwrap_or(0), path));
    }
    for (step, path) in uart_events {
        let bytes = fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        machine.schedule_uart_input(step, bytes)?;
    }
    if let Some(value) = mtvec {
        machine.primary.set_mtvec(value);
    }
    if let Some(value) = vex_irq_mask {
        machine.primary.set_vex_irq_mask(value)?;
    }
    if let Some(path) = &path {
        let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        image::load(&mut machine, file, raw).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    for (path, address) in binaries {
        let file = File::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        image::load_script_binary(&mut machine, file, address)
            .map_err(|e| format!("{}: {e}", path.display()))?;
    }
    if let Some(count) = harts {
        if count != 1 && machine.is_cortex_m() {
            return Err("--harts requires an RV32 machine".into());
        }
        if machine.hart_count() != 1 {
            return Err("--harts cannot override multiple CPU declarations".into());
        }
        machine.configure_harts(count);
    }
    for (address, value) in boot_writes {
        machine
            .bus
            .write(address, 4, value, &mut std::io::sink())
            .map_err(|e| e.to_string())?;
    }
    for (cpu, address) in boot_pcs {
        machine.set_named_entry(&cpu, address)?;
    }
    zephyr_boot::configure(&mut machine, &zephyr, path.as_deref())?;
    machine.validate_entry()?;

    let mut output = LineWriter::new(io::stdout().lock());
    if let Some(check) = uart_check {
        if transcript.is_some() || uart_reply.is_some() {
            return Err(
                "--uart-check cannot be combined with --uart-transcript or --uart-reply".into(),
            );
        }
        uart_check::run(&mut machine, &mut output, &check)?;
        output.flush().map_err(|e| format!("UART: {e}"))?;
        return Ok(ExitCode::SUCCESS);
    }
    let start = Instant::now();
    let result = if transcript.is_some() || uart_reply.is_some() {
        uart_transcript::run(
            &mut machine,
            max_steps,
            &mut output,
            transcript.as_deref(),
            uart_reply,
        )
    } else {
        machine.run(max_steps, &mut output)
    };
    let elapsed = start.elapsed();
    output.flush().map_err(|e| format!("UART: {e}"))?;
    let (stop, steps) = result?;
    if stats {
        eprintln!(
            "{stop:?}, {steps} instructions, {:.3} ms, pc={:#010x}",
            elapsed.as_secs_f64() * 1000.0,
            machine.primary.pc
        );
    }
    if stop == Stop::Limit {
        eprintln!(
            "penv: instruction limit reached at pc={:#010x}",
            machine.primary.pc
        );
        return Ok(ExitCode::from(2));
    }
    Ok(ExitCode::SUCCESS)
}

fn export_peripherals(
    mut args: impl Iterator<Item = std::ffi::OsString>,
) -> Result<ExitCode, String> {
    let usage = "expected 'peripherals export OUTPUT --platform REPL [--platform-root DIR]'";
    if args.next().as_deref() != Some(std::ffi::OsStr::new("export")) {
        return Err(usage.into());
    }
    let output = PathBuf::from(args.next().ok_or(usage)?);
    let (mut platform, mut root) = (None, None);
    while let Some(flag) = args.next() {
        let value = PathBuf::from(args.next().ok_or(usage)?);
        match flag.to_str() {
            Some("--platform") if platform.is_none() => platform = Some(value),
            Some("--platform-root") if root.is_none() => root = Some(value),
            _ => return Err(usage.into()),
        }
    }
    platform::export_peripherals(&platform.ok_or(usage)?, root.as_deref(), &output)?;
    Ok(ExitCode::SUCCESS)
}

fn parse_address(value: &str) -> Result<u32, String> {
    let parsed = match value.strip_prefix("0x") {
        Some(hex) => u32::from_str_radix(hex, 16),
        None => value.parse(),
    };
    parsed.map_err(|_| format!("invalid 32-bit number: {value}"))
}
