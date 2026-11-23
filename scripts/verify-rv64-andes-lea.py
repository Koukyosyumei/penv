#!/usr/bin/env python3
"""Run fixed RV64 AndeStar LEA, bit operation and GP-relative cases through the Penv CLI."""

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


def verify_bits(args, platform, tests, step):
    case = "Should Test RV64 Bits Above Thirty One"
    commands = []
    expected = []
    for mnemonic, source, bit, offset, pc in fixture_rows(tests[case], "Check Immediate Branch", 4):
        bit = int(bit)
        offset = int(offset) & 0x7ff
        function = {"nds.bbc": 0, "nds.bbs": 1}[mnemonic]
        word = ((offset & 0x400) << 21 | function << 30 | (offset & 0x3e0) << 20
                | (bit & 31) << 20 | 10 << 15 | 7 << 12 | (offset & 0x1e) << 7
                | (bit >> 5) << 7 | 0x5b)
        commands += ["--write-memory", f"0x2000:4:{word:#x}", "--set-gpr", f"cpu@x10:{source}",
                     *step, "--read-register-now", "cpu@x10"]
        expected += [("pc", int(pc, 16)), ("x10", int(source, 16))]
    verify(args, platform, case, commands, expected)

    case = "Should Extract And Insert Full Width Bit Fields On RV64"
    commands = []
    expected = []
    for mnemonic, source, msb, lsb, rd in fixture_rows(tests[case], "Check Bit Field", 6):
        function = {"nds.bfoz": 2, "nds.bfos": 3}[mnemonic]
        word = int(msb) << 26 | int(lsb) << 20 | 10 << 15 | function << 12 | 12 << 7 | 0x5b
        commands += ["--write-memory", f"0x2000:4:{word:#x}", "--set-gpr", f"cpu@x10:{source}",
                     *step, "--read-register-now", "cpu@x12"]
        expected += [("pc", 0x2004), ("x12", int(rd, 16))]
    verify(args, platform, case, commands, expected)


def gp_word(mnemonic, offset):
    opcode, function, width = {
        "nds.lbgp": (0x0b, 0, 1),
        "nds.lbugp": (0x0b, 2, 1),
        "nds.addigp": (0x0b, 1, 1),
        "nds.lhgp": (0x2b, 1, 2),
        "nds.lhugp": (0x2b, 5, 2),
        "nds.lwgp": (0x2b, 2, 4),
    }[mnemonic]
    word = opcode | 12 << 7 | function << 12
    word |= (offset & 0x800) << 9 | (offset & 0x7000) << 5 | (offset & 0x18000)
    if width == 4:
        word |= (offset & 0x7fc) << 20 | (offset & 0x20000) << 4 | (offset & 0x40000) << 13
    else:
        word |= (offset & 0x7fe) << 20 | (offset & 0x20000) << 14
        if width == 1:
            word |= (offset & 1) << 14
    return word


def verify_gp(args, platform, tests, step):
    case = "Should Sign Extend GP Loads On RV64"
    commands = ["--set-gpr", "cpu@x3:0x80000"]
    expected = []
    widths = {"Byte": 1, "Word": 2, "DoubleWord": 4}
    for mnemonic, access, address, offset, value, rd in fixture_rows(tests[case], "Check GP Load", 5):
        word = gp_word(mnemonic, int(offset, 0))
        commands += ["--write-memory", f"{address}:{widths[access]}:{value}",
                     "--write-memory", f"0x2000:4:{word:#x}", *step,
                     "--read-register-now", "cpu@x12"]
        expected += [("pc", 0x2004), ("x12", int(rd, 0))]
    verify(args, platform, case, commands, expected)

    case = "Should Preserve Full Width Global Pointer On RV64"
    commands = []
    expected = []
    for base, offset, rd in fixture_rows(tests[case], "Check ADDIGP", 3):
        word = gp_word("nds.addigp", int(offset, 0))
        commands += ["--set-gpr", f"cpu@x3:{base}", "--write-memory", f"0x2000:4:{word:#x}",
                     *step, "--read-register-now", "cpu@x12"]
        expected += [("pc", 0x2004), ("x12", int(rd, 0))]
    verify(args, platform, case, commands, expected)


def fixture_rows(body, keyword, count):
    prefix = keyword.split()
    rows = [cells[len(prefix):] for line in body.splitlines()
            if (cells := line.split())[:len(prefix)] == prefix]
    if "Create Machine                  bits=64" not in body or len(rows) != count:
        raise RuntimeError(f"unexpected fixed Robot fixture for {keyword}")
    return rows


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

    verify_bits(args, platform, tests, step)
    verify_gp(args, platform, tests, step)

    base = args.logs / "base.repl"
    write_platform(base, "rv64imac")
    verify(args, base, "extension-disabled", [
        "--write-memory", "0x2000:4:0x0ab5065b", "--set-mtvec", "cpu:0x1000",
        *step, "--read-register-now", "cpu@mcause", "--read-register-now", "cpu@mepc",
    ], [("pc", 0x1000), ("mcause", 2), ("mepc", 0x2000)])


if __name__ == "__main__":
    main()
