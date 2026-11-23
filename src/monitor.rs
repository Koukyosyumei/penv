use std::{
    collections::HashMap,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

use crate::{
    bus::CORTEX_M_STEP_HZ,
    machine::{Machine, Stop},
    uart::{UartHistory, UartWaitMode},
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
    Button(String, bool),
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
            result.map_err(|e| format!("{}:{}: {e}", script.display(), command.line))?
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
    }
    if !machine || boot.platform.is_none() || (boot.image.is_none() && boot.binaries.is_empty()) {
        return Err(format!(
            "{}: expected machine creation, platform and firmware loading",
            path.display()
        ));
    }
    Ok(boot)
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
