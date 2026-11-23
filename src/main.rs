mod andes_csr;
mod bitmanip;
mod bus;
mod clint;
mod collapsed_profile;
mod compressed;
mod constant_mmio;
mod crc;
mod csr;
mod designware_usb;
mod dwt;
mod ethernet;
mod ethernet_switch;
mod event_trace;
mod float;
mod ft5336;
mod host_pty;
mod iis2mdc;
mod image;
mod image64;
mod ina228;
mod limit_timer;
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
mod lps25hb;
mod lsm6dso;
mod machine;
mod memory_setup;
mod mii_phy;
mod monitor;
mod murax;
mod network_monitor;
mod network_platform;
mod network_repl;
mod network_resc;
mod nrf52840;
mod nrf_monitor;
mod nrf_repl;
mod ns16550;
mod nvic;
mod platform;
mod plic;
mod pmp;
mod pydev;
mod repl_parser;
mod repl_syntax;
mod resd;
mod rtc_clock;
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
mod semihosting;
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
mod sync_action;
mod systick;
mod thumb;
mod thumb_llvm;
pub mod time_handle;
pub mod time_source;
mod tmp108;
mod uart;
mod uart_check;
mod uart_transcript;
mod uart_trigger;
mod vector_bootstrap;
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

use machine::{LiteXUartAction, Machine, Stop};
use uart_trigger::UartTrigger;

const HELP: &str = "Run a bare-metal firmware image.

Usage: penv run [OPTIONS] IMAGE
       penv run --script PATH [OPTIONS] [LOCAL_IMAGE]
       penv switch LISTEN_PORT GUEST_PORT GUEST_PORT [GUEST_PORT ...]
       penv peripherals export OUTPUT --platform REPL [--platform-root DIR]
       penv check-platform PATH [--platform-root PATH]
       penv check-platform --from-string REPL [--platform-root PATH]
       penv validate-platform REPL
       penv thumb-assemble SOURCE
       penv thumb-disassemble ADDRESS HEXBYTES
       penv thumb-disassemble-memory REPL ELF ADDRESS SIZE

Thumb assembly tools require llvm-mc on PATH. HEXBYTES are in memory order.

Options:
  --framebuffer-event STEP:PATH  Save a LiteX framebuffer as PPM at a step (repeatable)
  --wait-for-frame STEP:PATH  Stop when the LiteX framebuffer matches a PNG or P6 RGB PPM
  --wait-for-frame-roi STEP:X:Y:WIDTH:HEIGHT:PATH  Wait for identical RGB pixels in a region
  --frame-waits-after-uart STEPS:TEXT  Start queued waits on UART text and give each STEPS
  --framebuffer-out PATH  Write the final LiteX framebuffer as PPM
  --asset-dir DIR    Resolve RESC firmware URLs to local files by filename
  --script PATH      Read boot settings from a Renode RESC file
  --raw              Load a flat binary (default: ELF)
  --rv64             Run a raw image on an RV64IM CPU
  --hex              Load an Intel HEX image on a Cortex-M platform
  --ram-mib N        RAM size in MiB, 1–1024 (default: 16)
  --map-ram BASE:SIZE Add a RAM region in bytes (decimal or 0x-prefixed)
  --load-binary [CPU@]BASE:PATH  Load a flat companion image into RAM (repeatable)
  --move-ram CPU@OLD:NEW  Move an exclusive CPU RAM window before execution
  --set-mtvec CPU:VALUE  Initialize the named RV64 machine trap vector
  --translate-read CPU:ADDRESS  Translate an RV64 address for a data read
  --halt-cpu CPU       Start the named RV64 CPU halted
  --halt-cpu-at CPU:ADDRESS  Halt the named RV64 CPU before executing this address
  --stop-when-halted   Stop once every CPU is halted
  --set-mstatus CPU:VALUE  Initialize the named RV64 CPU status fields
  --write-pmp-register CPU@NAME:VALUE  Write a named external PMP register
  --set-gpr CPU@xN:VALUE  Initialize an RV64 integer register (x1 through x31)
  --set-freg CPU@fN:VALUE  Initialize raw RV64 floating-point register bits
  --read-register CPU@NAME  Report a supported RV64 register after execution
  --read-register-now CPU@NAME  Report an RV64 register in setup argument order
  --set-reset-vector CPU:ADDRESS  Change a named RV64 reset vector during setup
  --set-pc CPU:ADDRESS  Set the named RV64 PC during setup
  --step-cpu CPU     Execute one setup step on a named, single RV64 CPU
  --reset-cpu CPU    Restore a named RV64 CPU during setup, preserving the reset vector
  --write-memory [CPU@]BASE:WIDTH:VALUE  Write a little-endian RAM value during setup
  --assemble CPU@BASE:INSTRUCTION  Assemble one RV64 instruction into RAM with llvm-mc
  --disassemble CPU@BASE:WIDTH  Decode one RV64 instruction with llvm-mc (2 or 4 bytes)
  --read-memory-now [CPU@]BASE:WIDTH  Read RAM in setup argument order
  --read-memory [CPU@]BASE:WIDTH  Report a RAM value after execution (repeatable)
  --dtb BASE:PATH    Load an RV64 boot DTB and pass its address in a1
  --clint BASE       Map a CLINT for the configured harts (decimal or 0x-prefixed)
  --litex-timer-alias BASE  Map the existing LiteX timer at an additional address
  --platform PATH    Load RAM, CPU and CLINT from a Renode REPL file
  --button-event STEP:NAME:STATE  Schedule a button press or release (repeatable)
  --led-events       Print board LED transitions to stderr
  --uart-steps       Prefix UART lines with their first output step
  --adxl-event STEP:NAME:X:Y:Z  Set nRF52840 ADXL372 acceleration in g at a step
  --adc-input CHANNEL:VOLTS  Set the initial STM32 ADC channel voltage (repeatable)
  --adc-event STEP:CHANNEL:VOLTS  Change an STM32 ADC channel voltage at a step (repeatable)
  --stm32f0-console  Require an STM32F0 REPL (legacy console option)
  --platform-root DIR  Root for working-directory-relative REPL includes
  --litex-panel-after-uart N:NAME:ACTION:TEXT  Toggle a button or assert an LED on UART match N
  --litex-phy-after-uart NAME:VALUE:TEXT  Set PHY VendorSpecific1 after matching UART text
  --litex-phy-event STEP:NAME:VALUE  Set PHY VendorSpecific1 at a step (repeatable)
  --litex-panel-after-uart N:NAME:ACTION:TEXT  Apply panel action on UART match N
  --litex-phy-after-uart NAME:VALUE:TEXT  Set PHY VendorSpecific1 after matching text
  --litex-temperature-after-uart NAME:CELSIUS:TEXT  Set SI70xx temperature after matching text
  --litex-temperature-event STEP:NAME:CELSIUS  Set an SI70xx temperature (repeatable)
  --litex-led-assert STEP:NAME:STATE  Assert a wired LiteX LED is on or off (repeatable)
  --litex-button-event STEP:NAME:ACTION  Press, Release, or Toggle at a step
  --uart-event STEP:PATH  Queue file bytes at a step (repeatable)
  --uart-input PATH  Queue bytes from a file for the platform UART
  --uart-check PATH  Check UART lines against a virtual-time plan
  --uart-transcript PATH  Record UART lines with instruction steps
  --uart-reply LINE FILE  Queue bytes once the UART emits the given line
  --f4-repl-timing   Preserve STM32F4 REPL timer and SysTick clocks
  --uart-stdin       Forward stdin to the platform UART while running
  --uart-stdin-reset  Enable Ctrl-] r to reload the machine on the UART session
  --uart-device NAME  Select the RV64 platform UART that receives input
  --ble-hci-controller PATH  Use two external HCI controllers with --ble-peer
  --ble-peer PATH    Run a second nRF52840 ELF on an in-process RADIO medium
  --i2s-input PATH   Feed PCM bytes to nRF52840 I2S
  --i2s-output PATH  Save PCM bytes transmitted by nRF52840 I2S
  --nrf-camera-image PATH  Preload a JPEG into nRF52840 ArduCAM
  --lsm9ds1-samples PATH  Load nRF52840 accelerometer samples in g
  --pdm-input PATH   Feed signed 16-bit little-endian PCM to nRF52840 PDM
  --uart-input-at N Delay UART input until execution step N
  --ethernet-loopback LOCAL:PEER  Link BeagleV MACs over localhost UDP ports
  --litex-uart-stdio  Bridge stdin to LiteX UART at 115200 baud and flush UART output
  --litex-uart-legacy-console  Use legacy LiteX UART console event handling
  --mtvec ADDRESS    Set the initial machine trap vector
  --vex-irq-mask N   Set the initial VexRiscv external interrupt mask
  --harts N          Start N RISC-V harts at the image entry (1–8)
  --tohost ADDRESS  Stop on an odd write to the RV32 test completion word
  --tohost-act ADDRESS  Decode RV32 Sail ACT console and completion writes
  --write-memory ADDRESS:WIDTH:VALUE  Write a little-endian RAM value before execution
  --read-memory-now ADDRESS:WIDTH    Read RAM in setup argument order
  --max-steps N      Stop after N steps (0 with memory setup; default: 10000000)
  --sd-image PATH    Attach a read-only image to the STM32H7 SDMMC slot
  --uart-log-dir DIR Save each simulation node UART to a numbered file
  --reset-at STEP    Reset the Cortex-M machine before this step (repeatable)
  --i2s-input PATH   Feed PCM bytes to nRF52840 I2S
  --i2s-output PATH  Save transmitted nRF52840 I2S PCM
  --pdm-input PATH   Feed signed little-endian PCM to nRF52840 PDM
  --ble-peer PATH    Run a second nRF52840 image on a shared radio medium
  --virtual-mips N   Set Cortex-M steps per virtual microsecond
  --renode-lrsc  Use Renode single-hart RV64 LR/SC reservations
  --renode-unmapped-data  Warn and return zero/drop for unmapped RV64 data accesses
  --renode-elf-load  Load the mapped RAM portions of RV64 ELF segments
  --robot-tohost ADDRESS   Install a 4 KiB single-hart Renode-style watcher
  --trace-amo CPU:FORMAT:PATH  Record aligned RV64 RAM AMO operands in text or binary
  --trace-pc CPU:FORMAT:PATH  Record RV64 PCs in text or binary ReTrace format
  --stats            Print execution statistics to stderr
  --                 Treat remaining arguments as positional
  -h, --help         Show help
  -V, --version      Show version";

type RamRead = (Option<String>, u64, usize);

enum SetupAction {
    Load(PathBuf, u64, Option<String>),
    Read(RamRead),
    Write(RamRead, u64),
    Move(String, u64, u64),
    ReadRegister(String, String),
    Disassemble(String, u64, usize),
    Assemble(String, u64, String),
    SetPc(String, u64),
    SetResetVector(String, u64),
    ResetCpu(String),
    StepCpu(String),
    HaltCpu(String),
    HaltCpuAt(String, u64),
    SetMstatus(String, u64),
    SetMtvec(String, u64),
    TranslateRead(String, u64),
    SetFreg(String, usize, u64),
    SetGpr(String, usize, u64),
    WritePmpRegister(String, String, u64),
}

fn ram_width(value: &str) -> Result<usize, String> {
    value
        .parse::<usize>()
        .ok()
        .filter(|width| matches!(width, 1 | 2 | 4 | 8))
        .ok_or_else(|| "memory access width must be 1, 2, 4 or 8 bytes".into())
}

fn write_ram(
    machine: &mut Machine,
    (cpu, address, width): &RamRead,
    value: u64,
) -> Result<(), String> {
    let hart = cpu
        .as_deref()
        .map(|cpu| machine.rv64_hart_by_name(cpu))
        .transpose()?;
    let data = match hart {
        Some(hart) => machine.bus.cpu_memory64(hart, *address, *width),
        None => machine.bus.memory64(*address, *width),
    }
    .ok_or_else(|| format!("unmapped RAM write at {address:#018x} ({width} bytes)"))?;
    data.copy_from_slice(&value.to_le_bytes()[..*width]);
    Ok(())
}

fn report_ram(machine: &Machine, (cpu, address, width): &RamRead) -> Result<(), String> {
    let hart = cpu
        .as_deref()
        .map(|cpu| machine.rv64_hart_by_name(cpu))
        .transpose()?;
    let value = rv64::ram_read(&machine.bus, hart, *address, *width)
        .or_else(|| machine.bus.renode_unmapped_data.then_some(0))
        .ok_or_else(|| format!("unmapped RAM read at {address:#018x} ({width} bytes)"))?;
    let context = cpu.as_deref().unwrap_or("sysbus");
    eprintln!("penv: memory {context}@{address:#018x}:{width} = {value:#018x}");
    Ok(())
}

fn llvm_mc(features: &str, mode: &str, input: &str) -> Result<std::process::Output, String> {
    use std::process::{Command, Stdio};

    let mut child = Command::new("llvm-mc")
        .args([mode, "--triple=riscv64", &format!("--mattr={features}")])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("could not start llvm-mc for RV64: {error}"))?;
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(input.as_bytes())
        .map_err(|error| format!("llvm-mc input: {error}"))?;
    child
        .wait_with_output()
        .map_err(|error| format!("llvm-mc: {error}"))
}

fn assemble_instruction(
    machine: &mut Machine,
    cpu: &str,
    address: u64,
    instruction: &str,
) -> Result<(), String> {
    let hart = machine.rv64_hart_by_name(cpu)?;
    let features = machine
        .hart_mut(hart)
        .rv64
        .as_ref()
        .expect("validated RV64 CPU")
        .isa()
        .llvm_features();
    if instruction.is_empty()
        || instruction.trim_start().starts_with('.')
        || instruction.contains(['\n', '\r', ';', ':'])
    {
        return Err("--assemble requires one instruction without directives or labels".into());
    }
    let output = llvm_mc(&features, "--show-encoding", &format!("{instruction}\n"))?;
    if !output.status.success() || !output.stderr.is_empty() {
        return Err(format!(
            "RV64 assembly at {cpu}@{address:#x}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut encodings = text
        .lines()
        .filter_map(|line| line.split_once("# encoding: [").map(|(_, bytes)| bytes));
    let bytes = encodings
        .next()
        .filter(|_| encodings.next().is_none())
        .and_then(|bytes| bytes.strip_suffix(']'))
        .ok_or("llvm-mc did not emit exactly one instruction encoding")?;
    let bytes = bytes
        .split(',')
        .map(|byte| {
            byte.trim()
                .strip_prefix("0x")
                .and_then(|byte| u8::from_str_radix(byte, 16).ok())
        })
        .collect::<Option<Vec<_>>>()
        .ok_or("llvm-mc emitted an unresolved instruction relocation")?;
    if !matches!(bytes.len(), 2 | 4) {
        return Err("RV64 assembly must emit a 2- or 4-byte instruction".into());
    }
    machine
        .bus
        .cpu_memory64(hart, address, bytes.len())
        .ok_or_else(|| format!("unmapped RAM write at {cpu}@{address:#x}"))?
        .copy_from_slice(&bytes);
    Ok(())
}

fn report_disassembly(
    machine: &mut Machine,
    cpu: &str,
    address: u64,
    width: usize,
) -> Result<(), String> {
    let hart = machine.rv64_hart_by_name(cpu)?;
    let features = machine
        .hart_mut(hart)
        .rv64
        .as_ref()
        .expect("validated RV64 CPU")
        .isa()
        .llvm_features();
    let Some(word) = rv64::ram_read(&machine.bus, Some(hart), address, width) else {
        eprintln!("penv: Disassembly error detected at {cpu}@{address:#x}: unmapped RAM");
        return Ok(());
    };
    let input = word.to_le_bytes()[..width]
        .iter()
        .map(|byte| format!("{byte:#04x}"))
        .collect::<Vec<_>>()
        .join(" ");
    let output = llvm_mc(&features, "--disassemble", &input)?;
    let decoded = String::from_utf8_lossy(&output.stdout);
    let mut lines = decoded
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && *line != ".text");
    if output.status.success()
        && output.stderr.is_empty()
        && let Some(line) = lines.next()
        && lines.next().is_none()
    {
        let assembly = line.split_whitespace().collect::<Vec<_>>().join(" ");
        let digits = width * 2;
        eprintln!("penv: disassembly {cpu}@{address:#x}: {word:0digits$x} {assembly}");
    } else {
        eprintln!(
            "penv: Disassembly error detected at {cpu}@{address:#x}: invalid instruction encoding"
        );
    }
    Ok(())
}

fn report_registers(machine: &mut Machine, reads: &[(String, String)]) -> Result<(), String> {
    for (cpu, name) in reads {
        let hart = machine.rv64_hart_by_name(cpu)?;
        let value = machine
            .hart_mut(hart)
            .rv64
            .as_ref()
            .expect("validated RV64 CPU")
            .monitor_register(name)
            .ok_or_else(|| format!("unsupported RV64 monitor register {name:?}"))?;
        eprintln!("penv: register {cpu}@{name} = {value:#018x}");
    }
    Ok(())
}

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
        Some(Some("check-platform")) => {
            let input = args
                .next()
                .ok_or("expected a platform path or --from-string REPL")?;
            let from_string = input == "--from-string";
            let input = if from_string {
                args.next()
                    .ok_or("expected REPL text after --from-string")?
            } else {
                input
            };
            let root = match args.next() {
                None => None,
                Some(flag) if flag == "--platform-root" => Some(PathBuf::from(
                    args.next().ok_or("expected a platform root")?,
                )),
                _ => return Err("expected --platform-root PATH".into()),
            };
            if args.next().is_some() {
                return Err("unexpected check-platform argument".into());
            }
            if from_string {
                network_repl::check_text(
                    input.to_str().ok_or("REPL text must be UTF-8")?,
                    root.as_deref(),
                )?;
            } else {
                let path = PathBuf::from(input);
                if root.is_none() && platform::check(&path).is_ok() {
                    return Ok(ExitCode::SUCCESS);
                }
                load_platform(&path, root.as_deref(), false)
                    .or_else(|_| network_repl::load(&path, root.as_deref()))?;
            }
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
        Some(Some("peripherals")) => return export_peripherals(args),
        _ => return Err("expected 'run', 'check-platform' or 'peripherals'; see --help".into()),
    }

    let mut raw = false;
    let mut hex = false;
    let mut litex_uart_stdio = false;
    let mut enable_litex_uart_legacy_console = false;
    let mut rv64 = false;
    let mut stats = false;
    let mut pc_trace = None;
    let mut stop_when_halted = false;
    let mut renode_elf_load = false;
    let mut renode_unmapped_data = false;
    let mut renode_lrsc = false;
    let mut positional = false;
    let mut ram_mib = None;
    let mut harts = None;
    let mut tohost = None;
    let mut max_steps = None;
    let mut extra_ram = Vec::new();
    let mut setup_actions = Vec::new();
    let mut memory_reads = Vec::new();
    let mut register_reads = Vec::new();
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
    let mut adxl_events = Vec::new();
    let mut led_events = false;
    let mut uart_input = None;
    let mut uart_transcript = None;
    let mut uart_reply = None;
    let mut uart_check = None;
    let mut f4_repl_timing = false;
    let mut uart_stdin = false;
    let mut uart_stdin_reset = false;
    let mut ethernet_loopback = None;
    let mut uart_device = None;
    let mut temperature_events = Vec::new();
    let mut uart_actions = Vec::new();
    let mut phy_events = Vec::new();
    let mut led_assertions = Vec::new();
    let mut button_events = Vec::new();
    let mut frame_waits = Vec::new();
    let mut frame_wait_trigger = None;
    let mut framebuffer_events = Vec::new();
    let mut framebuffer_out = None;
    let mut cli_mips = None;
    let mut camera_image = None;
    let mut lsm9ds1_samples = None;
    let mut i2s_input = None;
    let mut i2s_output = None;
    let mut pdm_input = None;
    let mut ble_peer = None;
    let mut sd_image = None;
    let mut uart_log_dir = None;
    let mut memory_setup = Vec::new();
    let mut ble_hci_controller = None;
    let mut uart_input_at = None;
    let mut uart_events = Vec::new();
    let mut reset_steps = Vec::new();
    let mut path = None;
    let mut uart_steps = false;
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
            Some(option @ ("--trace-pc" | "--trace-amo")) => {
                let error = format!("{option} requires CPU:text:PATH or CPU:binary:PATH");
                let specification = args
                    .next()
                    .and_then(|arg| arg.into_string().ok())
                    .ok_or_else(|| error.clone())?;
                let (cpu, rest) = specification
                    .split_once(':')
                    .filter(|(cpu, _)| !cpu.is_empty())
                    .ok_or_else(|| error.clone())?;
                let (format, path) = rest
                    .split_once(':')
                    .filter(|(_, path)| !path.is_empty())
                    .ok_or_else(|| error.clone())?;
                let binary = match format {
                    "text" => false,
                    "binary" => true,
                    _ => return Err(error.into()),
                };
                if pc_trace
                    .replace((
                        cpu.to_owned(),
                        binary,
                        option == "--trace-amo",
                        PathBuf::from(path),
                    ))
                    .is_some()
                {
                    return Err("specify only one of --trace-pc and --trace-amo, once".into());
                }
            }
            Some("--stats") => stats = true,
            Some("--stop-when-halted") => stop_when_halted = true,
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
                option @ ("--platform"
                | "--platform-root"
                | "--script"
                | "--uart-input"
                | "--uart-transcript"
                | "--uart-check"
                | "--asset-dir"
                | "--framebuffer-out"
                | "--sd-image"
                | "--uart-log-dir"
                | "--i2s-input"
                | "--i2s-output"
                | "--pdm-input"
                | "--nrf-camera-image"
                | "--lsm9ds1-samples"
                | "--ble-hci-controller"
                | "--ble-peer"),
            ) => {
                let path = args
                    .next()
                    .map(PathBuf::from)
                    .ok_or_else(|| format!("{option} requires a path"))?;
                let target = match option {
                    "--platform" => &mut platform_path,
                    "--script" => &mut script,
                    "--uart-input" => &mut uart_input,
                    "--uart-transcript" => &mut uart_transcript,
                    "--uart-check" => &mut uart_check,
                    "--asset-dir" => &mut asset_dir,
                    "--platform-root" => &mut platform_root,
                    "--pdm-input" => &mut pdm_input,
                    "--i2s-input" => &mut i2s_input,
                    "--i2s-output" => &mut i2s_output,
                    "--ble-peer" => &mut ble_peer,
                    "--sd-image" => &mut sd_image,
                    "--uart-log-dir" => &mut uart_log_dir,
                    "--framebuffer-out" => &mut framebuffer_out,
                    "--ble-hci-controller" => &mut ble_hci_controller,
                    "--nrf-camera-image" => &mut camera_image,
                    "--lsm9ds1-samples" => &mut lsm9ds1_samples,
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
            Some(
                option @ ("--litex-panel-after-uart"
                | "--litex-phy-after-uart"
                | "--litex-temperature-after-uart"),
            ) => {
                let argument = args
                    .next()
                    .and_then(|value| value.into_string().ok())
                    .ok_or_else(|| format!("{option} requires a UART action"))?;
                uart_actions.push(parse_litex_uart_action(option, &argument)?);
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
                let value = parse_litex_temperature(value)?;
                temperature_events.push((step, name.to_string(), value));
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
            Some("--f4-repl-timing") => f4_repl_timing = true,
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
            Some("--clint") => {
                let value = args
                    .next()
                    .and_then(|value| value.into_string().ok())
                    .ok_or("--clint requires BASE")?;
                if clint.replace(parse_address(&value)?).is_some() {
                    return Err("--clint may only be specified once".into());
                }
            }
            Some(option @ ("--reset-cpu" | "--step-cpu" | "--halt-cpu")) => {
                let cpu = args
                    .next()
                    .and_then(|arg| arg.into_string().ok())
                    .filter(|cpu| !cpu.is_empty())
                    .ok_or_else(|| format!("{option} requires CPU"))?;
                setup_actions.push(match option {
                    "--step-cpu" => SetupAction::StepCpu(cpu),
                    "--halt-cpu" => SetupAction::HaltCpu(cpu),
                    _ => SetupAction::ResetCpu(cpu),
                });
            }
            Some(option @ ("--set-pc" | "--set-reset-vector" | "--halt-cpu-at")) => {
                let error = || format!("{option} requires CPU:ADDRESS");
                let argument = args
                    .next()
                    .and_then(|arg| arg.into_string().ok())
                    .ok_or_else(error)?;
                let (cpu, address) = argument
                    .split_once(':')
                    .filter(|(cpu, _)| !cpu.is_empty())
                    .ok_or_else(error)?;
                let address = parse_address64(address)?;
                setup_actions.push(match option {
                    "--set-pc" => SetupAction::SetPc(cpu.to_owned(), address),
                    "--halt-cpu-at" => SetupAction::HaltCpuAt(cpu.to_owned(), address),
                    _ => SetupAction::SetResetVector(cpu.to_owned(), address),
                });
            }
            Some(option @ ("--set-freg" | "--set-gpr")) => {
                let floating = option == "--set-freg";
                let error = if floating {
                    "--set-freg requires CPU@fN:VALUE (f0 through f31)"
                } else {
                    "--set-gpr requires CPU@xN:VALUE (x1 through x31)"
                };
                let argument = args
                    .next()
                    .and_then(|arg| arg.into_string().ok())
                    .ok_or(error)?;
                let (target, value) = argument.split_once(':').ok_or(error)?;
                let (cpu, register) = target
                    .split_once('@')
                    .filter(|(cpu, _)| !cpu.is_empty())
                    .ok_or(error)?;
                let index = register
                    .strip_prefix(if floating { 'f' } else { 'x' })
                    .and_then(|index| index.parse::<usize>().ok())
                    .filter(|index| *index < 32 && (floating || *index != 0))
                    .ok_or(error)?;
                let value = parse_address64(value)?;
                setup_actions.push(if floating {
                    SetupAction::SetFreg(cpu.to_owned(), index, value)
                } else {
                    SetupAction::SetGpr(cpu.to_owned(), index, value)
                });
            }
            Some("--write-pmp-register") => {
                let error = "--write-pmp-register requires CPU@NAME:VALUE";
                let argument = args
                    .next()
                    .and_then(|arg| arg.into_string().ok())
                    .ok_or(error)?;
                let (target, value) = argument.split_once(':').ok_or(error)?;
                let (cpu, name) = target
                    .split_once('@')
                    .filter(|(cpu, name)| !cpu.is_empty() && !name.is_empty())
                    .ok_or(error)?;
                setup_actions.push(SetupAction::WritePmpRegister(
                    cpu.to_owned(),
                    name.to_ascii_lowercase(),
                    parse_address64(value)?,
                ));
            }
            Some(option @ ("--set-mtvec" | "--translate-read")) => {
                let error = || format!("{option} requires CPU:VALUE");
                let argument = args
                    .next()
                    .and_then(|arg| arg.into_string().ok())
                    .ok_or_else(error)?;
                let (cpu, value) = argument
                    .split_once(':')
                    .filter(|(cpu, _)| !cpu.is_empty())
                    .ok_or_else(error)?;
                let value = parse_address64(value)?;
                setup_actions.push(if option == "--set-mtvec" {
                    SetupAction::SetMtvec(cpu.to_owned(), value)
                } else {
                    SetupAction::TranslateRead(cpu.to_owned(), value)
                });
            }
            Some(option @ ("--set-mstatus" | "--read-register" | "--read-register-now")) => {
                let delimiter = if option == "--set-mstatus" { ':' } else { '@' };
                let error = || {
                    if option == "--set-mstatus" {
                        "--set-mstatus requires CPU:VALUE"
                    } else {
                        "--read-register requires CPU@NAME"
                    }
                };
                let value = args
                    .next()
                    .and_then(|value| value.into_string().ok())
                    .ok_or_else(error)?;
                let (cpu, value) = value
                    .split_once(delimiter)
                    .filter(|(cpu, value)| !cpu.is_empty() && !value.is_empty())
                    .ok_or_else(error)?;
                if option == "--set-mstatus" {
                    setup_actions.push(SetupAction::SetMstatus(
                        cpu.to_owned(),
                        parse_address64(value)?,
                    ));
                } else if option == "--read-register-now" {
                    setup_actions.push(SetupAction::ReadRegister(
                        cpu.to_owned(),
                        value.to_ascii_lowercase(),
                    ));
                } else {
                    register_reads.push((cpu.to_owned(), value.to_ascii_lowercase()));
                }
            }
            Some("--assemble") => {
                let error = "--assemble requires CPU@BASE:INSTRUCTION";
                let operand = args
                    .next()
                    .and_then(|arg| arg.into_string().ok())
                    .ok_or(error)?;
                let (cpu, operand) = operand
                    .split_once('@')
                    .filter(|(name, _)| !name.is_empty())
                    .ok_or(error)?;
                let (address, instruction) = operand.split_once(':').ok_or(error)?;
                setup_actions.push(SetupAction::Assemble(
                    cpu.to_owned(),
                    parse_address64(address)?,
                    instruction.to_owned(),
                ));
            }
            Some("--disassemble") => {
                let error = "--disassemble requires CPU@BASE:WIDTH (2 or 4 bytes)";
                let operand = args
                    .next()
                    .and_then(|arg| arg.into_string().ok())
                    .ok_or(error)?;
                let (cpu, operand) = operand
                    .split_once('@')
                    .filter(|(name, _)| !name.is_empty())
                    .ok_or(error)?;
                let (address, width) = operand.split_once(':').ok_or(error)?;
                let width = ram_width(width)?;
                if !matches!(width, 2 | 4) {
                    return Err(error.into());
                }
                setup_actions.push(SetupAction::Disassemble(
                    cpu.to_owned(),
                    parse_address64(address)?,
                    width,
                ));
            }
            Some(
                option @ ("--load-binary" | "--read-memory" | "--read-memory-now" | "--move-ram"
                | "--write-memory" | "--dtb"),
            ) => {
                let error = || match option {
                    "--load-binary" => "--load-binary requires [CPU@]BASE:PATH",
                    "--read-memory" | "--read-memory-now" => {
                        "memory read requires [CPU@]BASE:WIDTH"
                    }
                    "--move-ram" => "--move-ram requires CPU@OLD:NEW",
                    "--write-memory" => "--write-memory requires [CPU@]BASE:WIDTH:VALUE",
                    _ => "--dtb requires BASE:PATH",
                };
                let value = args
                    .next()
                    .and_then(|value| value.into_string().ok())
                    .ok_or_else(error)?;
                if matches!(option, "--write-memory" | "--read-memory-now") && !value.contains('@')
                {
                    memory_setup.push(memory_setup::MemorySetup::parse(option, &value)?);
                    continue;
                }
                let (base, path) = value
                    .split_once(':')
                    .filter(|(_, path)| !path.is_empty())
                    .ok_or_else(error)?;
                if option == "--dtb" {
                    if dtb
                        .replace((PathBuf::from(path), parse_address64(base)?))
                        .is_some()
                    {
                        return Err("--dtb may only be specified once".into());
                    }
                    continue;
                }
                let (cpu, base) = match base.split_once('@') {
                    Some((cpu, base)) if !cpu.is_empty() => (Some(cpu.to_owned()), base),
                    Some(_) => return Err(error().into()),
                    None => (None, base),
                };
                let address = parse_address64(base)?;
                if option == "--load-binary" {
                    setup_actions.push(SetupAction::Load(PathBuf::from(path), address, cpu));
                } else if option == "--move-ram" {
                    setup_actions.push(SetupAction::Move(
                        cpu.ok_or_else(error)?,
                        address,
                        parse_address64(path)?,
                    ));
                } else if option == "--write-memory" {
                    let (width, value) = path.split_once(':').ok_or_else(error)?;
                    let width = ram_width(width)?;
                    let value = parse_address64(value)?;
                    if value > u64::MAX >> (64 - width * 8) {
                        return Err("--write-memory value exceeds the requested width".into());
                    }
                    setup_actions.push(SetupAction::Write((cpu, address, width), value));
                } else {
                    let width = ram_width(path)?;
                    if option == "--read-memory-now" {
                        setup_actions.push(SetupAction::Read((cpu, address, width)));
                    } else {
                        memory_reads.push((cpu, address, width));
                    }
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
            Some("--renode-lrsc") => renode_lrsc = true,
            Some("--renode-unmapped-data") => renode_unmapped_data = true,
            Some("--renode-elf-load") => renode_elf_load = true,
            Some(option @ ("--tohost" | "--tohost-act" | "--robot-tohost")) => {
                let value = args.next().ok_or("--tohost requires an address")?;
                let value = value.to_str().ok_or("invalid tohost address")?;
                if tohost
                    .replace((parse_address64(value)?, option.to_owned()))
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
                    .filter(|&v| v > 0 || option == "--max-steps")
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
    let mut cpu_control = Vec::new();
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
    let mut sd_setup_only = false;
    let mut mpfs_buttons = Vec::new();
    let mut halt_hooks = Vec::new();
    let mut virtual_mips = None;
    let mut boot_writes = Vec::new();
    let mut zephyr = zephyr_boot::BootOptions::default();
    let mut cpu_registers = Vec::new();
    let mut log_function_names = false;
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
        if let Ok(content) = fs::read_to_string(script)
            && content
                .lines()
                .any(|line| line.trim_start().starts_with("machine PyDevFromString"))
        {
            if path.is_some() {
                return Err("CPU-less PyDev scripts do not accept a firmware override".into());
            }
            if let Some(pydev) = nrf_monitor::load(script, root)?.pydev {
                pydev.run(&mut io::stdout().lock())?;
                return Ok(ExitCode::SUCCESS);
            }
        }
        let boot = match monitor::load(script, root) {
            Ok(boot) if uart_log_dir.is_none() => boot,
            result => {
                let plan =
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
                        || !memory_setup.is_empty()
                        || !uart_actions.is_empty()
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
                let boot = plan.into_single_boot();
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
        sd_setup_only = boot.is_sd_setup_only() && boot.image.is_none() && boot.binaries.is_empty();
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
        cpu_registers = boot.cpu_registers;
        log_function_names = boot.log_function_names;
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
        sd_cards = boot.sd_cards;
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
        cpu_control = boot.cpu_control;
        silence_ranges = boot.silence_ranges;
        buttons = boot.buttons;
        video_analyzers = boot.video_analyzers;
        serial_analyzer = boot.serial_analyzer;
    }
    let has_firmware = path.is_some() || !binaries.is_empty();
    let setup_only = max_steps == Some(0) && !memory_setup.is_empty();
    let static_memory_script = monitor::is_static_memory_script(&runtime);
    if max_steps == Some(0)
        && !setup_only
        && !static_memory_script
        && !sd_setup_only
        && platform_path.is_none()
    {
        return Err("--max-steps requires a positive integer outside memory setup".into());
    }
    let memory_only = !has_firmware
        && platform_path.is_some()
        && (max_steps == Some(0) || static_memory_script || sd_setup_only);
    let inspect_only = path.is_none()
        && binaries.is_empty()
        && (!cpu_control.is_empty() || !cpu_registers.is_empty());
    let configuration_only = !has_firmware && monitor::is_configuration_only(&runtime);
    if !has_firmware && !memory_only && !inspect_only && !configuration_only {
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
    let pc_trace = pc_trace
        .map(|(cpu, binary, amo, path)| {
            File::options()
                .write(true)
                .create_new(true)
                .open(&path)
                .and_then(|file| machine::PcTrace::new(file, binary, amo))
                .map(|trace| (cpu, trace))
                .map_err(|error| format!("{}: {error}", path.display()))
        })
        .transpose()?;
    let boot_machine = || -> Result<Machine, String> {
        let mut machine = match platform_path.as_deref() {
            Some(platform_path) => {
                if ram_mib.is_some() || !extra_ram.is_empty() || clint.is_some() {
                    return Err(
                    "--platform defines RAM and CLINT and cannot be combined with manual mappings"
                        .into(),
                );
                }
                if f4_repl_timing {
                    network_repl::load_with_timing(platform_path, platform_root.as_deref(), true)?
                } else {
                    load_platform(platform_path, platform_root.as_deref(), stm32f0_console)
                        .or_else(|original| {
                            if stm32f0_console {
                                return Err(original);
                            }
                            network_repl::load(platform_path, platform_root.as_deref())
                                .map_err(|error| format!("{original}; network REPL: {error}"))
                        })?
                }
            }
            None => {
                if platform_root.is_some() {
                    return Err("--platform-root requires --platform".into());
                }
                Machine::new(ram_mib.unwrap_or(16) * 1024 * 1024)?
            }
        };
        machine.report_uart_steps(uart_steps);
        if let Some(path) = &camera_image {
            machine.bus.set_nrf_camera_image(
                fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?,
            )?;
        }
        if let Some(path) = &lsm9ds1_samples {
            machine.bus.set_nrf_lsm9ds1_samples(
                &fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?,
            )?;
        }
        for (step, name, acceleration) in &adxl_events {
            machine.schedule_adxl_input(*step, name.clone(), *acceleration)?;
        }
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
        machine.stop_when_halted = stop_when_halted;
        for (source, pc, target) in &halt_hooks {
            machine.halt_on_pc(source, *pc, target)?;
        }
        for attachment in &sd_cards {
            attachment.apply(
                &mut machine.bus,
                platform_root
                    .as_deref()
                    .unwrap_or(std::path::Path::new(".")),
                asset_dir.as_deref(),
            )?;
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
        if let Some((cpu, trace)) = &pc_trace {
            machine.trace_pc(cpu, trace.clone())?;
        }
        let mut reset_templates = Vec::new();
        for action in &setup_actions {
            if let SetupAction::ResetCpu(cpu) = action {
                let hart = machine.rv64_hart_by_name(cpu)?;
                if !reset_templates.iter().any(|(index, _)| *index == hart) {
                    let initial = machine
                        .hart_mut(hart)
                        .rv64
                        .clone()
                        .expect("validated RV64 CPU");
                    reset_templates.push((hart, initial));
                }
            }
        }
        let reset_state = cpu_control
            .iter()
            .any(|command| matches!(command, monitor::CpuControl::Reset))
            .then(|| machine.primary.clone());
        for command in cpu_registers.iter().cloned() {
            match command {
                monitor::CpuRegisterCommand::TrapVector(register, value) => {
                    if let Some(readback) = machine.primary.monitor_trap_vector(register, value)? {
                        eprintln!("{} {readback:#x}", register.name());
                    }
                }
                monitor::CpuRegisterCommand::ExternalPmp(path, config, index, value) => {
                    if let Some(readback) =
                        machine.monitor_external_pmp(&path, config, index, value)?
                    {
                        eprintln!("{readback:#x}");
                    }
                }
                monitor::CpuRegisterCommand::Integer(index, value) => {
                    if let Some(readback) =
                        machine.primary.monitor_integer_register(index, value)?
                    {
                        eprintln!("{readback:#x}");
                    }
                }
            }
        }
        if let Some(value) = vex_supervisor_irq_mask {
            machine.primary.set_vex_supervisor_irq_mask(value)?;
        }
        if log_function_names {
            machine.enable_function_logging()?;
        }
        if let Some(path) = path.as_deref() {
            let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
            let loaded = if hex {
                image::load_hex(&mut machine, file)
            } else {
                image::load(
                    &mut machine,
                    file,
                    raw,
                    script_elf_addresses,
                    renode_elf_load,
                )
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
        for (path, address) in &binaries {
            let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
            image::load_binary(&mut machine, file, *address)
                .map_err(|e| format!("{}: {e}", path.display()))?;
        }
        machine.bus.renode_unmapped_data = renode_unmapped_data;
        for action in &setup_actions {
            match action {
                SetupAction::Assemble(cpu, address, instruction) => {
                    assemble_instruction(&mut machine, cpu, *address, instruction)?;
                }
                SetupAction::Disassemble(cpu, address, width) => {
                    report_disassembly(&mut machine, cpu, *address, *width)?;
                }
                SetupAction::ReadRegister(cpu, name) => {
                    report_registers(&mut machine, &[(cpu.clone(), name.clone())])?;
                }
                SetupAction::SetPc(cpu, address) | SetupAction::SetResetVector(cpu, address) => {
                    let hart = machine.rv64_hart_by_name(cpu)?;
                    let cpu = machine
                        .hart_mut(hart)
                        .rv64
                        .as_mut()
                        .expect("validated RV64 CPU");
                    if matches!(action, SetupAction::SetPc(..)) {
                        cpu.set_program_counter(*address);
                    } else {
                        cpu.set_reset_vector(*address);
                    }
                }
                SetupAction::HaltCpu(cpu) => machine.halt_rv64_cpu(cpu)?,
                SetupAction::HaltCpuAt(cpu, address) => machine.halt_on_pc(cpu, *address, cpu)?,
                SetupAction::StepCpu(cpu) => {
                    let stop = machine.step_rv64_cpu(cpu, &mut io::stdout().lock())?;
                    if stop != Stop::Limit {
                        return Err(format!("CPU setup step stopped: {stop:?}"));
                    }
                }
                SetupAction::ResetCpu(cpu) => {
                    let hart = machine.rv64_hart_by_name(cpu)?;
                    let (_, initial) = reset_templates
                        .iter()
                        .find(|(index, _)| *index == hart)
                        .expect("captured CPU reset state");
                    machine
                        .hart_mut(hart)
                        .rv64
                        .as_mut()
                        .expect("validated RV64 CPU")
                        .reset_from(initial);
                    machine.bus.take_reservation64(hart);
                }
                SetupAction::SetMtvec(cpu, value) => {
                    let hart = machine.rv64_hart_by_name(cpu)?;
                    machine
                        .hart_mut(hart)
                        .rv64
                        .as_mut()
                        .expect("validated RV64 CPU")
                        .privilege
                        .set_vector(*value);
                }
                SetupAction::TranslateRead(cpu, address) => {
                    let physical = machine.rv64_translate_read(cpu, *address)?;
                    eprintln!("penv: translation {cpu}@{address:#018x} = {physical:#018x}");
                }
                SetupAction::SetMstatus(cpu, value) => {
                    let hart = machine.rv64_hart_by_name(cpu)?;
                    machine
                        .hart_mut(hart)
                        .rv64
                        .as_mut()
                        .expect("validated RV64 CPU")
                        .initialize_mstatus(*value);
                }
                SetupAction::WritePmpRegister(cpu, name, value) => {
                    let hart = machine.rv64_hart_by_name(cpu)?;
                    machine
                        .hart_mut(hart)
                        .rv64
                        .as_mut()
                        .expect("validated RV64 CPU")
                        .privilege
                        .write_external_pmp(name, *value)
                        .ok_or_else(|| format!("{cpu}: invalid external PMP register {name}"))?;
                }
                SetupAction::SetGpr(cpu, index, value) => {
                    let hart = machine.rv64_hart_by_name(cpu)?;
                    machine
                        .hart_mut(hart)
                        .rv64
                        .as_mut()
                        .expect("validated RV64 CPU")
                        .initialize_gpr(*index, *value);
                }
                SetupAction::SetFreg(cpu, index, value) => {
                    let hart = machine.rv64_hart_by_name(cpu)?;
                    machine
                        .hart_mut(hart)
                        .rv64
                        .as_mut()
                        .expect("validated RV64 CPU")
                        .initialize_freg(*index, *value)?;
                }
                SetupAction::Load(path, address, cpu) => {
                    let hart = cpu
                        .as_deref()
                        .map(|cpu| machine.rv64_hart_by_name(cpu))
                        .transpose()?;
                    let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
                    image::load_binary_in_context(&mut machine, file, *address, hart)
                        .map_err(|e| format!("{}: {e}", path.display()))?;
                }
                SetupAction::Read(read) => report_ram(&machine, read)?,
                SetupAction::Write(write, value) => write_ram(&mut machine, write, *value)?,
                SetupAction::Move(cpu, old, new) => {
                    let hart = machine.rv64_hart_by_name(cpu)?;
                    machine.bus.move_cpu_ram_alias(hart, *old, *new)?;
                }
            }
        }
        for (cpu, _, _) in &memory_reads {
            if let Some(cpu) = cpu {
                machine.rv64_hart_by_name(cpu)?;
            }
        }
        if let Some(count) = harts {
            if count != 1 && machine.is_cortex_m() {
                return Err("--harts requires a RISC-V machine".into());
            }
            if machine.hart_count() != 1 {
                return Err("--harts cannot override multiple CPU declarations".into());
            }
            if count != 1
                && machine
                    .primary
                    .rv64
                    .as_ref()
                    .is_some_and(|cpu| cpu.mhartid_override.is_some())
            {
                return Err("--harts cannot override a nonzero single hart ID".into());
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
        if harts.is_some() && machine.bus.has_cpu_ram_alias() {
            return Err("--harts cannot override a platform with CPU-scoped RAM".into());
        }
        if (renode_unmapped_data || renode_lrsc) && machine.primary.rv64.is_none() {
            return Err("--renode-unmapped-data and --renode-lrsc require an RV64 machine".into());
        }
        if renode_lrsc && machine.hart_count() != 1 {
            return Err("--renode-lrsc requires a single RV64 hart".into());
        }
        machine.bus.renode_lrsc = renode_lrsc;
        for (cpu, name) in &register_reads {
            let hart = machine.rv64_hart_by_name(cpu)?;
            if machine
                .hart_mut(hart)
                .rv64
                .as_ref()
                .expect("validated RV64 CPU")
                .monitor_register(name)
                .is_none()
            {
                return Err(format!("unsupported RV64 monitor register {name:?}"));
            }
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
        for &(address, value) in &boot_writes {
            machine
                .bus
                .write(address, 4, value, &mut io::sink())
                .map_err(|error| error.to_string())?;
        }
        for access in &memory_setup {
            access.apply(&mut machine.bus)?;
        }
        for command in cpu_control.iter().cloned() {
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
        if inspect_only || memory_only || max_steps == Some(0) {
            return Ok(machine);
        }
        zephyr_boot::configure(&mut machine, &zephyr, path.as_deref())?;
        if !configuration_only {
            machine.validate_entry()?;
        }
        if let Some((local, peer)) = ethernet_loopback {
            machine.bus.connect_rv64_ethernet(local, peer)?;
        }
        if uart_stdin && !machine.bus.has_uart_input() {
            return Err("--uart-stdin requires a platform UART with input support".into());
        }
        if let Some((address, option)) = &tohost {
            let address = *address;
            let option = option.as_str();
            if machine.primary.rv64.is_some() {
                match option {
                    "--robot-tohost" if machine.hart_count() == 1 => {
                        machine.bus.watch_robot_tohost(address)?
                    }
                    "--robot-tohost" => {
                        return Err("--robot-tohost requires a single RV64 hart".into());
                    }
                    "--tohost-act" => return Err("--tohost-act requires an RV32 machine".into()),
                    _ => machine.bus.watch_rv64_tohost(address)?,
                }
            } else if machine.is_cortex_m() || option == "--robot-tohost" {
                return Err("tohost mode requires a compatible RISC-V machine".into());
            } else {
                let address =
                    u32::try_from(address).map_err(|_| "RV32 tohost address exceeds 32 bits")?;
                machine
                    .bus
                    .watch_tohost(address, option == "--tohost-act")?;
            }
        }
        Ok(machine)
    };
    let mut machine = boot_machine()?;
    if inspect_only {
        return Ok(ExitCode::SUCCESS);
    }
    let mut uart_triggers = Vec::new();
    for (matches, name, action, pattern) in uart_actions {
        let trigger = UartTrigger::new(pattern, matches);
        machine.arm_litex_uart_action(trigger.fired.clone(), &name, action)?;
        uart_triggers.push(trigger);
    }
    if sd_setup_only || (setup_only && !static_memory_script) {
        return Ok(ExitCode::SUCCESS);
    }
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
    if max_steps == Some(0) && !static_memory_script {
        if let Some((_, trace)) = &pc_trace {
            trace
                .flush()
                .map_err(|error| format!("RV64 PC trace: {error}"))?;
        }
        for read in &memory_reads {
            report_ram(&machine, read)?;
        }
        report_registers(&mut machine, &register_reads)?;
        return Ok(ExitCode::SUCCESS);
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
        triggers: uart_triggers,
    };
    let start = Instant::now();
    let limit = match (max_steps, run_steps) {
        (Some(cli), Some(script)) => cli.min(script),
        (cli, script) => cli.or(script).unwrap_or(10_000_000),
    };
    if let Some(check) = uart_check {
        if uart_transcript.is_some() || uart_reply.is_some() {
            return Err("--uart-check cannot be combined with --uart-transcript or --uart-reply".into());
        }
        uart_check::run(&mut machine, &mut output, &check)?;
        output.flush().map_err(|e| format!("UART: {e}"))?;
        return Ok(ExitCode::SUCCESS);
    }
    let result = if uart_transcript.is_some() || uart_reply.is_some() {
        uart_transcript::run(
            &mut machine,
            limit,
            &mut output,
            uart_transcript.as_deref(),
            uart_reply,
        )
    } else if let Some(peer_path) = &ble_peer {
        let platform = platform_path
            .as_deref()
            .ok_or("--ble-peer requires a platform")?;
        let mut peer = load_platform(platform, platform_root.as_deref(), false)?;
        let image = File::open(peer_path).map_err(|e| format!("{}: {e}", peer_path.display()))?;
        image::load(&mut peer, image, false, None, false)
            .map_err(|e| format!("{}: {e}", peer_path.display()))?;
        peer.validate_entry()?;
        if let Some(controller_path) = ble_hci_controller {
            let load_controller = || -> Result<_, String> {
                let mut controller = platform::load(platform, platform_root.as_deref(), false)?;
                let file = File::open(&controller_path)
                    .map_err(|e| format!("{}: {e}", controller_path.display()))?;
                image::load(&mut controller, file, false, None, false)?;
                controller.validate_entry()?;
                Ok(controller)
            };
            let mut controllers = [load_controller()?, load_controller()?];
            machine.run_hci_radio_pair(&mut peer, &mut controllers, limit, &mut output)
        } else {
            machine.run_radio_pair(&mut peer, limit, &mut output)
        }
    } else if let Some(script) = script.as_deref().filter(|_| {
        !runtime.is_empty() && (machine.is_cortex_m() || static_memory_script)
    }) {
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
    if let Some((_, trace)) = &pc_trace {
        trace
            .flush()
            .map_err(|error| format!("RV64 PC trace: {error}"))?;
    }
    let (stop, steps) = result?;
    machine.finish_profile()?;
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
    if let Some(trace) = machine.bus.thumb_trace.take() {
        trace.finish()?;
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
    if stop == Stop::Paused {
        eprintln!("penv: machine paused at pc={:#010x}", machine.primary.pc);
    }
    if let Stop::Semihosting(exit) = stop {
        eprintln!("cpu.semihosting: {exit}");
    }
    if machine.cortex_m_locked_up() {
        eprintln!("penv: Cortex-M lockup at pc={:#010x}", machine.primary.pc);
    }
    for read in &memory_reads {
        report_ram(&machine, read)?;
    }
    report_registers(&mut machine, &register_reads)?;
    if let Stop::TestExit(code) = stop {
        eprintln!("penv: TEST FINISHED");
        return Ok(if code == 0 {
            ExitCode::SUCCESS
        } else {
            ExitCode::from(1)
        });
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

fn parse_litex_uart_action(
    option: &str,
    argument: &str,
) -> Result<(usize, String, LiteXUartAction, Vec<u8>), String> {
    let panel = option == "--litex-panel-after-uart";
    let format = || {
        format!(
            "{option} requires {}",
            if panel {
                "N:NAME:ACTION:TEXT"
            } else {
                "NAME:VALUE:TEXT"
            }
        )
    };
    let (count, rest) = if panel {
        let (count, rest) = argument.split_once(':').ok_or_else(format)?;
        let count = count
            .parse::<usize>()
            .ok()
            .filter(|&count| count != 0)
            .ok_or_else(format)?;
        (count, rest)
    } else {
        (1, argument)
    };
    let mut fields = rest.splitn(3, ':');
    let name = fields
        .next()
        .filter(|name| !name.is_empty())
        .ok_or_else(format)?;
    let value = fields.next().ok_or_else(format)?;
    let pattern = fields
        .next()
        .filter(|pattern| !pattern.is_empty())
        .ok_or_else(format)?;
    let action = match option {
        "--litex-panel-after-uart" => match value {
            "Toggle" => LiteXUartAction::Toggle,
            "on" => LiteXUartAction::AssertLed(true),
            "off" => LiteXUartAction::AssertLed(false),
            _ => return Err("LiteX panel action must be Toggle, on, or off".into()),
        },
        "--litex-phy-after-uart" => LiteXUartAction::Phy(
            u16::try_from(parse_address(value)?)
                .map_err(|_| "LiteX PHY value must fit in 16 bits")?,
        ),
        _ => LiteXUartAction::Temperature(parse_litex_temperature(value)?),
    };
    Ok((count, name.to_owned(), action, pattern.as_bytes().to_vec()))
}

fn parse_litex_temperature(value: &str) -> Result<i32, String> {
    let degrees = value
        .parse::<f64>()
        .ok()
        .filter(|value| (-40.0..=85.0).contains(value))
        .ok_or("LiteX temperature must be between -40 and 85 degrees Celsius")?;
    Ok((degrees * 100.0).round() as i32)
}

struct UartOutput<W: Write> {
    writer: LineWriter<W>,
    immediate: bool,
    triggers: Vec<UartTrigger>,
}

impl<W: Write> Write for UartOutput<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let written = self.writer.write(bytes)?;
        self.triggers
            .retain_mut(|trigger| !trigger.receive(&bytes[..written]));
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

fn parse_bus_address(value: &str) -> Result<u64, String> {
    let digits = value.replace('_', "");
    let parsed = match digits.strip_prefix("0x") {
        Some(hex) => u64::from_str_radix(hex, 16),
        None => digits.parse(),
    };
    parsed.map_err(|_| format!("invalid bus address: {value}"))
}

fn parse_address(value: &str) -> Result<u32, String> {
    let parsed = match value.strip_prefix("0x") {
        Some(hex) => u32::from_str_radix(hex, 16),
        None => value.parse(),
    };
    parsed.map_err(|_| format!("invalid 32-bit number: {value}"))
}

fn parse_address64(value: &str) -> Result<u64, String> {
    let digits = value.replace('_', "");
    let parsed = match digits.strip_prefix("0x") {
        Some(hex) => u64::from_str_radix(hex, 16),
        None => digits.parse(),
    };
    parsed.map_err(|_| format!("invalid 64-bit number: {value}"))
}

#[cfg(test)]
mod upstream_nunit {
    use super::parse_address64;

    #[test]
    fn hexadecimal_unsigned_long_1000() {
        assert_eq!(parse_address64("0x1000"), Ok(0x1000));
    }

    #[test]
    fn hexadecimal_unsigned_long_12ab() {
        assert_eq!(parse_address64("0x12AB"), Ok(0x12ab));
    }

    #[test]
    fn hexadecimal_unsigned_long_0ff() {
        assert_eq!(parse_address64("0x0ff"), Ok(0xff));
    }
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
    network_repl::export_peripherals(&platform.ok_or(usage)?, root.as_deref(), &output)?;
    Ok(ExitCode::SUCCESS)
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
