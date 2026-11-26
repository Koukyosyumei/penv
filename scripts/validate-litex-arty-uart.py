#!/usr/bin/env python3
"""Drive the fixed Arty boot and PHY Robot cases through UART."""

import argparse
from pathlib import Path
import re

from litex_live_uart import live_uart


def verify(penv, renode, assets, artifacts):
    command = [
        str(penv), "run", "--script", str(renode / "scripts/single-node/arty_litex_vexriscv.resc"),
        "--platform-root", str(renode), "--asset-dir", str(assets),
        "--litex-uart-stdio", "--max-steps", "500000000",
    ]
    panel = []
    groups = []
    prompt = 1

    def event(name, action):
        panel.extend(("--litex-panel-after-uart", f"{prompt}:cas.{name}:{action}:H2U "))

    def check_leds(mask):
        for pin in range(4):
            event(f"led{pin}", "on" if mask & (1 << pin) else "off")

    def instruction(group, command, response=None):
        nonlocal prompt
        group.append((command, response))
        prompt += 1

    leds = []
    check_leds(0)
    for mask in (1, 3, 7, 15, 0):
        instruction(leds, f"debug cas leds {mask}".encode())
        check_leds(mask)
    groups.append(("Should Control LEDs", leds))

    for kind, command_bytes, replies in (
        ("switch", b"debug cas switches", (b"0", b"1", b"3", b"7", b"F", b"0")),
        ("button", b"debug cas buttons read", (b"0 0", b"1 1", b"3 3", b"7 7", b"F F", b"0 F")),
    ):
        steps = []
        instruction(steps, command_bytes, replies[0])
        for pin, reply in enumerate(replies[1:5]):
            event(f"{kind}{pin}", "Toggle")
            instruction(steps, command_bytes, reply)
        for pin in range(4):
            event(f"{kind}{pin}", "Toggle")
        instruction(steps, command_bytes, replies[-1])
        if kind == "button":
            instruction(steps, b"debug cas buttons clear")
            instruction(steps, command_bytes, b"0 0")
        groups.append((f"Should Read {kind.title()}es" if kind == "switch" else "Should Read Buttons", steps))

    with live_uart(command + panel, artifacts, "arty-panel") as (expect, send):
        expect(rb"H2U ")
        print("PASS Should Boot", flush=True)
        for case, steps in groups:
            for command_bytes, response in steps:
                send(command_bytes)
                if response is not None:
                    expect(rb"\n" + re.escape(response) + rb"\r*\n")
                expect(rb"H2U ")
            # The next prompt-triggered check executes before the next guest command.
            if case == "Should Control LEDs":
                continue
            if case == "Should Read Switches":
                print("PASS Should Control LEDs", flush=True)
            print(f"PASS {case}", flush=True)

    command += [
        "--litex-phy-event", "0:eth.phy:0x0",
        "--litex-phy-after-uart", "eth.phy:0x4400:MDIO mode: 10Mbps / link: down\r\n",
    ]
    with live_uart(command, artifacts, "arty-phy") as (expect, send):
        expect(rb"MDIO mode: 10Mbps / link: down\r*\n")
        send(b"mdio_status")
        expect(rb"MDIO mode: 100Mbps / link: up\r*\n")
    print("PASS Should Read Ethernet PHY Status", flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("penv", "renode", "assets", "artifacts"):
        parser.add_argument(name, type=Path)
    args = parser.parse_args()
    verify(args.penv.resolve(), args.renode.resolve(), args.assets.resolve(), args.artifacts.resolve())
