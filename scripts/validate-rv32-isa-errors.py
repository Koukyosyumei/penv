"""Run pinned Renode RV32 CPU construction cases against the penv CLI."""
import argparse
import json
from pathlib import Path
import re
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("penv", type=Path)
    parser.add_argument("renode", type=Path)
    parser.add_argument("output", type=Path)
    group = parser.add_mutually_exclusive_group()
    group.add_argument("--vector", action="store_true", help="run fixed vector CPU construction inputs")
    group.add_argument("--valid", action="store_true", help="run supported valid-spelling inputs")
    args = parser.parse_args()
    unit_tests = args.renode / "tests/unit-tests"
    isa = (unit_tests / "riscv-isa-string.robot").read_text()
    extensions = (unit_tests / "riscv-extensions.robot").read_text()
    section = isa.split("\nShould Throw Construction Exception\n", 1)[1]
    inputs = [
        ("riscv-isa-string.robot", "Should Throw Construction Exception", cpu, error)
        for cpu, error in re.findall(r"(?m)^    (RV32\w+) {2,}(\S.*)$", section)
    ]
    invalid = re.search(
        r"(?m)^Should Fail On Unsupported ISA\n\s+Run Keyword And Expect Error\s+"
        r"\*([^*]+)\*\s+Create Machine\s+(RV32\w+)", extensions,
    )
    if len(inputs) != 4 or invalid is None:
        parser.error("the fixed construction-error fixtures changed")
    inputs.append(("riscv-extensions.robot", "Should Fail On Unsupported ISA", invalid[2], invalid[1]))

    if args.valid:
        valid = isa.split("\nShould Parse All Valid ISA Strings\n", 1)[1].split(
            "\nShould Handle Both 32 And 64 Bit Versions\n", 1
        )[0]
        # Leave inputs requiring V in their existing backlog.
        values = dict.fromkeys(re.findall(r"(?m)^    (\w+)\s*$", valid))
        inputs = [
            ("riscv-isa-string.robot", "Should Parse All Valid ISA Strings", value, None)
            for value in values
            if re.fullmatch(r"rv32(?:[imafdcgb_]+|_?e[mac]*)(?:_?z(?:icsr|ifencei|bs|icond))*(?:_xandes)?", value.lower())
        ]
        supported = re.findall(r"(?m)^    (RV32I_(?:Smepmp|Sscofpmf))\s*$", extensions)
        if not inputs or set(supported) != {"RV32I_Smepmp", "RV32I_Sscofpmf"}:
            parser.error("the fixed valid-construction fixtures changed")
        inputs.extend(
            ("riscv-extensions.robot", "Should Create CPUs With Supported ISAs", cpu, None)
            for cpu in supported
        )

    if args.vector:
        inputs = [
            ("riscv-isa-string.robot", "Should Parse All Valid ISA Strings", cpu, None)
            for cpu in re.findall(r"(?m)^    (RV32G_V)[ \t]*$", isa)
        ]
        inputs.extend(
            ("riscv-extensions.robot", "Should Create CPUs With Supported ISAs", cpu, None)
            for cpu in re.findall(r"(?m)^    (RV32I_(?:V|Zvfh|Zve32[xf]|Zve64[xfd]))[ \t]*$", extensions)
        )
        andes = (unit_tests / "riscv-andestar-v5.robot").read_text()
        for name, body in re.findall(
            r"(?ms)^(Should [^\n]+)\n(.*?)(?=^\S|\Z)", andes.split("*** Test Cases ***", 1)[1]
        ):
            fixture = re.search(r"(?m)^ +Create Vector Machine +(xandes\w+)[ \t]*$", body)
            if fixture:
                inputs.append(("riscv-andestar-v5.robot", name, "rv32gc_v_" + fixture[1], None))
        if len(inputs) != 40:
            parser.error(f"expected 40 RV32 vector construction cases, found {len(inputs)}")

    args.output.mkdir(parents=True, exist_ok=False)
    results = []
    for index, (source, name, cpu, expected) in enumerate(inputs):
        stem = f"{index}-{cpu}" if args.vector else cpu
        fixture = args.output / f"{stem}.repl"
        fixture.write_text(f'cpu: CPU.RiscV32 @ sysbus {{ cpuType: "{cpu}" }}\n')
        result = subprocess.run(
            [args.penv.resolve(), "check-platform", fixture],
            text=True, capture_output=True, timeout=10,
        )
        (args.output / f"{stem}.log").write_text(result.stdout + result.stderr)
        if expected is None:
            passed = result.returncode == 0
        else:
            passed = result.returncode == 1 and expected in result.stderr
        results.append(dict(source=source, case=name, parameter=cpu, expected=expected,
                            passed=passed, exit=result.returncode, log=f"{stem}.log"))
        print(f"{'PASS' if passed else 'FAIL'} {name} [{cpu}]")
    (args.output / "results.json").write_text(json.dumps(results, indent=2) + "\n")
    return 0 if all(row["passed"] for row in results) else 1


if __name__ == "__main__":
    raise SystemExit(main())
