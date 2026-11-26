"""Robot Remote endpoint for single-machine STM32F072 UART boot sequences."""

import hashlib
import re
import subprocess
import tempfile
import time
import urllib.request
from xmlrpc.server import SimpleXMLRPCServer


class Unsupported(Exception):
    pass


class PenvUnsupported(Unsupported):
    pass


class PenvExecutionError(RuntimeError):
    pass


class PenvStartupError(RuntimeError):
    pass


class AssertionFailed(RuntimeError):
    pass


# Renode removes ANSI sequences from text before returning it through XML-RPC.
ANSI_ESCAPE = re.compile(r"(?:\x1b[@-_]|[\x80-\x9f])[0-?]*[ -/]*[@-~]")
INVALID_XML = re.compile(r"[\x00-\x08\x0b\x0c\x0e-\x1f\ud800-\udfff\ufffe\uffff]")


def robot_result(value):
    if isinstance(value, str):
        value = ANSI_ESCAPE.sub("", value)
        if INVALID_XML.search(value):
            raise Unsupported("Robot XML-RPC cannot return text containing XML-invalid characters")
    elif isinstance(value, dict):
        value = {key: robot_result(entry) for key, entry in value.items()}
    elif isinstance(value, (list, tuple)):
        value = [robot_result(entry) for entry in value]
    return value


SVD_ASSET_ROOT = "https://dl.antmicro.com/projects/renode/svd/"
SVD_PINS = {
    "ATSAME70Q21.svd": (6211467, "2906bec73828b9e7eeb6d66d23b612bc07529cfc985242d0fb2effcfa33e26bb"),
    "FE310.svd.gz": (5786, "a7d96f7f152eeb9f078c00909160d5902527e67aa54313da2895b8f9780fc333"),
    "MAX32650.svd.gz": (72918, "8e7a8c936b9d67ac3943dd49fa2bbc4e4551b9bcd32841f489ceda6dd3dab8f3"),
    "MK64F12.svd.gz": (193508, "081039ece2b33e25669cff2872d3513b18fd705348d67d019e6d819927250c01"),
    "NRF52840.svd.gz": (99060, "e5249c6a4a4bc66adbf644b97cff3b497cfdb9727eac0b3f77be576d2edd7db4"),
    "STM32F103.svd.gz": (47847, "c8f03172f4dee4c233b450a7ac49d7e44e75fb4097e8c462b0d028489a39b5ec"),
    "STM32F40x.svd.gz": (74195, "d71b3e11c870ec83346eb994e98a57c5eacdf0082d2949ed495d7a3828bf41ec"),
    "STM32F429x.svd.gz": (80817, "d0a24f24db3b0f8a5918f034b18bbf696d8a48dec6755f16dfdf748cff3781fc"),
    "STM32F7x6.svd.gz": (98356, "4e7103fe3543ba690fb2a99436d7464442dee9a4edc4bf9274f1807dc60214d8"),
    "STM32L0x1.svd": (588308, "34080feee6ba2ea5f4b66a5148e76b48f76be2cdb7b5f84748dd195ad2018b28"),
    "STM32L1xx.svd.gz": (35312, "a0efa57cc3efa18746e5f1c0d10e36add6efbf6f8fdf04e0d575167db9a00b31"),
    "STM32L552.svd": (2964972, "5974f84507f310518d636bb8eacfcad04bd329dba5e9642cf71a12ccfe47e4d5"),
    "STM32MP157x.svd.gz": (241382, "90347bfc95866175329d280f40e1d13748670c7478209ad006b6b1a16961b3b7"),
    "STM32W108.svd.gz": (11469, "2363c5a7d9b749c09048b8b7a2914b2b46bb8343374bfafa3235e1279baf226d"),
}


class Endpoint:
    keywords = {
        "Set Default Uart Timeout": "set_timeout",
        "Reset Emulation": "reset",
        "Set Crash On Abort": "crash_on_abort",
        "Close Log File": "close_log",
        "Execute Command": "execute",
        "Create Terminal Tester": "terminal",
        "Start Emulation": "start",
        "Wait For Line On Uart": "wait",
    }

    def __init__(self, root, binary, out, assets, negative):
        self.root, self.binary, self.out, self.assets = root, binary, out, assets
        self.negative = negative
        self.timeout = "8"
        self.calls = []
        self.measurements = []
        self.reset()

    def begin_case(self, out):
        self.out = out
        self.calls.clear()
        self.measurements.clear()
        self.reset()

    def get_keyword_names(self):
        return list(self.keywords)

    def run_keyword(self, name, args, kwargs=None):
        call = dict(keyword=name, args=args, kwargs=kwargs)
        self.calls.append(call)
        try:
            kwargs = kwargs or {}
            if kwargs and (name != "Execute Command" or set(kwargs) != {"machine"}):
                raise Unsupported("named arguments")
            result = robot_result(getattr(self, self.keywords[name])(*args, **kwargs))
        except PenvUnsupported as exc:
            call.update(status="unsupported", category="penv_unimplemented", message=str(exc))
        except (Unsupported, TypeError) as exc:
            call.update(status="unsupported", category="adapter_missing", message=str(exc))
        except AssertionFailed as exc:
            call.update(status="error", category="assertion_failed", message=str(exc))
        except PenvStartupError as exc:
            call.update(status="error", category="penv_execution_failure", stage="launch",
                        message=str(exc))
        except PenvExecutionError as exc:
            call.update(status="error", category="penv_execution_failure", message=str(exc))
        except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as exc:
            call.update(status="error", category="environment", message=str(exc))
        else:
            call.update(status="ok")
            return {"status": "PASS", "return": "" if result is None else result}
        error = INVALID_XML.sub("", ANSI_ESCAPE.sub("", call["message"]))
        return {"status": "FAIL", "error": error}

    def set_timeout(self, timeout):
        self.timeout = str(timeout)

    def reset(self):
        self.commands = []
        self.image = False
        self.tester = False
        self.started = False
        self.waited = False

    def crash_on_abort(self, value):
        if str(value).lower() != "true":
            raise Unsupported("Set Crash On Abort requires True")

    def close_log(self):
        # Upstream teardown closes the log even when SAVE_LOGS is disabled.
        pass

    def execute(self, command):
        if self.tester or self.started or "\n" in command:
            raise Unsupported(f"Execute Command: {command}")
        if not self.commands and command == "mach create":
            pass
        elif len(self.commands) == 1 and command == (
            "machine LoadPlatformDescription @platforms/boards/stm32f072b_discovery.repl"
        ):
            pass
        elif len(self.commands) == 2 and re.fullmatch(
            r"sysbus Load(?:HEX|ELF) @https://dl\.antmicro\.com/projects/renode/[\w.-]+",
            command,
        ):
            self.fetch(command.split("@", 1)[1])
            self.image = True
        elif self.image and len(self.commands) == 3 and re.fullmatch(
            r"sysbus\.cpu VectorTableOffset 0x[0-9a-fA-F]+", command
        ):
            pass
        else:
            raise Unsupported(f"Execute Command: {command}")
        self.commands.append(command)
        return ""

    def fetch(self, url):
        filename = url.rsplit("/", 1)[1]
        pin = re.search(r"-s_(\d+)-([0-9a-f]{40})$", filename)
        if pin:
            size, digest, hash_data = int(pin[1]), pin[2], hashlib.sha1
        elif url == SVD_ASSET_ROOT + filename and filename in SVD_PINS:
            size, digest = SVD_PINS[filename]
            hash_data = hashlib.sha256
        else:
            raise Unsupported(f"unversioned upstream asset: {url}")
        asset = self.assets / filename
        if not asset.exists():
            with urllib.request.urlopen(url, timeout=30) as response:
                data = response.read(size + 1)
        else:
            data = asset.read_bytes()
        if len(data) != size or hash_data(data).hexdigest() != digest:
            raise ValueError(f"upstream asset integrity check failed: {url}")
        if not asset.exists():
            with tempfile.NamedTemporaryFile(dir=self.assets, delete=False) as staged:
                staged.write(data)
            from pathlib import Path
            Path(staged.name).replace(asset)
        self.calls[-1].update(asset=str(asset), sha256=hashlib.sha256(data).hexdigest())

    def terminal(self, uart):
        if uart != "sysbus.usart1" or not self.image or self.tester:
            raise Unsupported(f"Create Terminal Tester: {uart}")
        self.commands.append(f"CreateTerminalTester {uart}")
        self.tester = True
        return 0

    def start(self):
        if not self.tester or self.started:
            raise Unsupported("Start Emulation requires one attached terminal")
        # Start and the following UART wait share one CLI invocation.
        self.commands.append("start")
        self.started = True

    def wait(self, expected):
        if (not self.started or self.waited or not expected
                or any(c in expected for c in '\r\n"')):
            raise Unsupported("requires a single literal line wait after Start Emulation")
        self.waited = True
        resc = self.out / "boot.resc"
        deadline = "0.000001" if self.negative else self.timeout
        self.commands.append(f'WaitForLineOnUart "{expected}" timeout={deadline}')
        resc.write_text("\n".join(self.commands) + "\n")
        cmd = [str(self.binary), "run", "--script", str(resc), "--platform-root",
               str(self.root), "--asset-dir", str(self.assets), "--stats",
               "--max-steps", str(int(float(deadline) * 100_000_000))]
        measurement = dict(command=cmd, expected=expected, virtual_timeout_s=deadline,
                           stdout="uart.bin", stderr="penv.stderr", stage="launch",
                           result="startup_error")
        self.measurements.append(measurement)
        start = time.monotonic()
        with (self.out / "uart.bin").open("wb") as stdout, (self.out / "penv.stderr").open("wb") as stderr:
            try:
                result = subprocess.run(cmd, cwd=self.out, stdout=stdout, stderr=stderr, timeout=120)
                measurement["exit_code"] = result.returncode
            except subprocess.TimeoutExpired:
                measurement["result"] = "harness_timeout"
                raise
            finally:
                measurement["elapsed_s"] = time.monotonic() - start
        stderr = (self.out / "penv.stderr").read_text(errors="replace")
        assertion_error = "Terminal tester failed! Line containing "
        if "ScriptComplete," in stderr or assertion_error in stderr:
            measurement.update(stage="emulation", result="execution_error")
        lines = (self.out / "uart.bin").read_bytes().split(b"\n")[:-1]
        observed = next((line.replace(b"\r", b"").decode(errors="replace") for line in lines
                         if expected.encode() in line.replace(b"\r", b"")), None)
        measurement["matched_line"] = observed
        if result.returncode == 0 and observed is not None and measurement["stage"] == "emulation":
            measurement["result"] = "pass"
            return {"Line": observed}
        if assertion_error in stderr and "was not received (Limit)" in stderr:
            measurement["result"] = "fail"
        raise RuntimeError(stderr.strip() or "missing successful penv UART observation")

    def server(self):
        server = SimpleXMLRPCServer(("127.0.0.1", 0), logRequests=False)
        server.register_instance(self)
        return server
