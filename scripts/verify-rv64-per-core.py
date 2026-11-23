#!/usr/bin/env python3
"""Check fixed per-core-registration UART and memory assertions against Penv."""

import argparse
from pathlib import Path
import subprocess


ISOLATION = (
    "Core 0 read from 0x3000000 returned: 0x0",
    "Core 0 writing 0xB0B0B0B0 to per-core memory",
    "Core 0 read from 0x3000000 returned: 0xB0B0B0B0",
    "Core 1 read from 0x3000000 returned: 0x0",
    "Core 1 writing 0xBABABABA to per-core memory",
    "Core 1 read from 0x3000000 returned: 0xBABABABA",
)


def verify(args, name, expected, options=(), reads=()):
    fixture = args.renode / "tests/unit-tests/per-core-registration.repl"
    run = subprocess.run(
        [args.penv, "run", "--renode-unmapped-data", "--platform", fixture, "--max-steps",
         "2000000", *options, args.elf],
        capture_output=True, text=True, timeout=30,
    )
    (args.logs / f"{name}.uart").write_text(run.stdout)
    (args.logs / f"{name}.stderr").write_text(run.stderr)
    (args.logs / f"{name}.status").write_text(f"{run.returncode}\n")
    if run.returncode != 2 or "instruction limit reached at pc=" not in run.stderr:
        raise RuntimeError(f"{name}: unexpected process termination: {run.returncode}")
    output = iter(run.stdout.splitlines())
    for line in expected:
        if line not in output:
            raise RuntimeError(f"{name}: missing ordered UART line {line!r}")
    for context, address, value in reads:
        expected_read = f"penv: memory {context}@{address:#018x}:4 = {value:#018x}"
        if expected_read not in run.stderr.splitlines():
            raise RuntimeError(f"{name}: missing memory assertion {expected_read!r}")
    print(f"{name}: verified {len(expected)} ordered UART assertions and {len(reads)} memory values")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("elf", type=Path)
    parser.add_argument("--renode", type=Path, default=Path("../renode"))
    parser.add_argument("--penv", type=Path, default=Path("target/release/penv"))
    parser.add_argument("--logs", type=Path, required=True)
    args = parser.parse_args()
    args.logs.mkdir(parents=True, exist_ok=True)
    common = args.logs / "common.bin"
    common.write_bytes((0xDEADF00D).to_bytes(4, "little"))
    reads = (
        ("sysbus", 0x2010000, 0xB0B0B0B0),
        ("sysbus", 0x2020000, 0xBABABABA),
        ("cpu1", 0x3000000, 0xB0B0B0B0),
        ("cpu2", 0x3000000, 0xBABABABA),
    )
    read_options = [
        argument
        for cpu, address, _ in reads
        for argument in ("--read-memory", f"{'' if cpu == 'sysbus' else cpu + '@'}{address:#x}:4")
    ]
    verify(args, "isolation", ISOLATION, read_options, reads)
    verify(args, "common", (
        "Core 0 read from 0x1400000 returned: 0xDEADF00D",
        "Core 1 read from 0x1400000 returned: 0xDEADF00D",
    ), ("--load-binary", f"0x1400000:{common}"))

    values = (0xFEEDFACE, 0xFEE1DEAD)
    expected = tuple(f"Core {core} read from 0x3000000 returned: 0x{value:08X}"
                     for core, value in enumerate(values))
    files = [args.logs / f"core{core}.bin" for core in (1, 2)]
    for path, value in zip(files, values):
        path.write_bytes(value.to_bytes(4, "little"))
    for name, selectors in (
        ("write-global", ("0x2010000", "0x2020000")),
        ("write-context", ("cpu1@0x3000000", "cpu2@0x3000000")),
    ):
        options = [argument
                   for selector, path in zip(selectors, files)
                   for argument in ("--load-binary", f"{selector}:{path}")]
        verify(args, name, expected, options)


if __name__ == "__main__":
    main()
