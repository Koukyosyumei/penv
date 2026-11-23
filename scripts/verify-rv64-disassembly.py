#!/usr/bin/env python3
"""Check the fixed RV64 disassembly-only Robot cases."""

import argparse
from pathlib import Path
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("elf", type=Path)
    parser.add_argument("--renode", type=Path, default=Path("../renode"))
    parser.add_argument("--penv", type=Path, default=Path("target/release/penv"))
    parser.add_argument("--logs", type=Path, required=True)
    parser.add_argument("--representative", action="store_true")
    args = parser.parse_args()
    args.logs.mkdir(parents=True, exist_ok=True)
    plain = args.logs / "rv64g.repl"
    plain.write_text("""cpu: CPU.RiscV64 @ sysbus
    cpuType: "rv64g"
    timeProvider: empty
mem: Memory.MappedMemory @ sysbus 0
    size: 0x50000
""")
    failures = 0

    def check(name, platform, commands, required, elf=None):
        nonlocal failures
        result = subprocess.run(
            [args.penv, "run", "--platform", platform, "--max-steps", "0", *commands,
             *([elf] if elf else [])],
            capture_output=True, text=True, timeout=30,
        )
        (args.logs / f"{name}.stderr").write_text(result.stderr)
        (args.logs / f"{name}.status").write_text(f"{result.returncode}\n")
        passed = not result.returncode and not result.stdout and all(
            line in result.stderr for line in required
        )
        failures += not passed
        print(f"{'PASS' if passed else 'FAIL'} {name}")
        if not passed:
            print(result.stderr)

    fixture = args.renode / "tests/unit-tests/per-core-registration.repl"
    check("Disassemble Code From Per Core Memory", fixture, [
        "--write-memory", "cpu1@0x3000000:4:0x1234",
        "--write-memory", "cpu2@0x3000000:4:0x5678",
        "--disassemble", "cpu1@0x3000000:2",
        "--disassemble", "cpu2@0x3000000:2",
    ], ["cpu1@0x3000000: 1234 addi a3, sp, 296", "cpu2@0x3000000: 5678 lw a4, 108(a2)"], args.elf)
    if not args.representative:
        check("Should Handle Illegal Instruction When Disassembling", plain, [
            "--write-memory", "0:4:0", "--disassemble", "cpu@0:4",
            "--write-memory", "0:4:0x0000", "--disassemble", "cpu@0:4",
        ], ["Disassembly error detected at cpu@0x0: invalid instruction encoding\n"
            "penv: Disassembly error detected at cpu@0x0: invalid instruction encoding"])
        check("Should Handle Disassembly From Invalid Address", plain, [
            "--write-memory", "0x1234:4:0x02051613", "--disassemble", "cpu@0x1234:4",
            "--disassemble", "cpu@0x02000000:4",
        ], ["cpu@0x1234: 02051613 slli a2, a0, 32",
            "Disassembly error detected at cpu@0x2000000: unmapped RAM"])
    raise SystemExit(int(failures != 0))


if __name__ == "__main__":
    main()
