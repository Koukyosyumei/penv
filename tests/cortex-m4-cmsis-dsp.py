#!/usr/bin/env python3
"""Run the pinned Renode CMSIS-DSP Cortex-M4F ELF cohort."""
import argparse
import hashlib
from pathlib import Path
import re
import subprocess
from urllib.request import urlopen

ROOT = Path(__file__).resolve().parents[1]
SOURCE = "tests/unit-tests/arm-dsp-examples-cortex-m4.robot"
REVISION = "c38ac3babcd38a7144b9498e3ca4c2226b76aa37"
PLATFORM = """cpu: CPU.CortexM @ sysbus
    cpuType: "cortex-m4f"
    nvic: nvic
nvic: IRQControllers.NVIC @ sysbus 0xE000E000
    -> cpu@0
rom: Memory.MappedMemory @ sysbus 0x0
    size: 0x40000
ram: Memory.MappedMemory @ sysbus 0x20000000
    size: 0x100000
serial: UART.TrivialUart @ sysbus 0x40000000
"""


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("work", type=Path)
    parser.add_argument("--renode", type=Path, default=ROOT.parent / "renode")
    parser.add_argument("--penv", type=Path, default=ROOT / "target/release/penv")
    parser.add_argument("--case", help="Filter by ELF example name, e.g. arm_bayes_example")
    args = parser.parse_args()
    revision = subprocess.check_output(
        ["git", "-C", args.renode, "rev-parse", "HEAD"], text=True
    ).strip()
    if revision != REVISION:
        parser.error(f"expected Renode {REVISION}, found {revision}")

    suite = (args.renode / SOURCE).read_text()
    base_url = re.search(r"^\$\{URI\}\s+@(https://\S+)$", suite, re.M)[1]
    cases = re.findall(r"^(Should Successfully Run .+)\n((?:[ \t]+[^\n]+\n|\n)+)",
                       suite, re.M)
    if len(cases) != 13:
        parser.error(f"expected 13 upstream cases, found {len(cases)}")

    args.work.mkdir(parents=True, exist_ok=True)
    platform = args.work / "upstream.repl"
    platform.write_text(PLATFORM)
    failed = False
    selected = 0
    for title, body in cases:
        filename = re.search(r"LoadELF \$\{URI\}/(\S+)", body)[1]
        name, size, digest = re.fullmatch(r"(.+)-s_(\d+)-([0-9a-f]{40})", filename).groups()
        if args.case and name != args.case:
            continue
        selected += 1
        firmware = args.work / filename
        if not firmware.exists():
            with urlopen(f"{base_url}/{filename}", timeout=30) as response:
                data = response.read()
            if len(data) != int(size) or hashlib.sha1(data).hexdigest() != digest:
                raise ValueError(f"invalid download {filename}")
            firmware.write_bytes(data)
        data = firmware.read_bytes()
        if len(data) != int(size) or hashlib.sha1(data).hexdigest() != digest:
            raise ValueError(f"invalid cached firmware {firmware}")
        expectations = re.findall(r"Wait For Line On Uart\s+([^\n]+)", body)
        checks = args.work / f"{name}.tsv"
        checks.write_text("".join(
            f"wait\t8\t{line.split('  ', 1)[0]}\n"
            for line in expectations if "treatAsRegex=true" not in line
        ))
        # Even the regex case must exercise the original platform loader.
        has_regex = any("treatAsRegex=true" in line for line in expectations)
        command = [args.penv, "run", "--platform", platform]
        if has_regex:
            command += ["--max-steps", "1"]
        else:
            command += ["--uart-check", checks]
        result = subprocess.run([*command, firmware], capture_output=True)
        (args.work / f"{name}.uart").write_bytes(result.stdout)
        (args.work / f"{name}.log").write_bytes(result.stderr)
        if has_regex:
            verdict = "REGEX_UNVERIFIED" if result.returncode == 2 else "BLOCKED"
        else:
            verdict = "OBSERVED" if result.returncode == 0 else "BLOCKED"
        failed |= verdict != "OBSERVED"
        print(f"{verdict}\t{title}\tpenv={result.returncode}", flush=True)
    if not selected:
        parser.error(f"unknown case {args.case}")
    return int(failed)


if __name__ == "__main__":
    raise SystemExit(main())
