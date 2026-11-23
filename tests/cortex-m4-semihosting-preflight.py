#!/usr/bin/env python3
"""Check semihosting fixture construction for the pinned Cortex-M cases."""
import argparse
import csv
from itertools import takewhile
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[1]
SOURCE = "tests/unit-tests/arm-semihosting.robot"
REVISION = "c38ac3babcd38a7144b9498e3ca4c2226b76aa37"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("work", type=Path)
    parser.add_argument("--renode", type=Path, default=ROOT.parent / "renode")
    parser.add_argument("--penv", type=Path, default=ROOT / "target/release/penv")
    args = parser.parse_args()
    revision = subprocess.check_output(
        ["git", "-C", args.renode, "rev-parse", "HEAD"], text=True
    ).strip()
    if revision != REVISION:
        parser.error(f"expected Renode {REVISION}, found {revision}")

    suite = (args.renode / SOURCE).read_text().splitlines()

    def section(name):
        start = next(i for i, line in enumerate(suite) if line.startswith(f"${{{name}}} "))
        continuation = takewhile(lambda line: line.startswith("..."), suite[start + 1:])
        return "\n".join(line[3:].strip() for line in continuation if '"""' not in line)

    args.work.mkdir(parents=True, exist_ok=True)
    cpu = section("CORTEX_M").replace('"cortex-m85"', '"cortex-m4"')
    results = {}
    for name in ("COMMON_OUTPUT", "SPLIT_OUTPUT"):
        platform = args.work / f"{name}.repl"
        platform.write_text("\n".join((cpu, section("REPL"), section(name))) + "\n")
        result = subprocess.run(
            [args.penv, "platform", "check", "--file", platform], capture_output=True
        )
        (args.work / f"{name}.log").write_bytes(result.stdout + result.stderr)
        results[name] = result.returncode

    with (ROOT / "docs/stm32f407-upstream-cases.tsv").open() as ledger:
        rows = [row for row in csv.DictReader(ledger, delimiter="\t")
                if row["file"] == SOURCE and row["status"] != "not-applicable"]
    if len(rows) != 25:
        parser.error(f"expected 25 applicable ledger rows, found {len(rows)}")
    print("source_line\tcase\tparameters\tfixture\tpreflight_status")
    for row in rows:
        fixture = "SPLIT_OUTPUT" if "separate_stderr=True" in row["parameters"] else "COMMON_OUTPUT"
        print(row["line"], row["case"], row["parameters"], fixture, results[fixture], sep="\t")
    return int(any(results.values()))


if __name__ == "__main__":
    raise SystemExit(main())
