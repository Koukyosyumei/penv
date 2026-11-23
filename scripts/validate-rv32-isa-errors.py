"""Run pinned Renode RV32 construction-error inputs against the penv CLI."""
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

    args.output.mkdir(parents=True, exist_ok=False)
    payload = args.output / "ebreak.bin"
    payload.write_bytes(bytes.fromhex("73001000"))
    results = []
    for source, name, cpu, expected in inputs:
        fixture = args.output / f"{cpu}.repl"
        fixture.write_text(f'cpu: CPU.RiscV32 @ sysbus {{ cpuType: "{cpu}" }}\n'
                           "mem: Memory.MappedMemory @ sysbus 0x80000000 { size: 0x1000 }\n")
        result = subprocess.run(
            [args.penv.resolve(), "run", "--raw", "--platform", fixture, "--max-steps", "1", payload],
            text=True, capture_output=True, timeout=10,
        )
        (args.output / f"{cpu}.log").write_text(result.stdout + result.stderr)
        passed = result.returncode == 1 and expected in result.stderr
        results.append(dict(source=source, case=name, parameter=cpu, expected=expected,
                            passed=passed, exit=result.returncode))
        print(f"{'PASS' if passed else 'FAIL'} {name} [{cpu}]")
    (args.output / "results.json").write_text(json.dumps(results, indent=2) + "\n")
    return 0 if all(row["passed"] for row in results) else 1


if __name__ == "__main__":
    raise SystemExit(main())
