#!/usr/bin/env python3
"""Validate fixed Renode AMO trace inputs through its binary reader."""
import argparse
from pathlib import Path
import re
import subprocess
import sys


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--renode", type=Path, required=True)
    parser.add_argument("--penv", type=Path, default=Path("target/release/penv"))
    parser.add_argument("--logs", type=Path, required=True)
    parser.add_argument("--representative", action="store_true")
    parser.add_argument("--reset-before", action="store_true")
    args = parser.parse_args()
    args.logs.mkdir(parents=True, exist_ok=True)
    sys.path.insert(0, str(args.renode / "tools/execution_tracer"))
    if not args.representative:
        from execution_tracer_keywords import parse_binary_trace

    robot = (args.renode / "tests/unit-tests/execution-tracing.robot").read_text()

    def value(name):
        return int(re.search(r"^\$\{" + name + r"\}\s+(0x[0-9a-fA-F]+)", robot, re.M)[1], 0)

    addr = value("riscv_amoadd_d_address")
    old = value("riscv_amoadd_d_memory_before")
    new = value("riscv_amoadd_d_expected_sum")
    rd, rs2 = value("64_bit_value_1"), value("64_bit_value_2")
    platform = args.logs / "amo.repl"
    platform.write_text("""cpu: CPU.RiscV64 @ sysbus
    cpuType: "rv64imacv"
    timeProvider: empty
mem: Memory.MappedMemory @ sysbus 0x2000
    size: 0x40000
""")
    failures = 0
    for mode in ("text", "binary"):
        trace = args.logs / f"amo.{mode}"
        trace.unlink(missing_ok=True)
        run = subprocess.run([
            args.penv, "run", "--platform", platform, "--max-steps", "0",
            "--trace-amo", f"cpu:{mode}:{trace}",
            *(["--reset-cpu", "cpu"] if args.reset_before else []), "--set-pc", "cpu:0x2000",
            "--set-gpr", f"cpu@x15:{rd:#x}", "--set-gpr", f"cpu@x14:{rs2:#x}",
            "--set-gpr", f"cpu@x13:{addr:#x}", "--write-memory", f"{addr:#x}:8:{old:#x}",
            "--assemble", "cpu@0x2000:amoadd.d.aqrl a5, a4, (a3)", "--step-cpu", "cpu",
        ], capture_output=True, text=True, timeout=30)
        (args.logs / f"{mode}.stderr").write_text(run.stderr)
        (args.logs / f"{mode}.status").write_text(f"{run.returncode}\n")
        passed = False
        if run.returncode == 0 and not run.stdout and trace.exists():
            if mode == "text":
                lines = trace.read_text().splitlines()
            else:
                entries = parse_binary_trace(str(trace), disassemble=True)
                lines = entries[0].splitlines() if len(entries) == 1 else []
            passed = len(lines) == 3 and all(
                expected in " ".join(line.split()) for expected, line in zip((
                    "amoadd.d.aqrl a5, a4, (a3)",
                    f"AMO operands before - RD: 0x{rd:X}, RS1: 0x{addr:X} (memory value: 0x{old:X}), RS2: 0x{rs2:X}",
                    f"AMO operands after - RD: 0x{old:X}, RS1: 0x{addr:X} (memory value: 0x{new:X}), RS2: 0x{rs2:X}",
                ), lines)
            )
            (args.logs / f"{mode}.decoded").write_text("\n".join(lines) + "\n")
        failures += not passed
        print(f"{'PASS' if passed else 'FAIL'} AMO {mode} trace")
        if not passed:
            print(run.stderr)
        if args.representative:
            break
    raise SystemExit(int(failures != 0))


if __name__ == "__main__":
    main()
