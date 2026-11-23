#!/usr/bin/env python3
"""Drive the two fixed LiteX Linux SD-card cases through the live UART."""

import argparse
from pathlib import Path
import re

from litex_live_uart import live_uart


def verify(case, penv, renode, assets, artifacts):
    script = Path(__file__).resolve().parent / "fixtures" / f"litex-sd-{case}.resc"
    command = [
        str(penv), "run", "--script", str(script), "--platform-root", str(renode),
        "--asset-dir", str(assets), "--litex-uart-stdio", "--max-steps", "1600000000",
    ]
    with live_uart(command, artifacts, f"litex-sd-{case}") as (expect, send):
        def line(text):
            expect(re.escape(text) + rb"[^\n]*\n")

        def shell(command):
            expect(rb"(?:^|\n)\r*\$ ")
            send(command)

        line(b"mmc0: new SD card at address 0001")
        line(b"blk_queue_max_hw_sectors: set to minimum 8")
        line(b"blk_queue_max_segment_size: set to minimum 4096")
        if case == "root":
            line(b"mmcblk0: mmc0:0001 RENOD")
            line(b"VFS: Mounted root (ext4 filesystem) readonly ")
            line(b"Run /sbin/init as init process")
        else:
            line(b"mmcblk0: mmc0:0001 RENOD 64.0 MiB ")
            line(b"mmcblk0: p1")
        expect(rb"buildroot login:")
        send(b"root")
        if case == "root":
            expect(rb"root@buildroot")
        else:
            line(b"32-bit VexRiscv CPU with MMU integrated in a LiteX SoC")
        send(b'export PS1="$ "')

        if case == "root":
            shell(b"ls -la /")
            expect(rb"linuxrc(?:\x1b\[[0-9;]*m)* -> (?:\x1b\[[0-9;]*m)*bin/busybox[^\n]*\n")
        else:
            shell(b"mount /dev/mmcblk0p1 /mnt")
            shell(b"ls /mnt")
            line(b"readme.txt")
            shell(b"cat /mnt/readme.txt")
            expect(rb"(?:^|\n)\r*This is a test file\r*\n")
            shell(b'echo "This is a write test" > /mnt/readme2.txt')
            shell(b"ls /mnt")
            line(b"readme2.txt")
            shell(b"sync")
            shell(b"umount /mnt")
            shell(b"mount /dev/mmcblk0p1 /mnt")
            shell(b"ls /mnt")
            line(b"readme2.txt")
            shell(b"cat /mnt/readme2.txt")
            expect(rb"(?:^|\n)\r*This is a write test\r*\n")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("case", choices=("root", "fat"))
    for name in ("penv", "renode", "assets", "artifacts"):
        parser.add_argument(name, type=Path)
    args = parser.parse_args()
    verify(args.case, args.penv.resolve(), args.renode.resolve(), args.assets.resolve(), args.artifacts.resolve())
