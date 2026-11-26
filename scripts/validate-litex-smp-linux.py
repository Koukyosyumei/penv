#!/usr/bin/env python3
"""Drive the fixed LiteX SMP Linux Robot case through the live UART."""

import argparse
from pathlib import Path

from litex_live_uart import live_uart


def verify(penv, renode, assets, artifacts):
    command = [
        str(penv), "run", "--script", str(renode / "scripts/single-node/litex_vexriscv_smp.resc"),
        "--platform-root", str(renode), "--asset-dir", str(assets),
        "--litex-uart-stdio", "--max-steps", "1600000000",
    ]
    with live_uart(command, artifacts, "litex-smp") as (expect, send):
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


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("penv", "renode", "assets", "artifacts"):
        parser.add_argument(name, type=Path)
    args = parser.parse_args()
    verify(args.penv.resolve(), args.renode.resolve(), args.assets.resolve(), args.artifacts.resolve())
