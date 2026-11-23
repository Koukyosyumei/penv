"""Check fixed static memory-assertions Robot inputs with RV32 guest loads and stores."""

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
    definitions = []
    symbols = {}
    widths = {"Byte": 1, "Word": 2, "DoubleWord": 4}

    def address(operand, width, qualifiers):
        index = None
        offset = 0
        for qualifier in qualifiers:
            key, value = qualifier.split("=", 1)
            if key == "index":
                index = int(value)
            elif key == "offset":
                offset = int(value)
            else:
                raise ValueError(f"unknown memory qualifier {qualifier!r}")
        if operand in symbols:
            names = symbols[operand]
            if index is None:
                name, = names
            else:
                name = names[index]
        else:
            name = operand
        return f"{name} + ({offset * widths[width]})" if offset else name

    for line in body.splitlines():
        fields = re.split(r"\s{2,}", line.strip())
        keyword, *args = fields
        if keyword == "Add Symbol":
            symbol, base, _size = args
            entries = symbols.setdefault(symbol, [])
            label = f"{symbol}_{len(entries)}"
            entries.append(label)
            definitions.append(f".equ {label}, {base}")
        elif keyword.startswith("Store "):
            width = keyword[6:].replace(" ", "")
            store = WIDTH[width][0]
            operand, value, *qualifiers = args
            checks += [f"li t0, {address(operand, width, qualifiers)}",
                       f"li t1, {value}", f"{store} t1, 0(t0)"]
        elif " Should " in keyword:
            selector, relation = keyword.split(" Should ", 1)
            relation = relation.removeprefix("Be ")
            qualifiers = [value for value in args if value.startswith("offset=")]
            signed = "signed=true" in args
            args = [value for value in args if value not in qualifiers and value != "signed=true"]
            width = args.pop()
            operand, *expected = args
            branch, operands = BRANCH[relation]
            addresses = (
                symbols[operand] if selector in {"All Symbols", "Any Symbol"}
                else [address(operand, width, qualifiers)]
            )
            any_symbol = selector == "Any Symbol"
            if any_symbol:
                branch = {"bne": "beq", "beq": "bne", "blt": "bge", "bge": "blt"}[branch]
            if branch in {"bge", "blt"} and not signed:
                branch += "u"
            for target in addresses:
                checks += [
                    f"li t0, {target}", f"{WIDTH[width][1 if signed else 2]} t1, 0(t0)",
                    f"li t2, {expected[0] if expected else 0}",
                    f"{branch} {operands}, {'1f' if any_symbol else 'fail'}",
                ]
            if any_symbol:
                checks += ["j fail", "1:"]
    if not checks:
        raise ValueError("missing fixed raw memory inputs")
    return "\n".join([
        ".option norvc", ".text", ".globl _start", *definitions, "_start:", *checks,
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
        r"(?ms)^(Should Read (?:Signed )?(?:Byte|Word|Double Word)(?: Zero)? "
        r"(?:At Raw Address|Of Symbol(?: With (?:Negative )?Offset)?|Of (?:All Symbols|Any Symbol)))\n(.*?)(?=^\S|\Z)", source,
    ) if "Create Machine 32" in body]
    if len(cases) != 27:
        parser.error(f"expected 27 static RV32 memory cases, found {len(cases)}")
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
                      "SECTIONS { .text 0x1000 : { *(.text) } :code }\n")
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
                 "--tohost", "0x2000", "--max-steps", "256", elf],
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
