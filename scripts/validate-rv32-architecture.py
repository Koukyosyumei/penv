"""Run prebuilt RV32 architecture ELFs with tohost completion reporting."""

import argparse
import json
from pathlib import Path
import re
import subprocess
import sys


def run_case(args, elf, logfile):
    try:
        symbols = subprocess.run(
            [args.nm, "--defined-only", "--format=posix", str(elf)],
            capture_output=True, text=True, check=True,
        ).stdout
        addresses = re.findall(r"^tohost\s+\S\s+([0-9a-fA-F]+)(?:\s|$)", symbols, re.M)
        if len(addresses) != 1:
            raise ValueError("expected one defined tohost symbol")
        option = "--tohost-act" if args.act else "--tohost"
        with logfile.open("wb") as log:
            result = subprocess.run(
                [str(args.penv), "run", "--platform", str(args.platform),
                 option, "0x" + addresses[0], "--max-steps", str(args.max_steps), str(elf)],
                stdout=log, stderr=subprocess.PIPE, timeout=30,
            )
            if result.stderr:
                log.write(b"\n" + result.stderr)
        reports = re.findall(rb"^penv: tohost exit code ([0-9]+)$", result.stderr, re.M)
        code = int(reports[-1]) if reports else None
        if result.returncode == 0 and code == 0:
            return {"status": "pass", "tohost": 0}
        return {"status": "fail", "exit_code": result.returncode, "tohost": code}
    except (OSError, ValueError, subprocess.CalledProcessError, subprocess.TimeoutExpired) as error:
        return {"status": "error", "error": str(error)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--penv", type=lambda path: Path(path).resolve(), required=True)
    parser.add_argument("--platform", type=Path, required=True)
    parser.add_argument("--artifacts", type=Path, required=True)
    parser.add_argument("--nm", default="llvm-nm")
    parser.add_argument("--act", action="store_true", help="use the RV32 Sail ACT packet protocol")
    parser.add_argument("--max-steps", type=int, default=1_000_000)
    parser.add_argument("elf", type=Path, nargs="+")
    args = parser.parse_args()
    args.artifacts.mkdir(parents=True, exist_ok=False)
    passed = 0
    with (args.artifacts / "results.jsonl").open("w") as raw:
        for index, elf in enumerate(args.elf, 1):
            logfile = args.artifacts / f"{index:04d}-{elf.name}.log"
            result = run_case(args, elf, logfile)
            result.update(elf=str(elf), log=logfile.name)
            raw.write(json.dumps(result) + "\n")
            passed += result["status"] == "pass"
    print(f"{passed}/{len(args.elf)} passed")
    return 0 if passed == len(args.elf) else 1


if __name__ == "__main__":
    sys.exit(main())
