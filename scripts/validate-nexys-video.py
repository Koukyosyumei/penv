#!/usr/bin/env python3
"""Drive the fixed Nexys Video frame and ROI Robot cases through the live UART."""

import argparse
from pathlib import Path
import re

from litex_live_uart import live_uart


def verify(penv, renode, assets, artifacts):
    robot = renode / "tests/platforms/LiteX/Nexys_Video_LiteX_VexRiscv.robot"
    text = robot.read_text()
    full, roi = [
        assets / re.search(rf"\b{keyword} @\S+/([^/\s]+)", text)[1]
        for keyword in ("WaitForFrame", "WaitForFrameROI")
    ]
    command = [
        str(penv), "run", "--script", str(renode / "scripts/single-node/litex_nexys_video_vexriscv_linux.resc"),
        "--platform-root", str(renode), "--asset-dir", str(assets), "--litex-uart-stdio",
        "--max-steps", "1100000000", "--wait-for-frame", f"1:{full}",
        "--wait-for-frame-roi", f"1:0:0:75:80:{roi}",
        "--framebuffer-out", str(artifacts / "nexys-video-motd.ppm"),
    ]
    with live_uart(command, artifacts, "nexys-video", wait_for_exit=True) as (expect, send):
        expect(rb"Press Q or ESC to abort boot completely\.[^\n]*\n")
        send(b"Q", ending=b"")
        expect(rb"litex(?:\x1b\[[0-9;]*m)*>")
        send(b"sdcardboot")
        expect(rb"buildroot login:")
        send(b"root")
        expect(rb"root login on 'console'[^\n]*\n")
        send(b'export PS1="$ "')
        expect(rb"(?:^|\n)\r*\$ ")
        send(b"cat /etc/motd > /dev/tty0")

    matches = re.findall(
        r"^penv: framebuffer matched at step (\d+)$",
        (artifacts / "nexys-video.stderr.log").read_text(), re.MULTILINE,
    )
    if len(matches) != 2 or int(matches[0]) > int(matches[1]):
        raise RuntimeError(f"Expected ordered full-frame and ROI matches, got {matches!r}")
    print(f"PASS Frame Buffer Test at step {matches[0]}")
    print(f"PASS Frame Buffer with ROI Test at step {matches[1]}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("penv", "renode", "assets", "artifacts"):
        parser.add_argument(name, type=Path)
    args = parser.parse_args()
    verify(args.penv.resolve(), args.renode.resolve(), args.assets.resolve(), args.artifacts.resolve())
