use std::{
    collections::HashMap,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

use crate::{
    bus::CORTEX_M_STEP_HZ,
    machine::{Machine, Stop},
    uart::{UartLineEnd, UartLineMatch},
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
    Adc(String, usize, f64),
    WaitForUart(Option<String>, String, UartLineEnd),
    WriteUartLine(String, String, &'static [u8]),
}

struct UartLine<'a, W> {
    output: &'a mut W,
    line: UartLineMatch,
}

impl<W: Write> Write for UartLine<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let written = self.output.write(bytes)?;
        for &byte in &bytes[..written] {
            self.line.receive(byte);
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
    let mut retired = 0;
    for command in commands {
        let result = match &command.action {
            Action::RunFor(steps) if machine.is_cortex_m() => {
                let chunk = (*steps).min(budget);
                machine.run(chunk, output).map(|(stop, count)| {
                    retired += count;
                    budget -= chunk;
                    (stop != Stop::Limit || chunk < *steps).then_some(stop)
                })
            }
            Action::RunFor(_) => Err("RunFor requires a Cortex-M platform".into()),
            Action::Button(name, pressed) => {
                machine.bus.set_named_button(name, *pressed).map(|()| None)
            }
            Action::Adc(name, channel, volts) => machine
                .bus
                .set_named_adc_input(name, *channel, *volts)
                .map(|()| None),
            Action::WaitForUart(target, pattern, end) => {
                let start = machine.elapsed_steps();
                wait_for_uart(machine, target.as_deref(), pattern, *end, budget, output).and_then(
                    |(stop, count)| {
                        retired += count;
                        budget -= machine.elapsed_steps().wrapping_sub(start);
                        if stop == Stop::OutputMatch {
                            Ok(None)
                        } else {
                            let source = target.as_ref().map_or_else(String::new, |target| format!(" on {target}"));
                            Err(format!(
                                "UART line containing {pattern:?} was not received{source} ({stop:?})"
                            ))
                        }
                    },
                )
            }
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

fn wait_for_uart(
    machine: &mut Machine,
    target: Option<&str>,
    pattern: &str,
    end: UartLineEnd,
    budget: u64,
    output: &mut impl Write,
) -> Result<(Stop, u64), String> {
    if let Some(target) = target {
        machine.bus.watch_stm32_uart(target, pattern, end)?;
        let result = machine.run_until(budget, output, |bus, _| bus.stm32_uart_matched());
        machine.bus.clear_stm32_uart_watch();
        result
    } else {
        let mut matcher = UartLine {
            output,
            line: UartLineMatch::new(pattern, end),
        };
        machine.run_until(budget, &mut matcher, |_, uart| uart.line.matched)
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
        ["waitForUart", ..] => {
            let args = text["waitForUart".len()..].trim();
            let (target, line) = if args.starts_with('"') {
                (None, args)
            } else {
                let (target, line) = args
                    .split_once(char::is_whitespace)
                    .ok_or("expected waitForUart [USART] \"literal text\"")?;
                (Some(resolve(target)?.to_owned()), line.trim_start())
            };
            let (pattern, ending) = quoted_argument(line)
                .filter(|(pattern, _)| !pattern.is_empty())
                .ok_or("expected waitForUart [USART] \"literal text\" [LF|CR]")?;
            let end = match ending {
                "" | "LF" => UartLineEnd::Lf,
                "CR" => UartLineEnd::Cr,
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
        ["emulation", "RunFor", duration] => {
            Some(Action::RunFor(duration_steps(resolve(duration)?)?))
        }
        [device, state @ ("Press" | "Release")] => Some(Action::Button(
            resolve(device)?.to_owned(),
            *state == "Press",
        )),
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
        let mut uart = UartLine {
            output: &mut output,
            line: UartLineMatch::new("ADC reading: 745", UartLineEnd::Lf),
        };
        uart.write_all(b"ADC reading: \n745\r\n").unwrap();
        assert!(!uart.line.matched);
        uart.write_all(b"ADC reading: 7").unwrap();
        uart.write_all(b"45\r").unwrap();
        assert!(!uart.line.matched);
        uart.write_all(b"\n").unwrap();
        assert!(uart.line.matched);
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
