use std::{
    fs,
    io::{self, Write},
    path::Path,
};

use crate::machine::{Machine, Stop};

struct Terminal<'a, W> {
    output: &'a mut W,
    partial: Vec<u8>,
    expected: Option<&'a str>,
    observed: bool,
}

impl<W: Write> Write for Terminal<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.output.write_all(bytes)?;
        for &byte in bytes {
            match self.expected {
                None => self.observed = true,
                Some(expected) if byte == b'\n' => {
                    self.observed |= String::from_utf8_lossy(&self.partial).contains(expected);
                    self.partial.clear();
                }
                Some(_) if byte != b'\r' => self.partial.push(byte),
                Some(_) => {}
            }
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.output.flush()
    }
}

pub fn run(machine: &mut Machine, output: &mut impl Write, path: &Path) -> Result<(), String> {
    let script =
        fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let rate = u64::from(machine.bus.step_hz());
    let mut terminal = Terminal {
        output,
        partial: Vec::new(),
        expected: None,
        observed: false,
    };
    for (index, line) in script.lines().enumerate() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let error = |message: &str| format!("{}:{}: {message}", path.display(), index + 1);
        let fields: Vec<_> = line.split('\t').collect();
        match fields.as_slice() {
            ["send", text] if !text.is_empty() => {
                machine
                    .schedule_uart_input(machine.elapsed_steps() + 1, text.as_bytes().to_vec())?;
            }
            ["second", second] => {
                let expected = second.parse::<u64>().map_err(|_| error("invalid second"))?;
                if machine.elapsed_steps() / rate != expected {
                    return Err(error(&format!(
                        "expected virtual second {expected}, at {:.6} s",
                        machine.elapsed_steps() as f64 / rate as f64
                    )));
                }
            }
            ["wait", seconds, ..] | ["idle", seconds] => {
                let expected = match fields.as_slice() {
                    ["wait", _, text] if !text.is_empty() => Some(*text),
                    ["idle", _] => None,
                    _ => return Err(error("expected wait<TAB>seconds<TAB>text")),
                };
                let limit = seconds
                    .parse::<u64>()
                    .ok()
                    .and_then(|n| n.checked_mul(rate))
                    .filter(|&n| n != 0)
                    .ok_or_else(|| error("invalid timeout"))?;
                terminal.expected = expected;
                terminal.observed = false;
                let (stop, _) = machine.run_until(limit, &mut terminal, |_, uart| uart.observed)?;
                if !matches!(stop, Stop::Limit | Stop::OutputMatch) {
                    return Err(error(&format!("emulation stopped: {stop:?}")));
                }
                if terminal.observed != expected.is_some() {
                    return Err(error(&match expected {
                        Some(text) => {
                            format!("UART timeout after {seconds} s waiting for {text:?}")
                        }
                        None => format!("UART output during {seconds} s idle window"),
                    }));
                }
            }
            _ => return Err(error("expected wait, idle, send or second")),
        }
        eprintln!(
            "UART check {:.6} s: {line}",
            machine.elapsed_steps() as f64 / rate as f64
        );
    }
    Ok(())
}
