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
import io
import sys
from types import ModuleType, SimpleNamespace

def rpc(request):
    print(request, file=sys.__stdout__, flush=True)
    status, value = sys.stdin.readline().rstrip('\n').split(' ', 1)
    if status != 'OK':
        raise RuntimeError(bytes.fromhex(value).decode())
    return value

class Credential:
    def __init__(self, path):
        self.path = path
    @property
    def Size(self):
        return int(rpc('size ' + self.path.encode().hex()))

class BusRangeRegistration:
    def __init__(self, address, size, cpu=None):
        self.address = address
        self.size = size
        self.cpu = cpu

for name in ('Antmicro', 'Antmicro.Renode', 'Antmicro.Renode.Peripherals',
             'Antmicro.Renode.Peripherals.Bus'):
    module = sys.modules.setdefault(name, ModuleType(name))
    module.__path__ = []
sys.modules['Antmicro.Renode.Peripherals.Bus'].BusRangeRegistration = BusRangeRegistration

class Bus:
    def WhatIsAt(self, address, cpu):
        name = bytes.fromhex(rpc(f'lookup {address} {cpu.path.encode().hex()}')).decode()
        return SimpleNamespace(Peripheral=Credential(name)) if name else None
    def MoveRegistrationWithinContext(self, peripheral, registration, cpu):
        if registration.cpu.path != cpu.path:
            raise ValueError('registration CPU does not match the context')
        rpc(f'move {peripheral.path.encode().hex()} {cpu.path.encode().hex()} {registration.address} {registration.size}')
    def GetSymbolAddress(self, name):
        return int(rpc('symbol ' + name.encode().hex()))
    def ReadByte(self, address):
        return int(rpc('byte ' + str(address)))

class Machine:
    SystemBus = Bus()
    def __getitem__(self, path):
        return Credential(path)
    @property
    def UserState(self):
        return bytes.fromhex(rpc('get')).decode()
    @UserState.setter
    def UserState(self, value):
        if not isinstance(value, str):
            raise TypeError('UserState must be a string')
        rpc('set ' + value.encode().hex())

class HexValue(int):
    def __str__(self):
        return hex(self)

class CPU(Credential):
    Bus = Machine.SystemBus
    Machine = Machine()
    @property
    def PC(self):
        return HexValue(scope['pc'])
    def GetItState(self):
        state = scope.get('itstate', 0)
        if state == 0:
            self.DebugLog('Checking IT_STATE, while not in IT block')
        return state
    def DebugLog(self, message, *args):
        self.Log(0, message, *args)
    def Log(self, level, message, *args):
        if args:
            message = message.format(*args)
        rpc('log ' + str(level) + ' ' + message.encode().hex())

level = SimpleNamespace(Noisy=-1, Debug=0, Info=1, Warning=2, Error=3)
cpu = CPU(sys.argv[2])
scope = dict(machine=cpu.Machine, cpu=cpu, self=cpu, LogLevel=level)
request = SimpleNamespace(Value=0)
program = compile(sys.argv[1], '<CPU hook>', 'exec')
print('ready', flush=True)
for line in sys.stdin:
    fields = line.split()
    is_csr = fields[0] == 'csr'
    is_monitor = fields[0] == 'monitor'
    if is_csr:
        request.Csr = int(fields[1])
        request.IsRead = fields[2] == 'read'
        request.IsWrite = fields[2] == 'write'
        request.IsInit = False
        if request.IsWrite:
            request.Value = int(fields[3])
        scope['request'] = request
    elif fields[0] == 'thumb-block':
        scope.update(pc=int(fields[1]), itstate=int(fields[2]))
    elif not is_monitor:
        scope['pc'] = int(fields[0])
        if len(fields) == 4:
            scope.update(address=int(fields[1]), width=int(fields[2]), is_write=fields[3] == 'true')
    capture = io.StringIO() if is_monitor else sys.stderr
    try:
        with contextlib.redirect_stdout(capture):
            # RPCs use the original stdout even when script output is redirected.
            if is_monitor:
                code = bytes.fromhex(fields[1]).decode() if len(fields) > 1 else ''
                exec(compile(code, '<monitor>', 'exec'), scope)
            else:
                exec(program, scope)
    except Exception as error:
        marker = 'monitor-exception' if is_monitor else 'exception'
        print(marker + ' ' + str(error).encode().hex(), flush=True)
    else:
        if is_monitor:
            print('monitor-result ' + capture.getvalue().encode().hex(), flush=True)
        elif is_csr:
            print('csr-result ' + str(request.Value), flush=True)
        else:
            print('done', flush=True)
"#;

pub(crate) struct Hook {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    symbols: Option<PathBuf>,
    logger: Option<String>,
    monitor_output: String,
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
        Self::with_cpu(source, symbols, logger, "")
    }

    pub(crate) fn with_cpu(
        source: &str, symbols: Option<PathBuf>, logger: Option<String>, cpu: &str,
    ) -> Result<Self, String> {
        let mut child = Command::new("python3")
            .args(["-u", "-c", WORKER, source, cpu])
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
            monitor_output: String::new(),
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
        self.invoke(bus, format_args!("{pc}")).map(|_| ())
    }

    pub(crate) fn execute_monitor(
        &mut self, bus: &mut Bus, source: &str, symbols: Option<PathBuf>,
    ) -> Result<String, String> {
        self.symbols = symbols;
        let mut compatible = String::new();
        for line in source.lines() {
            let body = line.trim_start();
            if let Some(expression) = body.strip_prefix("print ").filter(|value| !value.starts_with(">>")) {
                let indent = line.len() - body.len();
                compatible.push_str(&line[..indent]);
                compatible.push_str("print(");
                compatible.push_str(expression);
                compatible.push(')');
            } else {
                compatible.push_str(line);
            }
            compatible.push('\n');
        }
        self.invoke(bus, format_args!("monitor {}", encode(&compatible)))?;
        Ok(std::mem::take(&mut self.monitor_output))
    }

    pub(crate) fn execute_thumb_block(&mut self, bus: &mut Bus, pc: u32, it: u8) -> Result<(), String> {
        self.invoke(bus, format_args!("thumb-block {pc} {it}")).map(|_| ())
    }

    pub(crate) fn execute_stack(&mut self, bus: &mut Bus, pc: u64, address: u64, width: usize, write: bool) -> Result<(), String> {
        self.invoke(bus, format_args!("{pc} {address} {width} {write}")).map(|_| ())
    }

    pub(crate) fn execute_csr(&mut self, bus: &mut Bus, csr: u32, value: Option<u64>) -> Result<u64, String> {
        let operation = if value.is_some() { "write" } else { "read" };
        let value = value.unwrap_or(0);
        self.invoke(bus, format_args!("csr {csr} {operation} {value}"))?
            .ok_or_else(|| "CSR hook returned no value".into())
    }

    fn invoke(&mut self, bus: &mut Bus, arguments: std::fmt::Arguments<'_>) -> Result<Option<u64>, String> {
        writeln!(self.input, "{arguments}").map_err(|e| e.to_string())?;
        loop {
            let request = self.reply()?;
            let (kind, argument) = request.split_once(' ').unwrap_or((&request, ""));
            let result = match kind {
                "done" => return Ok(None),
                "csr-result" => return argument.parse::<u64>().map(Some).map_err(|e| e.to_string()),
                "monitor-result" => {
                    self.monitor_output = decode(argument)?;
                    return Ok(None);
                }
                "monitor-exception" => return Err(decode(argument)?),
                "size" => decode(argument).and_then(|name| bus.scoped_peripheral_size(&name))
                    .map(|size| size.to_string()),
                "lookup" => (|| {
                    let (address, context) = argument.split_once(' ').ok_or("invalid bus lookup")?;
                    let address = address.parse::<u32>().map_err(|e| e.to_string())?;
                    let hart = bus.scoped_cpu_index(&decode(context)?)?;
                    Ok(encode(bus.scoped_peripheral_at(hart, address).unwrap_or_default()))
                })(),
                "move" => (|| {
                    let mut fields = argument.split_ascii_whitespace();
                    let peripheral = decode(fields.next().ok_or("missing peripheral")?)?;
                    let context = decode(fields.next().ok_or("missing CPU")?)?;
                    let address = fields.next().and_then(|value| value.parse().ok()).ok_or("invalid destination address")?;
                    let size = fields.next().and_then(|value| value.parse().ok()).ok_or("invalid registration size")?;
                    let hart = bus.scoped_cpu_index(&context)?;
                    bus.move_named_scoped_registration(&peripheral, hart, address, size)?;
                    Ok(String::new())
                })(),
                "exception" => {
                    emit(
                        bus,
                        3,
                        &format!("Python runtime error: {}", decode(argument)?),
                    );
                    return Ok(None);
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
