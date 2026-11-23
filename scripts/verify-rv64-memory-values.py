#!/usr/bin/env python3
"""Port the fixed immediate raw-address RV64 quadword assertions."""

import argparse
import operator
from pathlib import Path
import re
import subprocess


OPERATIONS = {
    "Equal": operator.eq,
    "Not Be Equal": operator.ne,
    "Less Than": operator.lt,
    "Less Or Equal": operator.le,
    "Greater Than": operator.gt,
    "Greater Or Equal": operator.ge,
    "Nonzero": operator.ne,
    "Zero": operator.eq,
}


def verify_after(args, platform, source):
    name = "Should Read Quad Word At Raw Address After"
    start = int(re.search(r"^\$\{initial_pc\}\s+(\S+)", source, re.M)[1], 0)
    block = re.search(r"^\$\{asm_quadword\}[^\n]*\n((?:\.\.\.[^\n]+\n)+)", source, re.M)[1]
    program = "\n".join(line[3:].strip() for line in block.splitlines())
    fixture = re.search(r"^" + name + r"\n((?:[ \t]+[^\n]*\n)+)", source, re.M)[1]
    assertion = re.search(r"Memory Should Be Equal\s+(\S+)\s+(\S+)\s+QuadWord\s+timeout=3", fixture)
    address, value = (int(item, 0) for item in assertion.groups())
    assembly, elf = (args.logs / f"quadword-after.{suffix}" for suffix in ("S", "elf"))
    assembly.write_text(".text\n.globl _start\n_start:\n" + program + "\n")
    subprocess.run([args.after, "-march=rv64gc", "-mabi=lp64d", "-nostdlib",
                    f"-Wl,-n,-Ttext={start:#x},-e,_start,--build-id=none", "-o", elf, assembly],
                   check=True)
    run = subprocess.run([
        args.penv, "run", "--platform", platform, "--max-steps", "30000",
        "--read-memory-now", f"{address:#x}:8", "--read-memory", f"{address:#x}:8", elf,
    ], capture_output=True, text=True, timeout=30)
    (args.logs / "quadword-after.stderr").write_text(run.stderr)
    (args.logs / "quadword-after.status").write_text(f"{run.returncode}\n")
    label = f"penv: memory sysbus@{address:#018x}:8 = "
    observed = [line.removeprefix(label) for line in run.stderr.splitlines() if line.startswith(label)]
    passed = run.returncode == 2 and not run.stdout and observed == [f"{0:#018x}", f"{value:#018x}"]
    print(f"{'PASS' if passed else 'FAIL'} {name}")
    return passed


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("robot", type=Path)
    parser.add_argument("--penv", type=Path, default=Path("target/release/penv"))
    parser.add_argument("--logs", required=True, type=Path)
    parser.add_argument("--signed", action="store_true", help="verify the signed fixture")
    parser.add_argument("--after", metavar="CC", help="also verify the running quadword fixture with this cross compiler")
    parser.add_argument("--isa", default="rv64imafd_zicsr_zifencei_c_v_zba_zbb_zbc_zbs_zfh_zacas_zcb",
                        help="override the fixture ISA for diagnostics")
    args = parser.parse_args()
    args.logs.mkdir(parents=True, exist_ok=True)
    platform = args.logs / "memory-values.repl"
    platform.write_text(f"""cpu: CPU.RiscV64 @ sysbus
    cpuType: "{args.isa}"
mem: Memory.MappedMemory @ sysbus 0x0
    size: 0x40000
""")
    source = args.robot.read_text()
    failed = 0
    cases = (("Should Read Signed Quad Word At Raw Address", 9),) if args.signed else (
        ("Should Read Quad Word At Raw Address", 9), ("Should Read Quad Word Zero At Raw Address", 1),
    )
    for case, count in cases:
        body = re.search(r"^" + re.escape(case) + r"\n((?:[ \t]+[^\n]*\n)+)", source, re.M)[1]
        store = re.search(r"Store Quad Word\s+(\S+)\s+(\S+)", body)
        address, written = (int(item, 0) for item in store.groups())
        assertions = []
        for line in body.splitlines():
            cells = re.split(r"\s{2,}", line.strip())
            if not cells[0].startswith("Memory Should "):
                continue
            signed = cells[-1] == "signed=true"
            if signed:
                cells.pop()
            if signed != args.signed:
                raise RuntimeError(f"unexpected signedness in {line!r}")
            operation = cells[0].removeprefix("Memory Should ").removeprefix("Be ")
            if int(cells[1], 0) != address or cells[-1] != "QuadWord":
                raise RuntimeError(f"unexpected fixed assertion {line!r}")
            operand = int(cells[2], 0) if len(cells) == 4 else 0
            assertions.append((operation, operand))
        if len(assertions) != count:
            raise RuntimeError(f"expected {count} assertions in {case}")
        value = str(written) if args.signed else f"{written:#x}"
        run = subprocess.run([
            args.penv, "run", "--platform", platform, "--max-steps", "0",
            "--write-memory", f"{address:#x}:8:{value}",
            *(["--read-memory-now", f"{address:#x}:8"] * count),
        ], capture_output=True, text=True, timeout=30)
        (args.logs / f"{case}.stderr").write_text(run.stderr)
        (args.logs / f"{case}.status").write_text(f"{run.returncode}\n")
        values = [int(value, 0) for value in re.findall(
            rf"^penv: memory sysbus@{address:#018x}:8 = (0x[0-9a-f]+)$", run.stderr, re.M
        )]
        if args.signed:
            values = [value - (1 << 64) if value >= (1 << 63) else value for value in values]
        passed = not run.returncode and not run.stdout and len(values) == count and all(
            OPERATIONS[op](value, operand) for (op, operand), value in zip(assertions, values)
        )
        print(f"{'PASS' if passed else 'FAIL'} {case} ({count} assertions)")
        failed += not passed
    if args.after:
        failed += not verify_after(args, platform, source)
    raise SystemExit(int(failed != 0))


if __name__ == "__main__":
    main()
