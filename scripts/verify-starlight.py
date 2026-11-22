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
            "40000000000" if args.recovery else ("24000000000" if link else "14000000000"),
            *link, *(["--uart-stdin-reset"] if args.recovery and not args.guest_reboot else []),
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

    def recover_ethernet(self, ip):
        self.checked_command("ifconfig eth0 down")
        self.checked_command(f"ifconfig eth0 {ip} up")
        output = self.checked_command("id -u; uname -m")
        if not re.search(rb"(?:^|\n)0\r*\nriscv64\r*\n?", output):
            raise RuntimeError(f"{self.name}: UART did not recover after Ethernet reopen")
        print(f"{self.name}: UART shell survived Ethernet reopen", flush=True)

    def restart(self, from_guest):
        if from_guest:
            # The FIT's active-low GPIO10 reset is usable even without gpio-restart.
            self.checked_command("devmem 0x119100a0 32 1")
            self.checked_command("devmem 0x119100a4 32 0")
            high = self.checked_command("devmem 0x119100a0 32")
            enabled = self.checked_command("devmem 0x119100a4 32")
            if b"0x00000001" not in high or b"0x00000000" not in enabled:
                raise RuntimeError(f"{self.name}: GPIO10 register readback failed")
            self.checked_command("sync")
            self.send(b"devmem 0x119100a0 32 0")
            self.login()
            return
        self.send(b"reboot")
        self.expect(rb"reboot: Restarting system")
        # The demo Linux image has no JH7100 restart handler. Apply the
        # machine reset that Renode's reset macro would perform.
        self.process.stdin.write(b"\x1dr")
        self.login()

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
    parser.add_argument("--switch-port", type=int,
                        help="connect the two guest ports to a local penv Ethernet switch")
    parser.add_argument("--recovery", action="store_true",
                        help="verify Ethernet reopen and reload one machine after Linux reboot")
    parser.add_argument("--guest-reboot", action="store_true",
                        help="drive the StarLight reset pin from the guest during recovery")
    args = parser.parse_args()
    if args.guest_reboot and not args.recovery:
        parser.error("--guest-reboot requires --recovery")
    if args.recovery and args.switch_port is None:
        parser.error("--recovery requires --switch-port and --ports")
    if args.switch_port is not None and not args.ports:
        parser.error("--switch-port requires --ports")
    args.logs.mkdir(parents=True, exist_ok=True)
    guests = []
    switch = None
    try:
        if args.ports:
            first, second = args.ports
            if args.switch_port is not None:
                switch = subprocess.Popen(
                    [args.penv, "switch", str(args.switch_port), str(first), str(second)],
                    stdout=subprocess.PIPE,
                )
                ready = switch.stdout.readline()
                if not ready.startswith(b"Listening on "):
                    raise RuntimeError("local Ethernet switch did not start")
                print(ready.decode().strip(), flush=True)
            for name, local, peer in (("starlight-a", first, second), ("starlight-b", second, first)):
                if args.switch_port is not None:
                    peer = args.switch_port
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
            if args.recovery:
                first.recover_ethernet("192.168.0.5")
                first.ping("192.168.0.6", 56)
                second.ping("192.168.0.5", 56)
                first.restart(args.guest_reboot)
                first.configure("66:34:B0:6C:DE:A0", "192.168.0.5")
                for size in (56, 3200):
                    first.ping("192.168.0.6", size)
                    second.ping("192.168.0.5", size)
                first.verify_interrupts()
                second.verify_interrupts()
        else:
            guests.append(Guest(args, "starlight"))
            guests[0].login()
    finally:
        for guest in guests:
            guest.close()
        if switch is not None:
            if switch.poll() is None:
                switch.terminate()
            switch.wait()
            switch.stdout.close()


if __name__ == "__main__":
    main()
