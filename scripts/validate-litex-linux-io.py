#!/usr/bin/env python3
"""Replay the fixed LiteX Linux boot, GPIO and SPI cases."""

import argparse
from pathlib import Path
import re

from litex_live_uart import live_uart


def verify(penv, renode, assets, artifacts):
    fixture = Path(__file__).resolve().parent / "fixtures/litex-linux-io.resc"
    command = [
        str(penv), "run", "--script", str(fixture), "--platform-root", str(renode),
        "--asset-dir", str(assets), "--litex-uart-stdio", "--max-steps", "1600000000",
    ]
    # Each command returns to the same PS1. Count only prompts at line starts.
    for count, device, action in (
        (1, "gpio_out.led", "off"), (4, "gpio_out.led", "off"),
        (5, "gpio_out.led", "on"), (6, "gpio_out.led", "off"),
        (10, "gpio_in.button", "Toggle"), (11, "gpio_in.button", "Toggle"),
    ):
        command += ["--litex-panel-after-uart", f"{count}:{device}:{action}:\n$ "]

    with live_uart(command, artifacts, "litex-linux-io") as (expect, send):
        def line(text):
            expect(re.escape(text) + rb"[^\n]*\n")

        def prompt():
            expect(rb"(?:^|\n)\$ ")

        def shell(text):
            send(text)
            prompt()

        expect(rb"buildroot login:")
        send(b"root")
        line(b"root login on 'console'")
        send(b'export PS1="$ "')
        prompt()
        print("PASS Should Boot", flush=True)

        for text in (b"cd /sys/class/gpio", b"echo 508 > export", b"cd gpio508",
                     b"echo 1 > value", b"echo 0 > value"):
            shell(text)

        for text in (b"cd /sys/class/gpio", b"echo 504 > export", b"cd gpio504"):
            shell(text)
        # A subsequent prompt establishes that the final LED assertion ran.
        print("PASS Should Control LED", flush=True)
        for value in (b"0", b"1", b"0"):
            send(b"cat value")
            expect(rb"\n" + value + rb"\r*\n")
            prompt()
        print("PASS Should Read Button", flush=True)

        send(b"spidev_test -D /dev/spidev0.0 --speed 1000000")
        for text in (
            b"spi mode: 0x0",
            b"bits per word: 8",
            b"max speed: 1000000 Hz (1000 KHz)",
            b"RX | FF FF FF FF FF FF 40 00 00 00 00 95 FF FF FF FF FF FF FF FF "
            b"FF FF FF FF FF FF FF FF FF FF F0 0D",
        ):
            line(text)
        print("PASS Should Handle SPI", flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("penv", "renode", "assets", "artifacts"):
        parser.add_argument(name, type=Path)
    args = parser.parse_args()
    verify(args.penv.resolve(), args.renode.resolve(), args.assets.resolve(), args.artifacts.resolve())
