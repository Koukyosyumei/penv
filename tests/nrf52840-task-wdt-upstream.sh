#!/bin/sh
# Match the NRF52840.robot watchdog cases against --uart-steps output.
set -eu
exec python3 - "$@" <<'PY'
import re
import subprocess
import sys

TASK = (
    b"Task watchdog sample application.",
    b"Main thread still alive...",
    b"Control thread started.",
    b"Main thread still alive...",
    b"Main thread still alive...",
    b"Main thread still alive...",
    b"Control thread getting stuck...",
    b"Main thread still alive...",
    b"Task watchdog channel 1 callback, thread: control",
    b"Resetting device...",
)
MODES = {
    "task": (100_000_000, TASK * 2),
    "hardware": (1_000_000, (b"Watchdog sample application",) * 2),
}
STAMP = re.compile(rb"^\[step ([0-9]+)\] (.*)$")


def match_uart(lines, rate, expected, log=None):
    index = last_match = last_step = 0
    for line in lines:
        if log is not None:
            log.write(line)
        if not line.endswith(b"\n"):
            continue
        match = STAMP.match(line)
        if match is None:
            continue
        step = int(match[1])
        if step < last_step:
            raise ValueError(f"UART timestamp moved backwards to step {step}")
        last_step = step
        if step - last_match > 8 * rate:
            break
        # Robot's default matcher consumes through the first matching line.
        if expected[index] in match[2]:
            index += 1
            last_match = step
            print(f"Match {index} at {step / rate:.9f} s: {expected[index - 1].decode()}", flush=True)
            if index == len(expected):
                return
    raise ValueError(f"Expected {expected[index].decode()} after {last_match / rate:.9f} s within 8 virtual seconds")


def run(log_path, mode, command):
    rate, expected = MODES[mode]
    if not command:
        with open(log_path, "rb") as log:
            match_uart(log, rate, expected)
        return
    if command[0] != "--" or len(command) == 1:
        raise ValueError("Expected -- followed by a simulator command")
    with open(log_path, "wb") as log:
        with subprocess.Popen(command[1:], stdout=subprocess.PIPE, stderr=subprocess.STDOUT) as simulator:
            try:
                match_uart(simulator.stdout, rate, expected, log)
            finally:
                # Stop consuming time after the last Robot assertion.
                if simulator.poll() is None:
                    simulator.terminate()


if len(sys.argv) < 2 or (len(sys.argv) > 2 and sys.argv[2] not in MODES):
    sys.exit("Usage: UART_LOG [task|hardware [-- SIMULATOR ARG ...]]")
try:
    run(sys.argv[1], sys.argv[2] if len(sys.argv) > 2 else "task", sys.argv[3:])
except (OSError, ValueError) as error:
    sys.exit(str(error))
PY
