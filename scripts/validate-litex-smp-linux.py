#!/usr/bin/env python3
"""Drive the fixed LiteX SMP Linux Robot case through the live UART."""

import argparse
import os
from pathlib import Path
import re
import selectors
import subprocess
import time


def verify(penv, renode, assets, artifacts):
    artifacts.mkdir(parents=True, exist_ok=True)
    command = [
        str(penv), "run", "--script", str(renode / "scripts/single-node/litex_vexriscv_smp.resc"),
        "--platform-root", str(renode), "--asset-dir", str(assets),
        "--litex-uart-stdio", "--max-steps", "1600000000",
    ]
    with (
        (artifacts / "litex-smp.stderr.log").open("wb") as stderr,
        (artifacts / "litex-smp.uart.log").open("wb") as uart,
        selectors.DefaultSelector() as selector,
        subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=stderr) as process,
    ):
        selector.register(process.stdout, selectors.EVENT_READ)
        deadline = time.monotonic() + 180
        pending = b""

        def expect(pattern):
            nonlocal pending
            while (match := re.search(pattern, pending)) is None:
                remaining = deadline - time.monotonic()
                if remaining <= 0 or not selector.select(remaining):
                    raise RuntimeError(f"SMP UART timed out waiting for {pattern!r}")
                chunk = os.read(process.stdout.fileno(), 4096)
                if not chunk:
                    raise RuntimeError(f"SMP UART closed waiting for {pattern!r}")
                uart.write(chunk)
                pending += chunk
            print(f"UART matched {match.group()!r}", flush=True)
            pending = pending[match.end():]

        def send(line):
            print(f"UART input {line!r}", flush=True)
            process.stdin.write(line + b"\r")
            process.stdin.flush()

        try:
            expect(rb"Linux version 5\.0\.9[^\n]*\n")
            expect(rb"smp: Brought up 1 node, 4 CPUs[^\n]*\n")
            expect(rb"Welcome to Buildroot\r*\n")
            expect(rb"buildroot login:")
            send(b"root")
            color = rb"(?:\x1b\[[0-9;]*m)*"
            expect(rb"root@buildroot" + color + rb":" + color + rb"~" + color + rb"#")
            send(b"cat /proc/cpuinfo")
            for hart in range(4):
                expect(rb"processor\t: " + str(hart).encode() + rb"\r*\n")
        finally:
            if process.poll() is None:
                process.terminate()
            try:
                status = process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                status = process.wait()
            (artifacts / "litex-smp.exit").write_text(f"{status}\n")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("penv", "renode", "assets", "artifacts"):
        parser.add_argument(name, type=Path)
    args = parser.parse_args()
    verify(args.penv.resolve(), args.renode.resolve(), args.assets.resolve(), args.artifacts.resolve())
