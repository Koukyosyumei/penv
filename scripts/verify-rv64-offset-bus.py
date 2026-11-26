#!/usr/bin/env python3
"""Port the fixed offset-bus-registrations Robot Sysbus read/write assertions."""

import argparse
from pathlib import Path
import subprocess


WINDOWS = ((0x80004000, 0x81000000), (0x90004000, 0x91000000))
WORDS = {
    "Byte": (1, 0xBE),
    "Word": (2, 0xFACE),
    "DoubleWord": (4, 0xDEADBEEF),
    "QuadWord": (8, 0xFEEDFACECAFEBEEF),
}


def verify(args, operation, size):
    # The pinned "Should Read Byte" case actually invokes the Word keyword.
    width, value = WORDS["Word" if (operation, size) == ("Read", "Byte") else size]
    options = []
    expected = []
    for backing, window in WINDOWS:
        global_address = f"{backing:#018x}"
        cpu_address = f"cpu@{window:#018x}"
        target = global_address if operation == "Read" else cpu_address
        options.extend(("--write-memory", f"{target}:{width}:{value:#x}"))
        options.extend(("--read-memory-now", f"{global_address}:{width}"))
        expected.append(f"penv: memory sysbus@{global_address}:{width} = {value:#018x}")
        if operation == "Read":
            options.extend(("--read-memory-now", f"{cpu_address}:{width}"))
            expected.append(f"penv: memory {cpu_address}:{width} = {value:#018x}")
    fixture = args.renode / "tests/unit-tests/offset-bus-registrations.repl"
    run = subprocess.run(
        [args.penv, "run", "--platform", fixture, "--max-steps", "0", *options],
        capture_output=True, text=True, timeout=30,
    )
    name = f"{operation.lower()}-{size.lower()}"
    (args.logs / f"{name}.stderr").write_text(run.stderr)
    (args.logs / f"{name}.status").write_text(f"{run.returncode}\n")
    if run.returncode or run.stdout or run.stderr.splitlines() != expected:
        raise RuntimeError(f"Sysbus Should {operation} {size}: {run.stderr}")
    print(f"Sysbus Should {operation} {size}: passed")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--renode", type=Path, default=Path("../renode"))
    parser.add_argument("--penv", type=Path, default=Path("target/release/penv"))
    parser.add_argument("--logs", type=Path, required=True)
    args = parser.parse_args()
    args.logs.mkdir(parents=True, exist_ok=True)
    for size in WORDS:
        for operation in ("Read", "Write"):
            verify(args, operation, size)


if __name__ == "__main__":
    main()
