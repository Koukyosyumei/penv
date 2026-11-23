#!/usr/bin/env python3
"""Report upstream RV64 ELF outcomes under the available Penv CPU configuration."""
import argparse
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import re
import runpy
import subprocess


SUPPORTED_ISA = "rv64gc_zba_zbb_zbc_zbs_zacas_zicond_zbkb_zbkx_zfh_zicboz"
ROBOT_ISA = "rv64gcv_zba_zbb_zbc_zbs_zfh_zacas_smepmp"


def result_status(proc, name, act):
    lines = proc.stderr.splitlines()
    if any("unsupported RV64 ISA" in line for line in lines):
        return "configuration-rejected"
    if proc.returncode or "penv: TEST FINISHED" not in lines or "penv: tohost exit code 0" not in lines:
        return "diagnostic-fail"
    if act:
        # The self-checking ACT ELF compares its signature with values embedded
        # during generation. Accept precisely its own final report.
        reports = re.findall(r'^RVCP-SUMMARY: Test File "([^"]+)": (\w+)$', proc.stdout, re.M)
        expected = Path(name).with_suffix(".S").name
        if reports != [(expected, "PASSED")]:
            return "signature-unverified"
    return "diagnostic-pass"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("isa", type=Path, help="directory of upstream riscv-tests ELF files")
    parser.add_argument("--act", action="store_true", help="use the fixed Renode ACT ELF selector")
    parser.add_argument("--robot-isa", action="store_true", help="request the exact upstream ACT ISA without fallback")
    parser.add_argument("--renode", type=Path, default=Path("../renode"))
    parser.add_argument("--penv", type=Path, default=Path("target/release/penv"))
    parser.add_argument("--logs", type=Path, required=True)
    parser.add_argument("--jobs", type=int, default=4)
    parser.add_argument("--smepmp", action="store_true", help="enable Smepmp in the diagnostic profile")
    parser.add_argument("--allow-unaligned", action="store_true")
    parser.add_argument("--ram-size", type=lambda value: int(value, 0), default=0x100000)
    parser.add_argument("--elf", action="append", help="select named ELF parameters for a diagnostic variant")
    args = parser.parse_args()
    if args.robot_isa and (not args.act or args.allow_unaligned or args.ram_size != 0x100000):
        parser.error("--robot-isa requires --act, 1 MiB RAM, and the default alignment policy")
    args.logs.mkdir(parents=True, exist_ok=True)
    suite, key = ("arch", "TESTS_ARCH_64") if args.act else ("isa", "TESTS_64")
    selector = args.renode / f"tests/unit-tests/riscv-{suite}-tests.py"
    names = runpy.run_path(str(selector))["get_variables"](str(args.isa))[key]
    if args.act:
        names = [str(Path(name).relative_to(args.isa)) for name in names]
    if not names:
        parser.error("the upstream selector found zero RV64 executable parameters")
    if args.elf:
        if missing := sorted(set(args.elf) - set(names)):
            parser.error("not selected by the upstream harness: " + ", ".join(missing))
        names = sorted(set(args.elf))
    platform = args.logs / "isa.repl"
    unaligned = "    allowUnalignedAccesses: true\n" if args.allow_unaligned else ""
    isa = ROBOT_ISA if args.robot_isa else SUPPORTED_ISA + ("_smepmp" if args.smepmp else "")
    platform.write_text(
        'cpu: CPU.RiscV64 @ sysbus\n'
        f'    cpuType: "{isa}"\n'
        f'{unaligned}'
        + ('uart: UART.NS16550 @ sysbus 0x10000000\n' if args.act else "")
        + 'mem: Memory.MappedMemory @ sysbus 0x80000000\n'
        f'    size: {args.ram_size:#x}\n'
    )

    def run(name):
        elf = args.isa / name
        symbol = subprocess.run(["llvm-nm", "--defined-only", elf], capture_output=True, text=True)
        match = re.search(r"^([a-fA-F0-9]+)\s+\S\s+tohost$", symbol.stdout, re.M)
        if symbol.returncode or match is None:
            status, log = "missing-symbol", symbol.stderr
        else:
            watch = "--robot-tohost" if args.act else "--tohost"
            proc = subprocess.run(
                [args.penv, "run", "--platform", platform, watch, f"0x{match[1]}",
                 "--max-steps", "2000000", elf],
                capture_output=True, text=True, errors="replace",
            )
            status = result_status(proc, name, args.act)
            log = f"process status {proc.returncode}\n{proc.stdout}{proc.stderr}"
        log_file = args.logs / f"{name}.log"
        log_file.parent.mkdir(parents=True, exist_ok=True)
        log_file.write_text(log)
        return name, status

    with ThreadPoolExecutor(max_workers=args.jobs) as pool:
        for name, status in pool.map(run, sorted(names)):
            print(name, status, sep="\t")


if __name__ == "__main__":
    main()
