#!/usr/bin/env python3
"""Verify the upstream StarLight demo over UART, optionally over a local link."""

import argparse
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import re
import subprocess
import tempfile
import time


class Guest:
    def __init__(self, args, name, link=()):
        self.log = (args.logs / f"{name}.uart.log").open("wb")
        self.errors = (args.logs / f"{name}.stderr.log").open("wb")
        self.offset = 0
        self.process = subprocess.Popen([
            args.penv, "run", "--script", args.script,
            "--platform-root", args.renode, "--asset-dir", args.assets,
            "--uart-device", "uart3", "--uart-stdin", "--max-steps",
            "40000000000" if args.recovery else ("24000000000" if link else "14000000000"),
            *link, *(["--uart-stdin-reset"] if args.recovery and not (args.guest_reboot or args.linux_reboot) else []),
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

    def verify_uboot(self):
        for line in (rb"OpenSBI v0\.9", rb"Platform Name\s+: StarFive", rb"U-Boot 2021\.01"):
            self.expect(line + rb"[^\r\n]*[\r\n]")
        self.expect(rb"dwmac\.10020000")

    def login(self, verify_uboot=True):
        if verify_uboot:
            self.verify_uboot()
        for banner in (b"Linux version 5.10.6", b"Run /init as init process", b"buildroot login:"):
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

    def restart(self, from_guest, from_linux):
        if from_linux:
            self.checked_command("zcat /proc/config.gz | grep -x CONFIG_POWER_RESET_GPIO_RESTART=y")
            bound = self.checked_command("ls /sys/bus/platform/drivers/restart-gpio")
            if b"gpio-restart" not in bound:
                raise RuntimeError(f"{self.name}: the Linux GPIO restart driver is not bound")
            self.send(b"reboot")
            self.expect(rb"reboot: Restarting system")
            self.login()
            return
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


def start_switch(args):
    switch = subprocess.Popen(
        [args.penv, "switch", str(args.switch_port), *map(str, args.ports)],
        stdout=subprocess.PIPE,
    )
    ready = switch.stdout.readline()
    if not ready.startswith(b"Listening on "):
        if switch.poll() is None:
            switch.terminate()
        switch.wait()
        switch.stdout.close()
        raise RuntimeError("local Ethernet switch did not start")
    print(ready.decode().strip(), flush=True)
    return switch


def external_pmp_script(args, directory):
    directory = Path(directory)
    board = directory / "beaglev_starlight.repl"
    upstream = (args.renode / "platforms/boards/beaglev_starlight.repl").resolve()
    board.write_text(
        f'using "{upstream}"\n'
        "pmp1: Miscellaneous.RiscVExternalPMP @ U74_1\n"
        "pmp2: Miscellaneous.RiscVExternalPMP @ U74_2\n"
    )
    script = directory / "beaglev_starlight.resc"
    source, replaced = re.subn(
        r"(?m)^machine LoadPlatformDescription @platforms/boards/beaglev_starlight\.repl$",
        f"machine LoadPlatformDescription @{board}",
        args.script.read_text(),
    )
    if replaced != 1:
        raise RuntimeError("expected one StarLight board registration in the upstream RESC")
    script.write_text(source)
    return script


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
    parser.add_argument("--upstream-cases", action="store_true",
                        help="verify the StarLight Robot cases")
    parser.add_argument("--upstream-boot-only", action="store_true",
                        help="verify the upstream two-machine U-Boot case")
    parser.add_argument("--external-pmp", action="store_true",
                        help="run the selected upstream cases with external PMP")
    parser.add_argument("--recovery", action="store_true",
                        help="verify Ethernet reopen and reload one machine after Linux reboot")
    reboot = parser.add_mutually_exclusive_group()
    reboot.add_argument("--guest-reboot", action="store_true",
                        help="drive the StarLight reset pin from the guest during recovery")
    reboot.add_argument("--linux-reboot", action="store_true",
                        help="require the Linux GPIO restart driver and run standard reboot")
    args = parser.parse_args()
    if (args.guest_reboot or args.linux_reboot) and not args.recovery:
        parser.error("--guest-reboot and --linux-reboot require --recovery")
    if args.recovery and args.switch_port is None:
        parser.error("--recovery requires --switch-port and --ports")
    if args.switch_port is not None and not args.ports:
        parser.error("--switch-port requires --ports")
    if args.upstream_cases and (args.switch_port is None or args.recovery):
        parser.error("--upstream-cases requires a switch and cannot be combined with --recovery")
    if args.upstream_boot_only and (args.upstream_cases or args.ports or args.recovery):
        parser.error("--upstream-boot-only cannot be combined with Ethernet or recovery")
    if args.external_pmp and not (args.upstream_cases or args.upstream_boot_only):
        parser.error("--external-pmp requires --upstream-cases or --upstream-boot-only")
    args.logs.mkdir(parents=True, exist_ok=True)
    args.script = args.renode / "scripts/single-node/beaglev_starlight.resc"
    suffix = " With External PMP" if args.external_pmp else ""
    temporary = None
    guests = []
    switch = None
    try:
        if args.external_pmp:
            temporary = tempfile.TemporaryDirectory(prefix="penv-starlight-pmp-")
            args.script = external_pmp_script(args, temporary.name)
        if args.upstream_boot_only:
            for name in ("machine0", "machine1"):
                guests.append(Guest(args, name))
            for guest in guests:
                guest.verify_uboot()
            print(f"PASS Should Boot U-Boot{suffix}", flush=True)
        elif args.ports:
            first, second = args.ports
            if args.switch_port is not None and not args.upstream_cases:
                switch = start_switch(args)
            for name, local, peer in (("starlight-a", first, second), ("starlight-b", second, first)):
                if args.switch_port is not None:
                    peer = args.switch_port
                guests.append(Guest(args, name, ("--ethernet-loopback", f"{local}:{peer}")))
            with ThreadPoolExecutor(max_workers=2) as pool:
                if args.upstream_cases:
                    list(pool.map(Guest.verify_uboot, guests))
                    print(f"PASS Should Boot U-Boot{suffix}", flush=True)
                    switch = start_switch(args)
                list(pool.map(lambda guest: guest.login(not args.upstream_cases), guests))
            first, second = guests
            first.configure("66:34:B0:6C:DE:A0", "192.168.0.5")
            second.configure("66:34:B0:6C:DE:A1", "192.168.0.6")
            if args.upstream_cases:
                print(f"PASS Should Provide Two Linux Machines With Ethernet Connection{suffix}", flush=True)
                second.ping("192.168.0.5", 56)
                print(f"PASS Should Ping{suffix}", flush=True)
                if not args.external_pmp:
                    second.ping("192.168.0.5", 3200)
                    print("PASS Should Ping Large Payload", flush=True)
            else:
                for size in (56, 3200):
                    first.ping("192.168.0.6", size)
                    second.ping("192.168.0.5", size)
            for guest in guests:
                guest.verify_interrupts()
            if args.recovery:
                first.recover_ethernet("192.168.0.5")
                first.ping("192.168.0.6", 56)
                second.ping("192.168.0.5", 56)
                first.restart(args.guest_reboot, args.linux_reboot)
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
        if temporary is not None:
            temporary.cleanup()


if __name__ == "__main__":
    main()
