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

from penv_robot import AssertionFailed, Endpoint, PenvExecutionError, PenvUnsupported, Unsupported


class LiveEndpoint(Endpoint):
    proc = None
    pending_state = None
    keywords = dict(Endpoint.keywords, **{
        "Wait For Prompt On Uart": "prompt",
        "Write Line To Uart": "write_line",
        "Create LED Tester": "create_led",
        "Assert LED State": "assert_led",
        "Execute Script": "execute_script",
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
            "Execute Command", "Execute Script", "Create Terminal Tester", "Start Emulation",
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

    def execute_script(self, script):
        if self.commands or self.proc or self.script:
            raise Unsupported("Execute Script requires a fresh monitor")
        candidate = (self.root / script.removeprefix("@")).resolve()
        if not candidate.is_relative_to(self.root):
            raise Unsupported("Execute Script requires a pinned upstream script")
        if not candidate.is_file():
            raise FileNotFoundError(candidate)
        self.script = candidate
        self.image = True
        self.ensure_running()
        return ""

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
        if command == "using sysbus":
            self.using_bus = True
            return ""
        if re.fullmatch(r"numbersMode (Hexadecimal|Decimal|Both)", command):
            self.numbers = command.split()[1]
            return ""
        if not self.commands and re.fullmatch(r'mach create(?: "[^"\r\n]+")?', command):
            self.commands.append(command)
            return ""
        if self.commands and not self.started and not self.proc:
            match = re.fullmatch(r"machine LoadPlatformDescription (@[\w./-]+)", command)
            if match and self.platform is None:
                candidate = (self.root / match[1][1:]).resolve()
                if candidate.is_relative_to(self.root) and candidate.is_file():
                    self.platform = candidate
                    return ""
            prefix = "machine LoadPlatformDescriptionFromString "
            if command.startswith(prefix) and self.platform is None and not self.image:
                arg = command[len(prefix):]
                if arg.startswith('"""') and arg.endswith('"""'):
                    repl = arg[3:-3]
                elif arg.startswith('"') and arg.endswith('"') and "\n" not in arg:
                    repl = arg[1:-1].replace(r'\"', '"')
                else:
                    raise Unsupported("invalid inline platform quoting")
                self.inline.append(repl)
                return ""
            if not self.image and re.fullmatch(
                r"sysbus Load(?:HEX|ELF) @https://dl\.antmicro\.com/projects/renode/[\w.-]+", command
            ) and (self.platform or self.inline):
                self.fetch(command.split("@", 1)[1])
                self.commands.append(command)
                self.image = True
                return ""
            if self.image and re.fullmatch(r"sysbus\.cpu VectorTableOffset 0x[0-9a-fA-F]+", command):
                self.commands.append(command)
                return ""
        scalar = r"(?:Read|Write)(?:Byte|Word|DoubleWord|QuadWord)"
        if self.using_bus and re.match(scalar + r" ", command):
            command = "sysbus " + command
        if re.fullmatch(r"sysbus " + scalar + r"(?: (?:0x[\da-fA-F]+|\d+)){1,2}", command):
            reading = command.startswith("sysbus Read")
            data = self.operation(command, assertion=reading)
            return self.format_number(data) if reading else data
        cpu = r"(?:sysbus\.)?cpu[\w]* "
        number = r"(?:0x[\da-fA-F]+|\d+)"
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
            platform.write_text("\n".join(self.inline) + "\n")
        cmd = [str(self.binary), "run", "--platform-root", str(self.root), "--monitor-stdio"]
        if self.script:
            cmd.extend(["--script", str(self.script), "--asset-dir", str(self.assets)])
        elif self.image:
            script = self.out / "boot.resc"
            script.write_text("\n".join([self.commands[0],
                              f"machine LoadPlatformDescription @{platform}",
                              *self.commands[1:]]) + "\n")
            cmd.extend(["--script", str(script), "--asset-dir", str(self.assets)])
        else:
            cmd.extend(["--platform", str(platform)])
        self.calls[-1]["command"] = cmd
        with (self.out / "penv.stderr").open("wb") as stderr:
            self.proc = subprocess.Popen(cmd, cwd=self.out, stdin=subprocess.PIPE,
                                         stdout=subprocess.PIPE, stderr=stderr, bufsize=0)
        try:
            status, _ = self.receive()
        except RuntimeError as exc:
            if re.search(r"unsupported (?:platform model|peripheral|CPU|monitor command|command)|"
                         r"unknown (?:model|CPU)", str(exc), re.I):
                raise PenvUnsupported(str(exc)) from exc
            raise
        if status != "READY":
            raise RuntimeError(f"expected penv READY, got {status}")
        self.process_command = cmd
        self.calls[-1]["penv_ready"] = True

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
        if status == "UNSUPPORTED" or status == "ERROR" and re.search(
                r"UART peripheral not found|unknown STM32 UART|unknown LED", value, re.I):
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

    def terminal(self, uart, timeout=None):
        if self.uart or not self.image or not re.fullmatch(r"(?:sysbus\.)?[\w.]+", uart):
            raise Unsupported(f"Create Terminal Tester: {uart}")
        if timeout is not None and "=" in timeout:
            timeout = self.option(timeout, "timeout")
        self.uart_timeout = self.duration(timeout if timeout is not None else self.timeout)
        result = int(self.operation(f"CreateTerminalTester {uart}").strip())
        self.uart = uart
        return result

    def start(self):
        if self.started or not self.image:
            raise Unsupported("Start Emulation needs a loaded image on one machine")
        self.ensure_running()
        self.started = True

    def wait_for(self, verb, expected, timeout):
        if not self.uart:
            raise Unsupported("requires a terminal tester")
        if timeout is not None and "=" in timeout:
            timeout = self.option(timeout, "timeout")
        deadline = self.duration(timeout if timeout is not None else self.uart_timeout)
        self.started = True
        return {"Line": self.operation(f"{verb} {self.literal(expected)} timeout={deadline}")}

    def wait(self, expected, timeout=None):
        return self.wait_for("WaitForLineOnUart", expected, timeout)

    def prompt(self, expected, timeout=None):
        return self.wait_for("WaitForTesterPrompt", expected, timeout)

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

    def assert_led(self, state, timeout=None):
        state = state.lower()
        if not self.started or not self.led or state not in {"true", "false"}:
            raise Unsupported("LED state needs a running machine and attached tester")
        if timeout is not None and "=" in timeout:
            timeout = self.option(timeout, "timeout")
        deadline = self.duration(timeout if timeout is not None else self.led_timeout)
        self.operation(f"waitForLed {self.led} {state} {deadline}", assertion=True)
