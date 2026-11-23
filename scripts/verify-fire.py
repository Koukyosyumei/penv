#!/usr/bin/env python3
"""Run the BeagleV-Fire SD boot case using the public Renode assets."""

import argparse
from pathlib import Path
import re
import subprocess
import time


def boot_script(upstream, destination):
    commands = upstream.read_text().splitlines()
    prefixes = (
        "using sysbus",
        "mach create",
        "machine LoadPlatformDescription ",
        "machine SdCardFromFile ",
        "sysbus LoadELF ",
    )
    boot = [line.strip() for line in commands if line.strip().startswith("$")]
    for prefix in prefixes:
        matches = [line.strip() for line in commands if line.strip().startswith(prefix)]
        if len(matches) != 1:
            raise RuntimeError(f"Expected one {prefix!r} command in {upstream}")
        boot.extend(matches)
    destination.write_text("\n".join(boot) + "\n")


def verify(args):
    args.logs.mkdir(parents=True, exist_ok=True)
    script = args.logs / "fire-sd-boot.resc"
    boot_script(args.renode / "scripts/single-node/beaglev-fire.resc", script)
    uart = args.logs / "fire.uart.log"
    stderr = args.logs / "fire.stderr.log"
    with uart.open("wb") as output, stderr.open("wb") as errors, uart.open("rb") as capture:
        guest = subprocess.Popen(
            [
                args.penv, "run", "--script", script, "--platform-root", args.renode,
                "--asset-dir", args.assets, "--uart-device", "mmuart0", "--uart-stdin",
                "--max-steps", str(args.max_steps),
            ],
            stdin=subprocess.PIPE, stdout=output, stderr=errors, bufsize=0,
        )
        pending = b""
        deadline = time.monotonic() + args.timeout

        def expect(pattern):
            nonlocal pending
            while True:
                running = guest.poll() is None
                pending += capture.read()
                if match := re.search(pattern, pending):
                    print(match[0].decode(errors="replace"), flush=True)
                    pending = pending[match.end():]
                    return
                if b"Kernel panic" in pending:
                    raise RuntimeError(f"Kernel panic before {pattern!r}. See {uart}")
                if not running or time.monotonic() >= deadline:
                    raise RuntimeError(f"Missing {pattern!r}. See {uart} and {stderr}")
                time.sleep(0.1)

        try:
            for pattern in (
                rb"Boot image passed CRC", rb"OpenSBI v1\.2", rb"U-Boot 2023\.07",
                rb"Model: BeagleBoard BeagleV-Fire", rb"Found U-Boot script /boot.scr",
                rb"Starting kernel", rb"Linux version[^\r\n]*",
                rb"console \[ttyS0\] enabled",
            ):
                expect(pattern)
            if args.until == "linux":
                return
            expect(rb"buildroot login:")
            guest.stdin.write(b"root\r")
            expect(rb"(?:^|\n)# ")
            guest.stdin.write(b"uname -m\r")
            expect(rb"\nriscv64\r*\n")
            guest.stdin.write(b"uname -n\r")
            expect(rb"\nbuildroot\r*\n")
            print("PASS Should Boot Buildroot From SD Card", flush=True)
        finally:
            if guest.poll() is None:
                guest.terminate()
            guest.wait()
            guest.stdin.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("assets", type=Path)
    parser.add_argument("--penv", type=Path, default=Path("target/release/penv"))
    parser.add_argument("--renode", type=Path, default=Path("../renode"))
    parser.add_argument("--logs", type=Path, default=Path("target/fire-verification"))
    parser.add_argument("--max-steps", type=int, default=6_000_000_000)
    parser.add_argument("--timeout", type=int, default=1200)
    parser.add_argument("--until", choices=("linux", "login"), default="login")
    args = parser.parse_args()
    verify(args)


if __name__ == "__main__":
    main()
