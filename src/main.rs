mod bus;
mod clint;
mod compressed;
mod constant_mmio;
mod csr;
mod designware_usb;
mod dwt;
mod ethernet;
mod ethernet_switch;
mod iis2mdc;
mod image;
mod image64;
mod litex;
mod litex_eth;
mod litex_ethernet;
mod litex_flash;
mod litex_i2c;
mod litex_i2s;
mod litex_panel;
mod litex_sd;
mod litex_tftp;
mod litex_video;
mod machine;
mod mii_phy;
mod monitor;
mod murax;
mod network_monitor;
mod network_platform;
mod network_repl;
mod network_resc;
mod nrf52840;
mod nrf_repl;
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
mod simulation;
mod stm32_adc;
mod stm32_can;
mod stm32_crc;
mod stm32_dma;
mod stm32_ethernet;
mod stm32_exti;
mod stm32_flash;
mod stm32_gpio;
mod stm32_gpio_board;
mod stm32_i2c;
mod stm32_iwdg;
mod stm32_rcc;
mod stm32_rtc;
mod stm32_spi;
mod stm32_timer;
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
mod stm32h7_crypto;
mod stm32h7_dma2d;
mod stm32h7_dsi;
mod stm32h7_ethernet;
mod stm32h7_exti;
mod stm32h7_fdcan;
mod stm32h7_flash;
mod stm32h7_gpio;
mod stm32h7_gpio_platform;
mod stm32h7_hsem;
mod stm32h7_i2c;
mod stm32h7_lptim;
mod stm32h7_ltdc;
mod stm32h7_net_control;
mod stm32h7_qspi;
mod stm32h7_rng;
mod stm32h7_rtc;
mod stm32h7_rtc_platform;
mod stm32h7_sdmmc;
mod stm32h7_spi;
mod stm32h7_timer;
mod sv32;
mod systick;
mod thumb;
mod uart;
mod veml7700;
mod zephyr_boot;

use std::{
    env,
    fs::{self, File},
    io::{self, LineWriter, Read, Write},
    path::PathBuf,
    process::ExitCode,
    sync::mpsc,
    thread,
    time::Instant,
};

use machine::{Machine, Stop};

const HELP: &str = "Run a bare-metal firmware image.

Usage: penv run [OPTIONS] IMAGE
       penv run --script PATH [OPTIONS] [LOCAL_IMAGE]
       penv switch LISTEN_PORT GUEST_PORT GUEST_PORT [GUEST_PORT ...]

Options:
  --framebuffer-event STEP:PATH  Save a LiteX framebuffer as PPM at a step (repeatable)
  --wait-for-frame STEP:PATH  Stop when the LiteX framebuffer matches a PNG or P6 RGB PPM
  --wait-for-frame-roi STEP:X:Y:WIDTH:HEIGHT:PATH  Wait for identical RGB pixels in a region
  --framebuffer-out PATH  Write the final LiteX framebuffer as PPM
  --asset-dir DIR    Resolve RESC firmware URLs to local files by filename
  --script PATH      Read boot settings from a Renode RESC file
  --raw              Load a flat binary (default: ELF)
  --rv64             Run a raw image on an RV64IM CPU
  --hex              Load an Intel HEX image on a Cortex-M platform
  --ram-mib N        RAM size in MiB, 1–1024 (default: 16)
  --map-ram BASE:SIZE Add a RAM region in bytes (decimal or 0x-prefixed)
  --load-binary BASE:PATH  Load a flat companion image into RAM (repeatable)
  --dtb BASE:PATH    Load an RV64 boot DTB and pass its address in a1
  --clint BASE       Map a CLINT for the configured harts (decimal or 0x-prefixed)
  --litex-timer-alias BASE  Map the existing LiteX timer at an additional address
  --platform PATH    Load RAM, CPU and CLINT from a Renode REPL file
  --button-event STEP:NAME:STATE  Schedule a button press or release (repeatable)
  --led-events       Print board LED transitions to stderr
  --adc-input CHANNEL:VOLTS  Set the initial STM32 ADC channel voltage (repeatable)
  --adc-event STEP:CHANNEL:VOLTS  Change an STM32 ADC channel voltage at a step (repeatable)
  --stm32f0-console  Require an STM32F0 REPL (legacy console option)
  --platform-root DIR  Root for working-directory-relative REPL includes
  --litex-phy-event STEP:NAME:VALUE  Set PHY VendorSpecific1 at a step (repeatable)
  --litex-temperature-event STEP:NAME:CELSIUS  Set an SI70xx temperature (repeatable)
  --litex-led-assert STEP:NAME:STATE  Assert a wired LiteX LED is on or off (repeatable)
  --litex-button-event STEP:NAME:ACTION  Press, Release, or Toggle at a step
  --uart-event STEP:PATH  Queue file bytes at a step (repeatable)
  --uart-input PATH  Queue bytes from a file for the platform UART
  --uart-stdin       Forward stdin to the platform UART while running
  --uart-stdin-reset  Enable Ctrl-] r to reload the machine on the UART session
  --uart-device NAME  Select the RV64 platform UART that receives input
  --uart-input-at N Delay UART input until execution step N
  --ethernet-loopback LOCAL:PEER  Link BeagleV MACs over localhost UDP ports
  --litex-uart-stdio  Bridge stdin to LiteX UART at 115200 baud and flush UART output
  --litex-uart-legacy-console  Use legacy LiteX UART console event handling
  --mtvec ADDRESS    Set the initial machine trap vector
  --vex-irq-mask N   Set the initial VexRiscv external interrupt mask
  --harts N          Start N RV32 harts at the image entry (1–8)
  --tohost ADDRESS  Stop on an odd write to the RV32 test completion word
  --tohost-act ADDRESS  Decode RV32 Sail ACT console and completion writes
  --max-steps N      Stop after N execution steps (default: 10000000)
  --sd-image PATH    Attach a read-only image to the STM32H7 SDMMC slot
  --uart-log-dir DIR Save each simulation node UART to a numbered file
  --reset-at STEP    Reset the Cortex-M machine before this step (repeatable)
  --i2s-input PATH   Feed PCM bytes to nRF52840 I2S
  --i2s-output PATH  Save transmitted nRF52840 I2S PCM
  --pdm-input PATH   Feed signed little-endian PCM to nRF52840 PDM
  --ble-peer PATH    Run a second nRF52840 image on a shared radio medium
  --virtual-mips N   Set Cortex-M steps per virtual microsecond
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
    let mut hex = false;
    let mut litex_uart_stdio = false;
    let mut enable_litex_uart_legacy_console = false;
    let mut rv64 = false;
    let mut stats = false;
    let mut positional = false;
    let mut ram_mib = None;
    let mut harts = None;
    let mut tohost = None;
    let mut max_steps = None;
    let mut extra_ram = Vec::new();
    let mut companion_images = Vec::new();
    let mut dtb = None;
    let mut clint = None;
    let mut litex_timer_aliases = Vec::new();
    let mut mtvec = None;
    let mut vex_irq_mask = None;
    let mut platform_path = None;
    let mut script = None;
    let mut asset_dir = None;
    let mut platform_root = None;
    let mut stm32f0_console = false;
    let mut adc_inputs = Vec::new();
    let mut adc_events = Vec::new();
    let mut led_events = false;
    let mut uart_input = None;
    let mut uart_stdin = false;
    let mut uart_stdin_reset = false;
    let mut ethernet_loopback = None;
    let mut uart_device = None;
    let mut uart_input_at = None;
    let mut temperature_events = Vec::new();
    let mut phy_events = Vec::new();
    let mut led_assertions = Vec::new();
    let mut button_events = Vec::new();
    let mut uart_events = Vec::new();
    let mut frame_waits = Vec::new();
    let mut framebuffer_events = Vec::new();
    let mut framebuffer_out = None;
    let mut cli_mips = None;
    let mut reset_steps = Vec::new();
    let mut i2s_input = None;
    let mut i2s_output = None;
    let mut pdm_input = None;
    let mut ble_peer = None;
    let mut sd_image = None;
    let mut uart_log_dir = None;
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
            Some("--litex-uart-stdio") => litex_uart_stdio = true,
            Some("--litex-uart-legacy-console") => enable_litex_uart_legacy_console = true,
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
                | "--asset-dir" | "--framebuffer-out" | "--sd-image" | "--uart-log-dir"
                | "--i2s-input" | "--i2s-output" | "--pdm-input" | "--ble-peer"),
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
                    "--pdm-input" => &mut pdm_input,
                    "--i2s-input" => &mut i2s_input,
                    "--i2s-output" => &mut i2s_output,
                    "--ble-peer" => &mut ble_peer,
                    "--sd-image" => &mut sd_image,
                    "--uart-log-dir" => &mut uart_log_dir,
                    "--framebuffer-out" => &mut framebuffer_out,
                    _ => &mut uart_input,
                };
                if target.replace(path).is_some() {
                    return Err(format!("{option} may only be specified once"));
                }
            }
            Some("--led-events") => led_events = true,
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
            Some("--virtual-mips") => {
                let value = args
                    .next()
                    .and_then(|raw| raw.into_string().ok())
                    .and_then(|raw| raw.parse::<u32>().ok())
                    .filter(|&mips| mips != 0 && mips <= u32::MAX / 1_000_000)
                    .ok_or("--virtual-mips requires a positive supported rate")?;
                cli_mips = Some(value);
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

    if raw && hex {
        return Err("--raw and --hex are mutually exclusive".into());
    }
    let mut binaries = Vec::new();
    let mut boot_pcs = Vec::new();
    let mut vector_tables = Vec::new();
    let mut runtime = Vec::new();
    let mut script_elf_addresses = None;
    let mut silence_ranges = Vec::new();
    let mut vex_supervisor_irq_mask = None;
    let mut buttons = Vec::new();
    let mut tftp = None;
    let mut video_analyzers = Vec::new();
    let mut serial_analyzer = None;
    let mut pcm_inputs = Vec::new();
    let mut pcm_outputs = Vec::new();
    let mut run_steps = None;
    let mut post_run = Vec::new();
    let mut sd_cards = Vec::new();
    let mut mpfs_buttons = Vec::new();
    let mut halt_hooks = Vec::new();
    let mut virtual_mips = None;
    let mut boot_writes = Vec::new();
    let mut zephyr = zephyr_boot::BootOptions::default();
    if asset_dir.is_some() && script.is_none() {
        return Err("--asset-dir requires --script".into());
    }
    if let Some(script) = script.as_deref() {
        if platform_path.is_some() || raw || hex {
            return Err("--script cannot be combined with --platform, --raw, or --hex".into());
        }
        let root = platform_root
            .as_deref()
            .unwrap_or(std::path::Path::new("."));
        let boot = match monitor::load(script, root) {
            Ok(boot) if uart_log_dir.is_none() => boot,
            result => {
                let mut plan =
                    network_monitor::Plan::load(script, root).map_err(|error| match result {
                        Ok(_) => error,
                        Err(original) => format!("{original}; network RESC: {error}"),
                    })?;
                if !plan.is_single_boot() || uart_log_dir.is_some() {
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
                        || framebuffer_out.is_some()
                        || sd_image.is_some()
                    {
                        return Err(
                            "single-machine overrides cannot be combined with a simulation script"
                                .into(),
                        );
                    }
                    return simulation::run(
                        plan,
                        platform_root.as_deref(),
                        asset_dir.as_deref(),
                        max_steps.unwrap_or(10_000_000),
                        stats,
                        uart_log_dir.as_deref(),
                    );
                }
                let boot = plan.nodes.pop().unwrap();
                virtual_mips = boot.boot.mips;
                boot_writes = boot.boot.writes;
                uart_device = uart_device.or(boot.boot.uart_input);
                zephyr = boot.boot.zephyr;
                let mut common = monitor::Boot::default();
                common.platform = boot.boot.platform;
                common.image = boot
                    .boot
                    .image
                    .map(|image| monitor::Asset::new(&image, &boot.source));
                common.binaries = boot
                    .boot
                    .binaries
                    .into_iter()
                    .map(|(image, address)| (monitor::Asset::new(&image, &boot.source), address))
                    .collect();
                common.boot_pcs = boot.boot.boot_pcs;
                common.mtvec = boot.boot.mtvec;
                common.mask = boot.boot.mask;
                common
            }
        };
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
        mtvec = mtvec.or(boot.mtvec.map(u64::from));
        vex_irq_mask = vex_irq_mask.or(boot.mask);
        vex_supervisor_irq_mask = boot.supervisor_mask;
        hex = boot.hex;
        if path.is_some() && boot.image.is_none() {
            return Err(
                "a firmware override requires a LoadELF or LoadHEX command in the script".into(),
            );
        }
        if path.is_none()
            && let Some(image) = boot.image
        {
            path = Some(image.resolve(root, asset_dir.as_deref())?);
        }
        for (value, address) in boot.binaries {
            let path = value.resolve(root, asset_dir.as_deref())?;
            binaries.push((path, u64::from(address)));
        }
        for (value, controller, capacity) in boot.sd_cards {
            let path = value.resolve(root, asset_dir.as_deref())?;
            sd_cards.push((path, controller, capacity));
        }
        mpfs_buttons = boot.mpfs_buttons;
        for (cpu, symbol, target, value) in boot.halt_hooks {
            let file = monitor::Asset::new(&value, script).resolve(root, asset_dir.as_deref())?;
            let address = fs::read(&file)
                .map_err(|e| e.to_string())
                .and_then(|bytes| image64::symbol(&bytes, &symbol))
                .map_err(|error| format!("{}: {error}", file.display()))?;
            halt_hooks.push((cpu, address, target));
        }
        for (device, input) in boot.pcm_inputs {
            pcm_inputs.push((device, input.resolve(root, asset_dir.as_deref())?));
        }
        pcm_outputs = boot.pcm_outputs;
        run_steps = boot.run_steps;
        post_run = boot.post_run;
        boot_pcs = boot.boot_pcs;
        vector_tables = boot.vector_tables;
        runtime = boot.runtime;
        silence_ranges = boot.silence_ranges;
        buttons = boot.buttons;
        video_analyzers = boot.video_analyzers;
        serial_analyzer = boot.serial_analyzer;
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
                load_platform(platform_path, platform_root.as_deref(), stm32f0_console).or_else(
                    |original| {
                        if stm32f0_console {
                            return Err(original);
                        }
                        network_repl::load(platform_path, platform_root.as_deref())
                            .map_err(|error| format!("{original}; network REPL: {error}"))
                    },
                )?
            }
            None => {
                if platform_root.is_some() {
                    return Err("--platform-root requires --platform".into());
                }
                Machine::new(ram_mib.unwrap_or(16) * 1024 * 1024)?
            }
        };
        machine.bus.report_led_events(led_events);
        for (channel, volts) in &adc_inputs {
            machine.bus.set_stm32f0_adc_input(*channel, *volts)?;
        }
        for (step, channel, volts) in &adc_events {
            machine.schedule_adc_input(*step, *channel, *volts)?;
        }
        for &base in &litex_timer_aliases {
            machine.bus.map_litex_timer_alias(base)?;
        }
        if enable_litex_uart_legacy_console {
            machine.bus.enable_litex_uart_legacy_console()?;
        }
        if script_elf_addresses.is_some() && !machine.is_cortex_m() {
            machine.bus.use_renode_unmapped_data();
        }
        if let Some(mips) = cli_mips.or(virtual_mips) {
            if !machine.is_cortex_m() {
                return Err("--virtual-mips requires Cortex-M".into());
            }
            machine
                .bus
                .set_step_hz(mips.checked_mul(1_000_000).ok_or("virtual MIPS overflow")?);
        }
        if let Some((name, server)) = &tftp {
            machine.bus.attach_litex_tftp(name, server.clone())?;
        }
        for analyzer in &video_analyzers {
            let name = analyzer.strip_prefix("sysbus.").unwrap_or(analyzer);
            let video = machine
                .bus
                .litex_video
                .as_ref()
                .is_some_and(|video| video.name == name);
            let uart = machine.bus.rv64_devices.has_uart(name);
            if !video && !uart && !machine.bus.has_nrf_uart(name) {
                return Err(format!("unknown analyzer peripheral {analyzer:?}"));
            }
        }
        if i2s_input.is_some() || i2s_output.is_some() {
            let input = if let Some(path) = &i2s_input {
                fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?
            } else {
                Vec::new()
            };
            machine
                .bus
                .configure_nrf_i2s_pcm(input, i2s_output.is_some())?;
        }
        if let Some(path) = &pdm_input {
            let bytes = fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            machine
                .bus
                .set_nrf_pdm_input(bytes)
                .map_err(|e| format!("{}: {e}", path.display()))?;
        }
        if let Some(path) = &sd_image {
            let image = fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
            machine.bus.attach_sd_image(image)?;
        }
        for &(base, size) in &silence_ranges {
            machine.bus.map_silencer(base, size)?;
        }
        for (gpio_name, button_name, pressed) in &mpfs_buttons {
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
            if let Some(controller) = machine
                .bus
                .rv64_devices
                .mpfs_sd
                .iter_mut()
                .find(|sd| sd.name == *name)
            {
                controller
                    .attach(path, *capacity)
                    .map_err(|error| format!("{}: {error}", path.display()))?;
            } else {
                let file =
                    File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
                let card = litex_sd::Card::new(file, *capacity)
                    .map_err(|error| format!("{}: {error}", path.display()))?;
                machine.bus.attach_litex_sd(name, card)?;
            }
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
        if machine.is_cortex_m()
            && let Some(name) = &serial_analyzer
        {
            machine.bus.select_uart_input(name);
        }
        if let Some(name) = uart_device.as_deref() {
            if machine.is_cortex_m() {
                machine.bus.select_uart_input(name);
            } else {
                machine.bus.rv64_devices.select_uart(name)?;
            }
        }
        for &step in &reset_steps {
            machine.schedule_reset(step)?;
        }
        for (step, path) in &uart_events {
            let bytes = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
            machine.schedule_uart_input(*step, bytes)?;
        }
        if let Some(value) = vex_irq_mask {
            machine.primary.set_vex_irq_mask(value)?;
        }
        if let Some(value) = vex_supervisor_irq_mask {
            machine.primary.set_vex_supervisor_irq_mask(value)?;
        }
        if let Some(path) = path.as_deref() {
            let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
            let loaded = if hex {
                image::load_hex(&mut machine, file)
            } else {
                image::load(&mut machine, file, raw, script_elf_addresses)
            };
            loaded.map_err(|e| format!("{}: {e}", path.display()))?;
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
        for (cpu, address) in &vector_tables {
            machine.set_named_vector_table(cpu, *address)?;
        }
        for (name, action) in &buttons {
            machine.bus.set_litex_button(name, *action)?;
        }
        for (step, name, action) in &button_events {
            if machine.is_cortex_m() {
                let pressed = match action {
                    litex_panel::ButtonAction::Press => true,
                    litex_panel::ButtonAction::Release => false,
                    _ => return Err("STM32 button events support press and release".into()),
                };
                machine.schedule_stm32_button(*step, name, pressed)?;
            } else {
                machine.schedule_button(*step, name.clone(), *action)?;
            }
        }
        for (step, name, expected) in &led_assertions {
            machine.schedule_led_assertion(*step, name.clone(), *expected)?;
        }
        for (step, name, value) in &temperature_events {
            machine.schedule_sensor_temperature(*step, name.clone(), *value)?;
        }
        for (step, name, value) in &phy_events {
            machine.schedule_phy_vendor_specific1(*step, name.clone(), *value)?;
        }
        for (step, path) in &framebuffer_events {
            machine.schedule_framebuffer_capture(*step, path.clone())?;
        }
        for (step, path, region) in &frame_waits {
            let data = fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?;
            let reference = litex_video::ReferenceFrame::parse(data)
                .and_then(|frame| match region {
                    Some(region) => frame.with_region(*region),
                    None => Ok(frame),
                })
                .map_err(|error| format!("{}: {error}", path.display()))?;
            machine.schedule_frame_wait(*step, reference)?;
        }
        if let Some((address, act)) = tohost {
            if machine.is_cortex_m() {
                return Err("--tohost requires an RV32 machine".into());
            }
            machine.bus.watch_tohost(address, act)?;
        }
        for &(address, value) in &boot_writes {
            machine
                .bus
                .write(address, 4, value, &mut io::sink())
                .map_err(|error| error.to_string())?;
        }
        zephyr_boot::configure(&mut machine, &zephyr, path.as_deref())?;
        machine.validate_entry()?;
        if let Some((local, peer)) = ethernet_loopback {
            machine.bus.connect_rv64_ethernet(local, peer)?;
        }
        if uart_stdin && !machine.bus.has_uart_input() {
            return Err("--uart-stdin requires a platform UART with input support".into());
        }
        Ok(machine)
    };
    let mut machine = boot_machine()?;
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
    };
    let start = Instant::now();
    let limit = match (max_steps, run_steps) {
        (Some(cli), Some(script)) => cli.min(script),
        (cli, script) => cli.or(script).unwrap_or(10_000_000),
    };
    let result = if let Some(peer_path) = &ble_peer {
        let platform = platform_path
            .as_deref()
            .ok_or("--ble-peer requires a platform")?;
        let mut peer = load_platform(platform, platform_root.as_deref(), false)?;
        let image = File::open(peer_path).map_err(|e| format!("{}: {e}", peer_path.display()))?;
        image::load(&mut peer, image, false, None)
            .map_err(|e| format!("{}: {e}", peer_path.display()))?;
        peer.validate_entry()?;
        machine.run_radio_pair(&mut peer, limit, &mut output)
    } else if let Some(script) = script.as_deref().filter(|_| !runtime.is_empty()) {
        monitor::run(&mut machine, script, &runtime, limit, &mut output)
    } else {
        run_machine(
            &mut machine,
            limit,
            &mut output,
            &boot_machine,
            uart_stdin,
            uart_stdin_reset,
        )
    };
    let elapsed = start.elapsed();
    output.flush().map_err(|e| format!("UART: {e}"))?;
    let (stop, steps) = result?;
    if let Some(path) = i2s_output
        && let Some(pcm) = machine.bus.nrf_i2s_pcm_output()
    {
        fs::write(&path, pcm).map_err(|e| format!("{}: {e}", path.display()))?;
    }

    for step in machine.intermediate_frame_matches() {
        eprintln!("penv: framebuffer matched at step {step}");
    }
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
        if machine.bus.litex_video.is_some() {
            machine.bus.capture_litex_framebuffer(&path)?;
        } else {
            machine.bus.capture_ltdc(&path)?;
        }
    }
    if let Stop::FrameMatched(step) = stop {
        eprintln!("penv: framebuffer matched at step {step}");
    }
    if stats {
        eprintln!(
            "{stop:?}, {steps} instructions, {:.3} ms, pc={:#010x}",
            elapsed.as_secs_f64() * 1000.0,
            machine.primary.program_counter()
        );
        for (name, level) in machine.bus.gpio_pwm_led_levels() {
            eprintln!("{name}={:.2}%", level * 100.0);
        }
        for (name, on) in machine.bus.h7_gpio_led_states() {
            eprintln!("{name}={}", if on { "on" } else { "off" });
        }
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
            machine.primary.program_counter()
        );
        return Ok(ExitCode::from(2));
    }
    Ok(ExitCode::SUCCESS)
}

struct UartOutput<W: Write> {
    writer: LineWriter<W>,
    immediate: bool,
}

impl<W: Write> Write for UartOutput<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let written = self.writer.write(bytes)?;
        if self.immediate {
            self.writer.flush()?;
        }
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
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
                machine
                    .bus
                    .queue_uart_input(uart)
                    .map_err(|error| error.to_string())?;
            } else {
                machine
                    .bus
                    .queue_uart_input(bytes)
                    .map_err(|error| error.to_string())?;
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

fn load_platform(
    path: &std::path::Path,
    root: Option<&std::path::Path>,
    stm32f0_console: bool,
) -> Result<Machine, String> {
    platform::load(path, root, stm32f0_console).or_else(|original| {
        nrf_repl::load(path, root, stm32f0_console)
            .map_err(|error| format!("{original}; nRF REPL: {error}"))
    })
}
