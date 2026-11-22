mod bus;
mod clint;
mod compressed;
mod csr;
mod image;
mod machine;
mod monitor;
mod murax;
mod ns16550;
mod nvic;
mod platform;
mod plic;
mod pmp;
mod stm32_crc;
mod stm32_exti;
mod stm32_gpio;
mod stm32_rcc;
mod stm32_timer;
mod stm32_uart;
mod sv32;
mod systick;
mod thumb;
mod uart;

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

Options:
  --asset-dir DIR    Resolve RESC firmware URLs to local files by filename
  --script PATH      Read boot settings from a Renode RESC file
  --raw              Load a flat binary (default: ELF32)
  --ram-mib N        RAM size in MiB, 1–1024 (default: 16)
  --map-ram BASE:SIZE Add a RAM region in bytes (decimal or 0x-prefixed)
  --clint BASE       Map a CLINT for the configured harts (decimal or 0x-prefixed)
  --platform PATH    Load RAM, CPU and CLINT from a Renode REPL file
  --stm32f0-console  Load STM32F0 REPLs with console-focused peripheral support
  --platform-root DIR  Root for working-directory-relative REPL includes
  --uart-event STEP:PATH  Queue file bytes at a step (repeatable)
  --uart-input PATH  Queue bytes from a file for the platform UART
  --uart-input-at N Delay UART input until execution step N
  --mtvec ADDRESS    Set the initial machine trap vector
  --vex-irq-mask N   Set the initial VexRiscv external interrupt mask
  --harts N          Start N RV32 harts at the image entry (1–8)
  --max-steps N      Stop after N execution steps (default: 10000000)
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
        _ => return Err("expected 'run'; see --help".into()),
    }

    let mut raw = false;
    let mut stats = false;
    let mut positional = false;
    let mut ram_mib = None;
    let mut harts = None;
    let mut max_steps = 10_000_000;
    let mut extra_ram = Vec::new();
    let mut clint = None;
    let mut mtvec = None;
    let mut vex_irq_mask = None;
    let mut platform_path = None;
    let mut script = None;
    let mut asset_dir = None;
    let mut platform_root = None;
    let mut stm32f0_console = false;
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
            Some("--stm32f0-console") => stm32f0_console = true,
            Some("--stats") => stats = true,
            Some(
                option @ ("--platform" | "--platform-root" | "--uart-input" | "--script"
                | "--asset-dir"),
            ) => {
                let path = args
                    .next()
                    .map(PathBuf::from)
                    .ok_or_else(|| format!("{option} requires a path"))?;
                let target = match option {
                    "--platform" => &mut platform_path,
                    "--script" => &mut script,
                    "--asset-dir" => &mut asset_dir,
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
            Some(option @ ("--ram-mib" | "--max-steps" | "--harts")) => {
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
        let boot = monitor::load(&script, root)?;
        platform_path = boot.platform;
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
            platform::load(&platform_path, platform_root.as_deref(), stm32f0_console)?
        }
        None => {
            if stm32f0_console {
                return Err("--stm32f0-console requires --platform".into());
            }
            if platform_root.is_some() {
                return Err("--platform-root requires --platform".into());
            }
            Machine::new(ram_mib.unwrap_or(16) * 1024 * 1024)?
        }
    };
    for (base, size) in extra_ram {
        machine.bus.map_ram(base, size)?;
    }
    if let Some(base) = clint {
        machine.bus.map_clint(base, harts.unwrap_or(1))?;
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
    if let Some(path) = path {
        let file = File::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        image::load(&mut machine, file, raw).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    for (path, address) in binaries {
        let file = File::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        image::load_binary(&mut machine, file, address)
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
    for (cpu, address) in boot_pcs {
        machine.set_named_entry(&cpu, address)?;
    }
    machine.validate_entry()?;

    let mut output = LineWriter::new(io::stdout().lock());
    let start = Instant::now();
    let result = machine.run(max_steps, &mut output);
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

fn parse_address(value: &str) -> Result<u32, String> {
    let parsed = match value.strip_prefix("0x") {
        Some(hex) => u32::from_str_radix(hex, 16),
        None => value.parse(),
    };
    parsed.map_err(|_| format!("invalid 32-bit number: {value}"))
}
