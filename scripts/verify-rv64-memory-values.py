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


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("robot", type=Path)
    parser.add_argument("--penv", type=Path, default=Path("target/release/penv"))
    parser.add_argument("--logs", required=True, type=Path)
    args = parser.parse_args()
    args.logs.mkdir(parents=True, exist_ok=True)
    platform = args.logs / "memory-values.repl"
    platform.write_text("""cpu: CPU.RiscV64 @ sysbus
    cpuType: "rv64imafd_zicsr_zifencei_c_v_zba_zbb_zbc_zbs_zfh_zacas_zcb"
mem: Memory.MappedMemory @ sysbus 0x0
    size: 0x40000
""")
    source = args.robot.read_text()
    failed = 0
    for case, count in (("Should Read Quad Word At Raw Address", 9),
                        ("Should Read Quad Word Zero At Raw Address", 1)):
        body = re.search(r"^" + re.escape(case) + r"\n((?:[ \t]+[^\n]*\n)+)", source, re.M)[1]
        store = re.search(r"Store Quad Word\s+(\S+)\s+(\S+)", body)
        address, written = (int(item, 0) for item in store.groups())
        assertions = []
        for line in body.splitlines():
            cells = re.split(r"\s{2,}", line.strip())
            if not cells[0].startswith("Memory Should "):
                continue
            operation = cells[0].removeprefix("Memory Should ").removeprefix("Be ")
            if int(cells[1], 0) != address or cells[-1] != "QuadWord":
                raise RuntimeError(f"unexpected fixed assertion {line!r}")
            operand = int(cells[2], 0) if len(cells) == 4 else 0
            assertions.append((operation, operand))
        if len(assertions) != count:
            raise RuntimeError(f"expected {count} assertions in {case}")
        run = subprocess.run([
            args.penv, "run", "--platform", platform, "--max-steps", "0",
            "--write-memory", f"{address:#x}:8:{written:#x}",
            *(["--read-memory-now", f"{address:#x}:8"] * count),
        ], capture_output=True, text=True, timeout=30)
        (args.logs / f"{case}.stderr").write_text(run.stderr)
        (args.logs / f"{case}.status").write_text(f"{run.returncode}\n")
        values = [int(value, 0) for value in re.findall(
            rf"^penv: memory sysbus@{address:#018x}:8 = (0x[0-9a-f]+)$", run.stderr, re.M
        )]
        passed = not run.returncode and not run.stdout and len(values) == count and all(
            OPERATIONS[op](value, operand) for (op, operand), value in zip(assertions, values)
        )
        print(f"{'PASS' if passed else 'FAIL'} {case} ({count} assertions)")
        failed += not passed
    raise SystemExit(int(failed != 0))


if __name__ == "__main__":
    main()
