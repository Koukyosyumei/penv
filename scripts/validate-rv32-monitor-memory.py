#!/usr/bin/env python3
"""Replay static RV32 Robot memory checks through scalar sysbus commands."""

import argparse
import json
import operator
from pathlib import Path
import re
import subprocess

WIDTHS = {"Byte": 1, "Word": 2, "DoubleWord": 4, "QuadWord": 8}
RELATIONS = {
    "Be Equal": operator.eq, "Not Be Equal": operator.ne,
    "Be Less Than": operator.lt, "Be Less Or Equal": operator.le,
    "Be Greater Than": operator.gt, "Be Greater Or Equal": operator.ge,
    "Be Zero": operator.eq, "Be Nonzero": operator.ne,
}


def script(body):
    commands = ["mach create", "machine LoadPlatformDescription @memory.repl"]
    symbols = {}
    checks = []

    def addresses(operand, width, options, selector):
        if selector == "Memory" or operand.startswith("address="):
            bases = [int(operand.removeprefix("address="), 0)]
        else:
            bases = symbols[operand.removeprefix("symbol=")]
            if selector not in ("All Symbols", "Any Symbol"):
                if "index" in options:
                    bases = [bases[int(options["index"])]]
                else:
                    base, = bases
                    bases = [base]
        offset = int(options.get("offset", 0)) * WIDTHS[width]
        return [base + offset for base in bases]

    for line in body.splitlines():
        if not line.strip():
            continue
        keyword, *args = re.split(r"\s{2,}", line.strip())
        if keyword == "Create Machine 32" or (keyword == "Create Machine" and args == ["32"]):
            continue
        if keyword == "Add Symbol":
            name, address, _ = args
            symbols.setdefault(name, []).append(int(address, 0))
            continue
        options = dict(arg.split("=", 1) for arg in args if "=" in arg)
        args = [arg for arg in args if "=" not in arg]
        if keyword.startswith("Store "):
            width = keyword.removeprefix("Store ").replace(" ", "")
            if "value" in options:
                operand = next(f"{key}={options[key]}" for key in ("address", "symbol") if key in options)
                value = options["value"]
            else:
                operand, value = args
            selector = "Symbol" if operand in symbols or operand.startswith("symbol=") else "Memory"
            address, = addresses(operand, width, options, selector)
            commands.append(f"sysbus Write{width} {address:#x} {value}")
        else:
            selector, relation = keyword.split(" Should ", 1)
            operand, *expected, width = args
            targets = addresses(operand, width, options, selector)
            commands.extend(f"sysbus Read{width} {target:#x}" for target in targets)
            checks.append((selector, relation, width, options.get("signed") == "true",
                           int(expected[0], 0) if expected else 0, len(targets)))
    return commands, checks


def verify(stdout, checks):
    values = [int(line, 16) for line in stdout.splitlines()]
    offset = 0
    for selector, relation, width, signed, expected, count in checks:
        group = values[offset:offset + count]
        if len(group) != count:
            return False
        offset += count
        modulus = 1 << (8 * WIDTHS[width])
        observed = [value - modulus if signed and value >= modulus // 2 else value for value in group]
        predicate = RELATIONS[relation]
        combine = any if selector == "Any Symbol" else all
        if not combine(predicate(value, expected) for value in observed):
            return False
    return offset == len(values)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("penv", "renode", "output"):
        parser.add_argument(name, type=Path)
    args = parser.parse_args()
    robot = (args.renode / "tests/unit-tests/memory-assertions.robot").read_text()
    source = robot.split("*** Test Cases ***", 1)[1]
    cases = [(name, body) for name, body in re.findall(r"(?ms)^(\S[^\n]+)\n(.*?)(?=^\S|\Z)", source)
             if re.search(r"(?m)^ +Create Machine +32[ \t]*$", body) and
             ((name.startswith("Should Read ") and not name.endswith(" After"))
              or name == "Should Not Confuse Symbol Name With Address On Store")]
    if len(cases) != 29:
        parser.error(f"expected 29 static RV32 cases, found {len(cases)}")
    isa = "rv32" + re.search(r"rv\$\{bitness\}([a-z0-9_]+)", robot)[1]
    args.output.mkdir(parents=True)
    successful = True
    with (args.output / "results.jsonl").open("w") as results:
        for mode, cpu in (("diagnostic", "rv32imac"), ("fixed", isa)):
            directory = args.output / mode
            directory.mkdir()
            (directory / "memory.repl").write_text(
                f'cpu: CPU.RiscV32 @ sysbus {{ cpuType: "{cpu}" }}\n'
                'mem: Memory.MappedMemory @ sysbus 0x0 { size: 0x40000 }\n'
            )
            for index, (name, body) in enumerate(cases):
                commands, checks = script(body)
                resc = directory / f"{index}.resc"
                resc.write_text("\n".join(commands) + "\n")
                run = subprocess.run(
                    [args.penv.resolve(), "run", "--script", resc.resolve(), "--max-steps", "0"],
                    capture_output=True, text=True, timeout=15,
                )
                resc.with_suffix(".stdout").write_text(run.stdout)
                resc.with_suffix(".stderr").write_text(run.stderr)
                passed = run.returncode == 0 and verify(run.stdout, checks)
                results.write(json.dumps(dict(case=name, mode=mode, passed=passed,
                                              exit_code=run.returncode, script=str(resc))) + "\n")
                print(f"{'PASS' if passed else 'FAIL'} {mode}: {name}")
                if mode == "diagnostic":
                    successful &= passed
    return 0 if successful else 1


if __name__ == "__main__":
    raise SystemExit(main())
