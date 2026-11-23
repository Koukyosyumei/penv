use std::{
    collections::HashMap,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use crate::{
    bus::{CORTEX_M_STEP_HZ, UartWatch, UnhandledAccess},
    machine::{CpuHook, Machine, Stop},
    uart::{BinaryUartPattern, FailingUartPattern, UartHistory, UartWaitMode},
};

#[derive(PartialEq, Eq, Debug)]
pub enum ImageFormat {
    Elf,
    Hex,
}

#[derive(Default)]
pub struct Boot {
    pub binaries: Vec<(String, u32)>,
    pub boot_pcs: Vec<(String, u32)>,
    pub platform: Option<PathBuf>,
    pub image: Option<(String, ImageFormat)>,
    pub vector_tables: Vec<(String, u32)>,
    pub mtvec: Option<u32>,
    pub mask: Option<u32>,
    pub runtime: Vec<RuntimeCommand>,
}

pub struct RuntimeCommand {
    line: usize,
    action: Action,
}

enum Action {
    RunFor(u64),
    CpuStep(String, u64),
    CpuCondition(String, u32),
    CpuXpsr(String),
    CpuWait(String, bool),
    CpuPaused(String, Option<bool>),
    CpuHalted(String, Option<bool>),
    ReadCpuPc(String),
    AddCpuHook(String, Option<u32>, CpuHook),
    NvicLog(String),
    NvicWrite(String, u32, u32),
    SynchroniseQuantum,
    CpuReset(String),
    CpuPc(String, u32),
    MachineReset(Vec<ResetHook>),
    ReadScalar(u64, ScalarWidth),
    WriteScalar(u64, ScalarWidth, u32),
    Tag(u32, u32, u64, bool),
    UnhandledAccess(UnhandledAccess),
    ReadBytes(u32, usize),
    FindBytes(Vec<u8>, u32, u32),
    WriteBytes(u32, Vec<u8>),
    Button(String, bool),
    ButtonPulse(String),
    UnregisterGpio(String),
    NvicGpio(String, u8, bool),
    NvicPending(String, u32),
    NvicClocked(String, bool),
    Frequency(String, u32),
    WaitForLed(String, bool, u64, u64),
    Adc(String, usize, f64),
    WaitForUart(Option<String>, String, UartWaitMode, Option<u64>),
    CreateUartTester(String, bool),
    DefaultUartTester(Option<usize>),
    WaitForTester(Option<usize>, String, u64),
    WaitForBytes(Option<usize>, BinaryUartPattern, bool, u64),
    SetUartFailure(Option<usize>, FailingUartPattern, bool),
    WriteUartLine(String, String, &'static [u8]),
}

enum ResetHook {
    Sleep(Duration),
    Log(String),
}

#[derive(Clone, Copy)]
enum ScalarWidth {
    Byte = 1,
    Word = 2,
    DoubleWord = 4,
}

impl ScalarWidth {
    fn read(self, machine: &mut Machine, address: u64) -> Result<u32, String> {
        let Ok(address32) = u32::try_from(address) else {
            let width = self as usize;
            let memory = machine.bus.high_memory(address, width)?;
            let mut bytes = [0; 4];
            bytes[..width].copy_from_slice(memory);
            return Ok(u32::from_le_bytes(bytes));
        };
        match self {
            Self::Byte => machine.bus.read_scalar::<1>(address32, true),
            Self::Word => machine.bus.read_scalar::<2>(address32, true),
            Self::DoubleWord => machine.bus.read_scalar::<4>(address32, true),
        }
        .map_err(|e| e.to_string())
    }
}

struct ScriptOutput<'a, W> {
    output: &'a mut W,
    history: Option<UartHistory>,
}

impl<W: Write> Write for ScriptOutput<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let written = self.output.write(bytes)?;
        if let Some(history) = &mut self.history {
            for &byte in &bytes[..written] {
                history.receive(byte);
            }
        }
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.output.flush()
    }
}

pub fn run(
    machine: &mut Machine,
    script: &Path,
    commands: &[RuntimeCommand],
    mut budget: u64,
    output: &mut impl Write,
) -> Result<(Stop, u64), String> {
    if commands.is_empty() {
        return machine.run(budget, output);
    }
    let mut output = ScriptOutput {
        output,
        history: None,
    };
    for command in commands {
        match &command.action {
            Action::WaitForUart(Some(target), ..) => machine.bus.record_stm32_uart(target),
            Action::WaitForUart(None, ..) => {
                output.history.get_or_insert_with(UartHistory::default);
            }
            _ => {}
        }
    }
    let mut retired = 0;
    for command in commands {
        let result = match &command.action {
            Action::RunFor(steps) | Action::CpuStep(_, steps) => {
                let valid_cpu = match &command.action {
                    Action::CpuStep(cpu, _) => machine.cortex_m_cpu(cpu).map(|_| ()),
                    _ if machine.is_cortex_m() => Ok(()),
                    _ => Err("RunFor requires a Cortex-M platform".into()),
                };
                valid_cpu.and_then(|()| {
                    if machine.primary.monitor_paused {
                        return Ok(None);
                    }
                    let chunk = (*steps).min(budget);
                    let start = machine.elapsed_steps();
                    machine.run(chunk, &mut output).map(|(stop, count)| {
                        retired += count;
                        budget -= machine.elapsed_steps().wrapping_sub(start);
                        (stop != Stop::Limit || (chunk < *steps && !machine.primary.monitor_paused))
                            .then_some(stop)
                    })
                })
            }
            Action::CpuCondition(cpu, condition) => machine.cortex_m_cpu(cpu).and_then(|thumb| {
                let result = if thumb.condition(*condition) {
                    "True"
                } else {
                    "False"
                };
                writeln!(output.output, "{result}")
                    .map_err(|e| e.to_string())
                    .map(|()| None)
            }),
            Action::CpuXpsr(cpu) => machine.cortex_m_cpu(cpu).and_then(|thumb| {
                writeln!(output.output, "0x{:08x}", thumb.xpsr())
                    .map_err(|e| e.to_string())
                    .map(|()| None)
            }),
            Action::CpuPaused(cpu, requested) | Action::CpuHalted(cpu, requested) => {
                machine.cortex_m_monitor_cpu(cpu).and_then(|hart| {
                    let state = if matches!(command.action, Action::CpuPaused(..)) {
                        &mut hart.monitor_paused
                    } else {
                        &mut hart.monitor_halted
                    };
                    if let Some(requested) = requested {
                        *state = *requested;
                        Ok(None)
                    } else {
                        let value = if *state { "True" } else { "False" };
                        writeln!(output.output, "{value}")
                            .map_err(|e| e.to_string())
                            .map(|()| None)
                    }
                })
            }
            Action::AddCpuHook(cpu, address, hook) => {
                machine.cortex_m_monitor_cpu(cpu).map(|hart| {
                    hart.monitor_hooks
                        .push((address.unwrap_or(hart.pc), hook.clone()));
                    None
                })
            }
            Action::NvicLog(target) => machine.bus.enable_nvic_log(target).map(|()| None),
            Action::NvicWrite(target, offset, value) => machine
                .bus
                .nvic_register_address(target, *offset)
                .and_then(|address| {
                    machine
                        .bus
                        .write_scalar(address, 4, *value, &mut output, true)
                        .map_err(|e| e.to_string())
                })
                .map(|()| None),
            Action::ReadCpuPc(cpu) => machine.cortex_m_monitor_cpu(cpu).and_then(|hart| {
                writeln!(output.output, "0x{:08x}", hart.pc)
                    .map_err(|e| e.to_string())
                    .map(|()| None)
            }),
            // Synchronous script actions run at completed instruction boundaries.
            Action::SynchroniseQuantum => Ok(None),
            Action::CpuWait(cpu, asserted) => {
                machine.set_cortex_m_cpu_wait(cpu, *asserted).map(|()| None)
            }
            Action::CpuPc(cpu, pc) => machine.set_named_entry(cpu, *pc).map(|()| None),
            Action::CpuReset(cpu) => machine.reset_named_cortex_m(cpu).map(|()| None),
            Action::MachineReset(hooks) => machine.reset_cortex_m_machine().and_then(|()| {
                for hook in hooks {
                    match hook {
                        ResetHook::Sleep(duration) => std::thread::sleep(*duration),
                        ResetHook::Log(message) => {
                            writeln!(output.output, "{message}").map_err(|e| e.to_string())?;
                        }
                    }
                }
                Ok(None)
            }),
            Action::ReadScalar(address, width) => width
                .read(machine, *address)
                .and_then(|value| {
                    let digits = *width as usize * 2;
                    writeln!(output.output, "0x{value:0digits$x}").map_err(|e| e.to_string())
                })
                .map(|()| None),
            Action::WriteScalar(address, width, value) => {
                let width = *width as usize;
                if let Ok(address) = u32::try_from(*address) {
                    machine
                        .bus
                        .write_scalar(address, width, *value, &mut output, true)
                        .map_err(|error| error.to_string())
                        .map(|()| None)
                } else {
                    machine.bus.high_memory(*address, width).map(|memory| {
                        memory.copy_from_slice(&value.to_le_bytes()[..width]);
                        None
                    })
                }
            }
            Action::Tag(start, size, default, throws) => machine
                .bus
                .tag(*start, *size, *default, *throws)
                .map(|()| None),
            Action::UnhandledAccess(policy) => {
                machine.bus.unhandled_access = Some(*policy);
                Ok(None)
            }
            Action::ReadBytes(address, count) => {
                read_bus_bytes(machine, *address, *count, output.output).map(|()| None)
            }
            Action::FindBytes(pattern, start, end) => machine
                .bus
                .find_bytes(pattern, *start, *end)
                .and_then(|addresses| {
                    write_hex_list(addresses, 8, output.output).map_err(|error| error.to_string())
                })
                .map(|()| None),
            Action::WriteBytes(address, bytes) => machine
                .bus
                .write_bytes(*address, bytes, &mut output)
                .map_err(|error| error.to_string())
                .map(|()| None),
            Action::Button(name, pressed) => {
                machine.bus.set_named_button(name, *pressed).map(|()| None)
            }
            Action::UnregisterGpio(target) => {
                machine.bus.unregister_stm32_gpio(target).map(|()| None)
            }
            Action::ButtonPulse(name) => machine
                .bus
                .set_named_button(name, true)
                .and_then(|()| machine.bus.set_named_button(name, false))
                .map(|()| None),
            Action::NvicGpio(target, line, high) => machine
                .bus
                .drive_nvic_line(target, *line, *high)
                .map(|()| None),
            Action::NvicPending(target, exception) => machine
                .bus
                .pend_nvic_exception(target, *exception)
                .map(|()| None),
            Action::Frequency(target, frequency) => {
                machine.bus.set_frequency(target, *frequency).map(|()| None)
            }
            Action::NvicClocked(target, clocked) => machine
                .bus
                .set_nvic_clocked(target, *clocked)
                .map(|()| None),
            Action::Adc(name, channel, volts) => machine
                .bus
                .set_named_adc_input(name, *channel, *volts)
                .map(|()| None),
            Action::CreateUartTester(target, binary) => machine
                .bus
                .create_uart_tester(target, *binary)
                .and_then(|id| writeln!(output.output, "{id}").map_err(|e| e.to_string()))
                .map(|()| None),
            Action::DefaultUartTester(id) => {
                machine.bus.set_default_uart_tester(*id).map(|()| None)
            }
            Action::SetUartFailure(id, pattern, register) => machine
                .bus
                .set_uart_failure(*id, pattern.clone(), *register)
                .map(|()| None),
            Action::WaitForBytes(id, pattern, match_start, timeout) => {
                let start = machine.elapsed_steps();
                machine.bus.watch_uart_bytes(*id, pattern, *match_start).and_then(|watch| {
                    let result = drive_uart_watch(machine, watch, (*timeout).min(budget), &mut output);
                    let matched = machine.bus.take_uart_byte_match(watch);
                    machine.bus.finish_stm32_uart_watch(watch);
                    result.and_then(|(stop, count)| {
                        retired += count;
                        budget -= machine.elapsed_steps().wrapping_sub(start);
                        let matched = matched.ok_or_else(|| format!("Terminal tester failed! Byte pattern was not received ({stop:?})"))?;
                        write_hex_list(matched.content, 2, output.output).map_err(|e| e.to_string())?;
                        for group in matched.groups {
                            match group {
                                Some(bytes) => write_hex_list(bytes, 2, output.output),
                                None => writeln!(output.output, "null"),
                            }.map_err(|e| e.to_string())?;
                        }
                        Ok(None)
                    })
                })
            }
            Action::WaitForTester(id, pattern, timeout) => {
                let start = machine.elapsed_steps();
                machine.bus.watch_uart_tester(*id, pattern).and_then(|watch| {
                    run_uart_watch(machine, watch, (*timeout).min(budget), &mut output)
                }).and_then(|(stop, count)| {
                    retired += count;
                    budget -= machine.elapsed_steps().wrapping_sub(start);
                    if stop == Stop::OutputMatch {
                        Ok(None)
                    } else {
                        Err(format!("Terminal tester failed! Line containing {pattern:?} was not received ({stop:?})"))
                    }
                })
            }
            Action::WaitForUart(target, pattern, end, timeout) => {
                let start = machine.elapsed_steps();
                wait_for_uart(
                    machine,
                    target.as_deref(),
                    pattern,
                    *end,
                    timeout.map_or(budget, |timeout| timeout.min(budget)),
                    &mut output,
                )
                .and_then(|(stop, count)| {
                    retired += count;
                    budget -= machine.elapsed_steps().wrapping_sub(start);
                    if stop == Stop::OutputMatch {
                        Ok(None)
                    } else {
                        let source = target
                            .as_ref()
                            .map_or_else(String::new, |target| format!(" on {target}"));
                        let kind = if matches!(end, UartWaitMode::Prompt) {
                            "prompt"
                        } else {
                            "line"
                        };
                        Err(format!(
                            "UART {kind} containing {pattern:?} was not received{source} ({stop:?})"
                        ))
                    }
                })
            }
            Action::WaitForLed(target, state, timeout, hold) => wait_for_led(
                machine,
                target,
                *state,
                *timeout,
                *hold,
                &mut budget,
                &mut output,
            )
            .map(|count| {
                retired += count;
                None
            }),
            Action::WriteUartLine(target, text, ending) => machine
                .bus
                .write_stm32_uart_line(target, text, ending)
                .map(|()| None),
        };
        if let Some(stop) =
            result.map_err(|e| format!("{}:{}: {e}", script.display(), command.line))?
        {
            return Ok((stop, retired));
        }
    }
    Ok((Stop::ScriptComplete, retired))
}

fn read_bus_bytes(
    machine: &mut Machine,
    address: u32,
    count: usize,
    output: &mut impl Write,
) -> Result<(), String> {
    let bytes = machine
        .bus
        .read_bytes(address, count)
        .map_err(|error| error.to_string())?;
    write!(output, "[").map_err(|error| error.to_string())?;
    for (index, byte) in bytes.enumerate() {
        let byte = byte.map_err(|error| error.to_string())?;
        let separator = if index == 0 { "" } else { ", " };
        write!(output, "{separator}0x{byte:02x}").map_err(|error| error.to_string())?;
    }
    writeln!(output, "]").map_err(|error| error.to_string())
}

fn write_hex_list<T: std::fmt::LowerHex>(
    values: impl IntoIterator<Item = T>,
    digits: usize,
    output: &mut impl Write,
) -> io::Result<()> {
    write!(output, "[")?;
    for (index, value) in values.into_iter().enumerate() {
        let separator = if index == 0 { "" } else { ", " };
        write!(output, "{separator}0x{value:0digits$x}")?;
    }
    writeln!(output, "]")
}

fn wait_for_led(
    machine: &mut Machine,
    target: &str,
    state: bool,
    timeout: u64,
    hold: u64,
    budget: &mut u64,
    output: &mut impl Write,
) -> Result<u64, String> {
    let index = machine.bus.gpio_led_index(target)?;
    let mut retired = 0;
    if machine.bus.gpio_led(index).state != state {
        let start = machine.elapsed_steps();
        let limit = timeout.min(*budget);
        let (stop, count) =
            machine.run_until(limit, output, |bus, _| bus.gpio_led(index).state == state)?;
        *budget -= machine.elapsed_steps().wrapping_sub(start);
        if stop != Stop::OutputMatch {
            return Err(format!(
                "LED {target} did not reach state {state} ({stop:?})"
            ));
        }
        retired += count;
    }

    let transitions = machine.bus.gpio_led(index).transitions;
    let limit = hold.min(*budget);
    let (stop, count) = machine.run_until(limit, output, |bus, _| {
        bus.gpio_led(index).transitions != transitions
    })?;
    if stop == Stop::OutputMatch {
        return Err(format!("LED {target} changed while holding state {state}"));
    }
    if stop != Stop::Limit || limit != hold {
        return Err(format!(
            "LED {target} hold ended before completion ({stop:?})"
        ));
    }
    *budget -= limit;
    Ok(retired + count)
}

fn drive_uart_watch(
    machine: &mut Machine,
    watch: UartWatch,
    budget: u64,
    output: &mut impl Write,
) -> Result<(Stop, u64), String> {
    if machine.bus.stm32_uart_matched(watch) {
        Ok((Stop::OutputMatch, 0))
    } else {
        machine.run_until(budget, output, |bus, _| bus.stm32_uart_matched(watch))
    }
}

fn run_uart_watch(
    machine: &mut Machine,
    watch: UartWatch,
    budget: u64,
    output: &mut impl Write,
) -> Result<(Stop, u64), String> {
    let result = drive_uart_watch(machine, watch, budget, output).and_then(|result| {
        if let Some(line) = machine.bus.uart_watch_failure(watch) {
            return Err(format!(
                "Terminal tester failed! Test failing entry has been found on UART:\n{line}"
            ));
        }
        Ok(result)
    });
    machine.bus.finish_stm32_uart_watch(watch);
    result
}

fn wait_for_uart(
    machine: &mut Machine,
    target: Option<&str>,
    pattern: &str,
    end: UartWaitMode,
    budget: u64,
    output: &mut ScriptOutput<'_, impl Write>,
) -> Result<(Stop, u64), String> {
    if let Some(target) = target {
        let watch = machine.bus.watch_stm32_uart(target, pattern, end)?;
        run_uart_watch(machine, watch, budget, output)
    } else {
        let history = output
            .history
            .as_mut()
            .expect("a script wait enables history before execution");
        history.begin_wait(pattern, end);
        let result = if history.matched() {
            Ok((Stop::OutputMatch, 0))
        } else {
            machine.run_until(budget, output, |_, uart| {
                uart.history.as_ref().is_some_and(UartHistory::matched)
            })
        };
        if let Some(history) = &mut output.history {
            history.finish_wait();
        }
        result
    }
}

pub fn load(path: &Path, root: &Path) -> Result<Boot, String> {
    let content = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut variables = HashMap::new();
    let mut macros = HashMap::new();
    let mut boot = Boot::default();
    let mut machine = false;
    let mut cursor = content.lines().enumerate();
    while let Some((line, raw)) = cursor.next() {
        let text = raw.trim();
        let error = |message: &str| format!("{}:{}: {message}", path.display(), line + 1);
        if text.is_empty() || text.starts_with('#') || text.starts_with(':') {
            continue;
        }
        if let Some(declaration) = text.strip_prefix("macro ") {
            let declaration = declaration.trim();
            if let Some((name, body)) = declaration.split_once(char::is_whitespace) {
                let body = body
                    .trim()
                    .strip_prefix("\"\"\"")
                    .and_then(|body| body.strip_suffix("\"\"\""))
                    .ok_or_else(|| error("expected triple-quoted macro body"))?;
                macros.insert(name, vec![(line, body)]);
                continue;
            }
            let name = declaration;
            if cursor
                .next()
                .is_none_or(|(_, line)| line.trim() != "\"\"\"")
            {
                return Err(error("expected triple-quoted macro body"));
            }
            let mut body = Vec::new();
            loop {
                let (index, command) = cursor.next().ok_or_else(|| error("unterminated macro"))?;
                if command.trim() == "\"\"\"" {
                    break;
                }
                body.push((index, command));
            }
            macros.insert(name, body);
            continue;
        }
        if let Some(assignment) = text.strip_prefix('$') {
            let (name, value) = assignment
                .split_once('=')
                .ok_or_else(|| error("expected variable assignment"))?;
            let name = name.trim();
            if let Some(name) = name.strip_suffix('?') {
                variables.entry(name.trim()).or_insert(value.trim());
            } else {
                variables.insert(name, value.trim());
            }
            continue;
        }
        if let Some(name) = text.strip_prefix("runMacro $") {
            let body = macros
                .get(name.trim())
                .ok_or_else(|| error("unknown macro"))?;
            for &(index, command) in body {
                execute(
                    command.trim(),
                    &variables,
                    &mut machine,
                    &mut boot,
                    path,
                    root,
                    index + 1,
                )
                .map_err(|e| format!("{}:{}: {e}", path.display(), index + 1))?;
            }
            continue;
        }
        execute(
            text,
            &variables,
            &mut machine,
            &mut boot,
            path,
            root,
            line + 1,
        )
        .map_err(|e| error(&e))?;
        if let Some(RuntimeCommand {
            line: command_line,
            action: Action::MachineReset(hooks),
        }) = boot.runtime.last_mut()
            && *command_line == line + 1
            && let Some(body) = macros.get("reset")
        {
            for (index, command) in body {
                parse_reset_hooks(command.trim(), hooks)
                    .map_err(|e| format!("{}:{}: {e}", path.display(), index + 1))?;
            }
        }
    }
    if !machine || boot.platform.is_none() || (boot.image.is_none() && boot.binaries.is_empty()) {
        return Err(format!(
            "{}: expected machine creation, platform and firmware loading",
            path.display()
        ));
    }
    Ok(boot)
}

fn parse_reset_hooks(mut commands: &str, hooks: &mut Vec<ResetHook>) -> Result<(), String> {
    while !commands.is_empty() && !commands.starts_with('#') {
        let (verb, argument) = commands
            .split_once(char::is_whitespace)
            .ok_or("expected reset macro sleep or log")?;
        let argument = argument.trim_start();
        let rest = match verb {
            "sleep" => {
                let end = argument.find(';').unwrap_or(argument.len());
                let seconds = argument[..end]
                    .trim()
                    .parse::<f64>()
                    .map_err(|_| "expected reset macro sleep duration in seconds")?;
                let duration = Duration::try_from_secs_f64(seconds)
                    .map_err(|_| "invalid reset macro sleep duration")?;
                hooks.push(ResetHook::Sleep(duration));
                &argument[end..]
            }
            "log" => {
                let (message, rest) = argument
                    .strip_prefix('"')
                    .and_then(|arg| arg.split_once('"'))
                    .ok_or("expected quoted reset macro log message")?;
                hooks.push(ResetHook::Log(message.into()));
                rest
            }
            _ => return Err(format!("unsupported reset macro operation {verb:?}")),
        };
        commands = rest.trim_start();
        if !commands.is_empty() {
            commands = commands
                .strip_prefix(';')
                .ok_or("expected ';' between reset macro commands")?
                .trim_start();
        }
    }
    Ok(())
}

fn execute(
    text: &str,
    variables: &HashMap<&str, &str>,
    machine: &mut bool,
    boot: &mut Boot,
    script: &Path,
    root: &Path,
    line: usize,
) -> Result<(), String> {
    if text.is_empty() || text.starts_with('#') || text == "using sysbus" {
        return Ok(());
    }
    let words: Vec<_> = text.split_whitespace().collect();
    let resolve = |value| match value {
        "$" => Err("missing variable name".to_string()),
        value => match value.strip_prefix('$') {
            Some(name) => variables
                .get(name)
                .copied()
                .ok_or_else(|| format!("unknown variable {name}")),
            None => Ok(value),
        },
    };
    let action = match words.as_slice() {
        ["CreateTerminalTester", target, options @ ..] => {
            let mut binary = None;
            for option in options {
                match option.split_once('=') {
                    Some(("binaryMode", value)) if binary.is_none() => {
                        binary = Some(parse_tester_bool(resolve(value)?)?);
                    }
                    Some(("defaultPauseEmulation", "true" | "True")) => {}
                    _ => {
                        return Err(format!(
                            "unsupported or duplicate UART tester option {option:?}"
                        ));
                    }
                }
            }
            Some(Action::CreateUartTester(
                resolve(target)?.to_owned(),
                binary.unwrap_or(false),
            ))
        }
        ["SetDefaultTester", id] => {
            let id = resolve(id)?;
            Some(Action::DefaultUartTester(
                (id != "null")
                    .then(|| crate::parse_address(id).map(|id| id as usize))
                    .transpose()?,
            ))
        }
        ["WaitForBytesOnUart", ..] => {
            let args = text["WaitForBytesOnUart".len()..].trim_start();
            let (pattern, arguments) = quoted_argument(args)
                .filter(|(pattern, _)| !pattern.is_empty())
                .ok_or("expected WaitForBytesOnUart \"bytes or regex\" [options]")?;
            let options = tester_wait_options(arguments, resolve, TesterOptionsKind::Bytes)?;
            let pattern = if options.regex {
                BinaryUartPattern::Regex(
                    regex::bytes::RegexBuilder::new(pattern)
                        .unicode(false)
                        .dot_matches_new_line(true)
                        .build()
                        .map_err(|e| format!("invalid UART byte regex: {e}"))?,
                )
            } else {
                let bytes = hex_pattern(pattern)?;
                if bytes.is_empty() || bytes.len() > crate::uart::UART_HISTORY_BYTES {
                    return Err("UART byte pattern must contain 1 to 65536 bytes".into());
                }
                BinaryUartPattern::Literal(bytes)
            };
            Some(Action::WaitForBytes(
                options.id,
                pattern,
                options.match_start,
                options.timeout,
            ))
        }
        ["WaitForLineOnUart", ..] => {
            let (pattern, arguments) = quoted_argument(
                text["WaitForLineOnUart".len()..].trim_start(),
            )
            .filter(|(pattern, _)| !pattern.is_empty())
            .ok_or(
                "expected WaitForLineOnUart \"literal text\" [testerId=number] [timeout=seconds]",
            )?;
            let options = tester_wait_options(arguments, resolve, TesterOptionsKind::Line)?;
            Some(Action::WaitForTester(
                options.id,
                pattern.to_owned(),
                options.timeout,
            ))
        }
        [
            command @ ("RegisterFailingUartString" | "UnregisterFailingUartString"),
            ..,
        ] => {
            let (pattern, arguments) = quoted_argument(text[command.len()..].trim_start())
                .filter(|(pattern, _)| !pattern.is_empty())
                .ok_or("expected quoted failing UART pattern")?;
            let options = tester_wait_options(arguments, resolve, TesterOptionsKind::Failure)?;
            let regex = options
                .regex
                .then(|| {
                    regex::Regex::new(pattern)
                        .map_err(|e| format!("invalid failing UART regex: {e}"))
                })
                .transpose()?;
            Some(Action::SetUartFailure(
                options.id,
                FailingUartPattern {
                    pattern: pattern.into(),
                    regex,
                },
                *command == "RegisterFailingUartString",
            ))
        }
        [command @ ("waitForUart" | "waitForUartPrompt"), ..] => {
            let args = text[command.len()..].trim();
            let expected = format!("expected {command} [USART] \"literal text\" [timeout=seconds]");
            let (target, line) = if args.starts_with('"') {
                (None, args)
            } else {
                let (target, line) = args
                    .split_once(char::is_whitespace)
                    .ok_or_else(|| expected.clone())?;
                (Some(resolve(target)?.to_owned()), line.trim_start())
            };
            let (pattern, options) = quoted_argument(line)
                .filter(|(pattern, _)| !pattern.is_empty())
                .ok_or(expected)?;
            let (ending, timeout) = match options.rsplit_once(char::is_whitespace) {
                Some((ending, timeout)) => (ending.trim_end(), timeout),
                None if options.starts_with("timeout=") => ("", options),
                None => (options, ""),
            };
            let timeout = if timeout.is_empty() {
                None
            } else {
                Some(duration_steps(
                    timeout
                        .strip_prefix("timeout=")
                        .ok_or("expected UART timeout=seconds")?,
                )?)
            };
            let end = match (*command, ending) {
                ("waitForUartPrompt", "") => UartWaitMode::Prompt,
                ("waitForUartPrompt", _) => {
                    return Err("unexpected argument after UART prompt".into());
                }
                (_, "" | "LF") => UartWaitMode::Lf,
                (_, "CR") => UartWaitMode::Cr,
                _ => return Err("expected waitForUart ending LF or CR".into()),
            };
            Some(Action::WaitForUart(
                target,
                pattern.to_owned(),
                end,
                timeout,
            ))
        }
        [target, "WriteLine", ..] => {
            let args = text[target.len()..].trim_start()["WriteLine".len()..].trim();
            let (line, ending) = quoted_argument(args)
                .ok_or("expected USART WriteLine \"literal text\" [line ending]")?;
            let ending: &[u8] = match ending {
                "" | "CR" | "true" => b"\r",
                "LF" => b"\n",
                "CRLF" => b"\r\n",
                "None" | "false" => b"",
                _ => {
                    return Err(
                        "expected USART WriteLine ending CR, LF, CRLF, None, true or false".into(),
                    );
                }
            };
            Some(Action::WriteUartLine(
                resolve(target)?.to_owned(),
                line.to_owned(),
                ending,
            ))
        }
        ["waitForLed", target, state, timeout, hold @ ..] if hold.len() <= 1 => {
            let state = resolve(state)?
                .parse()
                .map_err(|_| "expected LED state true or false")?;
            let timeout = duration_steps(resolve(timeout)?)?;
            let hold = hold
                .first()
                .map(|value| duration_steps(resolve(value)?))
                .transpose()?
                .unwrap_or(0);
            Some(Action::WaitForLed(
                resolve(target)?.to_owned(),
                state,
                timeout,
                hold,
            ))
        }
        [target, "PC", address] if !boot.runtime.is_empty() => {
            let cpu = resolve(target)?;
            Some(Action::CpuPc(
                cpu.strip_prefix("sysbus.").unwrap_or(cpu).into(),
                crate::parse_address(resolve(address)?)?,
            ))
        }
        ["sysbus", "UnhandledAccessBehaviour", policy] => {
            let policy = match resolve(policy)? {
                "DoNotReport" => UnhandledAccess::DoNotReport,
                "ThrowException" => UnhandledAccess::ThrowException,
                "ThrowExceptionIfNotTagged" => UnhandledAccess::ThrowExceptionIfNotTagged,
                other => return Err(format!("unsupported UnhandledAccessBehaviour {other:?}")),
            };
            Some(Action::UnhandledAccess(policy))
        }
        ["sysbus", "Tag", ..] => {
            let args = text["sysbus".len()..].trim_start()["Tag".len()..].trim_start();
            let (range, remainder) = args
                .strip_prefix('<')
                .and_then(|args| args.split_once('>'))
                .ok_or("expected Tag <start size>")?;
            let fields = range
                .split(|c: char| c.is_whitespace() || c == ',')
                .filter(|field| !field.is_empty())
                .collect::<Vec<_>>();
            let [start, size] = fields.as_slice() else {
                return Err("expected Tag <start size>".into());
            };
            let (_, options) =
                quoted_argument(remainder.trim()).ok_or("expected quoted tag name")?;
            let mut default = None;
            let mut throws = None;
            for option in options.split_whitespace() {
                if let Some(value) = option.strip_prefix("throwException=") {
                    if throws.is_some() {
                        return Err("duplicate throwException option".into());
                    }
                    throws = Some(
                        resolve(value)?
                            .to_ascii_lowercase()
                            .parse()
                            .map_err(|_| "invalid throwException value")?,
                    );
                } else if default.is_none() {
                    let value = resolve(option.strip_prefix("defaultValue=").unwrap_or(option))?;
                    default = Some(
                        match value.strip_prefix("0x") {
                            Some(hex) => u64::from_str_radix(hex, 16),
                            None => value.parse(),
                        }
                        .map_err(|_| {
                            format!("unsupported Tag option or invalid default: {option}")
                        })?,
                    );
                } else {
                    return Err(format!("unsupported Tag option {option}"));
                }
            }
            Some(Action::Tag(
                crate::parse_address(resolve(start)?)?,
                crate::parse_address(resolve(size)?)?,
                default.unwrap_or(0),
                throws.unwrap_or(false),
            ))
        }
        [
            "sysbus",
            method @ ("ReadByte" | "ReadWord" | "ReadDoubleWord"),
            address,
        ] => Some(Action::ReadScalar(
            crate::parse_bus_address(resolve(address)?)?,
            match *method {
                "ReadByte" => ScalarWidth::Byte,
                "ReadWord" => ScalarWidth::Word,
                _ => ScalarWidth::DoubleWord,
            },
        )),
        [
            "sysbus",
            method @ ("WriteByte" | "WriteWord" | "WriteDoubleWord"),
            address,
            value,
        ] => {
            let width = match *method {
                "WriteByte" => ScalarWidth::Byte,
                "WriteWord" => ScalarWidth::Word,
                _ => ScalarWidth::DoubleWord,
            };
            let value = crate::parse_address(resolve(value)?)?;
            if (u32::MAX >> (32 - width as usize * 8)) < value {
                return Err(format!("value {value:#x} does not fit {method}"));
            }
            Some(Action::WriteScalar(
                crate::parse_bus_address(resolve(address)?)?,
                width,
                value,
            ))
        }
        ["sysbus", "ReadBytes", address, count] => Some(Action::ReadBytes(
            crate::parse_address(resolve(address)?)?,
            crate::parse_address(resolve(count)?)? as usize,
        )),
        ["sysbus", "WriteBytes", ..] => {
            let args = text["sysbus".len()..].trim_start()["WriteBytes".len()..].trim_start();
            let (bytes, address) = byte_list(args, resolve)?;
            let address = crate::parse_address(resolve(address)?)?;
            Some(Action::WriteBytes(address, bytes))
        }
        ["sysbus", "FindBytes", ..] => {
            let args = text["sysbus".len()..].trim_start()["FindBytes".len()..].trim_start();
            let (pattern, remainder) = if args.starts_with('[') {
                byte_list(args, resolve)?
            } else {
                let (hex, remainder) = quoted_argument(args)
                    .ok_or("expected sysbus FindBytes \"hex bytes\" or [byte, ...]")?;
                (hex_pattern(hex)?, remainder)
            };
            let mut bounds = remainder.split_whitespace();
            let start = bounds
                .next()
                .map(|value| crate::parse_address(resolve(value)?))
                .transpose()?
                .unwrap_or(0);
            let end = bounds
                .next()
                .filter(|&value| value != "null")
                .map(|value| crate::parse_address(resolve(value)?))
                .transpose()?
                .unwrap_or(u32::MAX);
            if bounds.next().is_some() {
                return Err("expected sysbus FindBytes pattern [start [end]]".into());
            }
            Some(Action::FindBytes(pattern, start, end))
        }
        [target, "Step", count] => {
            let cpu = resolve(target)?;
            Some(Action::CpuStep(
                cpu.strip_prefix("sysbus.").unwrap_or(cpu).into(),
                u64::from(crate::parse_address(resolve(count)?)?),
            ))
        }
        [target, "EvaluateConditionCode", condition] => {
            let cpu = resolve(target)?;
            let condition = crate::parse_address(resolve(condition)?)?;
            if condition > 15 {
                return Err("expected an ARM condition code from 0 to 15".into());
            }
            Some(Action::CpuCondition(
                cpu.strip_prefix("sysbus.").unwrap_or(cpu).into(),
                condition,
            ))
        }
        [target, "GetRegister", register]
            if matches!(
                resolve(register)?
                    .trim_matches('"')
                    .to_ascii_lowercase()
                    .as_str(),
                "cpsr" | "xpsr"
            ) =>
        {
            let cpu = resolve(target)?;
            Some(Action::CpuXpsr(
                cpu.strip_prefix("sysbus.").unwrap_or(cpu).into(),
            ))
        }
        [target, "AddHook", ..] => {
            let cpu = resolve(target)?;
            let args = text[target.len()..].trim_start()["AddHook".len()..].trim_start();
            let (address, body) = if let Some(expression) = args.strip_prefix('`') {
                let (expression, body) = expression
                    .split_once('`')
                    .ok_or("unterminated AddHook address expression")?;
                let words: Vec<_> = expression.split_whitespace().collect();
                if !matches!(words.as_slice(), [name, "PC"] if name.strip_prefix("sysbus.").unwrap_or(name) == cpu.strip_prefix("sysbus.").unwrap_or(cpu))
                {
                    return Err("AddHook address expression must read the target CPU PC".into());
                }
                (None, body)
            } else {
                let (address, body) = args
                    .split_once(char::is_whitespace)
                    .ok_or("expected AddHook address and body")?;
                (Some(crate::parse_address(resolve(address)?)?), body)
            };
            Some(Action::AddCpuHook(
                cpu.strip_prefix("sysbus.").unwrap_or(cpu).into(),
                address,
                parse_cpu_hook(body.trim())?,
            ))
        }
        ["logLevel", "-1", target] => Some(Action::NvicLog(resolve(target)?.to_owned())),
        [target, "WriteDoubleWord", offset, value] => Some(Action::NvicWrite(
            resolve(target)?.to_owned(),
            crate::parse_address(resolve(offset)?)?,
            crate::parse_address(resolve(value)?)?,
        )),
        ["emulation", "SynchroniseQuantum"] => Some(Action::SynchroniseQuantum),
        [target, operation @ ("Pause" | "Resume" | "IsPaused")] => {
            let cpu = resolve(target)?;
            let requested = match *operation {
                "Pause" => Some(true),
                "Resume" => Some(false),
                _ => None,
            };
            Some(Action::CpuPaused(
                cpu.strip_prefix("sysbus.").unwrap_or(cpu).into(),
                requested,
            ))
        }
        [target, "IsHalted", value @ ..] if value.len() <= 1 => {
            let cpu = resolve(target)?;
            let requested = value
                .first()
                .map(|value| resolve(value).and_then(parse_tester_bool))
                .transpose()?;
            Some(Action::CpuHalted(
                cpu.strip_prefix("sysbus.").unwrap_or(cpu).into(),
                requested,
            ))
        }
        [target, "PC"] => {
            let cpu = resolve(target)?;
            Some(Action::ReadCpuPc(
                cpu.strip_prefix("sysbus.").unwrap_or(cpu).into(),
            ))
        }
        ["emulation", "RunToNearestSyncPoint"] => Some(Action::RunFor(1)),
        ["machine" | "mach", "Reset"] => Some(Action::MachineReset(Vec::new())),
        [target, "Reset"] => {
            let cpu = resolve(target)?;
            Some(Action::CpuReset(
                cpu.strip_prefix("sysbus.").unwrap_or(cpu).into(),
            ))
        }
        [target, "CpuWaitSignal", state @ ("Set" | "Unset")] => {
            let cpu = resolve(target)?;
            Some(Action::CpuWait(
                cpu.strip_prefix("sysbus.").unwrap_or(cpu).into(),
                *state == "Set",
            ))
        }
        ["emulation", "RunFor", duration] => {
            Some(Action::RunFor(duration_steps(resolve(duration)?)?))
        }
        ["sysbus", "Unregister", target] | ["machine" | "mach", "UnregisterFromParent", target] => {
            Some(Action::UnregisterGpio(resolve(target)?.to_owned()))
        }
        [device, "PressAndRelease"] => Some(Action::ButtonPulse(resolve(device)?.to_owned())),
        [device, state @ ("Press" | "Release")] => Some(Action::Button(
            resolve(device)?.to_owned(),
            *state == "Press",
        )),
        [target, "Frequency", value] => Some(Action::Frequency(
            resolve(target)?.to_owned(),
            crate::parse_address(resolve(value)?)?,
        )),
        [target, "Clocked", value] => Some(Action::NvicClocked(
            resolve(target)?.to_owned(),
            resolve(value)?
                .to_ascii_lowercase()
                .parse()
                .map_err(|_| "expected NVIC Clocked true or false")?,
        )),
        [target, "SetPendingIRQ", exception] => Some(Action::NvicPending(
            resolve(target)?.to_owned(),
            crate::parse_address(resolve(exception)?)?,
        )),
        [target, "OnGPIO", line, value] => {
            let line = crate::parse_address(resolve(line)?)?;
            if line >= 32 {
                return Err("expected NVIC IRQ line 0 through 31".into());
            }
            let high = resolve(value)?
                .parse()
                .map_err(|_| "expected NVIC level true or false")?;
            Some(Action::NvicGpio(
                resolve(target)?.to_owned(),
                line as u8,
                high,
            ))
        }
        [channel, "Volts", voltage] => {
            let target = resolve(channel)?;
            let target = target.strip_prefix("sysbus.").unwrap_or(target);
            let (name, channel) = target
                .split_once(".adc-channel")
                .ok_or("expected ADC channel path")?;
            Some(Action::Adc(
                name.to_owned(),
                channel.parse().map_err(|_| "invalid ADC channel number")?,
                resolve(voltage)?
                    .parse()
                    .map_err(|_| "invalid ADC voltage")?,
            ))
        }
        _ => None,
    };
    if let Some(action) = action {
        if boot.platform.is_none() || (boot.image.is_none() && boot.binaries.is_empty()) {
            return Err("runtime commands require a loaded platform and firmware".into());
        }
        boot.runtime.push(RuntimeCommand { line, action });
        return Ok(());
    }
    if !boot.runtime.is_empty() {
        return Err(format!(
            "unsupported monitor command after runtime commands: {text:?}"
        ));
    }
    match words.as_slice() {
        ["mach" | "machine", "create", name @ ..] if name.len() <= 1 => {
            if let Some(name) = name.first() {
                resolve(*name)?;
            }
            if std::mem::replace(machine, true) {
                return Err("multiple machines are unsupported".into());
            }
        }
        ["machine", "LoadPlatformDescription", value] if *machine && boot.platform.is_none() => {
            let file = resolve(value)?
                .strip_prefix('@')
                .ok_or("expected @platform-path")?;
            boot.platform = Some(local_path(file, script, root));
        }
        ["sysbus", command @ ("LoadELF" | "LoadHEX"), value]
            if boot.platform.is_some() && boot.image.is_none() =>
        {
            let file = resolve(value)?
                .strip_prefix('@')
                .ok_or("expected @firmware-path")?;
            let format = if *command == "LoadHEX" {
                ImageFormat::Hex
            } else {
                ImageFormat::Elf
            };
            boot.image = Some((file.to_string(), format));
        }
        ["sysbus", "LoadBinary", value, address] if boot.platform.is_some() => {
            let file = resolve(value)?
                .strip_prefix('@')
                .ok_or("expected @firmware-path")?;
            boot.binaries
                .push((file.to_string(), crate::parse_address(resolve(address)?)?));
        }
        [cpu, "PC", value] if boot.platform.is_some() => {
            boot.boot_pcs.push((
                cpu.strip_prefix("sysbus.").unwrap_or(cpu).to_string(),
                crate::parse_address(resolve(value)?)?,
            ));
        }
        [cpu, "VectorTableOffset", value] if boot.platform.is_some() => {
            boot.vector_tables.push((
                cpu.strip_prefix("sysbus.").unwrap_or(cpu).to_string(),
                crate::parse_address(resolve(value)?)?,
            ));
        }
        ["cpu" | "sysbus.cpu", "MTVEC", value] if boot.platform.is_some() => {
            boot.mtvec = Some(crate::parse_address(resolve(value)?)?);
        }
        ["cpu" | "sysbus.cpu", "SetMachineIrqMask", value] if boot.platform.is_some() => {
            boot.mask = Some(crate::parse_address(resolve(value)?)?);
        }
        ["showAnalyzer", "sysbus.uart" | "uart"] if boot.platform.is_some() => {
            // UART output is already connected to stdout.
        }
        _ => return Err(format!("unsupported monitor command {text:?}")),
    }
    Ok(())
}

fn parse_cpu_hook(body: &str) -> Result<CpuHook, String> {
    let body = body
        .strip_prefix('"')
        .and_then(|body| body.strip_suffix('"'))
        .ok_or("expected quoted AddHook body")?;
    if body == "machine.PauseAndRequestEmulationPause()" {
        return Ok(CpuHook::Pause);
    }
    if let Some(arguments) = body
        .strip_prefix("machine.SystemBus.WriteDoubleWord(")
        .and_then(|body| body.strip_suffix(')'))
    {
        let (address, value) = arguments
            .split_once(',')
            .ok_or("expected hook write address and value")?;
        return Ok(CpuHook::WriteDoubleWord(
            crate::parse_address(address.trim())?,
            crate::parse_address(value.trim())?,
        ));
    }
    let log = body
        .strip_prefix("monitor.Parse('log ")
        .and_then(|body| body.strip_suffix("')"))
        .ok_or("unsupported AddHook body")?
        .replace("\\\"", "\"");
    let (message, "") = quoted_argument(&log).ok_or("expected quoted AddHook log message")? else {
        return Err("expected one AddHook log message".into());
    };
    Ok(CpuHook::Log(message.to_owned()))
}

fn parse_tester_bool(value: &str) -> Result<bool, String> {
    value
        .to_ascii_lowercase()
        .parse()
        .map_err(|_| format!("expected true or false, got {value:?}"))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TesterOptionsKind {
    Line,
    Bytes,
    Failure,
}

struct TesterWaitOptions {
    id: Option<usize>,
    timeout: u64,
    match_start: bool,
    regex: bool,
}

fn tester_wait_options<'a>(
    options: &'a str,
    resolve: impl Fn(&'a str) -> Result<&'a str, String>,
    kind: TesterOptionsKind,
) -> Result<TesterWaitOptions, String> {
    let mut id = None;
    let mut timeout = None;
    let mut match_start = None;
    let mut regex = None;
    for option in options.split_whitespace() {
        match option.split_once('=') {
            Some(("testerId", value)) if id.is_none() => {
                id = Some(crate::parse_address(resolve(value)?)? as usize);
            }
            Some(("timeout", value)) if kind != TesterOptionsKind::Failure && timeout.is_none() => {
                timeout = Some(duration_steps(resolve(value)?)?);
            }
            Some(("matchStart", value))
                if kind == TesterOptionsKind::Bytes && match_start.is_none() =>
            {
                match_start = Some(parse_tester_bool(resolve(value)?)?);
            }
            Some(("treatAsRegex", value)) if kind != TesterOptionsKind::Line && regex.is_none() => {
                regex = Some(parse_tester_bool(resolve(value)?)?);
            }
            _ => {
                return Err(format!(
                    "unsupported or duplicate UART tester option {option:?}"
                ));
            }
        }
    }
    Ok(TesterWaitOptions {
        id,
        timeout: timeout.unwrap_or(8 * CORTEX_M_STEP_HZ),
        match_start: match_start.unwrap_or(false),
        regex: regex.unwrap_or(false),
    })
}

fn byte_list<'a>(
    args: &'a str,
    resolve: impl Fn(&'a str) -> Result<&'a str, String>,
) -> Result<(Vec<u8>, &'a str), String> {
    let (list, remainder) = args
        .strip_prefix('[')
        .and_then(|args| args.split_once(']'))
        .ok_or("expected [byte, ...]")?;
    let bytes = if list.trim().is_empty() {
        Vec::new()
    } else {
        list.split(',')
            .map(|number| {
                let value = crate::parse_address(resolve(number.trim())?)?;
                u8::try_from(value).map_err(|_| format!("byte value out of range: {value}"))
            })
            .collect::<Result<Vec<_>, String>>()?
    };
    Ok((bytes, remainder.trim()))
}

fn hex_pattern(value: &str) -> Result<Vec<u8>, String> {
    let hex: String = value
        .chars()
        .filter(|char| !matches!(char, ' ' | '\r' | '\n'))
        .collect();
    if !hex.is_ascii() || hex.len() % 2 != 0 {
        return Err("FindBytes requires complete hexadecimal bytes".into());
    }
    (0..hex.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&hex[index..index + 2], 16)
                .map_err(|_| "FindBytes requires complete hexadecimal bytes".into())
        })
        .collect()
}

fn quoted_argument(value: &str) -> Option<(&str, &str)> {
    let (text, rest) = value.strip_prefix('"')?.split_once('"')?;
    (rest.is_empty() || rest.starts_with(char::is_whitespace)).then(|| (text, rest.trim()))
}

fn duration_steps(value: &str) -> Result<u64, String> {
    let seconds = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .or_else(|| {
            value
                .strip_prefix('\'')
                .and_then(|value| value.strip_suffix('\''))
        })
        .unwrap_or(value);
    let seconds = seconds.strip_suffix('s').unwrap_or(seconds);
    let (whole, fraction) = seconds.split_once('.').unwrap_or((seconds, ""));
    let invalid =
        || format!("expected nonnegative decimal seconds at 10 ns resolution, got {value:?}");
    let whole: u64 = whole.parse().map_err(|_| invalid())?;
    let fraction = fraction.trim_end_matches('0');
    if fraction.len() > 8 || !fraction.bytes().all(|digit| digit.is_ascii_digit()) {
        return Err(invalid());
    }
    let ticks = fraction.parse::<u64>().unwrap_or(0)
        * (CORTEX_M_STEP_HZ / 10_u64.pow(fraction.len() as u32));
    whole
        .checked_mul(CORTEX_M_STEP_HZ)
        .and_then(|whole| whole.checked_add(ticks))
        .ok_or_else(invalid)
}

pub fn asset_path(
    value: &str,
    script: &Path,
    root: &Path,
    assets: Option<&Path>,
) -> Result<PathBuf, String> {
    if let Some((_, url)) = value.split_once("://") {
        let name = url
            .rsplit('/')
            .next()
            .filter(|name| !name.is_empty() && *name != "." && *name != "..")
            .ok_or_else(|| format!("invalid firmware URL {value}"))?;
        let assets = assets.ok_or_else(|| {
            format!("script references {value}; use --asset-dir with local assets")
        })?;
        return Ok(assets.join(name));
    }
    Ok(local_path(value, script, root))
}

pub fn local_path(value: &str, script: &Path, root: &Path) -> PathBuf {
    let local = script.parent().unwrap_or(Path::new(".")).join(value);
    if local.is_file() {
        local
    } else {
        root.join(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renode_button_press_and_release_latches_exti_between_steps() {
        use crate::{
            stm32_gpio::Stm32Gpio,
            stm32_gpio_board::{GpioButton, GpioEndpoints},
        };

        // nucleo_wba52cg.robot: Should have Working EXTI (GPIO/EXTI portion).
        const GPIO: u32 = 0x4800_0000;
        const EXTI: u32 = 0x4001_0400;
        const ISER: u32 = 0xe000_e100;
        let mut machine = Machine::without_default_ram();
        machine.bus.map_exti(EXTI, vec![Some(5)], 32).unwrap();
        machine.bus.map_nvic(0xc0, None).unwrap();
        let mut gpio = Stm32Gpio::new(GPIO, 0x400, [0; 3], 8);
        gpio.exti_routes[0] = 1;
        machine.bus.map_stm32_gpio(gpio).unwrap();
        let mut endpoints = GpioEndpoints::default();
        endpoints.connect(GPIO, "gpioPortA", 0, 16).unwrap();
        machine.bus.add_gpio_button(GpioButton::new(
            "button".into(),
            Some("gpioPortA".into()),
            endpoints,
            false,
        ));
        let mut output = io::sink();
        for (address, value) in [(EXTI, 1), (ISER, 1 << 5)] {
            machine.bus.write(address, 4, value, &mut output).unwrap();
        }

        let mut boot = Boot {
            platform: Some("board.repl".into()),
            image: Some(("test.hex".into(), ImageFormat::Hex)),
            ..Boot::default()
        };
        let script = Path::new("test.resc");
        execute(
            "sysbus.gpioPortA.button PressAndRelease",
            &HashMap::new(),
            &mut true,
            &mut boot,
            script,
            Path::new("."),
            1,
        )
        .unwrap();

        for (rising, falling) in [(1, 0), (0, 1)] {
            machine.bus.write(EXTI + 8, 4, rising, &mut output).unwrap();
            machine
                .bus
                .write(EXTI + 12, 4, falling, &mut output)
                .unwrap();
            for _ in 0..2 {
                assert_eq!(
                    run(&mut machine, script, &boot.runtime, 0, &mut output).unwrap(),
                    (Stop::ScriptComplete, 0)
                );
                assert_eq!(machine.bus.read::<4>(GPIO + 0x10).unwrap() & 1, 0);
                assert_eq!(machine.bus.read::<4>(EXTI + 20).unwrap(), 1);
                machine.bus.sample_nvic(0);
                assert_eq!(machine.bus.nvic.as_ref().unwrap().next_irq(0), Some((5, 0)));
                machine.bus.nvic.as_mut().unwrap().acknowledge(5);
                machine.bus.write(EXTI + 20, 4, 1, &mut output).unwrap();
                machine.bus.sample_nvic(0);
                assert_eq!(machine.bus.nvic.as_ref().unwrap().next_irq(0), None);
            }
        }
    }

    #[test]
    fn renode_resc_set_pending_irq_sets_nmi() {
        use crate::systick::ICSR;

        let mut machine = Machine::without_default_ram();
        machine.bus.map_nvic(0xc0, None).unwrap();
        machine.bus.set_nvic_name("nvic".into());
        machine.bus.map_systick().unwrap();
        let mut boot = Boot {
            platform: Some("board.repl".into()),
            image: Some(("test.hex".into(), ImageFormat::Hex)),
            ..Boot::default()
        };
        let script = Path::new("test.resc");
        execute(
            "nvic SetPendingIRQ 2",
            &HashMap::new(),
            &mut true,
            &mut boot,
            script,
            Path::new("."),
            1,
        )
        .unwrap();
        run(&mut machine, script, &boot.runtime, 0, &mut io::sink()).unwrap();
        assert_eq!(machine.bus.read::<4>(ICSR).unwrap(), 1 << 31);
        assert!(machine.bus.pend_nvic_exception("other", 2).is_err());
        assert!(machine.bus.pend_nvic_exception("nvic", 4).is_err());
    }

    #[test]
    fn led_wait_times_out_and_rejects_a_change_during_hold() {
        use crate::{stm32_gpio::Stm32Gpio, stm32_gpio_board::GpioLed};

        let mut machine = Machine::without_default_ram();
        let gpio = 0x4800_0000;
        machine.bus.map_ram(0, 0x100).unwrap();
        machine
            .bus
            .map_stm32_gpio(Stm32Gpio::new(gpio, 0x400, [1, 0, 0], 8))
            .unwrap();
        machine
            .bus
            .add_gpio_led(gpio, GpioLed::new("led".into(), 0, false));
        let mut output = io::sink();
        let mut budget = 2;
        let error =
            wait_for_led(&mut machine, "led", true, 0, 0, &mut budget, &mut output).unwrap_err();
        assert!(error.contains("did not reach state true"));
        assert_eq!(budget, 2);

        machine.bus.write(gpio + 0x18, 4, 1, &mut output).unwrap(); // set pin
        machine.bus.write(0, 2, 0x6001, &mut output).unwrap(); // str r1, [r0]
        machine.primary.configure_cortex_m(Some(0));
        machine.primary.pc = 0;
        machine.primary.registers[..2].copy_from_slice(&[gpio + 0x28, 1]); // clear pin
        let error =
            wait_for_led(&mut machine, "led", true, 0, 2, &mut budget, &mut output).unwrap_err();
        assert!(error.contains("changed while holding state true"));
        assert_eq!(machine.elapsed_steps(), 1);
    }

    #[test]
    fn parses_renode_uart_write_line_endings() {
        let mut boot = Boot {
            platform: Some("board.repl".into()),
            image: Some(("firmware.elf".into(), ImageFormat::Elf)),
            ..Boot::default()
        };
        let mut machine = true;
        let script = Path::new("test.resc");
        let root = Path::new(".");
        let variables = HashMap::new();
        for (suffix, expected) in [
            ("", b"\r".as_slice()),
            (" CR", b"\r"),
            (" LF", b"\n"),
            (" CRLF", b"\r\n"),
            (" None", b""),
            (" true", b"\r"),
            (" false", b""),
        ] {
            let command = format!("sysbus.usart1 WriteLine \"with spaces\"{suffix}");
            execute(
                &command,
                &variables,
                &mut machine,
                &mut boot,
                script,
                root,
                1,
            )
            .unwrap();
            let command = boot.runtime.pop().unwrap();
            assert!(matches!(command.action,
                Action::WriteUartLine(target, line, ending)
                if target == "sysbus.usart1" && line == "with spaces" && ending == expected
            ));
        }
        assert!(
            execute(
                "sysbus.usart1 WriteLine \"x\" invalid",
                &variables,
                &mut machine,
                &mut boot,
                script,
                root,
                1
            )
            .is_err()
        );
    }

    #[test]
    fn uart_match_requires_a_completed_line_and_never_spans_lines() {
        let mut output = Vec::new();
        let mut history = UartHistory::default();
        history.begin_wait("ADC reading: 745", UartWaitMode::Lf);
        let mut uart = ScriptOutput {
            output: &mut output,
            history: Some(history),
        };
        uart.write_all(b"ADC reading: \n745\r\n").unwrap();
        assert!(!uart.history.as_ref().unwrap().matched());
        uart.write_all(b"ADC reading: 7").unwrap();
        uart.write_all(b"45\r").unwrap();
        assert!(!uart.history.as_ref().unwrap().matched());
        uart.write_all(b"\n").unwrap();
        assert!(uart.history.as_ref().unwrap().matched());
        assert_eq!(output, b"ADC reading: \n745\r\nADC reading: 745\r\n");
    }

    #[test]
    fn parses_binary_addresses_and_named_cpu_entries() {
        let mut boot = Boot::default();
        let mut created = true;
        let path = Path::new("test.resc");
        let root = Path::new(".");
        let vars = HashMap::new();
        for (line, command) in [
            "machine LoadPlatformDescription @board.repl",
            "sysbus LoadBinary @https://example.org/firmware.bin 0x40000000",
            "sysbus.cpu_1 PC 0x40000004",
        ]
        .into_iter()
        .enumerate()
        {
            execute(
                command,
                &vars,
                &mut created,
                &mut boot,
                path,
                root,
                line + 1,
            )
            .unwrap();
        }
        assert_eq!(
            boot.binaries,
            [("https://example.org/firmware.bin".into(), 0x4000_0000)]
        );
        assert_eq!(boot.boot_pcs, [("cpu_1".into(), 0x4000_0004)]);
        assert_eq!(
            asset_path(&boot.binaries[0].0, path, root, Some(Path::new("assets"))).unwrap(),
            Path::new("assets/firmware.bin")
        );
    }

    #[test]
    fn reads_boot_commands_and_reports_macro_source_lines() {
        let directory = std::env::temp_dir().join(format!("penv-resc-{}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        let script = directory.join("boot.resc");
        let content = concat!(
            "$name?=\"Test\"\n",
            "$bin?=@firmware.elf\n",
            "mach create $name\n",
            "machine LoadPlatformDescription @board.repl\n",
            "macro reset\n",
            "\"\"\"\n",
            "  sysbus LoadELF $bin\n",
            "  sysbus.cpu MTVEC 0x80000020\n",
            "\"\"\"\n",
            "runMacro $reset\n",
            "cpu SetMachineIrqMask 0xffffffff\n",
        );
        fs::write(&script, content).unwrap();
        let boot = load(&script, &directory).unwrap();
        assert_eq!(boot.platform, Some(directory.join("board.repl")));
        assert_eq!(boot.image, Some(("firmware.elf".into(), ImageFormat::Elf)));
        assert_eq!(boot.mtvec, Some(0x8000_0020));
        assert_eq!(boot.mask, Some(u32::MAX));
        fs::write(&script, content.replace("MTVEC", "unknown")).unwrap();
        let result = load(&script, &directory);
        fs::remove_dir_all(directory).unwrap();
        assert!(
            matches!(result, Err(error) if error.contains("boot.resc:8: unsupported monitor command"))
        );
    }
}
