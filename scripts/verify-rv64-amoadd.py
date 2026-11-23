#!/usr/bin/env python3
"""Port pinned RV64 AMOADD ArrayMemory assertions with ordered CLI steps."""

import argparse
from pathlib import Path
import subprocess


def verify(args, platform, variant, commands, expected):
    run = subprocess.run(
        [args.penv, "run", "--platform", platform, "--max-steps", "0", *commands,
         args.elfs / f"{variant}.elf"],
        capture_output=True, text=True, timeout=30,
    )
    (args.logs / f"{variant}.stderr").write_text(run.stderr)
    (args.logs / f"{variant}.status").write_text(f"{run.returncode}\n")
    if run.returncode or run.stdout or run.stderr.splitlines() != expected:
        raise RuntimeError(f"{variant}: {run.stderr}")
    print(f"{variant}: verified {len(expected)} ordered assertions")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("elfs", type=Path)
    parser.add_argument("--penv", type=Path, default=Path("target/release/penv"))
    parser.add_argument("--logs", type=Path, required=True)
    args = parser.parse_args()
    args.logs.mkdir(parents=True, exist_ok=True)
    platform = args.logs / "amoadd.repl"
    platform.write_text("""dram: Memory.MappedMemory @ sysbus 0x80000000 {
    size: 0x80000000
}
mmio: Memory.ArrayMemory @ sysbus 0x100000000 {
    size: 0x10000
}
cpu: CPU.RiscV64 @ sysbus {
    cpuType: "rv64gc_zicsr_zifencei";
    hartId: 1;
    privilegedArchitecture: PrivilegedArchitecture.Priv1_10;
    timeProvider: empty;
    CyclesPerInstruction: 8;
    allowUnalignedAccesses: true
}
""")
    original = 0xFFFFFFFF80000000
    increment = 0xFFFFFFFFFFFFF800
    address = 0x100001000
    for variant, width, first, second, last_increment in (
        ("WORD", 4, 0x7FFFF800, 0xFFFFF800, original),
        ("DOUBLE", 8, 0xFFFFFFFF7FFFF800, 0xFFFFFFFF7FFFF000, increment),
    ):
        value = original & ((1 << (width * 8)) - 1)
        verify(args, platform, variant, [
            "--write-memory", f"{address:#x}:{width}:{value:#x}",
            "--set-gpr", f"cpu@x10:{address:#x}",
            "--set-gpr", f"cpu@x11:{increment:#x}",
            "--step-cpu", "cpu",
            "--read-register-now", "cpu@x12",
            "--read-register-now", "cpu@x11",
            "--read-memory-now", f"{address:#x}:{width}",
            "--set-gpr", f"cpu@x11:{last_increment:#x}",
            "--step-cpu", "cpu",
            "--read-register-now", "cpu@x12",
            "--read-memory-now", f"{address:#x}:{width}",
        ], [
            f"penv: register cpu@x12 = {original:#018x}",
            f"penv: register cpu@x11 = {increment:#018x}",
            f"penv: memory sysbus@{address:#018x}:{width} = {first:#018x}",
            f"penv: register cpu@x12 = {first:#018x}",
            f"penv: memory sysbus@{address:#018x}:{width} = {second:#018x}",
        ])
    verify(args, platform, "HARTID", ["--step-cpu", "cpu", "--read-register-now", "cpu@x10"],
           ["penv: register cpu@x10 = 0x0000000000000001"])


if __name__ == "__main__":
    main()
