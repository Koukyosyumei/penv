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
    rust_monitor = False
    keywords = dict(Endpoint.keywords, **MemoryKeywords.memory_keywords, **{
        "Wait For Prompt On Uart": "prompt",
        "Wait For Next Line On Uart": "wait_for_next_uart_line",
        "Wait For Bytes On Uart": "wait_for_uart_bytes",
        "PC Should Be Equal": "assert_program_counter",
        "Register Should Be Equal": "assert_cpu_register",
        "Set Default Tester": "set_default_tester",
        "Clear Terminal Tester Report": "clear_terminal_report",
        "Register Failing Uart String": "register_failing_uart",
        "Unregister Failing Uart String": "unregister_failing_uart",
        "Should Not Be On Uart": "uart_absence",
        "Create Log Tester": "create_log_tester",
        "Wait For Log Entry": "wait_for_log",
        "Should Not Be In Log": "log_absence",
        "Clear Log Tester History": "clear_log_history",
        "Register Failing Log String": "register_failing_log",
        "Unregister Failing Log String": "unregister_failing_log",
        "Write Line To Uart": "write_line",
        "Write Char On Uart": "write_char",
        "Write To Uart": "write_to_uart",
        "Send Key To Uart": "send_key_to_uart",
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
        self.log_pause = False
        self.memory_symbols = {}
        self.monitor_input = MonitorInput(self.root, self.out, self.assets)
        self.replaying = False
        self.inline = []
        self.using_bus = False
        self.numbers = "Hexadecimal"
        self.uart_testers = {}
        self.default_tester = None
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
        if self.rust_monitor and not self.replaying and kind == "Serialization":
            with tempfile.NamedTemporaryFile(dir=self.out, prefix="state-", suffix=".save", delete=False) as file:
                snapshot = Path(file.name)
            try:
                self.operation(f"SaveRestorableMonitorSnapshot {at_path(snapshot)}", assertion=True)
            except Exception:
                snapshot.unlink(missing_ok=True)
                raise
            self.pending_state = state, snapshot
            return
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
        if self.proc or self.script or any(
                not (self.rust_monitor and cmd.startswith(("SetDefaultUartTimeout ", "AttachMonitorLog ")))
                for cmd in self.commands):
            raise Unsupported("Requires must precede machine operations")
        if state not in self.states:
            raise Unsupported(f"required state {state!r} unavailable")
        recipe = self.states[state]
        if isinstance(recipe, Path):
            self.commands.append(f"RestoreMonitorSnapshot {at_path(recipe)}")
            self.ensure_running()
            return
        self.replaying = True
        try:
            for keyword, args in recipe:
                getattr(self, self.keywords[keyword])(*args)
        finally:
            self.replaying = False

    def assert_program_counter(self, value, *args):
        self.assert_cpu_value(None, value, args)

    def assert_cpu_register(self, register, value, *args):
        self.assert_cpu_value(str(register), value, args)

    def assert_cpu_value(self, register, value, args):
        positional = ("machine", "cpuName") if register is None else ("machine", "cpuName", "message")
        options = self.log_options(args, set(positional), positional)
        if not self.rust_monitor:
            raise Unsupported("CPU assertion requires the Rust monitor")
        cpu = options.get("cpuName") or "cpu"
        if not re.fullmatch(r"[\w.]+", cpu):
            raise Unsupported("invalid CPU name")
        if machine := options.get("machine"):
            self.operation(f"mach set {self.literal(machine)}")
        expected = int(str(value), 16 if str(value).lower().startswith("0x") else 10)
        command = (f"AssertMonitorPC {cpu} {expected}" if register is None else
                   f"AssertMonitorRegister {cpu} {self.literal(register)} {expected}")
        try:
            self.operation(command, assertion=True)
        except AssertionFailed as exc:
            if "message" in options:
                raise AssertionFailed(f"{options['message']}: {exc}") from exc
            raise

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
        self.log_pause = self.log_bool(options.get("defaultPauseEmulation", "false"))
        if self.log_pause and not self.rust_monitor:
            raise Unsupported("log tester pause-on-match requires the Rust monitor")
        self.log_timeout = self.duration(timeout)
        self.log_capacity = int(options.get("maxLogEntries", "1000000"))
        if not 1 <= self.log_capacity <= 1000000:
            raise Unsupported("log tester capacity outside supported range")
        command = f"AttachMonitorLog {self.log_capacity}"
        if self.proc:
            self.operation(command)
        elif self.rust_monitor:
            self.commands.append(command)

    def wait_for_log(self, pattern, *args):
        positional = ("timeout", "keep", "treatAsRegex", "pauseEmulation", "level", "startEmulation")
        return self.check_log("WaitForMonitorLog", pattern, args, positional)

    def log_absence(self, pattern, *args):
        positional = ("timeout", "treatAsRegex", "pauseEmulation", "level")
        return self.check_log("AssertAbsentMonitorLog", pattern, args, positional)

    def register_failing_log(self, pattern, *args):
        self.change_failing_log("RegisterFailingMonitorLog", pattern, args)

    def unregister_failing_log(self, pattern, *args):
        self.change_failing_log("UnregisterFailingMonitorLog", pattern, args)

    def change_failing_log(self, command, pattern, args):
        if self.log_timeout is None:
            raise AssertionFailed("Log tester is not available")
        options = self.log_options(args, {"treatAsRegex"}, ("treatAsRegex",))
        kind = "regex" if self.log_bool(options.get("treatAsRegex", "false")) else "literal"
        self.operation(f"{command} {kind} {pattern.encode().hex() or '-'}")

    def clear_log_history(self):
        if self.log_timeout is None:
            raise AssertionFailed("Log tester is not available")
        if self.proc:
            self.operation("ClearMonitorLog")
        elif self.rust_monitor:
            self.commands.append("ClearMonitorLog")

    def check_log(self, command, pattern, args, positional):
        if self.log_timeout is None:
            raise AssertionFailed("Log tester is not available")
        options = self.log_options(args, set(positional), positional)
        pause = self.log_bool(options["pauseEmulation"]) if "pauseEmulation" in options else self.log_pause
        if pause and not self.rust_monitor:
            raise Unsupported("log tester pause-on-match requires the Rust monitor")
        duration = self.duration(options.get("timeout", self.log_timeout))
        timeout = f"{duration}s" if self.rust_monitor else int(Decimal(duration) * 1000)
        starts = self.log_bool(options.get("startEmulation", "true"))
        if not self.rust_monitor and timeout and not (starts or self.started):
            raise Unsupported("cannot wait for virtual time while emulation is stopped")
        keep = "keep" if self.log_bool(options.get("keep", "false")) else "consume"
        kind = "regex" if self.log_bool(options.get("treatAsRegex", "false")) else "literal"
        levels = {"Noisy": -1, "Debug": 0, "Info": 1, "Warning": 2, "Error": 3, "null": "any"}
        level = levels.get(options.get("level", "null"))
        if level is None:
            raise Unsupported("unknown log level")
        flags = ""
        if self.rust_monitor:
            mode = "start" if starts else "no-start"
            flags = f" {mode}" + (" pause" if pause else " continue")
        result = self.operation(
            f"{command} {timeout} {kind} {keep} {level} {pattern.encode().hex() or '-'}{flags}")
        if not self.rust_monitor and timeout and starts:
            self.started = True
        return result

    def download_file(self, uri):
        if not PUBLIC_ASSET.fullmatch(uri):
            raise Unsupported("Download File requires a versioned public Renode asset")
        self.fetch(uri)
        return str(self.assets / uri.rsplit("/", 1)[1])

    def execute_script(self, script):
        if self.rust_monitor:
            return self.execute("ExecuteScript " + at_path(Path(script.removeprefix("@"))))
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

    def inspect_startup(self, command, *, chain=False):
        startup = ("\n".join(self.commands) + "\n").encode()
        query = command.encode()
        frame = "PENV-INSPECT-CHAIN" if chain else "PENV-INSPECT"
        request = (f"{frame} {len(startup)}\n".encode() + startup
                   + f"PENV-COMMAND {len(query)}\n".encode() + query)
        argv = [str(self.binary), "run", "--platform-root", str(self.root),
                "--monitor-stdio", "--script", "-"]
        begin = time.monotonic()
        process = subprocess.run(argv, input=request, capture_output=True, cwd=self.out, timeout=10)
        if process.returncode:
            raise PenvExecutionError(process.stderr.decode(errors="replace").strip())
        header, _, payload = process.stdout.partition(b"\n")
        reply, length = header.decode().split()
        if len(payload) != int(length):
            raise PenvExecutionError("invalid Rust monitor inspection frame")
        value = payload.decode()
        measurement = dict(command=argv, operation=command, reply=reply, value=value,
                           elapsed_s=time.monotonic() - begin, stage="monitor_inspection")
        self.calls[-1]["monitor"] = measurement
        with (self.out / "monitor.jsonl").open("a") as log:
            print(json.dumps(measurement), file=log)
        if reply in {"OK", "REJECTED"}:
            self.measurements.append(dict(measurement, result="pass"))
        if reply not in {"OK", "DEFER"}:
            raise PenvExecutionError(value)
        return reply, value

    def execute(self, command, *args, machine=None):
        if isinstance(command, list):
            if not all(isinstance(item, str) for item in command):
                raise Unsupported("unsupported Execute Command fragments")
            command = " ".join(command)
        command = command.strip()
        if self.rust_monitor:
            if args:
                if len(args) != 1 or machine is not None or not isinstance(args[0], str):
                    raise Unsupported("expected at most one Execute Command machine argument")
                machine = args[0].removeprefix("machine=")
            if machine is not None and not isinstance(machine, str):
                raise Unsupported("Execute Command machine must be a name")
            select = f"mach set {self.literal(machine)}" if machine and machine.strip() else None
            observes_state = bool(
                re.fullmatch(r"(?:printenv|lastLog(?:\s+\S+)?|emulation\s+IsStarted)", command)
                or re.fullmatch(r"(?:emulation\s+(?:GetTimeSourceInfo|SingleStepBlocking)|machine\s+(?:GetTimeSourceInfo|UserState))", command)
                or re.match(r"(?:sysbus\s+Read(?:Byte|Word|DoubleWord|QuadWord)|[\w.]+\s+GetRegister)\s", command)
                or re.fullmatch(r"sysbus IsPeripheralEnabled [\w.]+", command)
                or re.fullmatch(r"[\w.]+ (?:Config|Address)CSRRead \S+", command)
                or re.fullmatch(r"[\w.]+\s+(?:PC|SP|MSTATUS|MEPC|MCAUSE|MTVAL|SEPC|ExecutionMode|Frequency|PerformanceInMips|CSRValidation)", command)
            )
            if self.proc is None:
                if select:
                    self.inspect_startup(select)
                    self.commands.append(select)
                if re.fullmatch(r"(?:mach|machine)\s+set\s+.+", command, re.DOTALL):
                    self.inspect_startup(command)
                if ";" in command:
                    reply, value = self.inspect_startup(command, chain=True)
                    if reply == "OK":
                        self.commands.append(command)
                        return value
                # Preserve immediate command results and register write rejections.
                if re.match(r"[\w.]+\s+(?:AssembleBlock|SetRegister)(?:\s|$)", command):
                    return self.operation(command, assertion=" AssembleBlock " in command)
                # Rust parses and evaluates these read-only startup queries.
                if (re.match(r"(?:echo|lastLog|printenv)(?:\s|$)", command)
                        or re.fullmatch(r"emulation\s+IsStarted", command)):
                    reply, value = self.inspect_startup(command)
                    if reply == "OK":
                        return value
                    return self.operation(command, assertion=True)
                if observes_state:
                    return self.operation(command, assertion=True)
                if (re.match(r"(?:include|i|ExecuteScript|log|Save|Load|runMacro)(?:\s|$)", command)
                        or re.match(r"machine\s+LoadPlatformDescription(?:\s|$)", command)
                        or re.fullmatch(r'logLevel\s+(?:\S+|"[^"]*")', command)):
                    reply, value = self.inspect_startup(command)
                    if reply == "OK":
                        self.commands.append(command)
                        return value
                self.commands.append(command)
                self.calls[-1]["stage"] = "queued_monitor_command"
                return ""
            if select:
                self.operation(select)
            return self.operation(command, assertion=observes_state)
        if args or machine is not None:
            raise Unsupported("Execute Command machine context requires the Rust monitor")
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
        if re.fullmatch(r"(?:sysbus\.)?cpu PerformanceInMips [0-9]+", command):
            return self.operation(command)
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
        run_for = re.fullmatch(r"""emulation RunFor (\d+(?:\.\d+)?|"\d+(?:\.\d+)?"|'\d+(?:\.\d+)?')""", command)
        if run_for and self.started:
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
        if re.fullmatch(cpu + r"VectorRegisterLength " + number, command):
            return self.operation(command)
        if match := re.fullmatch(rf"({cpu.strip()}) SP(?: ({number}))?", command):
            target, value = match.groups()
            if value is not None:
                return self.operation(f'{target} SetRegister "sp" {value}')
            return self.format_number(self.operation(f'{target} GetRegister "sp"', assertion=True))
        stack_hook = re.fullmatch(rf"({cpu.strip()}) AddPreStackAccessHook (.+)", command)
        if stack_hook:
            target, source = stack_hook.groups()
            source = unquote(source)
            if source != 'cpu.Log(LogLevel.Info, "StackAccess: 0x{0:X}, {1}, {2}", address, width, is_write)':
                raise Unsupported("unsupported pre-stack-access hook body")
            return self.operation(f"TraceStackAccess {target}")
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
                self.operation(command + " 1")
                value = self.operation(command[:-len("Step")] + "PC", assertion=True)
            else:
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
        startup = None
        if self.rust_monitor:
            startup = ("\n".join(self.commands) + "\n").encode("utf-8")
            (self.out / "boot.resc").write_bytes(startup)
        if not self.rust_monitor and not self.script and (not self.commands or not (self.platform or self.inline)):
            raise Unsupported("requires a single machine and platform")
        platform = self.platform
        if self.inline:
            platform = self.out / "platform.repl"
            source = self.inline
            if self.platform:
                source = [f"using {json.dumps(str(self.platform))}", *source]
            platform.write_text("\n".join(source) + "\n")
        cmd = [str(self.binary), "run", "--platform-root", str(self.root), "--monitor-stdio"]
        if not self.rust_monitor and self.log_capacity is not None:
            cmd.extend(["--monitor-log-capacity", str(self.log_capacity)])
        if startup is not None:
            cmd.extend(["--script", "-", "--asset-dir", str(self.assets)])
        elif self.script:
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
                if startup is not None:
                    self.send_monitor_frame("PENV-STARTUP", startup)
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
                missing_asset = re.search(r"monitor asset unavailable: (" + PUBLIC_ASSET.pattern + r")$", error)
                if missing_asset and missing_asset[1] not in downloads:
                    self.close()
                    self.buffer = b""
                    downloads.add(missing_asset[1])
                    self.fetch(missing_asset[1])
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

    def send_monitor_frame(self, name, body):
        payload = memoryview(f"{name} {len(body)}\n".encode() + body)
        while payload:
            count = self.proc.stdin.write(payload)
            if not count:
                raise RuntimeError("penv closed the monitor stream")
            payload = payload[count:]

    def operation(self, command, assertion=False, uart_captures=False):
        self.ensure_running()
        begin = time.monotonic()
        try:
            if self.rust_monitor:
                self.send_monitor_frame("PENV-COMMAND", command.encode())
            else:
                self.proc.stdin.write((command + "\n").encode())
            status, value = self.receive()
        except (BrokenPipeError, RuntimeError) as exc:
            raise PenvExecutionError(f"penv monitor stopped during {command}: {exc}") from exc
        groups = []
        binary_result = None
        timestamp_ms = None
        if status == "MATCHBYTES":
            content, *fields = value.split("\t")
            binary_result = {"Content": bytes.fromhex(content),
                             "Groups": [bytes.fromhex(field) for field in fields],
                             "Timestamp": 0.0}
            status = "MATCH"
        elif status in {"MATCHCAPTURES", "MATCHTIMEDUART"}:
            fields = value.split("\t")
            if status == "MATCHTIMEDUART":
                timestamp_ns, *fields = fields
                timestamp_ms = int(timestamp_ns) / 1_000_000
            line, *captures = fields
            value = bytes.fromhex(line).decode()
            groups = [bytes.fromhex(field).decode() for field in captures]
            status = "MATCH"
        measurement = dict(command=self.process_command, operation=command, reply=status, value=value,
                           elapsed_s=time.monotonic() - begin, stage="emulation")
        if uart_captures:
            measurement["groups"] = groups
            if timestamp_ms is not None:
                measurement["timestamp_ms"] = timestamp_ms
        if status in {"MATCH", "REJECTED"} or status == "OK" and assertion:
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
        if binary_result is not None:
            return binary_result
        if uart_captures:
            result = {"Line": value, "Groups": groups, "IsFailingString": False}
            if timestamp_ms is not None:
                result["Timestamp"] = timestamp_ms
            return result
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

    @classmethod
    def uart_text(cls, value):
        if any(char in value for char in '"\\') or not value:
            return "hex:" + value.encode().hex()
        return cls.literal(value)

    @staticmethod
    def option(arg, name):
        if arg.startswith(name + "="):
            return arg.split("=", 1)[1]
        raise Unsupported(f"unsupported Robot keyword option {arg}")

    def set_timeout(self, timeout):
        if self.rust_monitor:
            command = f"SetDefaultUartTimeout {self.duration(timeout)}"
            if self.proc is None:
                self.commands.append(command)
            else:
                self.operation(command)
        else:
            super().set_timeout(timeout)

    def terminal(self, uart, *args):
        if not (self.image or self.rust_monitor) or not re.fullmatch(r"(?:sysbus\.)?[\w.]+", uart):
            raise Unsupported(f"Create Terminal Tester: {uart}")
        positional = ("timeout",)
        allowed = {"timeout", "defaultPauseEmulation"}
        if self.rust_monitor:
            positional += ("machine", "endLineOption", "defaultPauseEmulation",
                           "defaultWaitForEcho", "defaultMatchNextLine", "binaryMode")
            allowed.update(positional)
        options = self.log_options(args, allowed, positional)
        if self.rust_monitor:
            options = {key: value for key, value in options.items() if value.lower() not in {"none", "null"}}
        machine = f" machine={self.literal(options['machine'])}" if options.get("machine") else ""
        pause = self.uart_bool_option(options, "defaultPauseEmulation")
        if not self.rust_monitor:
            pause = ""
        legacy_timeout = None
        if not self.rust_monitor:
            legacy_timeout = self.duration(options.get("timeout", self.timeout))
        deadline = ""
        if self.rust_monitor and "timeout" in options:
            deadline = f" timeout={self.duration(options['timeout'])}"
        echo = ""
        if "defaultWaitForEcho" in options:
            enabled = self.log_bool(options["defaultWaitForEcho"])
            echo = f" defaultWaitForEcho={str(enabled).lower()}"
        ending = options.get("endLineOption")
        if ending and ending not in {"TreatLineFeedAsEndLine", "TreatCarriageReturnAsEndLine"}:
            raise Unsupported(f"invalid endLineOption {ending!r}")
        delimiter = f" endLineOption={ending}" if ending else ""
        first = ""
        if "defaultMatchNextLine" in options:
            first = f" defaultMatchNextLine={str(self.log_bool(options['defaultMatchNextLine'])).lower()}"
        binary = ""
        if "binaryMode" in options:
            binary = f" binaryMode={str(self.log_bool(options['binaryMode'])).lower()}"
        result = int(self.operation(f"CreateTerminalTester {uart}{machine}{deadline}{echo}{delimiter}{first}{binary}{pause}").strip())
        if legacy_timeout is not None:
            self.uart_testers.setdefault(result, (uart, legacy_timeout))
        return result

    @staticmethod
    def parse_tester_id(value):
        if value is None or str(value).lower() in {"null", "none"}:
            return None
        try:
            return int(value)
        except ValueError as exc:
            raise AssertionFailed(f"Invalid tester ID: {value}") from exc

    def clear_terminal_report(self, *args):
        if not self.rust_monitor:
            raise Unsupported("clearing terminal tester history requires the Rust monitor")
        options = self.log_options(args, {"testerId"}, ("testerId",))
        tester = self.parse_tester_id(options.get("testerId"))
        selector = f" testerId={tester}" if tester is not None else ""
        self.operation(f"ClearTerminalTesterReport{selector}")

    def select_uart(self, value=None):
        tester_id = self.parse_tester_id(value)
        if tester_id is None:
            tester_id = self.default_tester
        if tester_id is None:
            if len(self.uart_testers) != 1:
                if self.uart_testers:
                    raise AssertionFailed("There is more than one tester available - please specify ID of the desired tester.")
                raise AssertionFailed("There are no testers available.")
            tester_id = next(iter(self.uart_testers))
        if tester_id not in self.uart_testers:
            raise AssertionFailed(f"Tester for given ID={tester_id} was not found.")
        return tester_id, self.uart_testers[tester_id]

    def set_default_tester(self, value):
        tester_id = self.parse_tester_id(value)
        if tester_id is not None and not self.rust_monitor:
            self.select_uart(tester_id)
        self.operation(f"SetDefaultTester {tester_id if tester_id is not None else 'null'}")
        if not self.rust_monitor:
            self.default_tester = tester_id

    def register_failing_uart(self, pattern, *args):
        self.change_failing_uart("RegisterFailingUartString", pattern, args)

    def unregister_failing_uart(self, pattern, *args):
        self.change_failing_uart("UnregisterFailingUartString", pattern, args)

    def change_failing_uart(self, command, pattern, args):
        positional = ("treatAsRegex", "testerId")
        options = self.log_options(args, set(positional), positional)
        if self.rust_monitor:
            tester_id = self.parse_tester_id(options.get("testerId"))
        else:
            tester_id, _ = self.select_uart(options.get("testerId"))
        selector = f" testerId={tester_id}" if tester_id is not None else ""
        regex = self.log_bool(options.get("treatAsRegex", "false"))
        self.operation(f"{command} {self.uart_text(pattern)}{selector} treatAsRegex={str(regex).lower()}")

    def start(self):
        if not self.rust_monitor and (self.started or not self.image):
            raise Unsupported("Start Emulation needs a loaded image on one machine")
        if self.rust_monitor:
            self.operation("start")
        else:
            self.ensure_running()
        self.started = True

    @classmethod
    def uart_bool_option(cls, options, name):
        if name not in options:
            return ""
        return f" {name}={str(cls.log_bool(options[name])).lower()}"

    def wait_for(self, verb, expected, *args):
        positional = ("timeout", "testerId", "treatAsRegex", "includeUnfinishedLine")
        if verb == "WaitForTesterPrompt":
            positional = ("testerId", "timeout", "treatAsRegex")
        if self.rust_monitor and verb != "ShouldNotBeOnUart":
            positional += ("pauseEmulation",)
        allowed = set(positional)
        if self.rust_monitor and verb == "WaitForLineOnUart":
            positional += ("matchNextLine",)
            allowed.add("matchNextLine")
        options = self.log_options(args, allowed, positional)
        if self.rust_monitor:
            tester_id = self.parse_tester_id(options.get("testerId"))
            timeout = options.get("timeout")
        else:
            tester_id, (_, timeout) = self.select_uart(options.get("testerId"))
            timeout = options.get("timeout", timeout)
        selector = f" testerId={tester_id}" if tester_id is not None else ""
        deadline = f" timeout={self.duration(timeout)}" if timeout is not None else ""
        regex = self.log_bool(options.get("treatAsRegex", "false"))
        self.started = True
        unfinished = self.log_bool(options.get("includeUnfinishedLine", "false"))
        if unfinished and not self.rust_monitor:
            if verb == "ShouldNotBeOnUart":
                raise Unsupported("unfinished UART line absence requires the Rust monitor")
            verb = "WaitForTesterPrompt"
        captures = regex and verb != "ShouldNotBeOnUart"
        command = f"{verb} {self.uart_text(expected)}{selector}{deadline} treatAsRegex={str(regex).lower()}"
        if captures:
            command += " captureGroups=true"
        if unfinished and self.rust_monitor:
            command += " includeUnfinishedLine=true"
        if "matchNextLine" in options:
            command += f" matchNextLine={str(self.log_bool(options['matchNextLine'])).lower()}"
        return self.operation(command + self.uart_bool_option(options, "pauseEmulation"), uart_captures=True)

    def wait_for_next_uart_line(self, *args):
        if not self.rust_monitor:
            raise Unsupported("next UART line requires the Rust monitor")
        positional = ("timeout", "testerId", "pauseEmulation")
        options = self.log_options(args, set(positional), positional)
        pause = self.uart_bool_option(options, "pauseEmulation")
        tester = self.parse_tester_id(options.get("testerId"))
        selector = f" testerId={tester}" if tester is not None else ""
        deadline = f" timeout={self.duration(options['timeout'])}" if "timeout" in options else ""
        self.started = True
        return self.operation(f"WaitForNextLineOnUart{selector}{deadline}{pause}", uart_captures=True)

    def wait_for_uart_bytes(self, pattern, *args):
        if not self.rust_monitor:
            raise Unsupported("binary UART wait requires the Rust monitor")
        positional = ("timeout", "testerId", "treatAsRegex", "pauseEmulation", "matchStart")
        options = self.log_options(args, set(positional), positional)
        pause = self.uart_bool_option(options, "pauseEmulation")
        tester = self.parse_tester_id(options.get("testerId"))
        selector = f" testerId={tester}" if tester is not None else ""
        deadline = f" timeout={self.duration(options['timeout'])}" if "timeout" in options else ""
        regex = self.log_bool(options.get("treatAsRegex", "false"))
        start = self.log_bool(options.get("matchStart", "false"))
        self.started = True
        return self.operation(f"WaitForBytesOnUart {self.uart_text(pattern)}{selector}{deadline}"
                              f" treatAsRegex={str(regex).lower()} matchStart={str(start).lower()}{pause}")

    def wait(self, expected, *args):
        return self.wait_for("WaitForLineOnUart", expected, *args)

    def prompt(self, expected, *args):
        return self.wait_for("WaitForTesterPrompt", expected, *args)

    def uart_absence(self, expected, *args):
        self.wait_for("ShouldNotBeOnUart", expected, *args)

    def write_char(self, char, *args):
        options = self.log_options(args, {"testerId"}, ("testerId",))
        if self.rust_monitor:
            if len(char) != 1 or ord(char) > 0xFFFF:
                raise Unsupported("expected one UTF-16 code unit")
            return self.write_tester_uart(char, "None", False, options.get("testerId"))
        if not self.started or len(char) != 1 or not char.isascii():
            raise Unsupported("requires one ASCII character and a running UART tester")
        _, (uart, _) = self.select_uart(options.get("testerId"))
        self.operation(f"{uart} WriteLine {self.uart_text(char)} None")
        return {"Line": ""}

    def write_to_uart(self, content, *args):
        if not self.rust_monitor:
            raise Unsupported("raw UART input requires the Rust monitor")
        options = self.log_options(args, {"testerId"}, ("testerId",))
        return self.write_tester_uart(content, "None", False, options.get("testerId"))

    def send_key_to_uart(self, value, *args):
        try:
            key = int(str(value), 16 if str(value).lower().startswith("0x") else 10)
        except ValueError as error:
            raise Unsupported("expected a byte value") from error
        if not 0 <= key <= 255:
            raise Unsupported("expected a byte value")
        return self.write_to_uart(chr(key), *args)

    def write_tester_uart(self, content, ending, echo, tester, pause=""):
        tester = self.parse_tester_id(tester)
        selector = f" testerId={tester}" if tester is not None else ""
        mode = "default" if echo is None else str(echo).lower()
        encoded = content.encode().hex()
        argument = f"hex:{encoded}" if encoded else self.uart_text(content)
        self.operation(f"WriteTesterUart {argument} {ending} {mode}{selector}{pause}")
        return {"Line": "", "Timestamp": 0.0, "Groups": [], "IsFailingString": False}

    def uart_idle(self, timeout, *args):
        positional = ("testerId", "pauseEmulation") if self.rust_monitor else ("testerId",)
        options = self.log_options(args, set(positional), positional)
        tester = self.parse_tester_id(options.get("testerId"))
        selector = f" testerId={tester}" if tester is not None else ""
        pause = self.uart_bool_option(options, "pauseEmulation")
        self.started = True
        self.operation(f"TestMonitorUartIdle {self.duration(timeout)}{selector}{pause}")

    def write_line(self, content="", *args):
        positional = ("testerId", "waitForEcho")
        if self.rust_monitor:
            positional += ("pauseEmulation",)
        options = self.log_options(args, set(positional), positional)
        if self.rust_monitor:
            echo = self.log_bool(options["waitForEcho"]) if "waitForEcho" in options else None
            return self.write_tester_uart(content, "CR", echo, options.get("testerId"),
                                          self.uart_bool_option(options, "pauseEmulation"))
        if not self.started:
            raise Unsupported("requires a running UART tester")
        echo = self.log_bool(options.get("waitForEcho", "true"))
        tester_id, (uart, _) = self.select_uart(options.get("testerId"))
        self.operation(f"{uart} WriteLine {self.uart_text(content)} CR")
        if echo:
            self.prompt(content, f"testerId={tester_id}")
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
