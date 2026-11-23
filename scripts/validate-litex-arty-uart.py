#!/usr/bin/env python3
"""Drive the fixed Arty boot and PHY Robot cases through UART."""

import argparse
from pathlib import Path

from litex_live_uart import live_uart


def verify(penv, renode, assets, artifacts):
    command = [
        str(penv), "run", "--script", str(renode / "scripts/single-node/arty_litex_vexriscv.resc"),
        "--platform-root", str(renode), "--asset-dir", str(assets),
        "--litex-uart-stdio", "--max-steps", "500000000",
    ]
    with live_uart(command, artifacts, "arty-boot") as (expect, _):
        expect(rb"H2U")
    print("PASS Should Boot", flush=True)

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
