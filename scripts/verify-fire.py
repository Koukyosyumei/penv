#!/usr/bin/env python3
"""Run the BeagleV-Fire SD boot case using the public Renode assets."""

import argparse
import hashlib
from pathlib import Path
import re
import subprocess
import time


def verify(args):
    args.logs.mkdir(parents=True, exist_ok=True)
    script = args.renode / "scripts/single-node/beaglev-fire.resc"
    image = re.search(r"(?m)^\$image\?=@[^\s]+/([^/\s]+)$", script.read_text())
    if image is None:
        raise RuntimeError(f"Missing SD image in {script}")
    with (args.assets / image[1]).open("rb") as media:
        sector = media.read(512)
    if len(sector) != 512:
        raise RuntimeError("The SD image has no complete first sector")
    digest = hashlib.sha256(sector).hexdigest().encode()
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
            guest.stdin.write(b"dd if=/dev/mmcblk0 bs=512 count=1 2>/dev/null | sha256sum\r")
            expect(rb"\n" + digest + rb"[ \t]+-\r*\n")
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
