//! Synchronous CPU address callbacks with a Python scope and live bus access.
use crate::bus::Bus;
use std::{
    fs::File,
    io::{BufRead, BufReader, Read, Seek, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
};

const WORKER: &str = r#"
import contextlib
import sys
from types import SimpleNamespace

def rpc(request):
    print(request, file=sys.__stdout__, flush=True)
    status, value = sys.stdin.readline().rstrip('\n').split(' ', 1)
    if status != 'OK':
        raise RuntimeError(bytes.fromhex(value).decode())
    return value

class Bus:
    def GetSymbolAddress(self, name):
        return int(rpc('symbol ' + name.encode().hex()))
    def ReadByte(self, address):
        return int(rpc('byte ' + str(address)))

class Machine:
    SystemBus = Bus()
    @property
    def UserState(self):
        return bytes.fromhex(rpc('get')).decode()
    @UserState.setter
    def UserState(self, value):
        if not isinstance(value, str):
            raise TypeError('UserState must be a string')
        rpc('set ' + value.encode().hex())

class CPU:
    Bus = Machine.SystemBus
    def Log(self, level, message, *args):
        if args:
            message = message.format(*args)
        rpc('log ' + str(level) + ' ' + message.encode().hex())

level = SimpleNamespace(Noisy=-1, Debug=0, Info=1, Warning=2, Error=3)
cpu = CPU()
scope = dict(machine=Machine(), cpu=cpu, self=cpu, LogLevel=level)
program = compile(sys.argv[1], '<CPU address hook>', 'exec')
print('ready', flush=True)
for line in sys.stdin:
    fields = line.split()
    scope['pc'] = int(fields[0])
    if len(fields) == 4:
        scope.update(address=int(fields[1]), width=int(fields[2]), is_write=fields[3] == 'true')
    try:
        with contextlib.redirect_stdout(sys.stderr):
            # RPCs must use the original stdout while ordinary prints go to stderr.
            exec(program, scope)
    except Exception as error:
        print('exception ' + str(error).encode().hex(), flush=True)
    else:
        print('done', flush=True)
"#;

pub(crate) struct Hook {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    symbols: Option<PathBuf>,
    logger: Option<String>,
}

pub(crate) fn symbol(file: &Path, name: &str) -> Result<u64, String> {
    let mut source = File::open(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let mut ident = [0; 5];
    source.read_exact(&mut ident).map_err(|e| e.to_string())?;
    source.rewind().map_err(|e| e.to_string())?;
    match &ident {
        b"\x7fELF\x01" => crate::image::find_symbols(source, &[name])?
            .first()
            .map(|(_, address)| u64::from(*address))
            .ok_or_else(|| format!("symbol {name:?} not found")),
        b"\x7fELF\x02" => {
            let mut image = Vec::new();
            source.read_to_end(&mut image).map_err(|e| e.to_string())?;
            crate::image64::symbol(&image, name)
        }
        _ => Err("CPU hook symbol lookup requires ELF".into()),
    }
}

impl Hook {
    pub(crate) fn new(source: &str, symbols: Option<PathBuf>, logger: Option<String>) -> Result<Self, String> {
        let mut child = Command::new("python3")
            .args(["-u", "-c", WORKER, source])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| format!("start CPU Python hook: {e}"))?;
        let input = child.stdin.take().expect("piped stdin");
        let output = BufReader::new(child.stdout.take().expect("piped stdout"));
        let mut hook = Self {
            child,
            input,
            output,
            symbols,
            logger,
        };
        if hook.reply()? != "ready" {
            return Err("CPU Python hook failed to initialize".into());
        }
        Ok(hook)
    }

    fn reply(&mut self) -> Result<String, String> {
        let mut line = String::new();
        if self
            .output
            .read_line(&mut line)
            .map_err(|e| e.to_string())?
            == 0
        {
            return Err("CPU Python hook exited unexpectedly".into());
        }
        Ok(line.trim_end_matches(['\n', '\r']).to_owned())
    }

    pub(crate) fn execute(&mut self, bus: &mut Bus, pc: u64) -> Result<(), String> {
        self.invoke(bus, format_args!("{pc}"))
    }

    pub(crate) fn execute_stack(&mut self, bus: &mut Bus, pc: u64, address: u64, width: usize, write: bool) -> Result<(), String> {
        self.invoke(bus, format_args!("{pc} {address} {width} {write}"))
    }

    fn invoke(&mut self, bus: &mut Bus, arguments: std::fmt::Arguments<'_>) -> Result<(), String> {
        writeln!(self.input, "{arguments}").map_err(|e| e.to_string())?;
        loop {
            let request = self.reply()?;
            let (kind, argument) = request.split_once(' ').unwrap_or((&request, ""));
            let result = match kind {
                "done" => return Ok(()),
                "exception" => {
                    emit(
                        bus,
                        3,
                        &format!("Python runtime error: {}", decode(argument)?),
                    );
                    return Ok(());
                }
                "symbol" => decode(argument)
                    .and_then(|name| symbol(self.symbols.as_deref().ok_or("no firmware ELF for symbol lookup")?, &name))
                    .map(|address| address.to_string()),
                "byte" => argument
                    .parse::<u32>()
                    .map_err(|e| e.to_string())
                    .and_then(|address| {
                        bus.read_scalar::<1>(address, true)
                            .map_err(|e| e.to_string())
                    })
                    .map(|value| value.to_string()),
                "get" => Ok(encode(&bus.monitor_user_state)),
                "set" => decode(argument).map(|value| {
                    bus.monitor_user_state = value;
                    String::new()
                }),
                "log" => argument
                    .split_once(' ')
                    .ok_or_else(|| "invalid log request".to_string())
                    .and_then(|(level, text)| {
                        let level = level.parse::<i8>().map_err(|e| e.to_string())?;
                        let message = decode(text)?;
                        if let Some(logger) = &self.logger {
                            emit(bus, level, &format!("{logger}: {message}"));
                        } else {
                            emit(bus, level, &message);
                        }
                        Ok(String::new())
                    }),
                _ => Err(format!("unsupported CPU hook request {kind}")),
            };
            match result {
                Ok(value) => writeln!(self.input, "OK {value}"),
                Err(error) => writeln!(self.input, "ERR {}", encode(&error)),
            }
            .map_err(|e| e.to_string())?;
        }
    }
}

impl Drop for Hook {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn emit(bus: &Bus, level: i8, message: &str) {
    crate::monitor_log::record(level, message);
    if bus.monitor_log_enabled(level) {
        eprintln!("{message}");
    }
}

fn encode(text: &str) -> String {
    text.bytes().map(|byte| format!("{byte:02x}")).collect()
}

fn decode(text: &str) -> Result<String, String> {
    if !text.is_ascii() || !text.len().is_multiple_of(2) {
        return Err("invalid CPU hook text encoding".into());
    }
    let bytes = (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).map_err(|e| e.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    String::from_utf8(bytes).map_err(|e| e.to_string())
}
