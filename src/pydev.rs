use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
};

const WORKER: &str = r#"
import contextlib
from types import SimpleNamespace
import sys

program = compile(sys.argv[1], "<PyDev>", "exec")
class PeripheralLog:
    def NoisyLog(self, message):
        pass

scope = {"self": PeripheralLog()}
request = SimpleNamespace()
print(0, flush=True)
for line in sys.stdin:
    kind, offset, address, width, value, counter = line.split()
    request.Type = kind.upper()
    request.IsInit = kind == "init"
    request.IsRead = kind == "read"
    request.IsWrite = kind == "write"
    request.IsUser = False
    request.Offset = int(offset)
    request.Absolute = int(address)
    request.Length = int(width)
    request.Value = int(value)
    request.Counter = int(counter)
    scope["request"] = request
    with contextlib.redirect_stdout(sys.stderr):
        exec(program, scope)
    print(int(request.Value) & ((1 << 64) - 1), flush=True)
"#;

struct Worker {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
}

impl Worker {
    fn start(source: &str) -> Result<Self, String> {
        let mut child = Command::new("python3")
            .args(["-u", "-c", WORKER, source])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|error| format!("start Python peripheral: {error}"))?;
        let input = child.stdin.take().unwrap();
        let output = BufReader::new(child.stdout.take().unwrap());
        let mut worker = Self {
            child,
            input,
            output,
        };
        worker
            .read_reply()
            .map_err(|error| format!("initialize PyDev: {error}"))?;
        Ok(worker)
    }

    fn request(&mut self, request: std::fmt::Arguments<'_>) -> Result<u64, String> {
        writeln!(self.input, "{request}").map_err(|error| format!("PyDev request: {error}"))?;
        self.read_reply()
    }

    fn read_reply(&mut self) -> Result<u64, String> {
        let mut reply = String::new();
        self.output
            .read_line(&mut reply)
            .map_err(|error| format!("PyDev response: {error}"))?;
        reply
            .trim()
            .parse()
            .map_err(|_| "Python peripheral did not return a value".into())
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub struct Script {
    actions: Vec<Action>,
    hex: bool,
}

enum Action {
    Register {
        source: String,
        base: u64,
        size: u64,
        initable: bool,
    },
    Access {
        address: u64,
        width: u32,
        value: Option<u64>,
    },
}

pub(crate) struct Peripheral {
    worker: Worker,
    base: u64,
    end: u64,
    init_pending: bool,
    counter: u64,
}

impl Peripheral {
    pub fn new(source: &str, base: u64, size: u64, initable: bool) -> Result<Self, String> {
        let end = base
            .checked_add(size)
            .filter(|&end| end > base)
            .ok_or("invalid PyDev address range")?;
        Ok(Self {
            worker: Worker::start(source)?,
            base,
            end,
            init_pending: initable,
            counter: 0,
        })
    }

    pub fn overlaps(&self, start: u64, end: u64) -> bool {
        self.base < end && start < self.end
    }

    pub fn contains(&self, address: u64, width: u32) -> bool {
        address >= self.base
            && address
                .checked_add(u64::from(width))
                .is_some_and(|end| end <= self.end)
    }

    pub fn access(&mut self, address: u64, width: u32, value: Option<u64>) -> Result<u64, String> {
        if std::mem::take(&mut self.init_pending) {
            self.worker
                .request(format_args!("init 0 {address} {width} 0 0"))?;
        }
        let offset = address - self.base;
        let counter = self.counter;
        self.counter += 1;
        let mask = u64::MAX >> (64 - width * 8);
        let kind = if value.is_some() { "write" } else { "read" };
        let value = value.unwrap_or_default() & mask;
        self.worker
            .request(format_args!(
                "{kind} {offset} {address} {width} {value} {counter}"
            ))
            .map(|response| response & mask)
    }
}

fn number(text: &str) -> Result<u64, String> {
    text.strip_prefix("0x")
        .map_or_else(|| text.parse(), |hex| u64::from_str_radix(hex, 16))
        .map_err(|_| format!("invalid bus integer {text:?}"))
}

fn width(command: &str) -> Result<u32, String> {
    match command {
        "Byte" => Ok(1),
        "Word" => Ok(2),
        "DoubleWord" => Ok(4),
        "QuadWord" => Ok(8),
        _ => Err(format!("unsupported bus access width {command:?}")),
    }
}

impl Script {
    pub fn parse(content: &str) -> Result<Self, String> {
        let mut lines = content.lines();
        let mut actions = Vec::new();
        let mut machine = false;
        let mut hex = false;
        while let Some(line) = lines.next() {
            let line = line.trim();
            if line.is_empty() || line.starts_with(['#', ':']) {
                continue;
            }
            if let Some(rest) = line.strip_prefix("machine PyDevFromString \"\"\"")
                && machine
            {
                let mut source = String::new();
                if !rest.is_empty() {
                    source.push_str(rest);
                    source.push('\n');
                }
                let parameters = loop {
                    let line = lines.next().ok_or("unterminated PyDev script")?;
                    if let Some(parameters) = line.trim_start().strip_prefix("\"\"\"") {
                        break parameters.split_whitespace().collect::<Vec<_>>();
                    }
                    source.push_str(line);
                    source.push('\n');
                };
                let [base, size, initable] = parameters.as_slice() else {
                    return Err("expected PyDevFromString address, size and initable flag".into());
                };
                actions.push(Action::Register {
                    source,
                    base: number(base)?,
                    size: number(size)?,
                    initable: initable
                        .to_ascii_lowercase()
                        .parse()
                        .map_err(|_| "invalid PyDev initable flag")?,
                });
                continue;
            }
            let words: Vec<_> = line.split_whitespace().collect();
            match words.as_slice() {
                ["mach" | "machine", "create"] if !machine => machine = true,
                ["numbersMode", "Hexadecimal"] if actions.is_empty() => hex = true,
                ["sysbus", command, address] if machine && command.starts_with("Read") => {
                    actions.push(Action::Access {
                        address: number(address)?,
                        width: width(&command[4..])?,
                        value: None,
                    });
                }
                ["sysbus", command, address, value] if machine && command.starts_with("Write") => {
                    actions.push(Action::Access {
                        address: number(address)?,
                        width: width(&command[5..])?,
                        value: Some(number(value)?),
                    });
                }
                _ => return Err(format!("unsupported CPU-less PyDev command {line:?}")),
            }
        }
        Ok(Self { actions, hex })
    }

    pub fn run(self, output: &mut impl Write) -> Result<(), String> {
        let mut peripherals: Vec<Peripheral> = Vec::new();
        for action in self.actions {
            match action {
                Action::Register {
                    source,
                    base,
                    size,
                    initable,
                } => {
                    let end = base
                        .checked_add(size)
                        .filter(|&end| end > base)
                        .ok_or("invalid PyDev address range")?;
                    if peripherals
                        .iter()
                        .any(|device| device.base < end && base < device.end)
                    {
                        return Err("overlapping PyDev address ranges".into());
                    }
                    peripherals.push(Peripheral::new(&source, base, size, initable)?);
                }
                Action::Access {
                    address,
                    width,
                    value,
                } => {
                    let device = peripherals
                        .iter_mut()
                        .find(|device| device.contains(address, width))
                        .ok_or_else(|| format!("unmapped PyDev access at {address:#x}"))?;
                    let response = device.access(address, width, value)?;
                    if value.is_none() {
                        let value = response;
                        if self.hex {
                            writeln!(output, "{value:#x}")
                        } else {
                            writeln!(output, "{value}")
                        }
                        .map_err(|error| error.to_string())?;
                    }
                }
            }
        }
        Ok(())
    }
}
