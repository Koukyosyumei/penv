use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{self, Write},
    net::Ipv4Addr,
    path::{Path, PathBuf},
};

use crate::{
    bus::CORTEX_M_STEP_HZ,
    machine::{Machine, Stop},
    uart::{UartHistory, UartWaitMode},
};

#[derive(Default)]
pub struct Boot {
    pub binaries: Vec<(Asset, u32)>,
    pub network: Network,
    pub video_analyzers: Vec<String>,
    pub pcm_inputs: Vec<(String, Asset)>,
    pub pcm_outputs: Vec<(String, PathBuf)>,
    pub run_steps: Option<u64>,
    pub post_run: Vec<PostRun>,
    pub buttons: Vec<(String, crate::litex_panel::ButtonAction)>,
    pub silence_ranges: Vec<(u32, u32)>,
    pub boot_pcs: Vec<(String, u32)>,
    pub platform: Option<PathBuf>,
    pub image: Option<Asset>,
    pub hex: bool,
    pub vector_tables: Vec<(String, u32)>,
    pub runtime: Vec<RuntimeCommand>,
    pub use_virtual_elf_addresses: bool,
    pub mtvec: Option<u32>,
    pub mask: Option<u32>,
    pub supervisor_mask: Option<u32>,
    pub mpfs_buttons: Vec<(String, String, bool)>,
    pub halt_hooks: Vec<(String, String, String, String)>,
    symbols: Vec<String>,
    hooks: HashMap<String, String>,
    pub sd_cards: Vec<(Asset, String, u64)>,
    started: bool,
}

pub struct RuntimeCommand {
    line: usize,
    source: PathBuf,
    action: Action,
}

enum Action {
    RunFor(u64),
    Button(String, bool),
    ButtonPulse(String),
    NvicGpio(String, u8, bool),
    NvicClocked(String, bool),
    WaitForLed(String, bool, u64, u64),
    Adc(String, usize, f64),
    WaitForUart(Option<String>, String, UartWaitMode),
    WriteUartLine(String, String, &'static [u8]),
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
    _script: &Path,
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
            Action::RunFor(steps) if machine.is_cortex_m() => {
                let chunk = (*steps).min(budget);
                machine.run(chunk, &mut output).map(|(stop, count)| {
                    retired += count;
                    budget -= chunk;
                    (stop != Stop::Limit || chunk < *steps).then_some(stop)
                })
            }
            Action::RunFor(_) => Err("RunFor requires a Cortex-M platform".into()),
            Action::Button(name, pressed) => {
                machine.bus.set_named_button(name, *pressed).map(|()| None)
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
            Action::NvicClocked(target, clocked) => machine
                .bus
                .set_nvic_clocked(target, *clocked)
                .map(|()| None),
            Action::Adc(name, channel, volts) => machine
                .bus
                .set_named_adc_input(name, *channel, *volts)
                .map(|()| None),
            Action::WaitForUart(target, pattern, end) => {
                let start = machine.elapsed_steps();
                wait_for_uart(
                    machine,
                    target.as_deref(),
                    pattern,
                    *end,
                    budget,
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
            result.map_err(|e| format!("{}:{}: {e}", command.source.display(), command.line))?
        {
            return Ok((stop, retired));
        }
    }
    Ok((Stop::ScriptComplete, retired))
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

fn wait_for_uart(
    machine: &mut Machine,
    target: Option<&str>,
    pattern: &str,
    end: UartWaitMode,
    budget: u64,
    output: &mut ScriptOutput<'_, impl Write>,
) -> Result<(Stop, u64), String> {
    if let Some(target) = target {
        let index = machine.bus.watch_stm32_uart(target, pattern, end)?;
        let result = if machine.bus.stm32_uart_matched(index) {
            Ok((Stop::OutputMatch, 0))
        } else {
            machine.run_until(budget, output, |bus, _| bus.stm32_uart_matched(index))
        };
        machine.bus.finish_stm32_uart_watch(index);
        result
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

pub enum PostRun {
    Echo(String),
    DisposePcm(String),
}

#[derive(Debug)]
pub struct Asset {
    value: String,
    script: PathBuf,
}

#[derive(Default)]
pub struct Network {
    switch: Option<String>,
    server: Option<(String, Ipv4Addr)>,
    links: Vec<String>,
    port: Option<u16>,
    file: Option<(Asset, String)>,
}

pub struct TftpBoot {
    pub ethernet: String,
    pub ip: Ipv4Addr,
    pub port: u16,
    pub file: Asset,
    pub filename: String,
}

impl Network {
    pub fn into_tftp(self) -> Result<Option<TftpBoot>, String> {
        if self.switch.is_none() && self.server.is_none() {
            return Ok(None);
        }
        let Some((server, ip)) = self.server else {
            return Err("expected a local network server".into());
        };
        if !self.links.contains(&server) {
            return Err("expected the local network server to be connected to the switch".into());
        }
        let port = self
            .port
            .ok_or("expected StartTFTP for the local network server")?;
        let (file, filename) = self.file.ok_or("expected a file served over local TFTP")?;
        let mut peers = self.links.into_iter().filter(|name| *name != server);
        let ethernet = peers
            .next()
            .ok_or("expected an Ethernet connection to the local switch")?;
        if peers.next().is_some() {
            return Err("only one Ethernet connection to the local switch is supported".into());
        }
        Ok(Some(TftpBoot {
            ethernet,
            ip,
            port,
            file,
            filename,
        }))
    }

    fn is_server(&self, name: &str) -> bool {
        self.server
            .as_ref()
            .is_some_and(|(server, _)| server == name)
    }
}

fn unquote(text: &str) -> &str {
    text.strip_prefix('"')
        .and_then(|text| text.strip_suffix('"'))
        .unwrap_or(text)
}

struct Macro {
    script: PathBuf,
    commands: Vec<(usize, String)>,
}

#[derive(Default)]
struct ScriptState {
    variables: HashMap<String, String>,
    macros: HashMap<String, Macro>,
    visiting: HashSet<PathBuf>,
    boot: Boot,
    machine: bool,
}

pub fn load(path: &Path, root: &Path) -> Result<Boot, String> {
    let mut state = ScriptState::default();
    load_file(path, root, &mut state)?;
    let mut boot = state.boot;
    if let [
        RuntimeCommand {
            action: Action::RunFor(steps),
            ..
        },
    ] = boot.runtime.as_slice()
    {
        boot.run_steps = Some(*steps);
        boot.runtime.clear();
    } else if !boot.post_run.is_empty() {
        return Err("post-run operations require a single RunFor".into());
    }
    if !state.machine
        || boot.platform.is_none()
        || (boot.image.is_none() && boot.binaries.is_empty())
    {
        return Err(format!(
            "{}: expected machine creation, platform and firmware loading",
            path.display()
        ));
    }
    Ok(boot)
}

fn load_file(path: &Path, root: &Path, state: &mut ScriptState) -> Result<(), String> {
    let canonical = fs::canonicalize(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if state.visiting.len() >= 64 || !state.visiting.insert(canonical.clone()) {
        return Err(format!(
            "{}: recursive or excessively deep script include",
            path.display()
        ));
    }
    let content = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut cursor = content.lines().enumerate();
    while let Some((line, raw)) = cursor.next() {
        let text = raw.trim();
        let error = |message: &str| format!("{}:{}: {message}", path.display(), line + 1);
        if text.is_empty() || text.starts_with('#') || text.starts_with(':') {
            continue;
        }
        if let Some(name) = text
            .strip_prefix("set ")
            .filter(|name| !name.contains(char::is_whitespace))
        {
            if cursor
                .next()
                .is_none_or(|(_, line)| line.trim() != "\"\"\"")
            {
                return Err(error("expected triple-quoted hook body"));
            }
            let mut body = Vec::new();
            loop {
                let (_, command) = cursor.next().ok_or_else(|| error("unterminated hook"))?;
                let command = command.trim();
                if command == "\"\"\"" {
                    break;
                }
                if !command.is_empty() && !command.starts_with('#') {
                    body.push(command);
                }
            }
            let [assign, halt, ..] = body.as_slice() else {
                return Err(error("unsupported hook body"));
            };
            let (variable, hart) = assign
                .split_once(" = machine['sysbus.")
                .and_then(|(variable, suffix)| Some((variable, suffix.strip_suffix("']")?)))
                .ok_or_else(|| error("unsupported hook machine lookup"))?;
            if variable.is_empty()
                || hart.is_empty()
                || *halt != format!("{variable}.IsHalted = True")
                || !matches!(
                    body.get(2..),
                    Some([] | ["print(\"halted management CPU\")"])
                )
            {
                return Err(error("unsupported hook body"));
            }
            state.boot.hooks.insert(name.to_string(), hart.to_string());
            continue;
        }
        if let Some(name) = text.strip_prefix("macro ") {
            let name = name.trim();
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
                body.push((index, command.trim().to_string()));
            }
            state.macros.insert(
                name.into(),
                Macro {
                    script: path.into(),
                    commands: body,
                },
            );
            continue;
        }
        if let Some(assignment) = text.strip_prefix('$') {
            let (name, value) = assignment
                .split_once('=')
                .ok_or_else(|| error("expected variable assignment"))?;
            let name = name.trim();
            if let Some(name) = name.strip_suffix('?') {
                state
                    .variables
                    .entry(name.trim().into())
                    .or_insert_with(|| value.trim().into());
            } else {
                state.variables.insert(name.into(), value.trim().into());
            }
            continue;
        }
        let words = command_words(text).map_err(|e| error(&e))?;
        match words.as_slice() {
            ["set", name, value] if !state.boot.started => {
                let name = name.strip_prefix('$').unwrap_or(name);
                state.variables.insert(name.into(), (*value).into());
            }
            ["include" | "i", value] => {
                if state.boot.started {
                    return Err(error(
                        "commands after start are unsupported in a boot script",
                    ));
                }
                let file = resolve(&state.variables, value)
                    .map_err(|e| error(&e))?
                    .strip_prefix('@')
                    .ok_or_else(|| error("expected @script-path"))?;
                let included = local_path(&path_value(file), path, root);
                load_file(&included, root, state).map_err(|e| error(&e))?;
            }
            ["runMacro", name] if name.starts_with('$') => {
                let body = state
                    .macros
                    .get(&name[1..])
                    .ok_or_else(|| error("unknown macro"))?;
                for (index, command) in &body.commands {
                    let words = command_words(command)
                        .map_err(|e| format!("{}:{}: {e}", body.script.display(), index + 1))?;
                    execute(
                        command,
                        &words,
                        &state.variables,
                        &mut state.machine,
                        &mut state.boot,
                        &body.script,
                        root,
                        index + 1,
                    )
                    .map_err(|e| format!("{}:{}: {e}", body.script.display(), index + 1))?;
                }
            }
            _ => execute(
                text,
                &words,
                &state.variables,
                &mut state.machine,
                &mut state.boot,
                path,
                root,
                line + 1,
            )
            .map_err(|e| error(&e))?,
        }
    }
    state.visiting.remove(&canonical);
    Ok(())
}

fn resolve<'a>(variables: &'a HashMap<String, String>, value: &'a str) -> Result<&'a str, String> {
    match value.strip_prefix('$') {
        Some("") => Err("missing variable name".into()),
        Some(name) => variables
            .get(name)
            .map(String::as_str)
            .ok_or_else(|| format!("unknown variable {name}")),
        None => Ok(value),
    }
}

fn execute(
    text: &str,
    words: &[&str],
    variables: &HashMap<String, String>,
    machine: &mut bool,
    boot: &mut Boot,
    script: &Path,
    root: &Path,
    line: usize,
) -> Result<(), String> {
    if matches!(words, [] | ["using", "sysbus"]) {
        return Ok(());
    }
    let resolve = |value| resolve(variables, value);
    let action = match words {
        [command @ ("waitForUart" | "waitForUartPrompt"), ..] => {
            let args = text[command.len()..].trim();
            let expected = format!("expected {command} [USART] \"literal text\"");
            let (target, line) = if args.starts_with('"') {
                (None, args)
            } else {
                let (target, line) = args
                    .split_once(char::is_whitespace)
                    .ok_or_else(|| expected.clone())?;
                (Some(resolve(target)?.to_owned()), line.trim_start())
            };
            let (pattern, ending) = quoted_argument(line)
                .filter(|(pattern, _)| !pattern.is_empty())
                .ok_or(expected)?;
            let end = match (*command, ending) {
                ("waitForUartPrompt", "") => UartWaitMode::Prompt,
                ("waitForUartPrompt", _) => {
                    return Err("unexpected argument after UART prompt".into());
                }
                (_, "" | "LF") => UartWaitMode::Lf,
                (_, "CR") => UartWaitMode::Cr,
                _ => return Err("expected waitForUart ending LF or CR".into()),
            };
            Some(Action::WaitForUart(target, pattern.to_owned(), end))
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
        ["emulation", "RunFor", duration] => {
            Some(Action::RunFor(duration_steps(resolve(duration)?)?))
        }
        [device, "PressAndRelease"] => Some(Action::ButtonPulse(resolve(device)?.to_owned())),
        [device, state @ ("Press" | "Release")] => Some(Action::Button(
            resolve(device)?.to_owned(),
            *state == "Press",
        )),
        [target, "Clocked", value] => Some(Action::NvicClocked(
            resolve(target)?.to_owned(),
            resolve(value)?
                .to_ascii_lowercase()
                .parse()
                .map_err(|_| "expected NVIC Clocked true or false")?,
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
        boot.runtime.push(RuntimeCommand {
            line,
            source: script.to_owned(),
            action,
        });
        return Ok(());
    }
    if boot.started || !boot.runtime.is_empty() {
        if matches!(
            boot.runtime.last().map(|entry| &entry.action),
            Some(Action::RunFor(_))
        ) {
            match words {
                ["echo", value] => {
                    let value = unquote(resolve(value)?);
                    boot.post_run.push(PostRun::Echo(path_value(
                        value.strip_prefix('@').unwrap_or(value),
                    )));
                    return Ok(());
                }
                [device, "Dispose"] => {
                    let name = device.strip_prefix("sysbus.").unwrap_or(device);
                    boot.post_run.push(PostRun::DisposePcm(name.into()));
                    return Ok(());
                }
                _ => {}
            }
        }
        return Err("commands after start are unsupported in a boot script".into());
    }
    match words {
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
            boot.platform = Some(local_path(&path_value(file), script, root));
        }
        ["machine", "SdCardFromFile", value, controller, capacity]
        | [
            "machine",
            "SdCardFromFile",
            value,
            controller,
            capacity,
            "False" | "false",
        ] if boot.platform.is_some() => {
            let file = resolve(value)?
                .strip_prefix('@')
                .ok_or("expected @SD-image-path")?;
            let controller = controller.strip_prefix("sysbus.").unwrap_or(controller);
            let capacity = crate::parse_address64(resolve(capacity)?)?;
            boot.sd_cards
                .push((Asset::new(file, script), controller.to_string(), capacity));
        }
        ["sysbus", "LoadELF", value, options @ ..]
            if boot.platform.is_some() && boot.image.is_none() && options.len() <= 1 =>
        {
            boot.use_virtual_elf_addresses = match options {
                [] | ["false" | "False"] => false,
                ["true" | "True"] => true,
                _ => return Err("expected a boolean LoadELF address option".into()),
            };
            let file = resolve(value)?
                .strip_prefix('@')
                .ok_or("expected @firmware-path")?;
            boot.image = Some(Asset::new(file, script));
        }
        ["sysbus", "LoadHEX", value] if boot.platform.is_some() && boot.image.is_none() => {
            let file = resolve(value)?
                .strip_prefix('@')
                .ok_or("expected @firmware-path")?;
            boot.image = Some(Asset::new(file, script));
            boot.hex = true;
        }
        [cpu, "VectorTableOffset", value] if boot.platform.is_some() => {
            boot.vector_tables.push((
                cpu.strip_prefix("sysbus.").unwrap_or(cpu).to_string(),
                crate::parse_address(resolve(value)?)?,
            ));
        }
        ["sysbus", "LoadBinary" | "LoadFdt", value, address] if boot.platform.is_some() => {
            let file = resolve(value)?
                .strip_prefix('@')
                .ok_or("expected @firmware-path")?;
            boot.binaries.push((
                Asset::new(file, script),
                crate::parse_address(resolve(address)?)?,
            ));
        }
        [device, "LoadPCM" | "Output", value] if boot.platform.is_some() => {
            let device = device.strip_prefix("sysbus.").unwrap_or(device).to_string();
            let path = resolve(value)?
                .strip_prefix('@')
                .ok_or("expected @PCM-path")?;
            if words[1] == "LoadPCM" {
                boot.pcm_inputs.push((device, Asset::new(path, script)));
            } else {
                boot.pcm_outputs.push((device, root.join(path_value(path))));
            }
        }
        ["emulation", "CreateSwitch", name] if *machine && boot.network.switch.is_none() => {
            boot.network.switch = Some(unquote(resolve(name)?).into());
        }
        ["emulation", "CreateNetworkServer", name, ip]
            if *machine && boot.network.server.is_none() =>
        {
            let name = unquote(resolve(name)?).to_string();
            let ip = unquote(resolve(ip)?)
                .parse::<Ipv4Addr>()
                .map_err(|_| "expected IPv4 network server address")?;
            boot.network.server = Some((name, ip));
        }
        ["connector", "Connect", device, switch] if *machine => {
            let switch = unquote(resolve(switch)?);
            let device = unquote(resolve(device)?);
            if boot.network.switch.as_deref() != Some(switch)
                || boot.network.links.iter().any(|name| name == device)
            {
                return Err("invalid or duplicate local switch connection".into());
            }
            boot.network.links.push(device.into());
        }
        [name, "StartTFTP", port]
            if boot.network.is_server(name) && boot.network.port.is_none() =>
        {
            boot.network.port = Some(
                resolve(port)?
                    .parse::<u16>()
                    .ok()
                    .filter(|&port| port != 0)
                    .ok_or("invalid TFTP port")?,
            );
        }
        [module, "ServeFile", file, name]
            if module
                .strip_suffix(".tftp")
                .is_some_and(|name| boot.network.is_server(name))
                && boot.network.port.is_some()
                && boot.network.file.is_none() =>
        {
            let file = resolve(file)?
                .strip_prefix('@')
                .ok_or("expected @TFTP-file-path")?;
            let name = unquote(resolve(name)?);
            boot.network.file = Some((Asset::new(file, script), name.into()));
        }
        ["sysbus", "SilenceRange", start, size] if boot.platform.is_some() => {
            let start = start
                .strip_prefix('<')
                .ok_or("expected SilenceRange <start size>")?;
            let size = size
                .strip_suffix('>')
                .ok_or("expected SilenceRange <start size>")?;
            boot.silence_ranges
                .push((crate::parse_address(start)?, crate::parse_address(size)?));
        }
        ["sysbus", "LoadSymbolsFrom", value] if boot.platform.is_some() => {
            let file = resolve(value)?
                .strip_prefix('@')
                .ok_or("expected @symbol-path")?;
            boot.symbols.push(file.to_string());
        }
        ["emulation", "SetGlobalQuantum", value] => {
            let seconds: f64 = resolve(value)?
                .trim_matches('"')
                .parse()
                .map_err(|_| "invalid emulation quantum")?;
            if !seconds.is_finite() || seconds <= 0.0 {
                return Err("invalid emulation quantum".into());
            }
            // Single-machine execution already synchronizes after each instruction.
        }
        [target, "Pressed" | "Released"] if boot.platform.is_some() && target.contains('.') => {
            let (gpio, button) = target.rsplit_once('.').unwrap();
            boot.mpfs_buttons.push((
                gpio.trim_start_matches("sysbus.").to_string(),
                button.to_string(),
                words[1] == "Pressed",
            ));
        }
        [
            cpu,
            "AddHook",
            "`sysbus",
            "GetSymbolAddress",
            symbol,
            callback,
        ] if boot.platform.is_some() => {
            let symbol = symbol
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix("\"`"))
                .ok_or("expected quoted hook symbol")?;
            let hook = callback
                .strip_prefix('$')
                .and_then(|name| boot.hooks.get(name))
                .ok_or("unknown halt hook")?;
            let file = boot
                .symbols
                .last()
                .ok_or("hook requires a loaded symbol file")?;
            boot.halt_hooks.push((
                cpu.to_string(),
                symbol.to_string(),
                hook.clone(),
                file.clone(),
            ));
        }
        ["emulation", "SetSeed", value] => {
            // None of the supported devices consume Renode's random seed.
            crate::parse_address(resolve(value)?)?;
        }
        ["emulation", "SetGlobalSerialExecution", "True"] => {
            // The interpreter already executes harts serially.
        }
        [cpu, "PC", value] if boot.platform.is_some() => {
            boot.boot_pcs.push((
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
        ["cpu" | "sysbus.cpu", "SetSupervisorIrqMask", value] if boot.platform.is_some() => {
            boot.supervisor_mask = Some(crate::parse_address(resolve(value)?)?);
        }
        [button, action @ ("Press" | "Release" | "Toggle")] if boot.platform.is_some() => {
            let action = crate::litex_panel::ButtonAction::parse(action).unwrap();
            boot.buttons.push((button.to_string(), action));
        }
        ["start"]
            if boot.platform.is_some() && (boot.image.is_some() || !boot.binaries.is_empty()) =>
        {
            boot.started = true;
        }
        ["showAnalyzer", "sysbus.uart" | "uart"] if boot.platform.is_some() => {
            // UART output is already connected to stdout.
        }
        ["showAnalyzer", analyzer] if boot.platform.is_some() => {
            if analyzer
                .rsplit('.')
                .next()
                .is_some_and(|name| name.starts_with("uart") || name.starts_with("usart"))
            {
                return Ok(());
            }
            boot.video_analyzers.push(
                analyzer
                    .strip_prefix("sysbus.")
                    .unwrap_or(analyzer)
                    .to_string(),
            );
        }
        ["echo", ..] => {
            // Monitor output is separate from guest UART output.
        }
        _ => return Err(format!("unsupported monitor command {text:?}")),
    }
    Ok(())
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

fn path_value(value: &str) -> String {
    value.replace(r"\ ", " ")
}

fn command_words(text: &str) -> Result<Vec<&str>, String> {
    let mut words = Vec::new();
    let mut start = None;
    let mut quote = None;
    let mut escaped = false;
    for (index, character) in text.char_indices() {
        if std::mem::take(&mut escaped) {
            continue;
        }
        if character == '\\'
            && (quote.is_some()
                || start.is_some_and(|start| text[start..].starts_with('@'))
                    && text[index + 1..].starts_with(' '))
        {
            escaped = true;
            continue;
        }
        if quote.is_none() && character == '#' && start.is_none() {
            return Ok(words);
        }
        if quote.is_none() && character.is_whitespace() {
            if let Some(start) = start.take() {
                words.push(&text[start..index]);
            }
            continue;
        }
        let new_word = start.is_none();
        start.get_or_insert(index);
        if quote == Some(character) {
            quote = None;
        } else if new_word && matches!(character, '"' | '\'') {
            quote = Some(character);
        }
    }
    if quote.is_some() {
        return Err("unterminated quoted argument".into());
    }
    if let Some(start) = start {
        words.push(&text[start..]);
    }
    Ok(words)
}

impl Asset {
    pub(crate) fn new(value: &str, script: &Path) -> Self {
        Self {
            value: path_value(value),
            script: script.into(),
        }
    }

    pub fn resolve(&self, root: &Path, assets: Option<&Path>) -> Result<PathBuf, String> {
        let value = &self.value;
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
        Ok(local_path(value, &self.script, root))
    }
}

fn local_path(value: &str, script: &Path, root: &Path) -> PathBuf {
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
        use crate::{stm32_gpio::Stm32Gpio, stm32_gpio_board::GpioButton};

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
        machine.bus.add_gpio_button(GpioButton::new(
            "button".into(),
            Some("gpioPortA".into()),
            GPIO,
            0,
            false,
        ));
        let mut output = io::sink();
        for (address, value) in [(EXTI, 1), (ISER, 1 << 5)] {
            machine.bus.write(address, 4, value, &mut output).unwrap();
        }

        let mut boot = Boot {
            platform: Some("board.repl".into()),
            image: Some(Asset::new("test.hex", Path::new("test.resc"))),
            ..Boot::default()
        };
        let script = Path::new("test.resc");
        execute(
            "sysbus.gpioPortA.button PressAndRelease",
            &["sysbus.gpioPortA.button", "PressAndRelease"],
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
                machine
                    .bus
                    .sample_nvic(&crate::nvic::ActiveExceptions::default(), false);
                assert_eq!(
                    machine
                        .bus
                        .nvic
                        .as_ref()
                        .unwrap()
                        .next_irq(&crate::nvic::ActiveExceptions::default()),
                    Some((5, 0))
                );
                machine.bus.nvic.as_mut().unwrap().acknowledge(5);
                machine.bus.write(EXTI + 20, 4, 1, &mut output).unwrap();
                machine
                    .bus
                    .sample_nvic(&crate::nvic::ActiveExceptions::default(), false);
                assert_eq!(
                    machine
                        .bus
                        .nvic
                        .as_ref()
                        .unwrap()
                        .next_irq(&crate::nvic::ActiveExceptions::default()),
                    None
                );
            }
        }
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
        machine.primary.configure_cortex_m(Some(0), false);
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
            image: Some(Asset::new("firmware.elf", Path::new("test.resc"))),
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
                &command_words(&command).unwrap(),
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
                &["sysbus.usart1", "WriteLine", "\"x\"", "invalid"],
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
    fn fire_monitor_resolves_button_and_elf_hook() {
        let directory =
            std::env::temp_dir().join(format!("penv-fire-monitor-{}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        let script = directory.join("boot.resc");
        let source = r#"mach create
machine LoadPlatformDescription @beaglev-fire.repl
emulation SetGlobalQuantum "0.0008"
sysbus LoadSymbolsFrom @u-boot.elf
gpio0.UserButton Pressed
set hook
"""
e51 = machine['sysbus.e51']
e51.IsHalted = True
print("halted management CPU")
"""
u54_4 AddHook `sysbus GetSymbolAddress "_start"` $hook
sysbus LoadELF @hss.elf
"#;
        fs::write(&script, source).unwrap();
        let boot = load(&script, &directory).unwrap();
        assert_eq!(
            boot.mpfs_buttons,
            [("gpio0".into(), "UserButton".into(), true)]
        );
        assert_eq!(
            boot.halt_hooks,
            [(
                "u54_4".into(),
                "_start".into(),
                "e51".into(),
                "u-boot.elf".into()
            )]
        );
        fs::write(&script, source.replace("0.0008", "NaN")).unwrap();
        let result = load(&script, &directory);
        fs::remove_dir_all(directory).unwrap();
        assert!(matches!(result, Err(error) if error.contains("invalid emulation quantum")));
    }

    #[test]
    fn parses_binary_addresses_and_named_cpu_entries() {
        let mut boot = Boot::default();
        let mut created = true;
        let path = Path::new("test.resc");
        let root = Path::new(".");
        let vars = HashMap::new();
        for command in [
            "machine LoadPlatformDescription @board.repl",
            "sysbus LoadBinary @https://example.org/firmware.bin 0x40000000",
            "sysbus.cpu_1 PC 0x40000004",
        ] {
            execute(
                command,
                &command_words(command).unwrap(),
                &vars,
                &mut created,
                &mut boot,
                path,
                root,
                1,
            )
            .unwrap();
        }
        assert_eq!(boot.binaries.len(), 1);
        assert_eq!(boot.binaries[0].1, 0x4000_0000);
        assert_eq!(boot.boot_pcs, [("cpu_1".into(), 0x4000_0004)]);
        assert_eq!(
            boot.binaries[0]
                .0
                .resolve(root, Some(Path::new("assets")))
                .unwrap(),
            Path::new("assets/firmware.bin")
        );
    }

    #[test]
    fn starlight_monitor_commands_leave_only_guest_assets_to_load() {
        let path = Path::new("beaglev_starlight.resc");
        let root = Path::new("renode");
        let mut boot = Boot::default();
        let mut machine = false;
        let vars = HashMap::from([(
            "symbols".to_owned(),
            "@https://example.org/vmlinux".to_owned(),
        )]);
        for command in [
            "emulation SetSeed 0x1",
            "mach create BeagleV",
            "machine LoadPlatformDescription @platforms/boards/beaglev_starlight.repl",
            "emulation SetGlobalSerialExecution True",
            "showAnalyzer uart3",
            "sysbus LoadELF @fw_payload.elf",
            "sysbus LoadSymbolsFrom $symbols",
            "sysbus LoadBinary @image.fit 0xa0000000",
            "echo \"Script loaded.\"",
        ] {
            execute(
                command,
                &command_words(command).unwrap(),
                &vars,
                &mut machine,
                &mut boot,
                path,
                root,
                1,
            )
            .unwrap();
        }
        assert_eq!(
            boot.image.as_ref().map(|asset| asset.value.as_str()),
            Some("fw_payload.elf")
        );
        assert_eq!(boot.binaries.len(), 1);
        assert_eq!(
            (boot.binaries[0].0.value.as_str(), boot.binaries[0].1),
            ("image.fit", 0xa000_0000)
        );
    }

    #[test]
    fn reads_boot_commands_and_reports_macro_source_lines() {
        let directory = std::env::temp_dir().join(format!("penv-resc-{}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        let script = directory.join("boot.resc");
        let content = concat!(
            "$name?=\"Test Machine\"\n",
            "$bin?=@firmware\\ file.elf\n",
            "mach create \"Test Machine\"\n",
            "machine LoadPlatformDescription @board\\ file.repl\n",
            "macro reset\n",
            "\"\"\"\n",
            "  sysbus LoadELF $bin # boot image\n",
            "  sysbus.cpu MTVEC 0x80000020\n",
            "\"\"\"\n",
            "runMacro $reset\n",
            "cpu SetMachineIrqMask 0xffffffff\n",
            "start\n",
        );
        fs::write(&script, content).unwrap();
        let boot = load(&script, &directory).unwrap();
        assert_eq!(boot.platform, Some(directory.join("board file.repl")));
        assert_eq!(
            boot.image.as_ref().map(|image| image.value.as_str()),
            Some("firmware file.elf")
        );
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
