#!/usr/bin/env python3
"""Verify the fixed memory-access.robot RV64 instruction and monitor cases."""

import argparse
from pathlib import Path
import subprocess


def check(args, name, commands, status, required, elf=None):
    result = subprocess.run(
        [args.penv, "run", "--platform", args.logs / "memory-access.repl",
         "--max-steps", "0", *commands, *([elf] if elf else [])],
        capture_output=True, text=True, timeout=30,
    )
    (args.logs / f"{name}.stderr").write_text(result.stderr)
    (args.logs / f"{name}.status").write_text(f"{result.returncode}\n")
    if result.returncode != status or result.stdout or any(
        message not in result.stderr for message in required
    ):
        raise RuntimeError(f"{name}: unexpected result {result.returncode}: {result.stderr}")
    print(f"{name}: verified")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--penv", type=Path, default=Path("target/release/penv"))
    parser.add_argument("--logs", type=Path, required=True)
    parser.add_argument("--cc", required=True)
    args = parser.parse_args()
    args.logs.mkdir(parents=True, exist_ok=True)
    (args.logs / "memory-access.repl").write_text(
        """cpu: CPU.RiscV64 @ sysbus
    cpuType: "rv64g"
    privilegedArchitecture: PrivilegedArchitecture.Priv1_10
mem: Memory.MappedMemory @ sysbus 0x0
    size: 0x3000
"""
    )

    def program(name, instruction):
        source = args.logs / f"{name}.S"
        elf = source.with_suffix(".elf")
        source.write_text(
            f".text\n.globl _start\n_start:\nli x1, 0x8000000000000000\n"
            f"{instruction}\n.org 0x1000\nnop\n"
        )
        subprocess.run(
            [args.cc, "-march=rv64g", "-mabi=lp64d", "-nostdlib", "-static",
             "-Wl,-n,-Ttext=0x1000,-e,_start,--build-id=none", "-o", elf, source],
            check=True,
        )
        return elf

    setup = ["--set-mtvec", "cpu:0x2000"]
    step = ["--step-cpu", "cpu"]
    read_pc = ["--read-register-now", "cpu@pc"]
    illegal = "from an illegal address outside of the physical memory address space: 0x8000000000000000"
    check(args, "fetch", [
        *setup, *step, *step, *step, *read_pc, *step, "--read-register-now", "cpu@mcause",
    ], 1, [
        "register cpu@pc = 0x8000000000000000",
        f"Trying to execute code {illegal}",
    ], program("fetch", "jr x1"))
    if "cpu@mcause" in (args.logs / "fetch.stderr").read_text():
        raise RuntimeError("illegal fetch continued after the abort")
    check(args, "load", [
        *setup, *step, *step, *step, *read_pc, "--read-register-now", "cpu@mcause",
        "--read-register-now", "cpu@mtval",
    ], 0, [
        f"Trying to access memory {illegal}",
        "register cpu@pc = 0x0000000000002000",
        "register cpu@mcause = 0x0000000000000005",
        "register cpu@mtval = 0x8000000000000000",
    ], program("load", "lw x2, 0(x1)"))
    check(args, "monitor", [
        "--translate-read", "cpu:0x00ffffffffffffff",
        "--translate-read", "cpu:0x8000000000000000",
    ], 1, [
        "translation cpu@0x00ffffffffffffff = 0x00ffffffffffffff",
        "Failed to translate address 0x8000000000000000",
    ])


if __name__ == "__main__":
    main()
