#!/usr/bin/env python3
"""Check fixed RV64 LLVM Robot round trips and assembler diagnostics."""
import argparse
from pathlib import Path
import re
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("robot", type=Path)
    parser.add_argument("--logs", type=Path, required=True)
    parser.add_argument("--penv", type=Path, default=Path("target/release/penv"))
    parser.add_argument("--representative", action="store_true")
    args = parser.parse_args()
    args.logs.mkdir(parents=True, exist_ok=True)
    robot = args.robot.read_text()
    failures = 0

    def run(label, isa, commands, expected, status=0):
        nonlocal failures
        platform = args.logs / f"{isa}.repl"
        platform.write_text(f"""cpu: CPU.RiscV64 @ sysbus
    cpuType: "{isa}"
    timeProvider: empty
mem: Memory.MappedMemory @ sysbus 0
    size: 0x50000
""")
        result = subprocess.run([args.penv, "run", "--platform", platform, "--max-steps", "0", *commands],
                                capture_output=True, text=True, timeout=30)
        (args.logs / f"{label}.stderr").write_text(result.stderr)
        (args.logs / f"{label}.status").write_text(f"{result.returncode}\n")
        passed = not result.stdout and result.returncode == status and all(
            re.search(pattern, result.stderr, re.M) for pattern in expected
        )
        failures += not passed
        print(f"{'PASS' if passed else 'FAIL'} {label}")
        if not passed:
            print(result.stderr)
            for pattern in expected:
                if not re.search(pattern, result.stderr, re.M):
                    print(f"missing: {pattern}")

    def body(title):
        return re.search(r"^" + re.escape(title) + r"\n((?:[ \t]+[^\n]*\n|\n)+)", robot, re.M)[1]

    for isa in ("rv64imac", "rv64imafdc", "rv64gc", "rv64gcv"):
        commands = []
        expected = []
        address = 0
        for line in body(f"Should Assemble And Disassemble RISCV64{isa[4:].upper()}").splitlines():
            if not line.strip().startswith("Assemble And Disassemble "):
                continue
            keyword = line.split("  ")[-1].strip()
            if keyword == "compressed=True":
                keyword = "Assemble And Disassemble RV32IMA"
            rows = body(keyword)
            rows = re.sub(r"^\s*ELSE\n.*?^\s*END\n", "", rows, flags=re.M | re.S)
            for row in rows.splitlines():
                cells = re.split(r"\s{2,}", row.split("#", 1)[0].strip())
                if cells[0] not in ("RoundTrip LE", "DisasTest LE"):
                    continue
                action, encoding, mnemonic, *rest = cells
                operands, *_ = rest or [""]
                width = len(encoding) // 2
                if action == "RoundTrip LE":
                    commands += ["--assemble", f"cpu@{address:#x}:{mnemonic} {operands}".rstrip(),
                                 "--read-memory-now", f"{address:#x}:{width}"]
                    expected.append(re.escape(
                        f"memory sysbus@{address:#018x}:{width} = {int(encoding, 16):#018x}"))
                else:
                    commands += ["--write-memory", f"{address:#x}:{width}:0x{encoding}"]
                commands += ["--disassemble", f"cpu@{address:#x}:{width}"]
                expected.append(re.escape(
                    f"disassembly cpu@{address:#x}: {encoding} {mnemonic} {operands}".rstrip()) + "$")
        if not commands:
            raise RuntimeError(f"empty round-trip case for {isa}")
        run(f"Should Assemble And Disassemble RISCV64{isa[4:].upper()}", isa, commands, expected)
        if args.representative:
            break
    if not args.representative:
        for index, (instruction, diagnostic) in enumerate((
            ("illegalinstruction123", "unrecognized instruction mnemonic"),
            ("sw s123, 0(sp)", "invalid operand"),
            ("vsetvli a4, a0, e8, m1, tu, mu", "instruction requires.*'V'"),
        )):
            run(f"Should Handle Illegal Instruction When Assembling {index + 1}", "rv64g",
                ["--assemble", f"cpu@0:{instruction}"], [diagnostic], 1)
    raise SystemExit(int(failures != 0))


if __name__ == "__main__":
    main()
