"""Run a live LiteX UART session and retain its transcript and exit status."""

from contextlib import contextmanager
import os
import re
import selectors
import subprocess
import time


@contextmanager
def live_uart(command, artifacts, name, *, wait_for_exit=False):
    artifacts.mkdir(parents=True, exist_ok=True)
    with (
        (artifacts / f"{name}.stderr.log").open("wb") as stderr,
        (artifacts / f"{name}.uart.log").open("wb") as uart,
        selectors.DefaultSelector() as selector,
        subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=stderr) as process,
    ):
        selector.register(process.stdout, selectors.EVENT_READ)
        deadline = time.monotonic() + 180
        pending = b""

        def expect(pattern):
            nonlocal pending
            while (match := re.search(pattern, pending)) is None:
                remaining = deadline - time.monotonic()
                if remaining <= 0 or not selector.select(remaining):
                    raise RuntimeError(f"LiteX UART timed out waiting for {pattern!r}")
                chunk = os.read(process.stdout.fileno(), 4096)
                if not chunk:
                    raise RuntimeError(f"LiteX UART closed waiting for {pattern!r}")
                uart.write(chunk)
                pending += chunk
            print(f"UART matched {match.group()!r}", flush=True)
            pending = pending[match.end():]

        def send(line, ending=b"\r"):
            print(f"UART input {line!r}", flush=True)
            process.stdin.write(line + ending)
            process.stdin.flush()

        try:
            yield expect, send
            if wait_for_exit:
                while True:
                    remaining = deadline - time.monotonic()
                    if remaining <= 0 or not selector.select(remaining):
                        raise RuntimeError("LiteX UART session did not finish")
                    chunk = os.read(process.stdout.fileno(), 4096)
                    if not chunk:
                        break
                    uart.write(chunk)
                status = process.wait(timeout=max(0, deadline - time.monotonic()))
                if status != 0:
                    raise RuntimeError(f"LiteX UART session exited with status {status}")
        finally:
            if process.poll() is None:
                process.terminate()
            try:
                status = process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                status = process.wait()
            (artifacts / f"{name}.exit").write_text(f"{status}\n")
