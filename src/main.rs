mod bus;
mod clint;
mod compressed;
mod csr;
mod ethernet_switch;
mod image;
mod image64;
mod machine;
mod monitor;
mod murax;
mod ns16550;
mod nvic;
mod platform;
mod plic;
mod pmp;
mod rv64;
mod rv64_devices;
mod rv64_ethernet;
mod rv64_float;
mod rv64_gem;
mod rv64_mpfs;
mod rv64_mpfs_ihc;
mod rv64_mpfs_sd;
mod rv64_mpfs_wdog;
mod rv64_platform;
mod rv64_priv;
mod rv64_starfive;
mod rv64_vm;
mod stm32_exti;
mod stm32_uart;
mod sv32;
mod systick;
mod thumb;
mod uart;

use std::{
    env,
    fs::{self, File},
    io::{self, LineWriter, Read, Write},
    path::PathBuf,
    process::ExitCode,
    time::Instant,
};

use machine::{Machine, Stop};

const HELP: &str = "Run a bare-metal firmware image.

Usage: penv run [OPTIONS] IMAGE
       penv run --script PATH [OPTIONS] [LOCAL_ELF]
       penv switch LISTEN_PORT GUEST_PORT GUEST_PORT [GUEST_PORT ...]

Options:
  --asset-dir DIR    Resolve RESC firmware URLs to local files by filename
  --script PATH      Read boot settings from a Renode RESC file
  --raw              Load a flat binary (default: ELF)
  --rv64             Run a raw image on an RV64IM CPU
  --ram-mib N        RAM size in MiB, 1–1024 (default: 16)
  --map-ram BASE:SIZE Add a RAM region in bytes (decimal or 0x-prefixed)
  --load-binary BASE:PATH  Load a flat companion image into RAM (repeatable)
  --dtb BASE:PATH    Load an RV64 boot DTB and pass its address in a1
  --clint BASE       Map a CLINT for the configured harts (decimal or 0x-prefixed)
  --platform PATH    Load RAM, CPU and CLINT from a Renode REPL file
  --platform-root DIR  Root for working-directory-relative REPL includes
  --uart-event STEP:PATH  Queue file bytes at a step (repeatable)
  --uart-input PATH  Queue bytes from a file for the platform UART
  --uart-stdin       Forward stdin to the platform UART while running
  --uart-stdin-reset  Enable Ctrl-] r to reload the machine on the UART session
  --uart-device NAME  Select the RV64 platform UART that receives input
  --uart-input-at N Delay UART input until execution step N
  --ethernet-loopback LOCAL:PEER  Link BeagleV MACs over localhost UDP ports
  --mtvec ADDRESS    Set the initial machine trap vector
  --vex-irq-mask N   Set the initial VexRiscv external interrupt mask
  --harts N          Start N RISC-V harts at the image entry (1–8)
  --tohost ADDRESS   Watch an RV64 test completion word in RAM
  --renode-elf-load  Load the mapped RAM portions of RV64 ELF segments
  --robot-tohost ADDRESS   Install a 4 KiB single-hart Renode-style watcher
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
        Some(Some("switch")) => {
            let ports = args
                .map(|arg| {
                    arg.to_str()
                        .and_then(|port| port.parse::<u16>().ok())
                        .filter(|&port| port != 0)
                        .ok_or("switch ports must be nonzero UDP port numbers")
                })
                .collect::<Result<Vec<_>, _>>()?;
            let [local, _, _, ..] = ports.as_slice() else {
                return Err(
                    "expected switch LISTEN_PORT GUEST_PORT GUEST_PORT [GUEST_PORT ...]".into(),
                );
            };
            if ports
                .iter()
                .enumerate()
                .any(|(index, port)| ports[..index].contains(port))
            {
                return Err("switch ports must be distinct".into());
            }
            let guests = &ports[1..];
            ethernet_switch::run(*local, guests)
                .map_err(|error| format!("Ethernet switch: {error}"))?;
            return Ok(ExitCode::SUCCESS);
        }
        Some(Some("run")) => {}
        _ => return Err("expected 'run' or 'switch'; see --help".into()),
    }

    let mut raw = false;
    let mut rv64 = false;
    let mut stats = false;
    let mut tohost = None;
    let mut renode_elf_load = false;
    let mut positional = false;
    let mut ram_mib = None;
    let mut harts = None;
    let mut max_steps = 10_000_000;
    let mut extra_ram = Vec::new();
    let mut companion_images = Vec::new();
    let mut dtb = None;
    let mut clint = None;
    let mut mtvec = None;
    let mut vex_irq_mask = None;
    let mut platform_path = None;
    let mut script = None;
    let mut asset_dir = None;
    let mut platform_root = None;
    let mut uart_input = None;
    let mut uart_stdin = false;
    let mut uart_stdin_reset = false;
    let mut ethernet_loopback = None;
    let mut uart_device = None;
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
            Some("--rv64") => rv64 = true,
            Some("--stats") => stats = true,
            Some("--uart-stdin") => uart_stdin = true,
            Some("--uart-stdin-reset") => uart_stdin_reset = true,
            Some("--ethernet-loopback") => {
                let value = args.next().and_then(|value| value.into_string().ok());
                let ports = value
                    .as_deref()
                    .and_then(|value| value.split_once(':'))
                    .and_then(|(local, peer)| {
                        Some((local.parse::<u16>().ok()?, peer.parse::<u16>().ok()?))
                    })
                    .filter(|&(local, peer)| local != 0 && peer != 0 && local != peer)
                    .ok_or("--ethernet-loopback requires distinct nonzero LOCAL:PEER ports")?;
                if ethernet_loopback.replace(ports).is_some() {
                    return Err("--ethernet-loopback may only be specified once".into());
                }
            }
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
            Some("--uart-device") => {
                let name = args
                    .next()
                    .and_then(|name| name.into_string().ok())
                    .filter(|name| !name.is_empty())
                    .ok_or("--uart-device requires NAME")?;
                if uart_device.replace(name).is_some() {
                    return Err("--uart-device may only be specified once".into());
                }
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
                let previous = if option == "--mtvec" {
                    mtvec.replace(parse_address64(&value)?).is_some()
                } else {
                    vex_irq_mask.replace(parse_address(&value)?).is_some()
                };
                if previous {
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
            Some(option @ ("--load-binary" | "--dtb")) => {
                let error = || format!("{option} requires BASE:PATH");
                let value = args
                    .next()
                    .and_then(|value| value.into_string().ok())
                    .ok_or_else(error)?;
                let (base, path) = value
                    .split_once(':')
                    .filter(|(_, path)| !path.is_empty())
                    .ok_or_else(error)?;
                let image = (PathBuf::from(path), parse_address64(base)?);
                if option == "--load-binary" {
                    companion_images.push(image);
                } else if dtb.replace(image).is_some() {
                    return Err("--dtb may only be specified once".into());
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
                extra_ram.push((
                    parse_address64(base)?,
                    usize::try_from(parse_address64(size)?)
                        .map_err(|_| "RAM size exceeds host limits")?,
                ));
            }
            Some("--renode-elf-load") => renode_elf_load = true,
            Some(option @ ("--tohost" | "--robot-tohost")) => {
                let value = args.next().ok_or("--tohost requires an address")?;
                let value = value.to_str().ok_or("invalid tohost address")?;
                if tohost
                    .replace((parse_address64(value)?, option == "--robot-tohost"))
                    .is_some()
                {
                    return Err("only one tohost address may be specified".into());
                }
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
    let mut sd_cards = Vec::new();
    let mut buttons = Vec::new();
    let mut halt_hooks = Vec::new();
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
        mtvec = mtvec.or(boot.mtvec.map(u64::from));
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
            binaries.push((path, u64::from(address)));
        }
        for (value, controller, capacity) in boot.sd_cards {
            let path = monitor::asset_path(&value, &script, root, asset_dir.as_deref())?;
            sd_cards.push((path, controller, capacity));
        }
        buttons = boot.buttons;
        for (cpu, symbol, target, value) in boot.halt_hooks {
            let file = monitor::asset_path(&value, &script, root, asset_dir.as_deref())?;
            let address = fs::read(&file)
                .map_err(|e| e.to_string())
                .and_then(|bytes| image64::symbol(&bytes, &symbol))
                .map_err(|error| format!("{}: {error}", file.display()))?;
            halt_hooks.push((cpu, address, target));
        }
        boot_pcs = boot.boot_pcs;
    }
    if path.is_none() && binaries.is_empty() {
        return Err("missing firmware image; see run --help".into());
    }
    if rv64 && (!raw || platform_path.is_some()) {
        return Err("--rv64 requires --raw without --platform".into());
    }
    if uart_stdin_reset && !uart_stdin {
        return Err("--uart-stdin-reset requires --uart-stdin".into());
    }
    if uart_input_at.is_some() && uart_input.is_none() {
        return Err("--uart-input-at requires --uart-input".into());
    }
    if let Some(path) = uart_input {
        uart_events.insert(0, (uart_input_at.unwrap_or(0), path));
    }
    let boot_machine = || -> Result<Machine, String> {
        let mut machine = match platform_path.as_deref() {
            Some(platform_path) => {
                if ram_mib.is_some() || !extra_ram.is_empty() || clint.is_some() {
                    return Err(
                    "--platform defines RAM and CLINT and cannot be combined with manual mappings"
                        .into(),
                );
                }
                platform::load(platform_path, platform_root.as_deref())?
            }
            None => {
                if platform_root.is_some() {
                    return Err("--platform-root requires --platform".into());
                }
                Machine::new(ram_mib.unwrap_or(16) * 1024 * 1024)?
            }
        };
        for (gpio_name, button_name, pressed) in &buttons {
            let gpio = machine
                .bus
                .rv64_devices
                .mpfs_gpio
                .iter_mut()
                .find(|gpio| {
                    gpio.name == *gpio_name
                        && gpio
                            .button
                            .as_ref()
                            .is_some_and(|(name, _, _)| name == button_name)
                })
                .ok_or_else(|| format!("unknown GPIO button {gpio_name}.{button_name}"))?;
            gpio.set_button(*pressed);
        }
        for (source, pc, target) in &halt_hooks {
            machine.halt_on_pc(source, *pc, target)?;
        }
        for (path, name, capacity) in &sd_cards {
            let controller = machine
                .bus
                .rv64_devices
                .mpfs_sd
                .iter_mut()
                .find(|sd| sd.name == *name)
                .ok_or_else(|| format!("unknown MPFS SD controller {name:?}"))?;
            controller
                .attach(path, *capacity)
                .map_err(|error| format!("{}: {error}", path.display()))?;
        }
        if rv64 {
            machine.primary.rv64 = Some(Box::new(rv64::Rv64::new(rv64::Isa::IM)));
        }
        for &(base, size) in &extra_ram {
            machine.bus.map_ram64(base, size)?;
        }
        if let Some(base) = clint {
            machine.bus.map_clint(base, harts.unwrap_or(1))?;
        }
        if let Some(name) = uart_device.as_deref() {
            machine.bus.rv64_devices.select_uart(name)?;
        }
        for (step, path) in &uart_events {
            let bytes = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
            machine.schedule_uart_input(*step, bytes)?;
        }
        if let Some(value) = vex_irq_mask {
            machine.primary.set_vex_irq_mask(value)?;
        }
        if let Some(path) = path.as_deref() {
            let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
            image::load(&mut machine, file, raw, renode_elf_load)
                .map_err(|e| format!("{}: {e}", path.display()))?;
        }
        if let Some(value) = mtvec {
            if let Some(cpu) = &mut machine.primary.rv64 {
                cpu.privilege.set_vector(value);
            } else {
                machine
                    .primary
                    .set_mtvec(u32::try_from(value).map_err(|_| "mtvec exceeds 32 bits")?);
            }
        }
        for (path, address) in binaries.iter().chain(&companion_images) {
            let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
            image::load_binary(&mut machine, file, *address)
                .map_err(|e| format!("{}: {e}", path.display()))?;
        }
        if let Some(count) = harts {
            if count != 1 && machine.is_cortex_m() {
                return Err("--harts requires a RISC-V machine".into());
            }
            if machine.hart_count() != 1 {
                return Err("--harts cannot override multiple CPU declarations".into());
            }
            machine.configure_harts(count);
        }
        for (cpu, address) in &boot_pcs {
            machine.set_named_entry(cpu, *address)?;
        }
        if let Some((path, address)) = &dtb {
            let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
            image::load_rv64_dtb(&mut machine, file, *address)
                .map_err(|e| format!("{}: {e}", path.display()))?;
        }
        machine.validate_entry()?;
        if let Some((local, peer)) = ethernet_loopback {
            machine.bus.connect_rv64_ethernet(local, peer)?;
        }
        if uart_stdin && !machine.bus.has_uart_input() {
            return Err("--uart-stdin requires a platform UART with input support".into());
        }
        if let Some((address, robot)) = tohost {
            if machine.primary.rv64.is_none() {
                return Err("tohost monitoring requires an RV64 machine".into());
            }
            if robot {
                if machine.hart_count() != 1 {
                    return Err("--robot-tohost requires a single-hart RV64 machine".into());
                }
                machine.bus.watch_robot_tohost(address)?;
            } else {
                machine.bus.watch_rv64_tohost(address)?;
            }
        }
        Ok(machine)
    };
    let mut machine = boot_machine()?;

    let mut output = LineWriter::new(io::stdout().lock());
    let start = Instant::now();
    let result = run_machine(
        &mut machine,
        max_steps,
        &mut output,
        &boot_machine,
        uart_stdin,
        uart_stdin_reset,
    );
    let elapsed = start.elapsed();
    output.flush().map_err(|e| format!("UART: {e}"))?;
    let (stop, steps) = result?;
    if stats {
        eprintln!(
            "{stop:?}, {steps} instructions, {:.3} ms, pc={:#010x}",
            elapsed.as_secs_f64() * 1000.0,
            machine.primary.program_counter()
        );
    }
    if let Stop::TestExit(code) = stop {
        eprintln!("penv: TEST FINISHED");
        eprintln!("penv: tohost exit code {code}");
        return Ok(if code == 0 {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        });
    }
    if tohost.is_some() && stop != Stop::Limit {
        return Err("execution stopped before a tohost completion write".into());
    }
    if stop == Stop::Limit {
        eprintln!(
            "penv: instruction limit reached at pc={:#010x}",
            machine.primary.program_counter()
        );
        return Ok(ExitCode::from(2));
    }
    Ok(ExitCode::SUCCESS)
}

fn uart_stdin_receiver() -> Result<std::sync::mpsc::Receiver<io::Result<Vec<u8>>>, String> {
    let (sender, receiver) = std::sync::mpsc::sync_channel(8);
    std::thread::Builder::new()
        .name("uart-stdin".into())
        .spawn(move || {
            let mut input = io::stdin().lock();
            let mut buffer = [0; 4096];
            loop {
                let bytes = match input.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(count) => Ok(buffer[..count].to_vec()),
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => {
                        let _ = sender.send(Err(error));
                        break;
                    }
                };
                if sender.send(bytes).is_err() {
                    break;
                }
            }
        })
        .map_err(|error| format!("UART stdin: {error}"))?;
    Ok(receiver)
}

fn reset_machine(
    machine: &mut Machine,
    output: &mut impl Write,
    boot: &impl Fn() -> Result<Machine, String>,
) -> Result<(), String> {
    output.flush().map_err(|error| format!("UART: {error}"))?;
    // Release the old RAM and socket before reloading firmware and rebinding.
    drop(std::mem::replace(machine, Machine::without_default_ram()));
    *machine = boot()?;
    Ok(())
}

fn run_machine(
    machine: &mut Machine,
    mut remaining: u64,
    output: &mut impl Write,
    boot: &impl Fn() -> Result<Machine, String>,
    uart_stdin: bool,
    uart_stdin_reset: bool,
) -> Result<(Stop, u64), String> {
    let receiver = uart_stdin.then(uart_stdin_receiver).transpose()?;
    let mut retired = 0;
    let mut escaped = false;
    while remaining != 0 {
        for bytes in receiver
            .iter()
            .flat_map(|receiver| receiver.try_iter())
            .take(8)
        {
            let bytes = bytes.map_err(|error| format!("UART stdin: {error}"))?;
            if uart_stdin_reset {
                let mut uart = Vec::with_capacity(bytes.len());
                for byte in bytes {
                    match (std::mem::take(&mut escaped), byte) {
                        (false, 0x1d) => escaped = true,
                        (true, b'r') => {
                            uart.clear();
                            reset_machine(machine, output, boot)?;
                        }
                        (true, 0x1d) | (false, _) => uart.push(byte),
                        (true, _) => uart.extend([0x1d, byte]),
                    }
                }
                machine.bus.queue_uart_input(uart);
            } else {
                machine.bus.queue_uart_input(bytes);
            }
        }
        let batch = if uart_stdin {
            remaining.min(65_536)
        } else {
            remaining
        };
        let before = machine.elapsed_steps();
        let (stop, count) = machine.run(batch, output)?;
        remaining -= machine.elapsed_steps().wrapping_sub(before);
        retired += count;
        match stop {
            Stop::Reset if remaining != 0 => {
                escaped = false;
                reset_machine(machine, output, boot)?;
            }
            Stop::Reset | Stop::Limit => {
                // Login and shell prompts are often written without a newline.
                if uart_stdin {
                    output.flush().map_err(|error| format!("UART: {error}"))?;
                }
            }
            _ => return Ok((stop, retired)),
        }
    }
    Ok((Stop::Limit, retired))
}

fn parse_address(value: &str) -> Result<u32, String> {
    let parsed = match value.strip_prefix("0x") {
        Some(hex) => u32::from_str_radix(hex, 16),
        None => value.parse(),
    };
    parsed.map_err(|_| format!("invalid 32-bit number: {value}"))
}

fn parse_address64(value: &str) -> Result<u64, String> {
    let parsed = match value.strip_prefix("0x") {
        Some(hex) => u64::from_str_radix(hex, 16),
        None => value.parse(),
    };
    parsed.map_err(|_| format!("invalid 64-bit number: {value}"))
}
