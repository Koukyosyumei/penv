#!/usr/bin/env python3
"""Check the five fixed upstream IRQ merge sources through the platform CLI."""
import argparse
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parents[1]
SOURCE = "tests/unit-tests/RenodeTests/PlatformDescription/MergeAndCreationTests.cs"
REVISION = "c38ac3babcd38a7144b9498e3ca4c2226b76aa37"
CASES = (
    "ShouldCancelIrqConnection",
    "ShouldCancelAndReplaceIrqConnection",
    "ShouldCancelAndReplaceIrqConnectionsInRange",
    "ShouldCancelIrqConnectionsMultipleTimes",
    "ShouldMergeWithDefaultIrq",
)


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
    source = (args.renode / SOURCE).read_text()
    args.work.mkdir(parents=True, exist_ok=True)

    failed = False
    print("case\tpreflight_status")
    for name in CASES:
        match = re.search(
            rf'public void {name}\(\)\s+\{{\s+var source = @"((?:[^"]|"")*)";', source
        )
        if match is None:
            parser.error(f"could not extract the fixed fixture for {name}")
        fixture = args.work / f"{name}.repl"
        fixture.write_text(match[1].replace('""', '"').lstrip("\n") + "\n")
        result = subprocess.run(
            [args.penv, "platform", "check", "--file", fixture], capture_output=True
        )
        (args.work / f"{name}.log").write_bytes(result.stdout + result.stderr)
        failed |= result.returncode != 0
        print(f"{name}\t{result.returncode}")
    return int(failed)


if __name__ == "__main__":
    raise SystemExit(main())
