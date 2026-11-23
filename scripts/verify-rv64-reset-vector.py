#!/usr/bin/env python3
"""Check the fixed RV64 ResetVector monitor sequence without executing the CPU."""

import argparse
from pathlib import Path
import re
import subprocess


def verify(args, platform, name, commands, expected, elf=None):
    run = subprocess.run(
        [args.penv, "run", "--platform", platform, "--max-steps", "0", *commands,
         *([elf] if elf is not None else [])],
        capture_output=True, text=True, timeout=30,
    )
    (args.logs / f"{name}.stderr").write_text(run.stderr)
    (args.logs / f"{name}.status").write_text(f"{run.returncode}\n")
    values = [int(value, 16) for value in re.findall(
        r"^penv: register cpu@pc = (0x[0-9a-f]+)$", run.stderr, re.M
    )]
    if run.returncode or run.stdout or values != expected:
        raise RuntimeError(f"{name}: expected {expected!r}, got {values!r}: {run.stderr}")
    print(f"{name}: verified {len(expected)} PC assertions")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--penv", type=Path, default=Path("target/release/penv"))
    parser.add_argument("--logs", type=Path, required=True)
    parser.add_argument("--elf", type=Path, help="fixture ELF with entry 0x2000")
    args = parser.parse_args()
    args.logs.mkdir(parents=True, exist_ok=True)
    platform = args.logs / "reset-vector.repl"
    platform.write_text(
        """clint: IRQControllers.CoreLevelInterruptor @ sysbus 0x44000000
    frequency: 66000000
cpu: CPU.RiscV64 @ sysbus
    timeProvider: clint
    cpuType: "rv64gc_Zfh"
    resetVector: 0x1000
    renodeResetMstatus: true
mem: Memory.MappedMemory @ sysbus 0x1000
    size: 0x40000
"""
    )
    read = ["--read-register-now", "cpu@pc"]
    reset = ["--reset-cpu", "cpu"]
    sequence = [
        "--set-reset-vector", "cpu:0x1234", *read,
        "--set-reset-vector", "cpu:0x1238", *read,
        "--set-pc", "cpu:0x5678",
        "--set-reset-vector", "cpu:0x123c", *read,
        *reset, *read,
        "--set-reset-vector", "cpu:0x1240", *read,
    ]
    expected = [0x1234, 0x1238, 0x5678, 0x123C, 0x1240]
    verify(args, platform, "fixed", [
        *read, *sequence, "--set-reset-vector", "cpu:0x2468", *reset, *read, *sequence
    ], [0x1000, *expected, 0x2468, *expected])
    if args.elf:
        verify(args, platform, "elf", [
            *read, "--set-reset-vector", "cpu:0x1234", *read, *reset, *read
        ], [0x2000, 0x2000, 0x1234], args.elf)


if __name__ == "__main__":
    main()
