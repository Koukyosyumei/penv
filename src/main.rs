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
  --renode-lrsc  Use Renode single-hart RV64 LR/SC reservations
  --renode-unmapped-data  Warn and return zero/drop for unmapped RV64 data accesses
  --renode-elf-load  Load the mapped RAM portions of RV64 ELF segments
  --robot-tohost ADDRESS   Install a 4 KiB single-hart Renode-style watcher
  --max-steps N      Stop after N execution steps (0 configures only; default: 10000000)
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
        Some(Some("run")) => {}
        _ => return Err("expected 'run' or 'switch'; see --help".into()),
    }

    let mut raw = false;
    let mut rv64 = false;
    let mut stats = false;
    let mut pc_trace = None;
    let mut stop_when_halted = false;
    let mut tohost = None;
    let mut renode_elf_load = false;
    let mut renode_unmapped_data = false;
    let mut renode_lrsc = false;
    let mut positional = false;
    let mut ram_mib = None;
    let mut harts = None;
    let mut max_steps = 10_000_000;
    let mut extra_ram = Vec::new();
    let mut setup_actions = Vec::new();
    let mut memory_reads = Vec::new();
    let mut register_reads = Vec::new();
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
            Some("--trace-pc") => {
                let error = "--trace-pc requires CPU:text:PATH or CPU:binary:PATH";
                let specification = args
                    .next()
                    .and_then(|arg| arg.into_string().ok())
                    .ok_or(error)?;
                let (cpu, rest) = specification
                    .split_once(':')
                    .filter(|(cpu, _)| !cpu.is_empty())
                    .ok_or(error)?;
                let (format, path) = rest
                    .split_once(':')
                    .filter(|(_, path)| !path.is_empty())
                    .ok_or(error)?;
                let binary = match format {
                    "text" => false,
                    "binary" => true,
                    _ => return Err(error.into()),
                };
                if pc_trace
                    .replace((cpu.to_owned(), binary, PathBuf::from(path)))
                    .is_some()
                {
                    return Err("--trace-pc may only be specified once".into());
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
    if path.is_none() && binaries.is_empty() && !(max_steps == 0 && platform_path.is_some()) {
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
        .map(|(cpu, binary, path)| {
            File::options()
                .write(true)
                .create_new(true)
                .open(&path)
                .and_then(|file| machine::PcTrace::new(file, binary))
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
        machine.stop_when_halted = stop_when_halted;
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
        for (path, address) in &binaries {
            let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
            image::load_binary(&mut machine, file, *address)
                .map_err(|e| format!("{}: {e}", path.display()))?;
        }
        machine.bus.renode_unmapped_data = renode_unmapped_data;
        if let Some((cpu, trace)) = &pc_trace {
            machine.trace_pc(cpu, trace.clone())?;
        }
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
        if max_steps != 0 {
            machine.validate_entry()?;
        }
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
    if max_steps == 0 {
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
    if let Some((_, trace)) = &pc_trace {
        trace
            .flush()
            .map_err(|error| format!("RV64 PC trace: {error}"))?;
    }
    let (stop, steps) = result?;
    if stats {
        eprintln!(
            "{stop:?}, {steps} instructions, {:.3} ms, pc={:#010x}",
            elapsed.as_secs_f64() * 1000.0,
            machine.primary.program_counter()
        );
    }
    for read in &memory_reads {
        report_ram(&machine, read)?;
    }
    report_registers(&mut machine, &register_reads)?;
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
