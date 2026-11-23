mod bus;
mod clint;
mod compressed;
mod csr;
mod dwt;
mod image;
mod machine;
mod monitor;
mod murax;
mod nrf52840;
mod ns16550;
mod nvic;
mod platform;
mod plic;
mod pmp;
mod pydev;
mod semihosting;
mod stm32_adc;
mod stm32_can;
mod stm32_crc;
mod stm32_dma;
mod stm32_exti;
mod stm32_flash;
mod stm32_gpio;
mod stm32_gpio_board;
mod stm32_i2c;
mod stm32_rcc;
mod stm32_rtc;
mod stm32_spi;
mod stm32_timer;
mod stm32_uart;
mod sv32;
mod systick;
mod thumb;
mod thumb_llvm;
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
       penv validate-platform REPL
       penv thumb-assemble SOURCE
       penv thumb-disassemble ADDRESS HEXBYTES
       penv thumb-disassemble-memory REPL ELF ADDRESS SIZE

Thumb assembly tools require llvm-mc on PATH. HEXBYTES are in memory order.

Options:
  --asset-dir DIR    Resolve RESC firmware URLs to local files by filename
  --script PATH      Read boot settings from a Renode RESC file
  --raw              Load a flat binary (default: ELF32)
  --hex              Load an Intel HEX image on a Cortex-M platform
  --ram-mib N        RAM size in MiB, 1–1024 (default: 16)
  --map-ram BASE:SIZE Add a RAM region in bytes (decimal or 0x-prefixed)
  --clint BASE       Map a CLINT for the configured harts (decimal or 0x-prefixed)
  --platform PATH    Load RAM, CPU and CLINT from a Renode REPL file
  --button-event STEP:NAME:STATE  Schedule a button press or release (repeatable)
  --led-events       Print board LED transitions to stderr
  --uart-steps       Prefix UART lines with their first output step
  --adxl-event STEP:NAME:X:Y:Z  Set nRF52840 ADXL372 acceleration in g at a step
  --adc-input CHANNEL:VOLTS  Set the initial STM32 ADC channel voltage (repeatable)
  --adc-event STEP:CHANNEL:VOLTS  Change an STM32 ADC channel voltage at a step (repeatable)
  --stm32f0-console  Require an STM32F0 REPL (legacy console option)
  --platform-root DIR  Root for working-directory-relative REPL includes
  --reset-at STEP    Reset the Cortex-M machine before this step (repeatable)
  --uart-event STEP:PATH  Queue file bytes at a step (repeatable)
  --uart-input PATH  Queue bytes from a file for the platform UART
  --ble-hci-controller PATH  Use two external HCI controllers with --ble-peer
  --ble-peer PATH    Run a second nRF52840 ELF on an in-process RADIO medium
  --i2s-input PATH   Feed PCM bytes to nRF52840 I2S
  --i2s-output PATH  Save PCM bytes transmitted by nRF52840 I2S
  --nrf-camera-image PATH  Preload a JPEG into nRF52840 ArduCAM
  --lsm9ds1-samples PATH  Load nRF52840 accelerometer samples in g
  --pdm-input PATH   Feed signed 16-bit little-endian PCM to nRF52840 PDM
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
        Some(Some(
            command @ ("thumb-assemble" | "thumb-disassemble" | "thumb-disassemble-memory"),
        )) => {
            thumb_llvm::run(command, args)?;
            return Ok(ExitCode::SUCCESS);
        }
        Some(Some("validate-platform")) => {
            let path = args
                .next()
                .ok_or("validate-platform requires one REPL path")?;
            if args.next().is_some() {
                return Err("validate-platform requires one REPL path".into());
            }
            platform::load(std::path::Path::new(&path), None, false)?;
            return Ok(ExitCode::SUCCESS);
        }
        Some(Some("run")) => {}
        _ => return Err("expected 'run'; see --help".into()),
    }

    let mut raw = false;
    let mut hex = false;
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
    let mut adc_inputs = Vec::new();
    let mut adc_events = Vec::new();
    let mut adxl_events = Vec::new();
    let mut button_events = Vec::new();
    let mut led_events = false;
    let mut uart_steps = false;
    let mut uart_input = None;
    let mut pdm_input = None;
    let mut camera_image = None;
    let mut lsm9ds1_samples = None;
    let mut i2s_input = None;
    let mut i2s_output = None;
    let mut ble_peer = None;
    let mut ble_hci_controller = None;
    let mut uart_input_at = None;
    let mut uart_events = Vec::new();
    let mut reset_steps = Vec::new();
    let mut path = None;
    while let Some(arg) = args.next() {
        match arg.to_str().filter(|_| !positional) {
            Some("-h" | "--help") => {
                println!("{HELP}");
                return Ok(ExitCode::SUCCESS);
            }
            Some("--") => positional = true,
            Some("--raw") => raw = true,
            Some("--hex") => hex = true,
            Some("--stm32f0-console") => stm32f0_console = true,
            Some("--stats") => stats = true,
            Some(
                option @ ("--platform"
                | "--platform-root"
                | "--uart-input"
                | "--script"
                | "--asset-dir"
                | "--pdm-input"
                | "--i2s-input"
                | "--i2s-output"
                | "--ble-peer"
                | "--ble-hci-controller"
                | "--nrf-camera-image"
                | "--lsm9ds1-samples"),
            ) => {
                let path = args
                    .next()
                    .map(PathBuf::from)
                    .ok_or_else(|| format!("{option} requires a path"))?;
                let target = match option {
                    "--platform" => &mut platform_path,
                    "--script" => &mut script,
                    "--asset-dir" => &mut asset_dir,
                    "--pdm-input" => &mut pdm_input,
                    "--nrf-camera-image" => &mut camera_image,
                    "--lsm9ds1-samples" => &mut lsm9ds1_samples,
                    "--i2s-input" => &mut i2s_input,
                    "--i2s-output" => &mut i2s_output,
                    "--ble-peer" => &mut ble_peer,
                    "--ble-hci-controller" => &mut ble_hci_controller,
                    "--platform-root" => &mut platform_root,
                    _ => &mut uart_input,
                };
                if target.replace(path).is_some() {
                    return Err(format!("{option} may only be specified once"));
                }
            }
            Some("--led-events") => led_events = true,
            Some("--uart-steps") => uart_steps = true,
            Some("--adxl-event") => {
                let usage = "--adxl-event requires STEP:NAME:X:Y:Z";
                let value = args
                    .next()
                    .and_then(|value| value.into_string().ok())
                    .ok_or(usage)?;
                let fields: Vec<_> = value.split(':').collect();
                let [step, name, x, y, z] = fields.as_slice() else {
                    return Err(usage.into());
                };
                let step = step.parse().map_err(|_| "invalid --adxl-event step")?;
                let mut acceleration = [0; 3];
                for (axis, value) in acceleration.iter_mut().zip([x, y, z]) {
                    let gs: f64 = value.parse().map_err(|_| "invalid ADXL372 acceleration")?;
                    if !(-200.0..=200.0).contains(&gs) {
                        return Err("ADXL372 acceleration must be within ±200 g".into());
                    }
                    *axis = (gs * 10.0).round() as i16;
                }
                adxl_events.push((step, (*name).to_string(), acceleration));
            }
            Some("--button-event") => {
                let value = args
                    .next()
                    .and_then(|value| value.into_string().ok())
                    .ok_or("--button-event requires STEP:NAME:STATE")?;
                let mut fields = value.splitn(3, ':');
                let step = fields
                    .next()
                    .unwrap()
                    .parse()
                    .map_err(|_| "invalid --button-event step")?;
                let name = fields
                    .next()
                    .filter(|name| !name.is_empty())
                    .ok_or("--button-event requires a button name")?;
                let pressed = match fields.next() {
                    Some("press") => true,
                    Some("release") => false,
                    _ => return Err("--button-event state must be press or release".into()),
                };
                button_events.push((step, name.to_owned(), pressed));
            }
            Some(option @ ("--adc-input" | "--adc-event")) => {
                let usage = || {
                    format!(
                        "{option} requires {}",
                        if option == "--adc-event" {
                            "STEP:CHANNEL:VOLTS"
                        } else {
                            "CHANNEL:VOLTS"
                        }
                    )
                };
                let value = args
                    .next()
                    .and_then(|value| value.into_string().ok())
                    .ok_or_else(usage)?;
                let (step, input) = if option == "--adc-event" {
                    let (step, input) = value.split_once(':').ok_or_else(usage)?;
                    let step = step
                        .parse::<u64>()
                        .map_err(|_| "invalid --adc-event step")?;
                    (Some(step), input)
                } else {
                    (None, value.as_str())
                };
                let (channel, volts) = input.split_once(':').ok_or_else(usage)?;
                let channel = channel
                    .parse()
                    .map_err(|_| format!("invalid {option} channel"))?;
                let volts = volts
                    .parse()
                    .map_err(|_| format!("invalid {option} voltage"))?;
                if let Some(step) = step {
                    adc_events.push((step, channel, volts));
                } else {
                    adc_inputs.push((channel, volts));
                }
            }
            Some("--reset-at") => {
                let step = args
                    .next()
                    .and_then(|value| value.into_string().ok())
                    .and_then(|value| value.parse::<u64>().ok())
                    .ok_or("--reset-at requires a nonnegative step number")?;
                reset_steps.push(step);
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

    if raw && hex {
        return Err("--raw and --hex are mutually exclusive".into());
    }
    let mut binaries = Vec::new();
    let mut boot_pcs = Vec::new();
    if asset_dir.is_some() && script.is_none() {
        return Err("--asset-dir requires --script".into());
    }
    let mut named_analyzers = Vec::new();
    let mut uart_irqs = Vec::new();
    let mut performance_in_mips = None;
    let mut fault_on_unmapped_data = false;
    let mut report_unmapped_data = false;
    let mut pend_hard_fault = false;
    let mut thumb_trace = None;
    let mut ram_access_logs = Vec::new();
    let mut ram_io_pages = Vec::new();
    let mut cpu_hooks = Vec::new();
    let mut cpu_control = Vec::new();
    let mut gpio_changes = Vec::new();
    let mut itstate_log_hook = None;
    let mut opcode_counters = Vec::new();
    let mut opcode_counting = None;
    let mut opcode_queries = Vec::new();
    let mut opcode_hooks = Vec::new();
    let mut semihosting_arguments = None;
    let mut semihosting_directory = None;
    let mut semihosting_temporary_directory = None;
    if let Some(script) = script {
        if platform_path.is_some() || raw || hex {
            return Err("--script cannot be combined with --platform, --raw, or --hex".into());
        }
        let root = platform_root
            .as_deref()
            .unwrap_or(std::path::Path::new("."));
        let boot = monitor::load(&script, root)?;
        if let Some(pydev) = boot.pydev {
            if path.is_some() {
                return Err("CPU-less PyDev scripts do not accept a firmware override".into());
            }
            pydev.run(&mut io::stdout().lock())?;
            return Ok(ExitCode::SUCCESS);
        }
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
        named_analyzers = boot.named_analyzers;
        uart_irqs = boot.uart_irqs;
        performance_in_mips = boot.performance_in_mips;
        fault_on_unmapped_data = boot.fault_on_unmapped_data;
        report_unmapped_data = boot.report_unmapped_data;
        pend_hard_fault = boot.pend_hard_fault;
        thumb_trace = boot.thumb_trace;
        ram_access_logs = boot.ram_access_logs;
        ram_io_pages = boot.ram_io_pages;
        cpu_hooks = boot.cpu_hooks;
        cpu_control = boot.cpu_control;
        gpio_changes = boot.gpio_changes;
        itstate_log_hook = boot.itstate_log_hook;
        opcode_counters = boot.opcode_counters;
        opcode_counting = boot.opcode_counting;
        opcode_queries = boot.opcode_queries;
        opcode_hooks = boot.opcode_hooks;
        semihosting_arguments = boot.semihosting_arguments;
        semihosting_directory = boot.semihosting_directory;
        semihosting_temporary_directory = boot.semihosting_temporary_directory;
    }
    if path.is_none() && binaries.is_empty() {
        return Err("missing firmware image; see run --help".into());
    }
    let mut machine = match platform_path.as_ref() {
        Some(platform_path) => {
            if ram_mib.is_some() || !extra_ram.is_empty() || clint.is_some() {
                return Err(
                    "--platform defines RAM and CLINT and cannot be combined with manual mappings"
                        .into(),
                );
            }
            platform::load(platform_path, platform_root.as_deref(), stm32f0_console)?
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
    if fault_on_unmapped_data || report_unmapped_data {
        if !machine.has_cortex_m4() {
            return Err("UnhandledAccessBehaviour requires a Cortex-M4 platform".into());
        }
        machine.bus.fault_on_unmapped_thumb_data = fault_on_unmapped_data;
        machine.bus.report_unmapped_thumb_data = report_unmapped_data;
    }
    for change in gpio_changes {
        match change {
            monitor::GpioChange::Unregister(name) => {
                machine.bus.unregister_nrf_gpio_peripheral(&name)?
            }
            monitor::GpioChange::Bridge(connector, led) => {
                machine.bus.bridge_nrf_led(&connector, &led)?
            }
            monitor::GpioChange::RemoveConnector(connector) => {
                machine.bus.remove_nrf_gpio_connector(&connector)?
            }
        }
    }
    if let Some(arguments) = semihosting_arguments {
        let handler = machine
            .bus
            .semihosting
            .as_mut()
            .ok_or("ProgramArguments requires a semihosting handler")?;
        handler.program_arguments = arguments;
    }
    if let Some(directory) = semihosting_directory {
        let handler = machine
            .bus
            .semihosting
            .as_mut()
            .ok_or("WorkingDirectory requires a semihosting handler")?;
        handler.working_directory = Some(directory);
    }
    if let Some(directory) = semihosting_temporary_directory {
        let handler = machine
            .bus
            .semihosting
            .as_mut()
            .ok_or("TemporaryFilesDirectory requires a semihosting handler")?;
        handler.temporary_files_directory = Some(directory);
    }
    if let Some(mips) = performance_in_mips {
        if ble_peer.is_some() && mips != 100 {
            return Err("non-default PerformanceInMips requires a single nRF52840 node".into());
        }
        machine.bus.set_nrf_cpu_mips(mips)?;
    }
    for name in named_analyzers {
        if !machine.bus.has_nrf_uart(&name) {
            return Err(format!("unsupported UART analyzer target {name:?}"));
        }
    }
    if i2s_input.is_some() || i2s_output.is_some() {
        let input = if let Some(path) = i2s_input {
            fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?
        } else {
            Vec::new()
        };
        machine
            .bus
            .configure_nrf_i2s_pcm(input, i2s_output.is_some())?;
    }
    if let Some(path) = camera_image {
        let image = fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        machine.bus.set_nrf_camera_image(image)?;
    }
    if let Some(path) = lsm9ds1_samples {
        let samples = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        machine.bus.set_nrf_lsm9ds1_samples(&samples)?;
    }
    if let Some(path) = pdm_input {
        let bytes = fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        machine
            .bus
            .set_nrf_pdm_input(bytes)
            .map_err(|e| format!("{}: {e}", path.display()))?;
    }
    machine.bus.report_led_events(led_events);
    machine.report_uart_steps(uart_steps);
    for (step, name, pressed) in button_events {
        machine.schedule_button(step, &name, pressed)?;
    }
    for (channel, volts) in adc_inputs {
        machine.bus.set_stm32f0_adc_input(channel, volts)?;
    }
    for (step, name, acceleration) in adxl_events {
        machine.schedule_adxl_input(step, name, acceleration)?;
    }
    for (step, channel, volts) in adc_events {
        machine.schedule_adc_input(step, channel, volts)?;
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
    for step in reset_steps {
        machine.schedule_reset(step)?;
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
        let loaded = if hex {
            image::load_hex(&mut machine, file)
        } else {
            image::load(&mut machine, file, raw)
        };
        loaded.map_err(|e| format!("{}: {e}", path.display()))?;
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
    if !machine.is_cortex_m() {
        for (cpu, address) in &boot_pcs {
            machine.set_named_entry(cpu, *address)?;
        }
    }
    machine.validate_entry()?;
    let entry_pc = machine.primary.pc;
    if machine.is_cortex_m() {
        for (cpu, address) in boot_pcs {
            machine.set_cortex_m_monitor_pc(&cpu, address, &mut io::stderr().lock())?;
        }
    }

    if !opcode_counters.is_empty()
        || opcode_counting.is_some()
        || !opcode_queries.is_empty()
        || !opcode_hooks.is_empty()
    {
        if !machine.has_cortex_m4() || ble_peer.is_some() {
            return Err("opcode instrumentation requires a single Cortex-M4 machine".into());
        }
        for (name, pattern) in opcode_counters {
            machine.bus.thumb_opcodes.install(&name, &pattern)?;
        }
        machine.bus.thumb_opcodes.enabled = opcode_counting.unwrap_or(false);
        for (phase, mask, opcode, label) in opcode_hooks {
            machine
                .bus
                .thumb_opcode_hooks
                .install(phase, mask, opcode, label);
        }
        for name in &opcode_queries {
            machine.bus.thumb_opcodes.get(name)?;
        }
    }
    if itstate_log_hook.is_some() {
        if !machine.has_cortex_m4() || ble_peer.is_some() {
            return Err("single-step ITSTATE logging requires a single Cortex-M4 machine".into());
        }
        machine.bus.itstate_log_hook = itstate_log_hook;
    }
    if !cpu_hooks.is_empty() {
        if !machine.is_cortex_m() || ble_peer.is_some() {
            return Err("address hooks require a single Cortex-M machine".into());
        }
        for (pc, hook) in cpu_hooks {
            machine
                .bus
                .cpu_hooks
                .entry(pc.unwrap_or(entry_pc))
                .or_default()
                .push(hook);
        }
    }
    if !ram_access_logs.is_empty() || !ram_io_pages.is_empty() {
        if !machine.has_cortex_m4() || ble_peer.is_some() {
            return Err("RAM I/O logging requires a single Cortex-M4 machine".into());
        }
        for name in ram_access_logs {
            machine.bus.log_ram_access(&name)?;
        }
        for (address, enabled) in ram_io_pages {
            machine.bus.set_page_access_via_io(address, enabled);
        }
    }
    if let Some(config) = thumb_trace {
        if !machine.has_cortex_m4() || ble_peer.is_some() {
            return Err("execution tracing requires a single Cortex-M4 machine".into());
        }
        machine.bus.thumb_trace = Some(thumb::trace::Trace::create(&config.path, config.binary)?);
    }
    if pend_hard_fault {
        if !machine.has_cortex_m4() {
            return Err("nvic SetPendingIRQ 3 requires a Cortex-M4 platform".into());
        }
        machine
            .bus
            .nvic
            .as_mut()
            .ok_or("nvic SetPendingIRQ 3 requires an NVIC")?
            .set_hard_fault_pending(true);
    }

    if ble_hci_controller.is_some() && ble_peer.is_none() {
        return Err("--ble-hci-controller requires --ble-peer".into());
    }
    let mut output = LineWriter::new(io::stdout().lock());
    if !cpu_control.is_empty() {
        if !machine.is_cortex_m() || ble_peer.is_some() {
            return Err("CPU control commands require a single Cortex-M machine".into());
        }
        for command in cpu_control {
            match command {
                monitor::CpuControl::Pause(true) => machine.pause_cpu(),
                monitor::CpuControl::Pause(false) => machine.resume_cpu(),
                monitor::CpuControl::Step(count) => {
                    machine.monitor_step_cortex_m(count, &mut output)?
                }
                monitor::CpuControl::Read32(address) => {
                    let value = machine
                        .bus
                        .read::<4>(address)
                        .map_err(|error| error.to_string())?;
                    writeln!(output, "{value:#010x}").map_err(|error| error.to_string())?;
                }
                monitor::CpuControl::ReadWatchpoint(address, width) => {
                    machine.bus.add_thumb_read_watchpoint(address, width)
                }
                monitor::CpuControl::Halt(halted) => machine.set_cpu_halted(halted),
                monitor::CpuControl::Clock(target, enabled) => match target.as_str() {
                    "cpu" | "cpu0" if machine.primary.name.as_deref() == Some(&target) => {
                        machine.set_cpu_clocked(enabled)
                    }
                    "nvic" => machine
                        .bus
                        .systick
                        .as_mut()
                        .ok_or("Clocked requires SysTick")?
                        .set_clocked(enabled),
                    "dwt" => machine
                        .bus
                        .dwt
                        .as_mut()
                        .ok_or("Clocked requires DWT")?
                        .set_clocked(enabled),
                    _ => return Err(format!("unknown Clocked target {target:?}")),
                },
                monitor::CpuControl::Reset(kind) => machine.reset_cpu_control(kind, &mut output)?,
                monitor::CpuControl::ResetLog(message) => machine.reset_log = Some(message),
            }
        }
    }
    for command in uart_irqs {
        match command {
            monitor::UartIrq::Read(uart) => {
                let state = if machine.bus.nrf_uart_irq(&uart, None)? {
                    "set"
                } else {
                    "unset"
                };
                writeln!(output, "GPIO: {state}").map_err(|error| error.to_string())?;
            }
            monitor::UartIrq::Set(uart, level) => {
                machine.bus.nrf_uart_irq(&uart, Some(level))?;
            }
        }
    }
    let start = Instant::now();
    let result = if let Some(peer_path) = ble_peer {
        let platform = platform_path
            .as_deref()
            .ok_or("--ble-peer requires a platform")?;
        let mut peer = platform::load(platform, platform_root.as_deref(), false)?;
        let image = File::open(&peer_path).map_err(|e| format!("{}: {e}", peer_path.display()))?;
        image::load(&mut peer, image, false)
            .map_err(|e| format!("{}: {e}", peer_path.display()))?;
        peer.validate_entry()?;
        if let Some(controller_path) = ble_hci_controller {
            let load_controller = || -> Result<_, String> {
                let mut controller = platform::load(platform, platform_root.as_deref(), false)?;
                let file = File::open(&controller_path)
                    .map_err(|e| format!("{}: {e}", controller_path.display()))?;
                image::load(&mut controller, file, false)?;
                controller.validate_entry()?;
                Ok(controller)
            };
            let mut controllers = [load_controller()?, load_controller()?];
            machine.run_hci_radio_pair(&mut peer, &mut controllers, max_steps, &mut output)
        } else {
            machine.run_radio_pair(&mut peer, max_steps, &mut output)
        }
    } else {
        machine.run(max_steps, &mut output)
    };
    let elapsed = start.elapsed();
    output.flush().map_err(|e| format!("UART: {e}"))?;
    if let Some(path) = i2s_output
        && let Some(pcm) = machine.bus.nrf_i2s_pcm_output()
    {
        fs::write(&path, pcm).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    if let Some(trace) = machine.bus.thumb_trace.take() {
        trace.finish()?;
    }
    let (stop, steps) = result?;
    for name in opcode_queries {
        eprintln!("opcode {name}: {}", machine.bus.thumb_opcodes.get(&name)?);
    }
    if stats {
        eprintln!(
            "{stop:?}, {steps} instructions, {:.3} ms, pc={:#010x}",
            elapsed.as_secs_f64() * 1000.0,
            machine.primary.pc
        );
    }
    if stop == Stop::Paused {
        eprintln!("penv: machine paused at pc={:#010x}", machine.primary.pc);
    }
    if let Stop::Semihosting(exit) = stop {
        eprintln!("cpu.semihosting: {exit}");
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
