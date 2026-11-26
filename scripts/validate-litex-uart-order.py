#!/usr/bin/env python3
"""Check the ordered UART waits from the fixed LiteX Tock and SMP OpenSBI Robot cases."""

import argparse
from pathlib import Path
import re

CASES = {
    "tock": (
        re.escape("LiteX+VexRiscv on ArtyA7: initialization complete, entering main loop."),
        *(re.escape(f"D{device} says hello") for device in (1, 2) * 3),
    ),
    "opensbi": (
        re.escape("OpenSBI v0.6"),
        re.escape("Litex/VexRiscv SMP"),
        r"Platform Max HARTs\s+: 4",
    ),
}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("case", choices=CASES)
    parser.add_argument("uart", type=Path)
    args = parser.parse_args()

    lines = enumerate(args.uart.read_text(errors="replace").splitlines(), 1)
    for expected in CASES[args.case]:
        for lineno, line in lines:
            if re.search(expected, line):
                print(f"{lineno}: {line}")
                break
        else:
            parser.exit(1, f"{args.case}: did not observe {expected!r} after the preceding UART line\n")


if __name__ == "__main__":
    main()
