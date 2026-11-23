#!/usr/bin/env python3
"""Compare the fixed RV64 PC execution traces byte for byte."""
import argparse
from pathlib import Path
import struct
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--penv", type=Path, default=Path("target/release/penv"))
    parser.add_argument("--logs", type=Path, required=True)
    parser.add_argument("--representative", action="store_true")
    args = parser.parse_args()
    args.logs.mkdir(parents=True, exist_ok=True)
    failures = 0
    base = 0x2000000000
    pcs = [base, base + 4, base + 6]
    for mode in ("text", "binary"):
        for scoped in (False, True):
            name = "Should Dump 64-bit PCs" + (" As Binary" if mode == "binary" else "")
            if scoped:
                name += " For Isolated Memory"
            registration = (
                f"new Bus.BusPointRegistration {{ address: {base:#x}; cpu: cpu }}"
                if scoped else f"{base:#x}"
            )
            platform = args.logs / ("isolated.repl" if scoped else "shared.repl")
            platform.write_text(f"""cpu: CPU.RiscV64 @ sysbus
    cpuType: "rv64imacv"
    timeProvider: empty
mem: Memory.MappedMemory @ sysbus {registration} {{ size: 0x40000 }}
""")
            trace = args.logs / f"{name}.trace"
            trace.unlink(missing_ok=True)
            run = subprocess.run([
                args.penv, "run", "--platform", platform, "--max-steps", "0",
                "--trace-pc", f"cpu:{mode}:{trace}",
                "--set-pc", f"cpu:{base:#x}",
                "--write-memory", f"cpu@{base:#x}:4:0x00000013",
                "--write-memory", f"cpu@{base + 4:#x}:2:0x0001",
                "--write-memory", f"cpu@{base + 6:#x}:4:0x00310093",
                *(["--step-cpu", "cpu"] * 3),
            ], capture_output=True, timeout=30)
            (args.logs / f"{name}.stderr").write_bytes(run.stderr)
            (args.logs / f"{name}.status").write_text(f"{run.returncode}\n")
            expected = (b"ReTrace\x05\x08\x00" + b"".join(struct.pack("<QB", pc, 0) for pc in pcs)
                        if mode == "binary" else "".join(f"0x{pc:X}\n" for pc in pcs).encode())
            actual = trace.read_bytes() if trace.exists() else None
            passed = run.returncode == 0 and not run.stdout and actual == expected
            print(f"{'PASS' if passed else 'FAIL'} {name}")
            if not passed:
                print(f"  expected {expected!r}, got {actual!r}: {run.stderr.decode(errors='replace')}")
            failures += not passed
            if args.representative:
                raise SystemExit(int(failures != 0))
    raise SystemExit(int(failures != 0))


if __name__ == "__main__":
    main()
