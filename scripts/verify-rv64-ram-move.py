#!/usr/bin/env python3
"""Verify stopped-machine RAM context assertions from per-core-registration.robot."""

import argparse
from pathlib import Path
import re
import subprocess


READ = re.compile(r"^penv: memory ([^@]+)@(0x[0-9a-f]+):(\d+) = (0x[0-9a-f]+)$", re.M)


def verify(args, name, options, expected=(), rejection=False):
    fixture = args.renode / "tests/unit-tests/per-core-registration.repl"
    result = subprocess.run(
        [args.penv, "run", "--platform", fixture, "--max-steps", "0",
         "--renode-unmapped-data", *options],
        capture_output=True, text=True, timeout=30,
    )
    (args.logs / f"{name}.stdout").write_text(result.stdout)
    (args.logs / f"{name}.stderr").write_text(result.stderr)
    (args.logs / f"{name}.status").write_text(f"{result.returncode}\n")
    if rejection:
        if result.returncode != 1 or "no RAM registration exclusive" not in result.stderr:
            raise RuntimeError(f"{name}: expected rejection for non-exclusive registration")
        return
    actual = [
        (cpu, int(address, 16), int(width), int(value, 16))
        for cpu, address, width, value in READ.findall(result.stderr)
    ]
    if result.returncode or result.stdout or actual != list(expected):
        raise RuntimeError(f"{name}: status {result.returncode}, memory values {actual!r}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--renode", type=Path, default=Path("../renode"))
    parser.add_argument("--penv", type=Path, default=Path("target/release/penv"))
    parser.add_argument("--logs", type=Path, required=True)
    args = parser.parse_args()
    args.logs.mkdir(parents=True, exist_ok=True)

    def write(context_address, value):
        path = args.logs / f"word-{value:04x}.bin"
        path.write_bytes(value.to_bytes(2, "little"))
        return ("--load-binary", f"{context_address}:{path}")

    verify(args, "cluster", [
        *write("cpu1@0x3000000", 0x1234),
        *write("cpuA1@0x3000000", 0x5678),
        *write("cpuB1@0x3000000", 0x9ABC),
        "--read-memory", "cpu1@0x3000000:2",
        "--read-memory", "cpuA2@0x3000000:2",
        "--read-memory", "cpuB2@0x3000000:2",
        "--read-memory", "0x2010000:2",
        "--read-memory", "0x2030000:2",
        "--read-memory", "0x2040000:2",
    ], [
        ("cpu1", 0x3000000, 2, 0x1234),
        ("cpuA2", 0x3000000, 2, 0x5678),
        ("cpuB2", 0x3000000, 2, 0x9ABC),
        ("sysbus", 0x2010000, 2, 0x1234),
        ("sysbus", 0x2030000, 2, 0x5678),
        ("sysbus", 0x2040000, 2, 0x9ABC),
    ])
    verify(args, "reject-global", ["--move-ram", "cpu1@0x0:0xaaa0000"], rejection=True)
    verify(args, "reject-cluster", ["--move-ram", "cpuA1@0x3000000:0xaaa0000"], rejection=True)
    verify(args, "move", [
        *write("0x2010000", 0x1234),
        "--read-memory-now", "cpu1@0x3000000:2",
        "--move-ram", "cpu1@0x3000000:0xaaa0000",
        "--read-memory-now", "cpu1@0xaaa0000:2",
        "--read-memory-now", "cpu1@0x3000000:2",
        *write("cpu1@0xaaa0000", 0x5678),
        "--read-memory", "0x2010000:2",
    ], [
        ("cpu1", 0x3000000, 2, 0x1234),
        ("cpu1", 0xAAA0000, 2, 0x1234),
        ("cpu1", 0x3000000, 2, 0),
        ("sysbus", 0x2010000, 2, 0x5678),
    ])
    print("Verified cluster visibility and exclusive CPU RAM moves")


if __name__ == "__main__":
    main()
