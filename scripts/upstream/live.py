"""Translate supported Robot monitor operations to a persistent penv machine."""

from decimal import Decimal, InvalidOperation
import json
import os
from pathlib import Path
import re
import select
import subprocess
import tempfile
import time

from memory_keywords import MemoryKeywords
from monitor_input import MonitorInput, PUBLIC_ASSET, at_path, script_asset_urls, unquote, words
from penv_robot import (AssertionFailed, Endpoint, PenvExecutionError, PenvStartupError,
                        PenvUnsupported, Unsupported)


class LiveEndpoint(MemoryKeywords, Endpoint):
    proc = None
    pending_state = None
    keywords = dict(Endpoint.keywords, **MemoryKeywords.memory_keywords, **{
        "Wait For Prompt On Uart": "prompt",
        "Should Not Be On Uart": "uart_absence",
        "Create Log Tester": "create_log_tester",
        "Wait For Log Entry": "wait_for_log",
        "Write Line To Uart": "write_line",
        "Write Char On Uart": "write_char",
        "Test If Uart Is Idle": "uart_idle",
        "Create LED Tester": "create_led",
        "Assert LED State": "assert_led",
        "Assert And Hold LED State": "hold_led",
        "Execute Script": "execute_script",
        "Download File": "download_file",
        "Allocate Temporary File": "temporary_file",
        "Allocate Temporary Directory": "temporary_directory",
        "Handle Hot Spot": "hotspot",
        "Provides": "provides",
        "Requires": "requires",
    })

    def reset(self):
        self.close()
        super().reset()
        self.platform = None
        self.script = None
        self.script_urls = {}
        self.log_level = None
        self.log_capacity = None
        self.log_timeout = None
        self.memory_symbols = {}
        self.monitor_input = MonitorInput(self.root, self.out, self.assets)
        self.replaying = False
        self.inline = []
        self.using_bus = False
        self.numbers = "Hexadecimal"
        self.uart = None
        self.uart_timeout = None
        self.led = None
        self.led_timeout = None
        self.buffer = b""

    states = None

    def begin_case(self, out):
        self.pending_state = None
        super().begin_case(out)

    def publish_state(self):
        if self.pending_state:
            state, recipe = self.pending_state
            self.states[state] = recipe

    def provides(self, state, kind="Serialization"):
        if "=" in state:
            state = self.option(state, "state")
        if "=" in kind:
            kind = self.option(kind, "type")
        if self.replaying or kind != "Reexecution":
            raise Unsupported(f"Provides {kind} requires a snapshot-capable adapter")
        replay = {
            "Execute Command", "Execute Script", "Download File", "Create Terminal Tester", "Start Emulation",
            "Wait For Line On Uart", "Wait For Prompt On Uart", "Write Line To Uart",
            "Create LED Tester", "Assert LED State", "Handle Hot Spot",
            "Set Default Uart Timeout", "Set Crash On Abort",
        }
        calls = [call for call in self.calls[:-1] if call["keyword"] != "Reset Emulation"]
        if self.proc is None or any(call["status"] != "ok" or call["keyword"] not in replay
                                    for call in calls):
            raise Unsupported("state includes operations without a supported replay")
        self.pending_state = state, [(call["keyword"], call["args"]) for call in calls]

    def requires(self, state):
        if "=" in state:
            state = self.option(state, "state")
        if self.commands or self.proc or self.script:
            raise Unsupported("Requires must precede machine operations")
        if state not in self.states:
            raise Unsupported(f"required state {state!r} unavailable")
        self.replaying = True
        try:
            for keyword, args in self.states[state]:
                getattr(self, self.keywords[keyword])(*args)
        finally:
            self.replaying = False

    def temporary_file(self):
        with tempfile.NamedTemporaryFile(dir=self.out, delete=False) as file:
            return file.name

    def temporary_directory(self, name):
        if not re.fullmatch(r"[\w.-]+", name) or name in {".", ".."}:
            raise Unsupported("temporary directory requires a simple name")
        path = self.out / name
        path.mkdir()
        return str(path)

    def hotspot(self, action):
        if action != "None":
            raise Unsupported(f"HotSpot action {action} requires additional lifecycle support")

    @staticmethod
    def log_options(args, allowed, positional):
        provided = {}
        for i, argument in enumerate(args):
            name, separator, value = str(argument).partition("=")
            if not separator:
                if i >= len(positional):
                    raise Unsupported("unsupported log keyword arguments")
                name, value = positional[i], argument
            if name not in allowed or name in provided:
                raise Unsupported(f"unsupported or duplicate log keyword option {name}")
            provided[name] = str(value)
        return provided

    @staticmethod
    def log_bool(value):
        token = value.lower()
        if token not in {"true", "false"}:
            raise Unsupported(f"expected a boolean log option, got {value}")
        return token == "true"

    def create_log_tester(self, timeout, *args):
        if isinstance(timeout, str) and timeout.startswith("timeout="):
            timeout = self.option(timeout, "timeout")
        options = self.log_options(args, {"defaultPauseEmulation", "maxLogEntries"},
                                   ("defaultPauseEmulation", "maxLogEntries"))
        if self.log_bool(options.get("defaultPauseEmulation", "false")):
            raise Unsupported("log tester pause-on-match requires asynchronous emulation")
        self.log_timeout = self.duration(timeout)
        self.log_capacity = int(options.get("maxLogEntries", "1000000"))
        if not 1 <= self.log_capacity <= 1000000:
            raise Unsupported("log tester capacity outside supported range")
        if self.proc:
            self.operation(f"AttachMonitorLog {self.log_capacity}")

    def wait_for_log(self, pattern, *args):
        if self.log_timeout is None:
            raise Unsupported("create a log tester before waiting")
        positional = ("timeout", "keep", "treatAsRegex", "pauseEmulation", "level", "startEmulation")
        options = self.log_options(args, set(positional), positional)
        if self.log_bool(options.get("pauseEmulation", "false")):
            raise Unsupported("log tester pause-on-match requires asynchronous emulation")
        timeout = int(Decimal(self.duration(options.get("timeout", self.log_timeout))) * 1000)
        starts = self.log_bool(options.get("startEmulation", "true"))
        if timeout and not (starts or self.started):
            raise Unsupported("cannot wait for virtual time while emulation is stopped")
        keep = "keep" if self.log_bool(options.get("keep", "false")) else "consume"
        kind = "regex" if self.log_bool(options.get("treatAsRegex", "false")) else "literal"
        levels = {"Noisy": -1, "Debug": 0, "Info": 1, "Warning": 2, "Error": 3, "null": "any"}
        level = levels.get(options.get("level", "null"))
        if level is None:
            raise Unsupported("unknown log level")
        result = self.operation(
            f"WaitForMonitorLog {timeout} {kind} {keep} {level} {pattern.encode().hex() or '-'}")
        if timeout and starts:
            self.started = True
        return result

    def download_file(self, uri):
        if not PUBLIC_ASSET.fullmatch(uri):
            raise Unsupported("Download File requires a versioned public Renode asset")
        self.fetch(uri)
        return str(self.assets / uri.rsplit("/", 1)[1])

    def execute_script(self, script):
        return self.load_script(self.monitor_input.path(script.removeprefix("@")))

    def include(self, tokens):
        if not tokens:
            raise Unsupported("include requires a file")
        files = [self.monitor_input.path(token) for token in tokens]
        extensions = {file.suffix for file in files}
        if len(extensions) != 1 or len(files) > 1 and extensions != {".cs"}:
            raise Unsupported("include accepts multiple files only for a C# plugin")
        file = files[0]
        self.calls[-1]["include"] = {"files": [str(item) for item in files], "format": file.suffix}
        if file.suffix in {".cs", ".py"}:
            raise PenvUnsupported(f"penv has no monitor loader for {file.suffix} includes")
        if file.suffix == ".repl":
            if self.proc or self.platform or self.inline or self.script:
                raise Unsupported("REPL inclusion needs one machine before its platform is loaded")
            if not self.commands:
                self.commands.append("mach create")
                self.monitor_input.name_machine("machine")
            self.platform = file
            self.calls[-1]["include"]["stage"] = "platform selected for penv load"
        else:
            output = self.load_script(file)
            self.calls[-1]["include"]["stage"] = "penv monitor ready"
            return output
        return ""

    def load_script(self, candidate):
        if self.proc or self.script or self.platform or self.inline or len(self.commands) > 1:
            raise Unsupported("RESC inclusion requires a fresh machine or monitor")
        self.script = self.out / "include.resc"
        self.script.write_text("\n".join([*self.monitor_input.script_prefix(), *self.commands,
                                          *([self.log_level] if self.log_level else []),
                                          f"include {at_path(candidate)}"]) + "\n")
        urls = script_asset_urls(candidate, self.root)
        urls.update(PUBLIC_ASSET.findall("\n".join(self.monitor_input.script_prefix())))
        self.script_urls = {url.rsplit("/", 1)[-1]: url for url in urls}
        self.image = True
        return self.ensure_running()

    def close(self):
        proc = getattr(self, "proc", None)
        if proc is not None:
            self.proc = None
            try:
                proc.stdin.close()
                proc.wait(timeout=2)
            except (BrokenPipeError, subprocess.TimeoutExpired):
                proc.kill()
                proc.wait()
            finally:
                proc.stdout.close()

    def execute(self, command, *fragments):
        if isinstance(command, list):
            if fragments or not all(isinstance(item, str) for item in command):
                raise Unsupported("unsupported Execute Command fragment arguments")
            command = " ".join(command)
        elif fragments:
            raise Unsupported(f"Execute Command machine context or arguments: {fragments}")
        command = command.strip()
        assigned = self.monitor_input.assign(command)
        if assigned is not None:
            name, value = assigned
            self.calls[-1]["variable"] = {"name": name, "token": value}
            return ""
        tokens = words(command)
        if tokens and tokens[0] in {"include", "i"}:
            return self.include(tokens[1:])
        expanded = self.monitor_input.substitute(command)
        if expanded != command:
            self.calls[-1]["expanded_command"] = expanded
            command = expanded
        if re.fullmatch(r'logLevel (?:-1|[0-3]|"(?:Noisy|Debug|Info|Warning|Error)")', command):
            if not self.proc:
                self.log_level = command
                return ""
            return self.operation(command)
        if len(tokens) in {2, 3} and tokens[:2] == ["machine", "UserState"]:
            return self.operation(command)
        if command == "using sysbus":
            self.using_bus = True
            return ""
        if re.fullmatch(r"numbersMode (Hexadecimal|Decimal|Both)", command):
            self.numbers = command.split()[1]
            return ""
        selected = re.fullmatch(r'mach set "([^"\r\n]+)"', command)
        if selected and self.commands and self.commands[0] == f'mach create "{selected[1]}"':
            return ""
        if re.fullmatch(r"(?:sysbus\.)?[\w.]+ (?:Press|Release|PressAndRelease)", command):
            return self.operation(command)
        machine = re.fullmatch(r'mach (?:create|add)(?: "([^"\r\n]+)")?', command)
        if machine and not self.commands and not self.proc and not self.script:
            name = machine[1] or "machine"
            self.commands.append(f'mach create "{name}"' if machine[1] else "mach create")
            self.monitor_input.name_machine(name)
            return ""
        if self.commands and not self.started and not self.proc:
            tokens = words(command)
            if tokens[:2] == ["machine", "LoadPlatformDescription"] and len(tokens) == 3 and self.platform is None:
                self.platform = self.monitor_input.path(tokens[2])
                return ""
            prefix = "machine LoadPlatformDescriptionFromString "
            if command.startswith(prefix) and not self.image:
                arg = command[len(prefix):]
                if not ((arg.startswith('"""') and arg.endswith('"""') and len(arg) >= 6)
                        or re.fullmatch(r"""(['"])(?:\\[\s\S]|(?!\1)[^\\])*\1""", arg)):
                    raise Unsupported("invalid inline platform quoting")
                repl = unquote(arg)
                self.inline.append(repl)
                return ""
            if (not self.image and tokens[:1] == ["sysbus"] and len(tokens) == 3
                    and tokens[1] in {"LoadELF", "LoadHEX"} and (self.platform or self.inline)):
                value = tokens[2].removeprefix("@")
                if PUBLIC_ASSET.fullmatch(value):
                    self.fetch(value)
                    source = tokens[2]
                else:
                    source = at_path(self.monitor_input.path(tokens[2]))
                self.commands.append(f"sysbus {tokens[1]} {source}")
                self.image = True
                return ""
            if (tokens[:2] == ["sysbus", "LoadBinary"] and len(tokens) == 4
                    and (self.platform or self.inline)):
                value = tokens[2].lstrip("@")
                if PUBLIC_ASSET.fullmatch(value):
                    self.fetch(value)
                    source = "@" + value
                else:
                    source = at_path(self.monitor_input.path(tokens[2]))
                self.commands.append(f"sysbus LoadBinary {source} {tokens[3]}")
                self.image = True
                return ""
            if (self.image and any(item.startswith("sysbus LoadBinary ") for item in self.commands)
                    and re.fullmatch(r"(?:sysbus\.)?cpu[\w]* PC (?:0x[\da-fA-F]+|\d+)", command)):
                self.commands.append(command)
                return ""
            if self.image and re.fullmatch(r"sysbus\.cpu VectorTableOffset 0x[0-9a-fA-F]+", command):
                self.commands.append(command)
                return ""
        wiring = re.fullmatch(
            r"""machine LoadPlatformDescriptionFromString ["'](gpioPort[A-Z]):\s*\{\s*(\d+)\s*->\s*(gpioPort[A-Z])@(\d+)\s*\}["']""", command)
        if wiring:
            return self.operation("ConnectMonitorGpio " + " ".join(wiring.groups()))
        tokens = words(command)
        if len(tokens) == 3 and re.fullmatch(r"sysbus\.[\w.]+", tokens[0]):
            sensor, method, argument = tokens
            if method == "Illuminance":
                value = argument.removeprefix("illuminance=")
                try:
                    lux = Decimal(value)
                except InvalidOperation:
                    raise Unsupported(f"invalid illuminance {argument!r}")
                if not lux.is_finite():
                    raise Unsupported(f"invalid illuminance {argument!r}")
                return self.operation(f"{sensor} Illuminance {value}")
            if method == "FeedIlluminanceSamplesFromRESD":
                file = self.monitor_input.path(argument)
                return self.operation(f"{sensor} {method} {at_path(file)}")
        if re.fullmatch(r"(?:sysbus\.)?[\w.]+ (?:VendorSpecific1|Acceleration[XYZ]) \S+", command):
            return self.operation(command)
        if re.fullmatch(r"(?:sysbus\.)?[\w.]+\.led\d+ State", command):
            return self.operation(command, assertion=True)
        if (re.fullmatch(r"(?:sysbus\.)?[\w.]+ OnGPIO (?:0x[0-9a-fA-F]+|\d+) (?:true|false)", command, re.I)
                or re.fullmatch(r"showAnalyzer (?:sysbus\.)?[\w.]+(?: LoggingUartAnalyzer)?", command)):
            return self.operation(command)
        if re.fullmatch(r"(?:sysbus\.)?timer\d+ Frequency|sysbus IsPeripheralEnabled (?:sysbus\.)?timer\d+", command):
            return self.operation(command, assertion=True).strip()
        if re.fullmatch(r"sysbus LogPeripheralAccess (?:sysbus\.)?rcc|(?:sysbus\.)?rcc WriteDoubleWord \S+ \S+", command):
            return self.operation(command)
        if re.fullmatch(r"emulation RunFor \d+(?:\.\d+)?", command) and self.started:
            return self.operation(command)
        if command == "emulation GetTimeSourceInfo":
            return self.operation(command, assertion=True)
        scalar = r"(?:Read|Write)(?:Byte|Word|DoubleWord|QuadWord)"
        if self.using_bus and re.match(scalar + r" ", command):
            command = "sysbus " + command
        if re.fullmatch(r"sysbus " + scalar + r"(?: (?:0x[\da-fA-F]+|\d+)){1,2}", command):
            reading = command.startswith("sysbus Read")
            data = self.operation(command, assertion=reading)
            return self.format_number(data) if reading else data
        cpu = r"(?:sysbus\.)?(?:cpu|u54|e51)[\w]* "
        number = r"(?:0x[\da-fA-F]+|\d+)"
        assembly = re.fullmatch(
            rf'({cpu.strip()}) AssembleBlock (0x[0-9a-fA-F]+|[0-9]+|`(?:sysbus\.)?[\w.]+ PC`) (.+)',
            command, flags=re.DOTALL)
        if assembly:
            target, address, suffix = assembly.groups()
            arguments = re.fullmatch(
                r'("""[\s\S]*?"""|"[^"]*")(?:\s+(null|"[\w-]+"))?(?:\s+(true|false))?',
                suffix, flags=re.IGNORECASE)
            if not arguments:
                raise Unsupported("AssembleBlock needs quoted code and optional triple and dialect")
            body, triple, alternate = arguments.groups()
            if alternate and alternate.lower() == "true":
                raise Unsupported("alternate assembly dialect is unavailable")
            triple = triple.strip('"') if triple and triple.lower() != "null" else "default"
            marker = 3 if body.startswith('"""') else 1
            body = body[marker:-marker]
            if address.startswith("`"):
                address = self.operation(address[1:-1]).strip()
            return self.operation(
                f"AssembleMonitorBlock {target.removeprefix('sysbus.')} {address} {triple} {body.encode().hex()}")
        if re.fullmatch(cpu + r'ResetVector(?: ' + number + r')?', command):
            reading = command.endswith(" ResetVector")
            value = self.operation(command, assertion=reading)
            return self.format_number(value) if reading else value
        if re.fullmatch(cpu + r'Reset', command):
            return self.operation(command)
        if re.fullmatch(cpu + r'(?:MTVEC|STVEC)(?: ' + number + r')?', command):
            reading = command.endswith((" MTVEC", " STVEC"))
            value = self.operation(command, assertion=reading)
            return self.format_number(value) if reading else value
        if re.fullmatch(cpu + r'ExecutionMode(?: (?:SingleStep|Continuous))?', command):
            return self.operation(command, assertion=command.endswith(" ExecutionMode"))
        if re.fullmatch(cpu + r'(?:GetRegister "?[\w]+"?|SetRegister "?[\w]+"? ' + number
                        + r'|MSTATUS|MEPC|MCAUSE|MTVAL)', command):
            reading = " SetRegister " not in command
            value = self.operation(command, assertion=reading)
            return self.format_number(value) if reading else value
        if re.fullmatch(cpu + r"(?:PC(?: " + number + r")?|IsHalted(?: (?:true|false|True|False))?|"
                        r"Pause|Resume|IsPaused|Step(?: " + number + r")?|"
                        r"EvaluateConditionCode " + number + r'|GetRegister "?[cx]psr"?)', command):
            reading = command.endswith((" PC", " IsHalted", " IsPaused")) or any(
                name in command for name in (" EvaluateConditionCode ", " GetRegister "))
            if command.endswith(" Step"):
                command += " 1"
            value = self.operation(command, assertion=reading)
            return self.format_number(value) if value.lstrip().startswith("0x") else value
        if self.started and re.fullmatch(r"sysbus\.[\w.]+\.adc-channel\d+ Volts \d+(?:\.\d+)?", command):
            return self.operation(command)
        raise Unsupported(f"Execute Command: {command}")

    def format_number(self, value):
        raw = value.strip()
        num = int(raw, 16)
        if self.numbers == "Decimal":
            return f"{num}\r\n"
        hex_value = "0x" + raw[2:].upper()
        if self.numbers == "Both":
            hex_value += f" ({num})"
        return hex_value + "\r\n"

    def receive(self):
        fd = self.proc.stdout.fileno()
        deadline = time.monotonic() + 120

        def more():
            remaining = deadline - time.monotonic()
            if remaining <= 0 or not select.select([fd], [], [], remaining)[0]:
                raise subprocess.TimeoutExpired(self.proc.args, 120)
            chunk = os.read(fd, 65536)
            if not chunk:
                raise RuntimeError((self.out / "penv.stderr").read_text(errors="replace")
                                   or "penv closed monitor connection")
            self.buffer += chunk

        while b"\n" not in self.buffer:
            more()
        header, self.buffer = self.buffer.split(b"\n", 1)
        status, size = header.decode("ascii").split()
        size = int(size)
        while len(self.buffer) < size:
            more()
        payload, self.buffer = self.buffer[:size], self.buffer[size:]
        return status, payload.decode(errors="replace")

    def ensure_running(self):
        if self.proc is not None:
            return
        if not self.script and (not self.commands or not (self.platform or self.inline)):
            raise Unsupported("requires a single machine and platform")
        platform = self.platform
        if self.inline:
            platform = self.out / "platform.repl"
            source = self.inline
            if self.platform:
                source = [f"using {json.dumps(str(self.platform))}", *source]
            platform.write_text("\n".join(source) + "\n")
        cmd = [str(self.binary), "run", "--platform-root", str(self.root), "--monitor-stdio"]
        if self.log_capacity is not None:
            cmd.extend(["--monitor-log-capacity", str(self.log_capacity)])
        if self.script:
            cmd.extend(["--script", str(self.script), "--asset-dir", str(self.assets)])
        elif self.image:
            script = self.out / "boot.resc"
            script.write_text("\n".join([self.commands[0],
                              f"machine LoadPlatformDescription {at_path(platform)}",
                              *([self.log_level] if self.log_level else []),
                              *self.commands[1:]]) + "\n")
            cmd.extend(["--script", str(script), "--asset-dir", str(self.assets)])
        else:
            cmd.extend(["--platform", str(platform)])
        self.calls[-1].update(command=cmd, stage="launch")
        downloads = set()
        while True:
            with (self.out / "penv.stderr").open("ab") as stderr:
                self.proc = subprocess.Popen(cmd, cwd=self.out, stdin=subprocess.PIPE,
                                             stdout=subprocess.PIPE, stderr=stderr, bufsize=0)
            try:
                status, startup_output = self.receive()
                break
            except RuntimeError as exc:
                lines = str(exc).splitlines()
                error = lines[-1] if lines else str(exc)
                missing = re.search(re.escape(str(self.assets)) + r"/([^:\n]+): No such file", error)
                if missing and missing[1] in self.script_urls and missing[1] not in downloads:
                    self.close()
                    self.buffer = b""
                    downloads.add(missing[1])
                    self.fetch(self.script_urls[missing[1]])
                    continue
                if "monitor session cannot operate multiple machines" in error:
                    raise Unsupported(error) from exc
                if re.search(r"unsupported (?:platform model|peripheral|CPU|monitor command|command)|"
                             r"unknown (?:model|CPU)|commands after start are unsupported|unsupported.*(?:hook|AddHook)", error, re.I):
                    raise PenvUnsupported(str(exc)) from exc
                if re.search(r"No such file|Permission denied|missing firmware image", error, re.I):
                    raise
                raise PenvStartupError(str(exc)) from exc
        if status != "READY":
            raise RuntimeError(f"expected penv READY, got {status}")
        self.process_command = cmd
        self.calls[-1].update(penv_ready=True, stage="emulation")
        if self.log_level and not self.image:
            self.operation(self.log_level)
        return startup_output

    def operation(self, command, assertion=False):
        self.ensure_running()
        begin = time.monotonic()
        try:
            self.proc.stdin.write((command + "\n").encode())
            status, value = self.receive()
        except (BrokenPipeError, RuntimeError) as exc:
            raise PenvExecutionError(f"penv monitor stopped during {command}: {exc}") from exc
        measurement = dict(command=self.process_command, operation=command, reply=status, value=value,
                           elapsed_s=time.monotonic() - begin, stage="emulation")
        if status == "MATCH" or status == "OK" and assertion:
            measurement["result"] = "pass"
            self.measurements.append(measurement)
        elif status == "FAIL" or status == "ERROR" and re.search(
                r"LED .* did not reach state .* \(Limit\)", value):
            measurement["result"] = "fail"
            self.measurements.append(measurement)
        elif status not in {"OK", "UNSUPPORTED"}:
            measurement["result"] = "execution_error"
            self.measurements.append(measurement)
        self.calls[-1]["monitor"] = measurement
        with (self.out / "monitor.jsonl").open("a") as log:
            print(json.dumps(measurement), file=log)
        if status == "UNSUPPORTED" and any(
                f"could not start {tool}" in value for tool in ("llvm-mc", "ld.lld")):
            raise OSError(value)
        if status == "UNSUPPORTED" or status == "ERROR" and re.search(
                r"UART peripheral not found|unknown STM32 UART|unknown LED|not a Cortex-M CPU|"
                r"requires a Cortex-M platform|unsupported monitor", value, re.I):
            raise PenvUnsupported(value)
        if measurement.get("result") == "fail":
            raise AssertionFailed(value)
        if status not in {"OK", "MATCH"}:
            raise PenvExecutionError(value)
        return value

    @staticmethod
    def duration(value):
        try:
            duration = Decimal(str(value))
            if duration.is_finite() and 0 <= duration <= 120:
                return format(duration, "f")
        except InvalidOperation:
            pass
        raise Unsupported(f"invalid virtual timeout {value!r}")

    @staticmethod
    def literal(value):
        if not value or any(c in value for c in '\r\n"'):
            raise Unsupported("requires a nonempty single-line literal")
        return '"' + value + '"'

    @staticmethod
    def option(arg, name):
        if arg.startswith(name + "="):
            return arg.split("=", 1)[1]
        raise Unsupported(f"unsupported Robot keyword option {arg}")

    def terminal(self, uart, *args):
        if self.uart or not self.image or not re.fullmatch(r"(?:sysbus\.)?[\w.]+", uart):
            raise Unsupported(f"Create Terminal Tester: {uart}")
        options = self.log_options(args, {"timeout", "defaultPauseEmulation"}, ("timeout",))
        if "defaultPauseEmulation" in options:
            self.log_bool(options["defaultPauseEmulation"])
        self.uart_timeout = self.duration(options.get("timeout", self.timeout))
        result = int(self.operation(f"CreateTerminalTester {uart}").strip())
        self.uart = uart
        return result

    def start(self):
        if self.started or not self.image:
            raise Unsupported("Start Emulation needs a loaded image on one machine")
        self.ensure_running()
        self.started = True

    def wait_for(self, verb, expected, *args):
        if not self.uart:
            raise Unsupported("requires a terminal tester")
        options = self.log_options(args, {"timeout", "treatAsRegex", "includeUnfinishedLine"}, ("timeout",))
        deadline = self.duration(options.get("timeout", self.uart_timeout))
        regex = self.log_bool(options.get("treatAsRegex", "false"))
        self.started = True
        if self.log_bool(options.get("includeUnfinishedLine", "false")):
            if verb == "ShouldNotBeOnUart":
                raise Unsupported("unfinished UART line absence is unsupported")
            verb = "WaitForTesterPrompt"
        line = self.operation(f"{verb} {self.literal(expected)} timeout={deadline} treatAsRegex={str(regex).lower()}")
        match = re.search(expected, line) if regex else None
        return {"Line": line, "Groups": list(match.groups()) if match else []}

    def wait(self, expected, *args):
        return self.wait_for("WaitForLineOnUart", expected, *args)

    def prompt(self, expected, *args):
        return self.wait_for("WaitForTesterPrompt", expected, *args)

    def uart_absence(self, expected, *args):
        self.wait_for("ShouldNotBeOnUart", expected, *args)

    def write_char(self, char):
        if not self.started or not self.uart or len(char) != 1 or not char.isascii():
            raise Unsupported("requires one ASCII character and a running UART tester")
        self.operation(f"{self.uart} WriteLine {self.literal(char)} None")
        return {"Line": ""}

    def uart_idle(self, timeout):
        if not self.uart:
            raise Unsupported("requires a UART tester")
        self.started = True
        self.operation(f"TestMonitorUartIdle {self.duration(timeout)}")

    def write_line(self, content, option="waitForEcho=true"):
        if not self.started or not self.uart:
            raise Unsupported("requires a running UART tester")
        echo = self.option(option, "waitForEcho").lower()
        if echo not in {"true", "false"}:
            raise Unsupported(option)
        self.operation(f"{self.uart} WriteLine {self.literal(content)} CR")
        if echo == "true":
            self.prompt(content)
        return {"Line": ""}

    def create_led(self, led, timeout=None):
        if self.led or not re.fullmatch(r"(?:sysbus\.)?[\w.]+", led):
            raise Unsupported(f"Create LED Tester: {led}")
        if timeout is not None and "=" in timeout:
            timeout = self.option(timeout, "defaultTimeout")
        self.led_timeout = self.duration(timeout if timeout is not None else 0)
        self.led = led
        return 0

    def hold_led(self, state, timeout, hold):
        self.assert_led(state, timeout, self.duration(hold))

    def assert_led(self, state, timeout=None, hold=None):
        state = state.lower()
        if not self.started or not self.led or state not in {"true", "false"}:
            raise Unsupported("LED state needs a running machine and attached tester")
        if timeout is not None and "=" in timeout:
            timeout = self.option(timeout, "timeout")
        deadline = self.duration(timeout if timeout is not None else self.led_timeout)
        interval = "" if hold is None else f" {hold}"
        self.operation(f"waitForLed {self.led} {state} {deadline}{interval}", assertion=True)
