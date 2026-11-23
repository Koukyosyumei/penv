#!/usr/bin/env python3
"""Replay LiteX_Linux_VexRiscv.robot: Should Handle I2C on a live shell."""

import argparse
from pathlib import Path

from litex_live_uart import live_uart


def verify(penv, renode, assets, artifacts):
    fixture = Path(__file__).resolve().parent / "fixtures/litex-linux-io.resc"
    command = [
        str(penv), "run", "--script", str(fixture),
        "--platform-root", str(renode.resolve()), "--asset-dir", str(assets.resolve()),
        "--litex-uart-stdio", "--max-steps", "1600000000",
        "--litex-temperature-after-uart", "i2c.si7021:36:4368\r\n$ ",
    ]
    with live_uart(command, artifacts, "litex-linux-i2c") as (expect, send):
        expect(rb"buildroot login:")
        send(b"root")
        expect(rb"root login on 'console'[^\n]*\n")
        send(b'export PS1="$ "')
        expect(rb"(?:^|\n)\$ ")
        send(b"i2cdetect -y 0")
        expect(rb"00: {10}-- -- UU(?: --){10} *\r?\n")
        for row in (b"10", b"20", b"30", b"40", b"50", b"60"):
            expect(row + rb":(?: --){16} *\r?\n")
        expect(rb"70:(?: --){8} *\r?\n")
        expect(rb"(?:^|\n)\$ ")
        send(b"cd /sys/class/i2c-dev/i2c-0/device/0-0005/iio:device0")
        expect(rb"(?:^|\n)\$ ")
        send(b"cat in_temp_raw")
        expect(rb"\n4368\r*\n")
        expect(rb"(?:^|\n)\$ ")
        send(b"cat in_temp_raw")
        expect(rb"\n7724\r*\n")
        expect(rb"(?:^|\n)\$ ")
        print("PASS Should Handle I2C", flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("penv", "renode", "assets", "artifacts"):
        parser.add_argument(name, type=Path)
    args = parser.parse_args()
    verify(args.penv.resolve(), args.renode.resolve(), args.assets.resolve(), args.artifacts.resolve())
