use std::{
    fs::{self, File},
    io::{self, Write},
    path::{Path, PathBuf},
};

use crate::machine::{Machine, Stop};

struct Transcript<'a, W> {
    output: &'a mut W,
    trace: Option<File>,
    line: Vec<u8>,
    step: u64,
    first_step: u64,
    prompt: Option<String>,
    matched: bool,
}

impl<W> Transcript<'_, W> {
    fn record_line(&mut self) -> io::Result<()> {
        let line = self.line.strip_suffix(b"\r").unwrap_or(&self.line);
        if let Some(trace) = &mut self.trace {
            write!(trace, "{}\t{}\t", self.first_step, self.step)?;
            trace.write_all(line)?;
            trace.write_all(b"\n")?;
        }
        Ok(())
    }
}

impl<W: Write> Write for Transcript<'_, W> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.output.write_all(data)?;
        for &byte in data {
            if self.line.is_empty() {
                self.first_step = self.step;
            }
            if byte != b'\n' {
                self.line.push(byte);
                continue;
            }
            self.record_line()?;
            let line = self.line.strip_suffix(b"\r").unwrap_or(&self.line);
            self.matched |= self
                .prompt
                .as_ref()
                .is_some_and(|prompt| prompt.as_bytes() == line);
            self.line.clear();
        }
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.output.flush()
    }
}

pub fn run(
    machine: &mut Machine,
    limit: u64,
    output: &mut impl Write,
    path: Option<&Path>,
    reply: Option<(String, PathBuf)>,
) -> Result<(Stop, u64), String> {
    let trace = path
        .map(|path| File::create(path).map_err(|e| format!("{}: {e}", path.display())))
        .transpose()?;
    let (prompt, mut input) = if let Some((prompt, path)) = reply {
        let bytes = fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        (Some(prompt), Some(bytes))
    } else {
        (None, None)
    };
    let mut transcript = Transcript {
        output,
        trace,
        line: Vec::new(),
        step: 0,
        first_step: 0,
        prompt,
        matched: false,
    };
    let mut retired = 0;
    let mut stop = Stop::Limit;
    for step in 1..=limit {
        transcript.step = step;
        let (result, count) = machine.run(1, &mut transcript)?;
        retired += count;
        if transcript.matched
            && let Some(bytes) = input.take()
        {
            machine.schedule_uart_input(step.saturating_add(1), bytes)?;
            transcript.prompt = None;
        }
        if result != Stop::Limit {
            stop = result;
            break;
        }
    }
    if !transcript.line.is_empty() {
        transcript
            .record_line()
            .map_err(|e| format!("UART transcript: {e}"))?;
    }
    if let Some(prompt) = transcript.prompt {
        return Err(format!("UART reply line {prompt:?} was not observed"));
    }
    Ok((stop, retired))
}
