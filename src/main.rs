mod bus;
mod clint;
mod compressed;
mod csr;
mod image;
mod litex;
mod litex_eth;
mod litex_flash;
mod litex_i2c;
mod litex_panel;
mod litex_sd;
mod litex_tftp;
mod litex_video;
mod machine;
mod monitor;
mod murax;
mod ns16550;
mod nvic;
mod platform;
mod plic;
mod pmp;
mod stm32_exti;
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
  --framebuffer-out PATH  Write the final LiteX framebuffer as PPM
  --asset-dir DIR    Resolve RESC firmware URLs to local files by filename
  --script PATH      Read boot settings from a Renode RESC file
  --raw              Load a flat binary (default: ELF32)
  --ram-mib N        RAM size in MiB, 1–1024 (default: 16)
  --map-ram BASE:SIZE Add a RAM region in bytes (decimal or 0x-prefixed)
  --clint BASE       Map a CLINT for the configured harts (decimal or 0x-prefixed)
  --litex-timer-alias BASE  Map the existing LiteX timer at an additional address
  --platform PATH    Load RAM, CPU and CLINT from a Renode REPL file
  --platform-root DIR  Root for working-directory-relative REPL includes
  --uart-event STEP:PATH  Queue file bytes at a step (repeatable)
  --uart-input PATH  Queue bytes from a file for the platform UART
  --uart-input-at N Delay UART input until execution step N
  --litex-uart-legacy-console  Use legacy LiteX UART console event handling
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
    let mut enable_litex_uart_legacy_console = false;
    let mut stats = false;
    let mut positional = false;
    let mut ram_mib = None;
    let mut harts = None;
    let mut max_steps = 10_000_000;
    let mut extra_ram = Vec::new();
    let mut clint = None;
    let mut litex_timer_aliases = Vec::new();
    let mut mtvec = None;
    let mut vex_irq_mask = None;
    let mut platform_path = None;
    let mut script = None;
    let mut asset_dir = None;
    let mut platform_root = None;
    let mut uart_input = None;
    let mut uart_input_at = None;
    let mut uart_events = Vec::new();
    let mut framebuffer_out = None;
    let mut path = None;
    while let Some(arg) = args.next() {
        match arg.to_str().filter(|_| !positional) {
            Some("-h" | "--help") => {
                println!("{HELP}");
                return Ok(ExitCode::SUCCESS);
            }
            Some("--") => positional = true,
            Some("--raw") => raw = true,
            Some("--litex-uart-legacy-console") => enable_litex_uart_legacy_console = true,
            Some("--stats") => stats = true,
            Some(
                option @ ("--platform" | "--platform-root" | "--uart-input" | "--script"
                | "--asset-dir" | "--framebuffer-out"),
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
                    "--framebuffer-out" => &mut framebuffer_out,
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
            Some("--litex-timer-alias") => {
                let value = args
                    .next()
                    .and_then(|value| value.into_string().ok())
                    .ok_or("--litex-timer-alias requires BASE")?;
                litex_timer_aliases.push(parse_address(&value)?);
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
    let mut script_elf_addresses = None;
    let mut silence_ranges = Vec::new();
    let mut vex_supervisor_irq_mask = None;
    let mut buttons = Vec::new();
    let mut sd_card = None;
    let mut tftp = None;
    let mut video_analyzers = Vec::new();
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
        if let Some(server) = boot.network.into_tftp()? {
            let file = server.file.resolve(root, asset_dir.as_deref())?;
            let data = fs::read(&file).map_err(|e| format!("{}: {e}", file.display()))?;
            tftp = Some((
                server.ethernet,
                litex_tftp::Server::new(server.ip, server.port, server.filename, data),
            ));
        }
        platform_path = boot.platform;
        script_elf_addresses = Some(boot.use_virtual_elf_addresses);
        mtvec = mtvec.or(boot.mtvec);
        vex_irq_mask = vex_irq_mask.or(boot.mask);
        vex_supervisor_irq_mask = boot.supervisor_mask;
        if path.is_some() && boot.image.is_none() {
            return Err("a firmware override requires a LoadELF command in the script".into());
        }
        if path.is_none()
            && let Some(image) = boot.image
        {
            path = Some(image.resolve(root, asset_dir.as_deref())?);
        }
        for (value, address) in boot.binaries {
            let path = value.resolve(root, asset_dir.as_deref())?;
            binaries.push((path, address));
        }
        if let Some((controller, image, capacity)) = boot.sd_card {
            let path = image.resolve(root, asset_dir.as_deref())?;
            sd_card = Some((controller, path, capacity));
        }
        boot_pcs = boot.boot_pcs;
        silence_ranges = boot.silence_ranges;
        buttons = boot.buttons;
        video_analyzers = boot.video_analyzers;
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
            platform::load(&platform_path, platform_root.as_deref())?
        }
        None => {
            if platform_root.is_some() {
                return Err("--platform-root requires --platform".into());
            }
            Machine::new(ram_mib.unwrap_or(16) * 1024 * 1024)?
        }
    };
    for base in litex_timer_aliases {
        machine.bus.map_litex_timer_alias(base)?;
    }
    if enable_litex_uart_legacy_console {
        machine.bus.enable_litex_uart_legacy_console()?;
    }
    if script_elf_addresses.is_some() && !machine.is_cortex_m() {
        machine.bus.use_renode_unmapped_data();
    }
    if let Some((name, server)) = tftp {
        machine.bus.attach_litex_tftp(&name, server)?;
    }
    for analyzer in video_analyzers {
        if machine
            .bus
            .litex_video
            .as_ref()
            .is_none_or(|video| video.name != analyzer)
        {
            return Err(format!("unknown analyzer peripheral {analyzer:?}"));
        }
    }
    if framebuffer_out.is_some() && machine.bus.litex_video.is_none() {
        return Err("--framebuffer-out requires a LiteX framebuffer".into());
    }
    if let Some((controller, path, capacity)) = sd_card {
        let file = File::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let card =
            litex_sd::Card::new(file, capacity).map_err(|e| format!("{}: {e}", path.display()))?;
        machine.bus.attach_litex_sd(&controller, card)?;
    }
    for (base, size) in silence_ranges {
        machine.bus.map_silencer(base, size)?;
    }
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
    if let Some(value) = vex_supervisor_irq_mask {
        machine.primary.set_vex_supervisor_irq_mask(value)?;
    }
    if let Some(path) = path {
        let file = File::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        image::load(&mut machine, file, raw, script_elf_addresses)
            .map_err(|e| format!("{}: {e}", path.display()))?;
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
        if count != 1 && machine.bus.litex_cpu_timer.is_some() {
            return Err("--harts cannot override a single-hart LiteX CPU timer".into());
        }
        if machine.hart_count() != 1 {
            return Err("--harts cannot override multiple CPU declarations".into());
        }
        machine.configure_harts(count);
    }
    for (cpu, address) in boot_pcs {
        machine.set_named_entry(&cpu, address)?;
    }
    for (name, action) in buttons {
        machine.bus.set_litex_button(&name, action)?;
    }
    machine.validate_entry()?;

    let mut output = LineWriter::new(io::stdout().lock());
    let start = Instant::now();
    let result = machine.run(max_steps, &mut output);
    let elapsed = start.elapsed();
    output.flush().map_err(|e| format!("UART: {e}"))?;
    let (stop, steps) = result?;
    if let Some(path) = framebuffer_out {
        let video = machine.bus.litex_video.as_ref().unwrap();
        if video.active.is_none() {
            return Err("LiteX framebuffer DMA is disabled".into());
        }
        let file = File::create(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        video
            .render(&machine.bus, io::BufWriter::new(file))
            .map_err(|e| format!("{}: {e}", path.display()))?;
    }
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
