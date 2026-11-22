#!/usr/bin/env python3
"""Verify the upstream StarLight demo over UART, optionally over a local link."""

import argparse
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import re
import subprocess
import time


class Guest:
    def __init__(self, args, name, link=()):
        self.log = (args.logs / f"{name}.uart.log").open("wb")
        self.errors = (args.logs / f"{name}.stderr.log").open("wb")
        self.offset = 0
        self.process = subprocess.Popen([
            args.penv, "run", "--script", args.renode / "scripts/single-node/beaglev_starlight.resc",
            "--platform-root", args.renode, "--asset-dir", args.assets,
            "--uart-device", "uart3", "--uart-stdin", "--max-steps",
            "24000000000" if link else "14000000000", *link,
        ], stdin=subprocess.PIPE, stdout=self.log, stderr=self.errors, bufsize=0)
        self.name = name
        self.deadline = time.monotonic() + args.timeout

    def expect(self, pattern):
        # Use a separate open file description so reading never moves the child's write offset.
        with open(self.log.name, "rb") as capture:
            capture.seek(self.offset)
            pending = b""
            while time.monotonic() < self.deadline:
                pending += capture.read()
                match = re.search(pattern, pending)
                if match:
                    self.offset += match.end()
                    return match
                if self.process.poll() is not None:
                    break
                time.sleep(0.1)
        raise RuntimeError(f"{self.name}: missing {pattern!r}; see {self.log.name}")

    def send(self, line):
        self.process.stdin.write(line + b"\r")

    def login(self):
        for banner in (b"OpenSBI v0.9", b"U-Boot 2021.01", b"Linux version 5.10.6",
                       b"Run /init as init process", b"buildroot login:"):
            self.expect(re.escape(banner))
        self.send(b"root")
        self.expect(rb"Password:")
        self.send(b"starfive")
        self.expect(rb"\r?\n# ")
        self.send(b"id -u; uname -m")
        self.expect(rb"\r*\n0\r*\nriscv64\r*\n")
        print(f"{self.name}: authenticated root, uid 0, riscv64", flush=True)

    def configure(self, mac, ip):
        # Keep UART operations short and wait for each shell response.
        self.send(b"stty -echo")
        self.expect(rb"(?:^|\n)# ")
        for command in ("ifconfig eth0 down", f"ifconfig eth0 hw ether {mac}",
                        "ifconfig eth0 mtu 440", f"ifconfig eth0 {ip} up"):
            self.checked_command(command)
        self.irq_baseline = self.interrupts()

    def ping(self, ip, size):
        output = self.checked_command(f"ping -A -q -c 10 -W 3 -s {size} {ip}")
        if b"10 packets transmitted, 10 packets received, 0% packet loss" not in output:
            raise RuntimeError(f"{self.name}: packet loss pinging {ip}; see {self.log.name}")
        print(f"{self.name}: {ip}, {size} bytes, 10/10 replies", flush=True)

    def checked_command(self, command):
        self.send(command.encode())
        prompt = self.expect(rb"(?:^|\n)# ")
        self.send(b"echo STATUS=$?")
        # Kernel console messages can appear between the marker and the prompt.
        match = self.expect(rb"STATUS=(\d+)(?=\D)[\s\S]*?\n# ")
        if match[1] != b"0":
            raise RuntimeError(f"{self.name}: {command} returned {match[1]!r}; see {self.log.name}")
        return prompt.string[:prompt.start()]

    def interrupts(self):
        output = self.checked_command("grep ' eth0$' /proc/interrupts")
        match = re.search(rb"\d+:\s+(\d+)\s+(\d+)", output)
        if not match:
            raise RuntimeError(f"{self.name}: missing eth0 IRQ counters; see {self.log.name}")
        return int(match[1]) + int(match[2])

    def verify_interrupts(self):
        after = self.interrupts()
        if after <= self.irq_baseline:
            raise RuntimeError(f"{self.name}: eth0 IRQ counter did not advance")
        print(f"{self.name}: Linux eth0 IRQ count {self.irq_baseline} -> {after}", flush=True)

    def close(self):
        if self.process.poll() is None:
            self.process.terminate()
            self.process.wait()
        self.process.stdin.close()
        self.log.close()
        self.errors.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("assets", type=Path)
    parser.add_argument("--penv", type=Path, default=Path("target/release/penv"))
    parser.add_argument("--renode", type=Path, default=Path("../renode"))
    parser.add_argument("--logs", type=Path, default=Path("target/starlight-verification"))
    parser.add_argument("--timeout", type=int, default=1200, help="host seconds per run")
    parser.add_argument("--ports", type=int, nargs=2, metavar=("PORT_A", "PORT_B"),
                        help="also verify Ethernet between two guests on these localhost ports")
    args = parser.parse_args()
    args.logs.mkdir(parents=True, exist_ok=True)
    guests = []
    try:
        if args.ports:
            first, second = args.ports
            for name, local, peer in (("starlight-a", first, second), ("starlight-b", second, first)):
                guests.append(Guest(args, name, ("--ethernet-loopback", f"{local}:{peer}")))
            with ThreadPoolExecutor(max_workers=2) as pool:
                list(pool.map(Guest.login, guests))
            first, second = guests
            first.configure("66:34:B0:6C:DE:A0", "192.168.0.5")
            second.configure("66:34:B0:6C:DE:A1", "192.168.0.6")
            for size in (56, 3200):
                first.ping("192.168.0.6", size)
                second.ping("192.168.0.5", size)
            for guest in guests:
                guest.verify_interrupts()
        else:
            guests.append(Guest(args, "starlight"))
            guests[0].login()
    finally:
        for guest in guests:
            guest.close()


if __name__ == "__main__":
    main()
