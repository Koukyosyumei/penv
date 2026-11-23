#!/usr/bin/env python3
"""Report upstream RV64 ELF outcomes under the available Penv CPU configuration."""
import argparse
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import re
import runpy
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("isa", type=Path, help="directory of upstream riscv-tests ELF files")
    parser.add_argument("--renode", type=Path, default=Path("../renode"))
    parser.add_argument("--penv", type=Path, default=Path("target/release/penv"))
    parser.add_argument("--logs", type=Path, required=True)
    parser.add_argument("--jobs", type=int, default=4)
    parser.add_argument("--allow-unaligned", action="store_true")
    parser.add_argument("--ram-size", type=lambda value: int(value, 0), default=0x100000)
    parser.add_argument("--elf", action="append", help="select named ELF parameters for a diagnostic variant")
    args = parser.parse_args()
    args.logs.mkdir(parents=True, exist_ok=True)
    selector = args.renode / "tests/unit-tests/riscv-isa-tests.py"
    names = runpy.run_path(str(selector))["get_variables"](str(args.isa))["TESTS_64"]
    if not names:
        parser.error("the upstream selector found zero RV64 executable parameters")
    if args.elf:
        if missing := sorted(set(args.elf) - set(names)):
            parser.error("not selected by the upstream harness: " + ", ".join(missing))
        names = sorted(set(args.elf))
    platform = args.logs / "isa.repl"
    unaligned = "    allowUnalignedAccesses: true\n" if args.allow_unaligned else ""
    platform.write_text(
        'cpu: CPU.RiscV64 @ sysbus\n'
        '    cpuType: "rv64gc_zba_zbb_zbc_zbs_zacas_zicond_zbkb_zbkx_zfh_zicboz"\n'
        f'{unaligned}'
        'mem: Memory.MappedMemory @ sysbus 0x80000000\n'
        f'    size: {args.ram_size:#x}\n'
    )

    def run(name):
        elf = args.isa / name
        symbol = subprocess.run(["llvm-nm", "--defined-only", elf], capture_output=True, text=True)
        match = re.search(r"^([a-fA-F0-9]+)\s+\S\s+tohost$", symbol.stdout, re.M)
        if symbol.returncode or match is None:
            status, log = "missing-symbol", symbol.stderr
        else:
            proc = subprocess.run(
                [args.penv, "run", "--platform", platform, "--tohost", f"0x{match[1]}",
                 "--max-steps", "2000000", elf],
                capture_output=True, text=True, errors="replace",
            )
            status = ("diagnostic-pass" if proc.returncode == 0 and
                      "penv: tohost exit code 0\n" in proc.stderr else "diagnostic-fail")
            log = f"process status {proc.returncode}\n{proc.stdout}{proc.stderr}"
        (args.logs / f"{name}.log").write_text(log)
        return name, status

    with ThreadPoolExecutor(max_workers=args.jobs) as pool:
        for name, status in pool.map(run, sorted(names)):
            print(name, status, sep="\t")


if __name__ == "__main__":
    main()
