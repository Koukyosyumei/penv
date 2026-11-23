"""Check fixed memory-assertions Robot inputs with RV32 guest loads and stores."""

import argparse
import json
from pathlib import Path
import re
import subprocess


WIDTH = {"Byte": ("sb", "lb", "lbu"), "Word": ("sh", "lh", "lhu"),
         "DoubleWord": ("sw", "lw", "lw")}
BRANCH = {
    "Equal": ("bne", "t1, t2"), "Not Be Equal": ("beq", "t1, t2"),
    "Less Than": ("bge", "t1, t2"), "Less Or Equal": ("blt", "t2, t1"),
    "Greater Than": ("bge", "t2, t1"), "Greater Or Equal": ("blt", "t1, t2"),
    "Zero": ("bne", "t1, t2"), "Nonzero": ("beq", "t1, t2"),
}


def program(body):
    checks = []
    for line in body.splitlines():
        fields = re.split(r"\s{2,}", line.strip())
        keyword, *args = fields
        if keyword.startswith("Store "):
            store = WIDTH[keyword[6:].replace(" ", "")][0]
            checks += [f"li t0, {args[0]}", f"li t1, {args[1]}", f"{store} t1, 0(t0)"]
        elif keyword.startswith("Memory Should "):
            relation = keyword[14:].removeprefix("Be ")
            signed = args[-1] == "signed=true"
            if signed:
                args.pop()
            width = args.pop()
            address, *expected = args
            branch, operands = BRANCH[relation]
            if branch in {"bge", "blt"} and not signed:
                branch += "u"
            checks += [
                f"li t0, {address}", f"{WIDTH[width][1 if signed else 2]} t1, 0(t0)",
                f"li t2, {expected[0] if expected else 0}", f"{branch} {operands}, fail",
            ]
    if not checks:
        raise ValueError("missing fixed raw memory inputs")
    return "\n".join([
        ".option norvc", ".text", ".globl _start", "_start:", *checks,
        "li t1, 1", "j done", "fail: li t1, 3", "done: li t0, 0x2000",
        "sw t1, 0(t0)", "j .", "",
    ])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("penv", "renode", "output"):
        parser.add_argument(name, type=Path)
    args = parser.parse_args()
    source = (args.renode / "tests/unit-tests/memory-assertions.robot").read_text()
    cases = [(name, body) for name, body in re.findall(
        r"(?ms)^(Should Read [^\n]+ At Raw Address)\n(.*?)(?=^\S|\Z)", source,
    ) if "Create Machine 32" in body]
    if len(cases) != 9:
        parser.error(f"expected nine static raw-address RV32 cases, found {len(cases)}")
    isa = "rv32" + re.search(r"rv\$\{bitness\}([a-z0-9_]+)", source)[1]
    args.output.mkdir(parents=True, exist_ok=False)
    results = []
    for mode, cpu_type in (("diagnostic", isa.replace("_v_", "_", 1)), ("fixed", isa)):
        (args.output / f"{mode}.repl").write_text(
            f'cpu: CPU.RiscV32 @ sysbus {{ cpuType: "{cpu_type}" }}\n'
            'mem: Memory.MappedMemory @ sysbus 0x0 { size: 0x40000 }\n'
        )
    linker = args.output / "layout.ld"
    linker.write_text("ENTRY(_start)\nPHDRS { code PT_LOAD FLAGS(5); }\n"
                      "SECTIONS { .text 0x10 : { *(.text) } :code }\n")
    for index, (name, body) in enumerate(cases):
        asm = args.output / f"{index}.S"
        elf = asm.with_suffix(".elf")
        asm.write_text(program(body))
        subprocess.run(
            ["clang", "--target=riscv32-unknown-elf", "-march=rv32imac", "-mabi=ilp32",
             "-nostdlib", "-fuse-ld=lld", f"-Wl,-T,{linker}", asm, "-o", elf],
            check=True, timeout=15,
        )
        for mode in ("diagnostic", "fixed"):
            run = subprocess.run(
                [args.penv.resolve(), "run", "--platform", args.output / f"{mode}.repl",
                 "--tohost", "0x2000", "--max-steps", "128", elf],
                capture_output=True, text=True, timeout=15,
            )
            log = args.output / f"{index}-{mode}.log"
            log.write_text(run.stdout + run.stderr)
            passed = run.returncode == 0 and "penv: tohost exit code 0" in run.stderr
            results.append(dict(case=name, mode=mode, passed=passed, exit_code=run.returncode, log=log.name))
            print(f"{'PASS' if passed else 'FAIL'} {mode}: {name}")
    (args.output / "results.jsonl").write_text("".join(json.dumps(result) + "\n" for result in results))
    return 0 if all(result["passed"] for result in results if result["mode"] == "diagnostic") else 1


if __name__ == "__main__":
    raise SystemExit(main())
