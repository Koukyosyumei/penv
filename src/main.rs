mod andes_csr;
mod bitmanip;
mod bus;
mod clint;
mod compressed;
mod csr;
mod float;
mod image;
mod litex;
mod litex_eth;
mod litex_flash;
mod litex_i2c;
mod litex_i2s;
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
mod vector_bootstrap;

use std::{
    cell::Cell,
    env,
    fs::{self, File},
    io::{self, LineWriter, Read, Write},
    path::PathBuf,
    process::ExitCode,
    rc::Rc,
    sync::mpsc,
    thread,
    time::Instant,
};

use machine::{Machine, Stop};

const HELP: &str = "Run a bare-metal firmware image.

Usage: penv run [OPTIONS] IMAGE
       penv run --script PATH [OPTIONS] [LOCAL_ELF]
       penv check-platform PATH

Options:
  --framebuffer-event STEP:PATH  Save a LiteX framebuffer as PPM at a step (repeatable)
  --wait-for-frame STEP:PATH  Stop when the LiteX framebuffer matches a PNG or P6 RGB PPM
  --wait-for-frame-roi STEP:X:Y:WIDTH:HEIGHT:PATH  Wait for identical RGB pixels in a region
  --frame-waits-after-uart STEPS:TEXT  Start queued waits on UART text and give each STEPS
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
  --litex-phy-event STEP:NAME:VALUE  Set PHY VendorSpecific1 at a step (repeatable)
  --litex-temperature-event STEP:NAME:CELSIUS  Set an SI70xx temperature (repeatable)
  --litex-led-assert STEP:NAME:STATE  Assert a wired LiteX LED is on or off (repeatable)
  --litex-button-event STEP:NAME:ACTION  Press, Release, or Toggle at a step
  --uart-event STEP:PATH  Queue file bytes at a step (repeatable)
  --uart-input PATH  Queue bytes from a file for the platform UART
  --uart-input-at N Delay UART input until execution step N
  --litex-uart-stdio  Bridge stdin to LiteX UART at 115200 baud and flush UART output
  --litex-uart-legacy-console  Use legacy LiteX UART console event handling
  --mtvec ADDRESS    Set the initial machine trap vector
  --vex-irq-mask N   Set the initial VexRiscv external interrupt mask
  --harts N          Start N RV32 harts at the image entry (1–8)
  --tohost ADDRESS  Stop on an odd write to the RV32 test completion word
  --tohost-act ADDRESS  Decode RV32 Sail ACT console and completion writes
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
        Some(Some("check-platform")) => {
            let path = args.next().ok_or("expected 'check-platform PATH'")?;
            if args.next().is_some() {
                return Err("expected 'check-platform PATH'".into());
            }
            platform::check(PathBuf::from(path).as_path())?;
            return Ok(ExitCode::SUCCESS);
        }
        Some(Some("run")) => {}
        _ => return Err("expected 'run' or 'check-platform'; see --help".into()),
    }

    let mut raw = false;
    let mut litex_uart_stdio = false;
    let mut enable_litex_uart_legacy_console = false;
    let mut stats = false;
    let mut positional = false;
    let mut ram_mib = None;
    let mut harts = None;
    let mut tohost = None;
    let mut max_steps = None;
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
    let mut temperature_events = Vec::new();
    let mut phy_events = Vec::new();
    let mut led_assertions = Vec::new();
    let mut button_events = Vec::new();
    let mut uart_events = Vec::new();
    let mut frame_waits = Vec::new();
    let mut frame_wait_trigger = None;
    let mut framebuffer_events = Vec::new();
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
            Some("--litex-uart-stdio") => litex_uart_stdio = true,
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
            Some("--litex-phy-event") => {
                const FORMAT: &str = "--litex-phy-event requires STEP:NAME:VALUE";
                let argument = args
                    .next()
                    .and_then(|value| value.into_string().ok())
                    .ok_or(FORMAT)?;
                let (step, phy) = argument.split_once(':').ok_or(FORMAT)?;
                let (name, value) = phy
                    .split_once(':')
                    .filter(|(name, _)| !name.is_empty())
                    .ok_or(FORMAT)?;
                let step = step.parse::<u64>().map_err(|_| FORMAT)?;
                let value = u16::try_from(parse_address(value)?)
                    .map_err(|_| "--litex-phy-event requires a 16-bit register value")?;
                phy_events.push((step, name.to_string(), value));
            }
            Some("--litex-temperature-event") => {
                const FORMAT: &str = "--litex-temperature-event requires STEP:NAME:CELSIUS";
                let argument = args
                    .next()
                    .and_then(|value| value.into_string().ok())
                    .ok_or(FORMAT)?;
                let (step, sensor) = argument.split_once(':').ok_or(FORMAT)?;
                let (name, value) = sensor
                    .split_once(':')
                    .filter(|(name, _)| !name.is_empty())
                    .ok_or(FORMAT)?;
                let step = step.parse::<u64>().map_err(|_| FORMAT)?;
                let value = value
                    .parse::<f64>()
                    .ok()
                    .filter(|value| (-40.0..=85.0).contains(value))
                    .ok_or("--litex-temperature-event requires -40 to 85 degrees Celsius")?;
                temperature_events.push((step, name.to_string(), (value * 100.0).round() as i32));
            }
            Some("--litex-led-assert") => {
                const FORMAT: &str = "--litex-led-assert requires STEP:NAME:on|off";
                let value = args
                    .next()
                    .and_then(|value| value.into_string().ok())
                    .ok_or(FORMAT)?;
                let (step, led) = value.split_once(':').ok_or(FORMAT)?;
                let (name, state) = led
                    .split_once(':')
                    .filter(|(name, _)| !name.is_empty())
                    .ok_or(FORMAT)?;
                let step = step.parse::<u64>().map_err(|_| FORMAT)?;
                let expected = match state {
                    "on" => true,
                    "off" => false,
                    _ => return Err(FORMAT.into()),
                };
                led_assertions.push((step, name.to_string(), expected));
            }
            Some("--litex-button-event") => {
                const FORMAT: &str = "--litex-button-event requires STEP:NAME:ACTION";
                let value = args
                    .next()
                    .and_then(|value| value.into_string().ok())
                    .ok_or(FORMAT)?;
                let (step, button) = value.split_once(':').ok_or(FORMAT)?;
                let (name, action) = button
                    .split_once(':')
                    .filter(|(name, _)| !name.is_empty())
                    .ok_or(FORMAT)?;
                let step = step
                    .parse::<u64>()
                    .map_err(|_| "--litex-button-event requires a nonnegative step number")?;
                let action = litex_panel::ButtonAction::parse(action)
                    .ok_or("--litex-button-event action must be Press, Release, or Toggle")?;
                button_events.push((step, name.to_string(), action));
            }
            Some(option @ ("--framebuffer-event" | "--wait-for-frame")) => {
                let format = || format!("{option} requires STEP:PATH");
                let value = args
                    .next()
                    .and_then(|value| value.into_string().ok())
                    .ok_or_else(format)?;
                let (step, path) = value
                    .split_once(':')
                    .filter(|(_, path)| !path.is_empty())
                    .ok_or_else(format)?;
                let step = step.parse::<u64>().map_err(|_| format())?;
                let event = (step, PathBuf::from(path));
                if option == "--framebuffer-event" {
                    framebuffer_events.push(event);
                } else {
                    frame_waits.push((event.0, event.1, None));
                }
            }
            Some("--frame-waits-after-uart") => {
                const FORMAT: &str =
                    "--frame-waits-after-uart requires positive STEPS and nonempty TEXT";
                let value = args
                    .next()
                    .and_then(|value| value.into_string().ok())
                    .ok_or(FORMAT)?;
                let (steps, text) = value
                    .split_once(':')
                    .filter(|(_, text)| !text.is_empty())
                    .ok_or(FORMAT)?;
                let steps = steps
                    .parse::<u64>()
                    .ok()
                    .filter(|steps| *steps != 0)
                    .ok_or(FORMAT)?;
                if frame_wait_trigger
                    .replace((steps, text.as_bytes().to_vec()))
                    .is_some()
                {
                    return Err("--frame-waits-after-uart may only be specified once".into());
                }
            }
            Some("--wait-for-frame-roi") => {
                const FORMAT: &str = "--wait-for-frame-roi requires STEP:X:Y:WIDTH:HEIGHT:PATH";
                let value = args
                    .next()
                    .and_then(|value| value.into_string().ok())
                    .ok_or(FORMAT)?;
                let mut fields = value.splitn(6, ':');
                let step = fields
                    .next()
                    .and_then(|field| field.parse::<u64>().ok())
                    .ok_or(FORMAT)?;
                let mut number = || {
                    fields
                        .next()
                        .and_then(|field| field.parse::<u16>().ok())
                        .ok_or(FORMAT)
                };
                let region = litex_video::FrameRegion {
                    x: number()?,
                    y: number()?,
                    width: number()?,
                    height: number()?,
                };
                let path = fields
                    .next()
                    .filter(|path| !path.is_empty())
                    .ok_or(FORMAT)?;
                frame_waits.push((step, PathBuf::from(path), Some(region)));
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
            Some(option @ ("--tohost" | "--tohost-act")) => {
                let value = args
                    .next()
                    .and_then(|value| value.into_string().ok())
                    .ok_or("--tohost requires a physical RAM address")?;
                if tohost
                    .replace((parse_address(&value)?, option == "--tohost-act"))
                    .is_some()
                {
                    return Err("only one tohost address may be specified".into());
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
                    max_steps = Some(value);
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
    let mut cpu_control = Vec::new();
    let mut script_elf_addresses = None;
    let mut silence_ranges = Vec::new();
    let mut vex_supervisor_irq_mask = None;
    let mut buttons = Vec::new();
    let mut sd_card = None;
    let mut tftp = None;
    let mut video_analyzers = Vec::new();
    let mut pcm_inputs = Vec::new();
    let mut pcm_outputs = Vec::new();
    let mut run_steps = None;
    let mut post_run = Vec::new();
    let mut cpu_registers = Vec::new();
    let mut log_function_names = false;
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
        cpu_registers = boot.cpu_registers;
        log_function_names = boot.log_function_names;
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
        for (device, input) in boot.pcm_inputs {
            pcm_inputs.push((device, input.resolve(root, asset_dir.as_deref())?));
        }
        pcm_outputs = boot.pcm_outputs;
        run_steps = boot.run_steps;
        post_run = boot.post_run;
        cpu_control = boot.cpu_control;
        silence_ranges = boot.silence_ranges;
        buttons = boot.buttons;
        video_analyzers = boot.video_analyzers;
    }
    let inspect_only = path.is_none()
        && binaries.is_empty()
        && (!cpu_control.is_empty() || !cpu_registers.is_empty());
    if path.is_none() && binaries.is_empty() && !inspect_only {
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
    if inspect_only {
        machine.primary.initialize_reset_pc();
    }
    let reset_state = cpu_control
        .iter()
        .any(|command| matches!(command, monitor::CpuControl::Reset))
        .then(|| machine.primary.clone());
    for command in cpu_registers {
        match command {
            monitor::CpuRegisterCommand::TrapVector(register, value) => {
                if let Some(readback) = machine.primary.monitor_trap_vector(register, value)? {
                    eprintln!("{} {readback:#x}", register.name());
                }
            }
            monitor::CpuRegisterCommand::ExternalPmp(path, config, index, value) => {
                if let Some(readback) = machine.monitor_external_pmp(&path, config, index, value)? {
                    eprintln!("{readback:#x}");
                }
            }
            monitor::CpuRegisterCommand::Integer(index, value) => {
                if let Some(readback) = machine.primary.monitor_integer_register(index, value)? {
                    eprintln!("{readback:#x}");
                }
            }
        }
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
    if log_function_names {
        machine.enable_function_logging()?;
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
    for command in cpu_control {
        match command {
            monitor::CpuControl::Pc(cpu, value) => {
                if let Some(pc) = machine.monitor_pc(&cpu, value)? {
                    eprintln!("{pc:#x}");
                }
            }
            monitor::CpuControl::ResetVector(address) => {
                machine.primary.monitor_reset_vector(address)?
            }
            monitor::CpuControl::Reset => machine
                .primary
                .monitor_reset(reset_state.as_ref().unwrap())?,
        }
    }
    if inspect_only {
        return Ok(ExitCode::SUCCESS);
    }
    for (name, action) in buttons {
        machine.bus.set_litex_button(&name, action)?;
    }
    for (step, name, action) in button_events {
        machine.schedule_button(step, name, action)?;
    }
    for (step, name, expected) in led_assertions {
        machine.schedule_led_assertion(step, name, expected)?;
    }
    for (step, name, value) in temperature_events {
        machine.schedule_sensor_temperature(step, name, value)?;
    }
    for (step, name, value) in phy_events {
        machine.schedule_phy_vendor_specific1(step, name, value)?;
    }
    for (step, path) in framebuffer_events {
        machine.schedule_framebuffer_capture(step, path)?;
    }
    let frame_wait_trigger = if let Some((budget, text)) = frame_wait_trigger {
        if frame_waits.is_empty() || frame_waits.iter().any(|(step, _, _)| *step != 0) {
            return Err("--frame-waits-after-uart requires frame waits with step 0".into());
        }
        let fired = Rc::new(Cell::new(false));
        machine.arm_frame_waits(Rc::clone(&fired), budget);
        Some(UartTrigger {
            text,
            suffix: Vec::new(),
            fired,
        })
    } else {
        None
    };
    for (step, path, region) in frame_waits {
        let data = fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        let reference = litex_video::ReferenceFrame::parse(data)
            .and_then(|frame| match region {
                Some(region) => frame.with_region(region),
                None => Ok(frame),
            })
            .map_err(|error| format!("{}: {error}", path.display()))?;
        machine.schedule_frame_wait(step, reference)?;
    }
    if let Some((address, act)) = tohost {
        if machine.is_cortex_m() {
            return Err("--tohost requires an RV32 machine".into());
        }
        machine.bus.watch_tohost(address, act)?;
    }
    machine.validate_entry()?;
    for action in &post_run {
        if let monitor::PostRun::DisposePcm(name) = action
            && !machine
                .bus
                .litex_i2s
                .iter()
                .any(|device| device.name == *name && device.transmit)
        {
            return Err(format!("unknown LiteX I2S transmitter {name:?}"));
        }
    }

    for (name, path) in pcm_inputs {
        let device = machine
            .bus
            .litex_i2s
            .iter_mut()
            .find(|device| device.name == name && !device.transmit)
            .ok_or_else(|| format!("unknown LiteX I2S receiver {name:?}"))?;
        device.load_pcm(fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?)?;
    }
    for (name, path) in pcm_outputs {
        let device = machine
            .bus
            .litex_i2s
            .iter_mut()
            .find(|device| device.name == name && device.transmit)
            .ok_or_else(|| format!("unknown LiteX I2S transmitter {name:?}"))?;
        device.set_output(File::create(&path).map_err(|e| format!("{}: {e}", path.display()))?);
    }

    if litex_uart_stdio {
        let (sender, receiver) = mpsc::sync_channel(4);
        machine.bus.attach_litex_uart_input(receiver)?;
        thread::Builder::new()
            .name("litex-uart-stdin".into())
            .spawn(move || {
                let mut stdin = io::stdin().lock();
                let mut buffer = [0; 256];
                loop {
                    match stdin.read(&mut buffer) {
                        Ok(0) => break,
                        Ok(count) => {
                            if sender.send(Ok(buffer[..count].to_vec())).is_err() {
                                break;
                            }
                        }
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                        Err(error) => {
                            let _ = sender.send(Err(error));
                            break;
                        }
                    }
                }
            })
            .map_err(|error| format!("UART stdin: {error}"))?;
    }
    let mut output = UartOutput {
        writer: LineWriter::new(io::stdout().lock()),
        immediate: litex_uart_stdio,
        trigger: frame_wait_trigger,
    };
    let start = Instant::now();
    let limit = match (max_steps, run_steps) {
        (Some(cli), Some(script)) => cli.min(script),
        (cli, script) => cli.or(script).unwrap_or(10_000_000),
    };
    let result = machine.run(limit, &mut output);
    let elapsed = start.elapsed();
    output.flush().map_err(|e| format!("UART: {e}"))?;
    for step in machine.intermediate_frame_matches() {
        eprintln!("penv: framebuffer matched at step {step}");
    }
    let (stop, steps) = result?;
    machine.verify_required_events_completed()?;
    let completed_run_for = stop == Stop::Limit && run_steps == Some(limit);
    if completed_run_for {
        for action in post_run {
            match action {
                monitor::PostRun::Echo(message) => eprintln!("{message}"),
                monitor::PostRun::DisposePcm(name) => {
                    let device = machine
                        .bus
                        .litex_i2s
                        .iter_mut()
                        .find(|device| device.name == name && device.transmit)
                        .expect("validated LiteX I2S transmitter");
                    device
                        .dispose()
                        .map_err(|error| format!("{name}: {error}"))?;
                }
            }
        }
    }
    for device in &mut machine.bus.litex_i2s {
        device
            .flush()
            .map_err(|error| format!("{}: {error}", device.name))?;
    }
    if let Some(path) = framebuffer_out {
        machine.bus.capture_litex_framebuffer(&path)?;
    }
    if let Stop::FrameMatched(step) = stop {
        eprintln!("penv: framebuffer matched at step {step}");
    }
    if stats {
        eprintln!(
            "{stop:?}, {steps} instructions, {:.3} ms, pc={:#010x}",
            elapsed.as_secs_f64() * 1000.0,
            machine.primary.pc
        );
    }
    if let Stop::Tohost(code) = stop {
        eprintln!("penv: tohost exit code {code}");
        return Ok(if code == 0 {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        });
    }
    if tohost.is_some() && (stop != Stop::Limit || completed_run_for) {
        return Err("execution stopped before a tohost completion write".into());
    }
    if stop == Stop::Limit && !completed_run_for {
        eprintln!(
            "penv: instruction limit reached at pc={:#010x}",
            machine.primary.pc
        );
        return Ok(ExitCode::from(2));
    }
    Ok(ExitCode::SUCCESS)
}

struct UartTrigger {
    text: Vec<u8>,
    suffix: Vec<u8>,
    fired: Rc<Cell<bool>>,
}

struct UartOutput<W: Write> {
    writer: LineWriter<W>,
    immediate: bool,
    trigger: Option<UartTrigger>,
}

impl<W: Write> Write for UartOutput<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let written = self.writer.write(bytes)?;
        if let Some(trigger) = &mut self.trigger {
            trigger.suffix.extend_from_slice(&bytes[..written]);
            if trigger
                .suffix
                .windows(trigger.text.len())
                .any(|part| part == trigger.text)
            {
                trigger.fired.set(true);
                self.trigger = None;
            } else {
                let discard = trigger.suffix.len().saturating_sub(trigger.text.len() - 1);
                trigger.suffix.drain(..discard);
            }
        }
        if self.immediate {
            self.writer.flush()?;
        }
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

fn parse_address(value: &str) -> Result<u32, String> {
    let parsed = match value.strip_prefix("0x") {
        Some(hex) => u32::from_str_radix(hex, 16),
        None => value.parse(),
    };
    parsed.map_err(|_| format!("invalid 32-bit number: {value}"))
}
