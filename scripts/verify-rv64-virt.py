#!/usr/bin/env python3
"""Verify OpenSBI, Linux and UART login on the RV64 virt platform."""

import argparse
from pathlib import Path
import re
import subprocess
import time


def verify(args):
    args.logs.mkdir(parents=True, exist_ok=True)
    uart = args.logs / "virt.uart.log"
    stderr = args.logs / "virt.stderr.log"
    with uart.open("wb") as output, uart.open("rb") as capture, stderr.open("wb") as errors:
        guest = subprocess.Popen([
            args.penv, "run", "--platform", args.platform,
            "--dtb", f"0x88000000:{args.work / 'virt-padded.dtb'}",
            "--load-binary", f"0x80200000:{args.work / 'Image'}",
            "--load-binary", f"0x84000000:{args.work / 'rootfs.cpio'}",
            "--raw", "--uart-stdin", "--max-steps", "5000000000",
            args.work / "opensbi-1.6/build/platform/generic/firmware/fw_jump.bin",
        ], stdin=subprocess.PIPE, stdout=output, stderr=errors, bufsize=0)
        pending = b""
        deadline = time.monotonic() + args.timeout

        def expect(pattern):
            nonlocal pending
            while True:
                running = guest.poll() is None
                pending += capture.read()
                if match := re.search(pattern, pending):
                    pending = pending[match.end():]
                    return
                if not running or b"Kernel panic" in pending or time.monotonic() >= deadline:
                    raise RuntimeError(f"Missing {pattern!r}; see {uart} and {stderr}")
                time.sleep(0.1)

        try:
            for banner in (rb"OpenSBI v1\.6", rb"Platform Name\s+: Penv RV64 virt",
                           rb"Linux version 5\.10\.6", rb"10000000\.serial: ttyS0",
                           rb"Run /init as init process", rb"buildroot login:"):
                expect(banner)
            guest.stdin.write(b"root\r")
            expect(rb"Password:")
            guest.stdin.write(b"starfive\r")
            expect(rb"\n# ")
            guest.stdin.write(b"id -u; uname -m\r")
            expect(rb"\n0\r*\nriscv64\r*\n")
            print("PASS RV64 virt OpenSBI and Linux UART login (uid 0, riscv64)", flush=True)
        finally:
            if guest.poll() is None:
                guest.terminate()
            guest.wait()
            guest.stdin.close()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("work", type=Path, help="directory with OpenSBI, Image, initramfs and DTB")
    parser.add_argument("--platform", type=Path, default=Path("platforms/rv64_virt.repl"))
    parser.add_argument("--penv", type=Path, default=Path("target/release/penv"))
    parser.add_argument("--logs", type=Path, default=Path("target/rv64-virt-verification"))
    parser.add_argument("--timeout", type=int, default=600)
    verify(parser.parse_args())
