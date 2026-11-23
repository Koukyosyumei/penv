"""Drive the upstream Minerva BIOS SFL transfer through penv's live UART."""

import binascii
import os
from pathlib import Path
import selectors
import struct
import subprocess
import sys
import time


def frame(command, data):
    body = bytes([command]) + data
    return bytes([len(data)]) + struct.pack(">H", binascii.crc_hqx(body, 0)) + body


def verify(penv, renode_root, artifacts):
    cmd = [
        penv, "run", "--script", str(renode_root / "scripts/single-node/litex_minerva.resc"),
        "--platform-root", str(renode_root), "--asset-dir", str(artifacts / "assets"),
        "--litex-timer-alias", "0xe0002000", "--litex-uart-legacy-console",
        "--litex-uart-stdio", "--max-steps", "3000000000",
    ]
    address = struct.pack(">I", 0x40000000)
    # Write '~' to LiteX UART TX at 0xe0001800, then loop.
    payload = struct.pack("<5I", 0xe00022b7, 0x80028293, 0x07e00313, 0x0062a023, 0x0000006f)

    with (
        (artifacts / "minerva.stderr.log").open("wb") as stderr,
        (artifacts / "minerva.uart.log").open("wb") as uart,
        selectors.DefaultSelector() as selector,
        subprocess.Popen(cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=stderr) as process,
    ):
        selector.register(process.stdout, selectors.EVENT_READ)
        deadline = time.monotonic() + 180
        pending = b""

        def expect(marker):
            nonlocal pending
            while marker not in pending:
                remaining = deadline - time.monotonic()
                if remaining <= 0 or not selector.select(remaining):
                    raise RuntimeError(f"Minerva UART timed out waiting for {marker!r}")
                chunk = os.read(process.stdout.fileno(), 4096)
                if not chunk:
                    raise RuntimeError(f"Minerva UART closed while waiting for {marker!r}")
                uart.write(chunk)
                pending += chunk
            pending = pending.split(marker, 1)[1]

        def send(data):
            process.stdin.write(data)
            process.stdin.flush()

        try:
            expect(b"sL5DdSMmkekro\n")
            send(b"z6IHG7cYDID6o\n" + frame(1, address + payload))
            expect(b"K")
            send(frame(2, address))
            expect(b"Executing booted program at 0x40000000")
            expect(b"~")
        finally:
            if process.poll() is None:
                process.terminate()

    print("Minerva loaded and executed an RV32 payload over UART")


if __name__ == "__main__":
    if len(sys.argv) != 4:
        sys.exit(f"Usage: {sys.argv[0]} PENV RENODE_ROOT ARTIFACT_DIR")
    verify(str(Path(sys.argv[1]).resolve()), Path(sys.argv[2]), Path(sys.argv[3]))
