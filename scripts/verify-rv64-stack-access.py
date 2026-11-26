#!/usr/bin/env python3
"""Run the fixed stack-access hook assembly and check each observable event."""

import argparse
from pathlib import Path
import re
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("robot", type=Path)
    parser.add_argument("--logs", required=True, type=Path)
    parser.add_argument("--cc", required=True)
    parser.add_argument("--penv", type=Path, default=Path("target/release/penv"))
    parser.add_argument("--case", help="Run only the case with this name suffix")
    args = parser.parse_args()
    args.logs.mkdir(parents=True, exist_ok=True)
    platform = args.logs / "stack-access.repl"
    platform.write_text("""clint: IRQControllers.CoreLevelInterruptor @ sysbus 0x44000000
    frequency: 66000000
cpu: CPU.RiscV64 @ sysbus
    cpuType: "rv64gc_V"
    timeProvider: clint
    traceStackAccess: true
mem: Memory.MappedMemory @ sysbus 0x1000
    size: 0x40000
""")
    source = args.robot.read_text()
    pc = int(re.search(r"^\$\{initial_pc\}\s+(\S+)", source, re.M)[1], 0)
    sp = int(re.search(r"^\$\{initial_sp\}\s+(\S+)", source, re.M)[1], 0)
    cases = re.findall(r"^Should Trigger Stack Access Hook ([^\n]+)\n((?:[ \t]+[^\n]*\n)+)", source, re.M)
    if len(cases) != 21:
        parser.error("expected 21 fixed Robot cases")
    failed = 0
    for name, body in cases:
        if args.case and args.case != name:
            continue
        if vector := re.search(r'AssembleBlock \$\{initial_pc\} "([^"]+)"', body):
            assembly = vector[1]
            steps = int(re.search(r"cpu Step (\d+)", body)[1])
            width = int(re.search(r"e(8|64),", assembly)[1])
            stride = re.search(r"li a0, (\d+)", assembly)
            stride = int(stride[1]) if stride else width // 8
            count = 8
            offset = 0
        else:
            assembly = re.search(r"Execute Instruction\s+([^\n]+)", body)[1]
            steps = count = 1
            width = int(re.search(r"Should Have Fired At[^\n]+\s+(\d+)", body)[1])
            offset = int(re.search(r"(\d+)\(sp\)", assembly)[1])
            stride = 0
        stem = args.logs / name.lower().replace(" ", "-")
        asm = stem.with_suffix(".S")
        elf = stem.with_suffix(".elf")
        asm.write_text(".text\n.globl _start\n_start:\n" + assembly + "\n")
        subprocess.run([args.cc, "-march=rv64gcv", "-mabi=lp64d", "-nostdlib", "-static",
                        f"-Wl,-n,-Ttext={pc:#x},-e,_start,--build-id=none", "-o", elf, asm], check=True)
        run = subprocess.run([args.penv, "run", "--platform", platform, "--max-steps", "0",
                              "--set-mstatus", "cpu:0x2200", "--set-gpr", f"cpu@x2:{sp:#x}",
                              *(["--step-cpu", "cpu"] * steps), elf],
                             capture_output=True, text=True, timeout=30)
        kind = "True" if "Write Hook Should Have Fired At" in body else "False"
        expected = [(f"{sp + offset + i * stride:X}", str(width), kind) for i in range(count)]
        actual = re.findall(r"^penv: cpu: StackAccess: 0x([0-9A-F]+), (\d+), (True|False)$",
                            run.stderr, re.M)
        passed = run.returncode == 0 and not run.stdout and actual == expected
        stem.with_suffix(".stderr").write_text(run.stderr)
        stem.with_suffix(".status").write_text(f"{run.returncode}\n")
        failed += not passed
        print(f"{'PASS' if passed else 'FAIL'} Should Trigger Stack Access Hook {name}")
        if not passed:
            print(f"  expected {expected!r}, got {actual!r}")
    raise SystemExit(int(failed != 0))


if __name__ == "__main__":
    main()
