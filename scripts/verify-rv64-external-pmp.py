#!/usr/bin/env python3
"""Port the four fixed RV64 external PMP cases and exercise the omitted config loop."""

import argparse
from pathlib import Path
import re
import subprocess


CONFIG = 0x0303030303030303
START = 0x1000


def verify(args, platform, name, commands, expected, elf=None):
    run = subprocess.run(
        [args.penv, "run", "--platform", platform, "--max-steps", "0", *commands,
         *([elf] if elf else [])],
        capture_output=True, text=True, timeout=30,
    )
    (args.logs / f"{name}.stderr").write_text(run.stderr)
    (args.logs / f"{name}.status").write_text(f"{run.returncode}\n")
    observed = [(reg, int(value, 16)) for reg, value in re.findall(
        r"^penv: register cpu@(\w+) = (0x[0-9a-f]+)$", run.stderr, re.M
    )]
    if (run.returncode or run.stdout or observed != expected
            or "cpu: Enabling External PMP" not in run.stderr
            or re.search(r"invalid (?:external )?PMP|trap", run.stderr, re.I)):
        raise RuntimeError(f"{name}: expected {expected!r}, got {observed!r}: {run.stderr}")
    print(f"{name}: verified {len(expected)} register observations")


def monitor(args, platform, name, writes):
    commands = []
    for reg, value in writes:
        commands += ["--write-pmp-register", f"cpu@{reg}:{value}", "--read-register-now", f"cpu@{reg}"]
    verify(args, platform, name, commands, writes)


def cpu(args, platform, name, blocks):
    source = args.logs / f"{name}.S"
    elf = source.with_suffix(".elf")
    source.write_text(".text\n.globl _start\n_start:\n" + "\n".join(
        assembly for assembly, _, _ in blocks
    ) + "\n")
    subprocess.run([
        args.cc, "-march=rv64gc", "-mabi=lp64d", "-nostdlib", "-static",
        "-Wl,-n,-Ttext=0x1000,-e,_start,--build-id=none", "-o", elf, source,
    ], check=True)
    commands = []
    expected = []
    pc = START

    def read(reg, value):
        commands.extend(("--read-register-now", f"cpu@{reg}"))
        expected.append((reg, value))

    read("pc", pc)
    for _, sizes, registers in blocks:
        for size in sizes:
            commands.extend(("--step-cpu", "cpu"))
            pc += size
            read("pc", pc)
        for reg, value in registers:
            read(reg, value)
    verify(args, platform, name, commands, expected, elf)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--penv", type=Path, default=Path("target/release/penv"))
    parser.add_argument("--logs", type=Path, required=True)
    parser.add_argument("--cc", required=True, help="RV64 GNU cross compiler")
    args = parser.parse_args()
    args.logs.mkdir(parents=True, exist_ok=True)
    platform = args.logs / "external-pmp.repl"
    platform.write_text("""dram: Memory.MappedMemory @ sysbus 0x80000000
    size: 0x80000000
mtvec: Memory.MappedMemory @ sysbus 0x1000
    size: 0x40000
cpu: CPU.RiscV64 @ sysbus
    cpuType: "rv64gc_zicsr"
    timeProvider: empty
    resetVector: 0x1000
pmp: Miscellaneous.RiscVExternalPMP @ cpu
    numberOfPMPEntries: 16
""")

    monitor(args, platform, "address-monitor", [(f"pmpaddr{i}", i) for i in range(16)])
    address = [("li a0, 0", [2], [])]
    for i in range(16):
        address += [
            (f"csrw pmpaddr{i}, a0\ncsrr t0, pmpaddr{i}", [4, 4], [("x10", i), ("x5", i)]),
            ("addi a0, a0, 1", [2], []),
        ]
    address.append(("", [], [("x10", 16)]))
    cpu(args, platform, "address-cpu", address)

    # Both fixed Robot config loops are NOT RUN under the pinned runner.
    monitor(args, platform, "config-monitor", [])
    config = [(f"li a0, {CONFIG}", [4, 4, 2, 4, 2, 4], [("x10", CONFIG)])]
    cpu(args, platform, "config-cpu", config)

    monitor(args, platform, "config-monitor-supplemental", [("pmpcfg0", CONFIG)])
    cpu(args, platform, "config-cpu-supplemental", config + [
        ("csrw pmpcfg0, a0\ncsrr t0, pmpcfg0", [4, 4], [("x10", CONFIG), ("x5", CONFIG)]),
    ])


if __name__ == "__main__":
    main()
