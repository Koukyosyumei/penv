#!/usr/bin/env python3
"""Run the fixed RV64 AndeStar LEA Robot operands through the Penv monitor CLI."""

import argparse
from pathlib import Path
import re
import subprocess


CASES = {
    "Should Execute LEA B ZE On RV64": ("nds.lea.b.ze", 8),
    "Should Execute LEA H ZE On RV64": ("nds.lea.h.ze", 9),
    "Should Execute LEA W ZE On RV64": ("nds.lea.w.ze", 10),
    "Should Execute LEA D ZE On RV64": ("nds.lea.d.ze", 11),
    "Should Preserve Full Width LEA H On RV64": ("nds.lea.h", 5),
    "Should Preserve Full Width LEA W On RV64": ("nds.lea.w", 6),
    "Should Preserve Full Width LEA D On RV64": ("nds.lea.d", 7),
}


def verify(args, platform, label, commands, expected):
    result = subprocess.run(
        [args.penv, "run", "--platform", platform, "--max-steps", "0", *commands],
        capture_output=True, text=True, timeout=30,
    )
    (args.logs / f"{label}.stderr").write_text(result.stderr)
    (args.logs / f"{label}.status").write_text(f"{result.returncode}\n")
    observed = [(name, int(value, 16)) for name, value in re.findall(
        r"^penv: register cpu@(\w+) = (0x[0-9a-f]+)$", result.stderr, re.M
    )]
    if result.returncode or result.stdout or observed != expected:
        raise RuntimeError(f"{label}: expected {expected!r}, got {result.stderr}")
    print(f"PASS {label}")


def write_platform(path, isa):
    path.write_text(f"""cpu: CPU.RiscV64 @ sysbus
    cpuType: "{isa}"
    timeProvider: empty
mem: Memory.MappedMemory @ sysbus 0x0
    size: 0x100000
""")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("robot", type=Path)
    parser.add_argument("--penv", type=Path, default=Path("target/release/penv"))
    parser.add_argument("--logs", type=Path, required=True)
    args = parser.parse_args()
    args.logs.mkdir(parents=True, exist_ok=True)
    platform = args.logs / "andestar.repl"
    write_platform(platform, "rv64imac_xandesperf")

    tests = dict(re.findall(
        r"^(Should [^\n]+)\n((?:[ \t]+[^\n]*\n)+)", args.robot.read_text(), re.M
    ))
    step = ["--set-pc", "cpu:0x2000", "--step-cpu", "cpu", "--read-register-now", "cpu@pc"]
    for case, (mnemonic, function) in CASES.items():
        body = tests[case]
        operands = [line.split()[3:] for line in body.splitlines()
                    if line.split()[:4] == ["Check", "Register", "Instruction", mnemonic]]
        if "Create Machine                  bits=64" not in body or len(operands) != 3:
            raise RuntimeError(f"unexpected fixed Robot fixture for {case}")
        word = function << 25 | 11 << 20 | 10 << 15 | 12 << 7 | 0x5b
        commands = ["--write-memory", f"0x2000:4:{word:#x}"]
        expected = []
        for _, lhs, rhs, rd in operands:
            commands += ["--set-gpr", f"cpu@x10:{lhs}", "--set-gpr", f"cpu@x11:{rhs}",
                         *step, "--read-register-now", "cpu@x12"]
            expected += [("pc", 0x2004), ("x12", int(rd, 16))]
        verify(args, platform, case, commands, expected)

    base = args.logs / "base.repl"
    write_platform(base, "rv64imac")
    verify(args, base, "extension-disabled", [
        "--write-memory", "0x2000:4:0x0ab5065b", "--set-mtvec", "cpu:0x1000",
        *step, "--read-register-now", "cpu@mcause", "--read-register-now", "cpu@mepc",
    ], [("pc", 0x1000), ("mcause", 2), ("mepc", 0x2000)])


if __name__ == "__main__":
    main()
