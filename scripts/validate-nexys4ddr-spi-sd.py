#!/usr/bin/env python3
"""Drive the fixed Nexys4DDR boot and SPI SD Robot cases through the live UART."""

import argparse
from pathlib import Path
import re

from litex_live_uart import live_uart


def verify(penv, renode, assets, artifacts):
    script = Path(__file__).resolve().parent / "fixtures/nexys4ddr-spi-sd.resc"
    command = [
        str(penv), "run", "--script", str(script),
        "--platform-root", str(renode), "--asset-dir", str(assets),
        "--litex-uart-stdio", "--max-steps", "500000000",
    ]
    with live_uart(command, artifacts, "nexys4ddr-spi-sd") as (expect, send):
        expect(rb"Press Q or ESC to abort boot completely\.[^\n]*\n")
        send(b"Q", ending=b"")
        expect(rb"litex(?:\x1b\[[0-9;]*m)*>")
        print("PASS Should Boot", flush=True)

        send(b"spisdcardboot")
        for line in (
            b"SD Card via SPI Initialising", b"Read FAT16 Boot Sector", b"[mkfs.fat]",
            b"Root Directory", b"File 1 [README  .TXT]",
        ):
            expect(re.escape(line) + rb"[^\n]*\n")
        print("PASS Should List Readme File", flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("penv", "renode", "assets", "artifacts"):
        parser.add_argument(name, type=Path)
    args = parser.parse_args()
    verify(args.penv.resolve(), args.renode.resolve(), args.assets.resolve(), args.artifacts.resolve())
