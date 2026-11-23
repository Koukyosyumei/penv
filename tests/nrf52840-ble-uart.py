"""Apply NRF52840.robot's ordered waits to two independent UART buffers."""

import argparse
from collections import deque
import re
import subprocess
import sys


STEP_HZ = 100_000_000
QUANTUM = STEP_HZ // 100_000
LINE = re.compile(rb"^\[step (\d+)\] (primary|peer): (.*)\n$")


def check(lines, hci, log=None):
    boot = b"*** Booting Zephyr OS build v3.6.0 ***" if hci else b"Booting Zephyr"
    connection = b"Connected:" if hci else b"Connected: C0:00:AA:BB:CC:DD"
    waits = deque((
        (b"primary", boot),
        (b"peer", boot),
        (b"primary", b"Bluetooth initialized"),
        (b"peer", b"Bluetooth initialized"),
        (b"primary", b"Scanning successfully started"),
        (b"peer", b"Advertising successfully started"),
        (b"primary", connection),
        (b"peer", b"Connected"),
        (b"peer", b"HRS notifications enabled"),
        (b"primary", b"[SUBSCRIBED]"),
        (b"primary", b"[NOTIFICATION]"),
    ))
    buffers = {b"primary": deque(), b"peer": deque()}
    started = STEP_HZ if hci else 0
    now = waiting_since = started
    count = 0
    for line in lines:
        if log is not None:
            log.write(line)
        event = LINE.match(line)
        if event is None:
            continue
        when, sender, content = event.groups()
        step = int(when)
        if step < now or step % QUANTUM != 0:
            raise ValueError(f"invalid synchronized UART observation at step {step}")
        now = step
        if now - waiting_since > 8 * STEP_HZ:
            break
        buffers[sender].append((step, content))

        while waits:
            node, expected = waits[0]
            while buffers[node]:
                observed, message = buffers[node].popleft()
                if expected in message:
                    break
            else:
                break
            waits.popleft()
            count += 1
            waiting_since = now
            print(
                f"UART wait {count} at {now / STEP_HZ:.5f} s "
                f"({node.decode()} line at {observed / STEP_HZ:.5f} s): "
                f"{expected.decode()}",
                flush=True,
            )
            if not waits:
                if not hci and not re.search(rb"\blength 2\b", message):
                    raise ValueError("expected a two-byte Heart Rate Measurement")
                return

    node, expected = waits[0]
    raise ValueError(f"missing {node.decode()} UART line: {expected.decode()}")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("case", choices=("heart-rate", "hci"))
    parser.add_argument("log")
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    hci = args.case == "hci"
    if not args.command:
        with open(args.log, "rb") as log:
            check(log, hci)
        return

    command = args.command
    if command[0] == "--":
        command = command[1:]
    with open(args.log, "wb") as log, subprocess.Popen(
        command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT
    ) as simulator:
        try:
            check(simulator.stdout, hci, log)
        finally:
            if simulator.poll() is None:
                simulator.terminate()


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError) as error:
        sys.exit(str(error))
