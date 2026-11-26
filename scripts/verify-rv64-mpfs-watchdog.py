#!/usr/bin/env python3
"""Replay fixed PolarFire watchdog UART and peripheral log assertions."""
import argparse
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import re
import subprocess
import time


def cases(robot):
    text = robot.read_text()
    bodies = dict(re.findall(r"^(Should [^\n]+)\n((?:[ \t]+[^\n]*\n|\n)+)", text, re.M))
    common = text.split("Run Should Ignore Writing Other Values Test\n", 1)[1].split("*** Test Cases ***")[0]
    for name, body in bodies.items():
        if "mpfs-watchdog-interrupt" not in body:
            continue
        name_on_disk = re.search(r"@(https://\S+)", body)[1].rsplit("/", 1)[1]
        target = common if "Run Should Ignore Writing Other Values Test" in body else body
        uart = re.findall(r"Wait For Line On Uart\s{2,}([^\n]+)", target)
        required = re.findall(r"Wait For Log Entry\s{2,}([^\n]+)", target)
        excluded = re.findall(r"Should Not Be In Log\s{2,}([^\n]+)", target)
        yield name, name_on_disk, uart, [line.split("   timeout=")[0].strip() for line in required], excluded


def verify(args, name, asset, expected, required, excluded):
    stem = asset.split("case-")[1].split(".elf")[0]
    script = args.logs / f"{stem}.resc"
    original = (args.renode / "scripts/single-node/polarfire-soc.resc").read_text()
    script.write_text(re.sub(r"(?m)^\$bin\?=.*$", f"$bin?=@{(args.assets / asset).resolve()}", original))
    uart, stderr = (args.logs / f"{stem}.{suffix}" for suffix in ("uart", "stderr"))
    with uart.open("wb") as tx, stderr.open("wb") as errors:
        proc = subprocess.Popen([args.penv, "run", "--script", script, "--platform-root", args.renode,
                                 "--uart-device", "mmuart0", "--max-steps", str(args.max_steps)],
                                stdout=tx, stderr=errors)
        deadline = time.monotonic() + args.timeout
        passed = False
        found = 0
        try:
            while True:
                messages = stderr.read_text(errors="replace")
                lines = iter(uart.read_text(errors="replace").splitlines())
                found = 0
                for marker in expected:
                    if not any(marker in line for line in lines):
                        break
                    found += 1
                if any(marker in messages for marker in excluded):
                    break
                if found == len(expected) and all(marker in messages for marker in required):
                    passed = True
                    break
                if proc.poll() is not None or time.monotonic() >= deadline:
                    break
                time.sleep(0.05)
        finally:
            if proc.poll() is None:
                proc.terminate()
            (args.logs / f"{stem}.process-status").write_text(f"{proc.wait()}\n")
    (args.logs / f"{stem}.status").write_text(f"{int(not passed)}\n")
    print(f"{'PASS' if passed else 'FAIL'} {name} ({found}/{len(expected)} UART assertions)", flush=True)
    return passed


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--renode", type=Path, default=Path("../renode"))
    parser.add_argument("--penv", type=Path, default=Path("target/release/penv"))
    parser.add_argument("--assets", type=Path, required=True)
    parser.add_argument("--logs", type=Path, required=True)
    parser.add_argument("--case", help="Only run the indicated asset variant, e.g. a")
    parser.add_argument("--timeout", type=int, default=90)
    parser.add_argument("--max-steps", type=int, default=1_200_000_000)
    parser.add_argument("--jobs", type=int, default=1)
    args = parser.parse_args()
    args.logs.mkdir(parents=True, exist_ok=True)
    selected = list(cases(args.renode / "tests/platforms/PolarFireSoC.robot"))
    if len(selected) != 7:
        parser.error("expected seven fixed watchdog cases")
    if args.case:
        selected = [case for case in selected if f"case-{args.case}.elf" in case[1]]
    if not selected:
        parser.error("unknown watchdog case")
    with ThreadPoolExecutor(max_workers=args.jobs) as pool:
        outcomes = list(pool.map(lambda case: verify(args, *case), selected))
    raise SystemExit(int(not all(outcomes)))


if __name__ == "__main__":
    main()
