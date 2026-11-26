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


def program(body, sweeps):
    checks = []
    definitions = []
    symbols = {}
    widths = {"Byte": 1, "Word": 2, "DoubleWord": 4, "QuadWord": 8}

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
        if operand.startswith("address="):
            name = operand.removeprefix("address=")
        elif operand.startswith("symbol=") or operand in symbols:
            operand = operand.removeprefix("symbol=")
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
            label = f"symbol_{len(definitions)}"
            entries.append(label)
            definitions.append(f".equ {label}, {base}")
        elif keyword.startswith("Load Program "):
            key = keyword.removeprefix("Load Program ").lower()
            if key == "doubleword ind":
                checks.append("li s0, 0")
            checks.extend(sweeps[key])
        elif keyword.startswith("Store "):
            width = keyword[6:].replace(" ", "")
            operand, value, *qualifiers = args
            value = value.removeprefix("value=")
            checks.append(f"li t0, {address(operand, width, qualifiers)}")
            if width == "QuadWord":
                low, high = int(value, 0) & 0xffffffff, int(value, 0) >> 32
                checks += [f"li t1, {low}", "sw t1, 0(t0)", f"li t1, {high}", "sw t1, 4(t0)"]
            else:
                checks += [f"li t1, {value}", f"{WIDTH[width][0]} t1, 0(t0)"]
        elif " Should " in keyword:
            selector, relation = keyword.split(" Should ", 1)
            relation = relation.removeprefix("Be ")
            qualifiers = [value for value in args if value.startswith("offset=")]
            signed = "signed=true" in args
            args = [value for value in args if value not in qualifiers and value != "signed=true"]
            if args[-1] == "timeout=3":
                args.pop()  # The guest check runs after one sweep.
            width = args.pop()
            operand, *expected = args
            if selector == "Memory":
                operand = f"address={operand}"
            if relation == "Independently Be Equal" and selector == "All Symbols":
                value, = expected
                # Each watched word is written once per sweep. Latch matches
                # across sweeps, even after later writes replace the value.
                for index, target in enumerate(symbols[operand]):
                    checks += [
                        f"li t0, {target}", "lw t1, 0(t0)", f"li t2, {value}",
                        "bne t1, t2, 1f", f"ori s0, s0, {1 << index}", "1:",
                    ]
                checks += [f"li t0, {(1 << len(symbols[operand])) - 1}", "bne s0, t0, .L3"]
                continue
            branch, operands = BRANCH[relation]
            addresses = (
                symbols[operand] if selector in {"All Symbols", "Any Symbol"}
                else [address(operand, width, qualifiers)]
            )
            if width == "QuadWord":
                target, = addresses
                value, = expected
                if relation not in {"Equal", "Not Be Equal"}:
                    raise ValueError(f"unsupported QuadWord comparison {relation}")
                low, high = int(value, 0) & 0xffffffff, int(value, 0) >> 32
                checks += [
                    f"li t0, {target}", "lw t1, 0(t0)", f"li t2, {low}", "xor t1, t1, t2",
                    "lw t3, 4(t0)", f"li t2, {high}", "xor t3, t3, t2", "or t1, t1, t3",
                    f"{branch} t1, zero, fail",
                ]
                continue
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
        r"(?ms)^(Should Read (?:Signed )?(?:Byte|Word|Double Word|Quad Word)(?: Zero)? "
        r"(?:At Raw Address|Of Symbol(?: With (?:Negative )?Offset)?|Of (?:All Symbols|Any Symbol|All Symbols Independently))(?: After)?|"
        r"Should Not Confuse Symbol Name With Address On Store)\n(.*?)(?=^\S|\Z)", source,
    ) if re.search(r"(?m)^ +Create Machine +32[ \t]*$", body)]
    if len(cases) != 42:
        parser.error(f"expected 42 RV32 memory cases, found {len(cases)}")
    sweeps = {}
    for key in (*(width.lower() for width in WIDTH), "doubleword_ind"):
        match = re.search(
            rf"(?m)^\$\{{asm_{key}\}}[^\n]*\n((?:\.\.\.[ \t]+[^\n]+\n)+)", source,
        )
        instructions = [line[3:].strip() for line in match[1].splitlines()]
        if instructions.pop() != "j .L3":
            parser.error(f"unexpected final branch in upstream {key} sweep")
        sweeps[key.replace("_", " ")] = instructions
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
        asm.write_text(program(body, sweeps))
        max_steps = "256"
        if name.endswith("Independently After"):
            max_steps = "1400000"
        elif name.endswith(" After"):
            max_steps = "210000"
        subprocess.run(
            ["clang", "--target=riscv32-unknown-elf", "-march=rv32imac", "-mabi=ilp32",
             "-nostdlib", "-fuse-ld=lld", f"-Wl,-T,{linker}", asm, "-o", elf],
            check=True, timeout=15,
        )
        for mode in ("diagnostic", "fixed"):
            run = subprocess.run(
                [args.penv.resolve(), "run", "--platform", args.output / f"{mode}.repl",
                 "--tohost", "0x2000", "--max-steps", max_steps, elf],
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
