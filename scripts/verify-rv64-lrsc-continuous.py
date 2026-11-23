#!/usr/bin/env python3
"""Verify the two fixed continuous PolarFire LR/SC workloads."""

import argparse
from pathlib import Path
import re
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--renode", type=Path, default=Path("../renode"))
    parser.add_argument("--penv", type=Path, default=Path("target/release/penv"))
    parser.add_argument("--cc", required=True)
    parser.add_argument("--logs", type=Path, required=True)
    parser.add_argument("--representative", action="store_true")
    args = parser.parse_args()
    args.logs.mkdir(parents=True, exist_ok=True)
    robot = (args.renode / "tests/unit-tests/riscv-lr-sc-instructions.robot").read_text()
    counter, iterations = (int(re.search(r"\$\{" + name + r"\}=\s+Set Variable\s+(\S+)", robot)[1], 0)
                           for name in ("shared_memory_counter", "increments_per_core"))
    failures = 0
    cases = [
        ("Two LR/SC Loops Should Increment To Correct Sum", "LRSC"),
        ("LR/SC And Amoadd Loops Should Increment To Correct Sum", "AMOADD"),
    ]
    for case, first in cases[:1] if args.representative else cases:
        commands = ["--stop-when-halted", "--halt-cpu", "e51", "--halt-cpu", "u54_3", "--halt-cpu", "u54_4",
                    "--write-memory", f"{counter:#x}:8:0", "--read-memory", f"{counter:#x}:8"]
        expected = [f"memory sysbus@{counter:#018x}:8 = {2 * iterations:#018x}"]
        for index, key in enumerate((first, "LRSC"), 1):
            cpu = f"u54_{index}"
            stem = args.logs / f"{first}-{cpu}"
            code = re.search(r"\$\{ASSEMBLY_" + key + r"_LOOP\}(?:=)?\s+[^\n]+\n"
                             r"((?:[ \t]*\.\.\.\s+[^\n]+\n)+)", robot)[1]
            assembly = [line.split("...", 1)[1].strip().rstrip(";") for line in code.splitlines()]
            if assembly.pop() != "j 0":
                raise RuntimeError("expected the fixed loop's terminal jump")
            # Renode halts at the jump and never executes it.
            stem.with_suffix(".S").write_text(
                ".text\n.globl _start, finished\n_start:\n" + "\n".join(assembly) + "\nfinished: j .\n"
            )
            pc = 0x80000000 + 0x100 * (index - 1)
            elf, binary = stem.with_suffix(".elf"), stem.with_suffix(".bin")
            subprocess.run([args.cc, "-march=rv64gc", "-mabi=lp64d", "-nostdlib",
                            f"-Wl,-n,-Ttext={pc:#x},--no-relax,--build-id=none", "-o", elf, stem.with_suffix(".S")],
                           check=True)
            prefix = args.cc.removesuffix("gcc")
            subprocess.run([prefix + "objcopy", "-j", ".text", "-O", "binary", elf, binary], check=True)
            symbols = subprocess.check_output([prefix + "nm", elf], text=True)
            end = int(re.search(r"^([a-f0-9]+) T finished$", symbols, re.M)[1], 16)
            commands.extend(("--load-binary", f"{pc:#x}:{binary}", "--set-pc", f"{cpu}:{pc:#x}",
                             "--halt-cpu-at", f"{cpu}:{end:#x}",
                             "--read-register", f"{cpu}@pc", "--read-register", f"{cpu}@x11"))
            for register, value in ((10, counter), (11, iterations), (12, 1), (13, 0x1234)):
                commands.extend(("--set-gpr", f"{cpu}@x{register}:{value:#x}"))
            expected.extend((f"register {cpu}@pc = {end:#018x}", f"register {cpu}@x11 = {0:#018x}"))

        initial_elf = args.logs / f"{first}-u54_1.elf"
        run = subprocess.run(
            [args.penv, "run", "--platform-root", args.renode,
             "--platform", args.renode / "platforms/cpus/polarfire-soc.repl",
             "--max-steps", "1000000000", *commands, initial_elf], capture_output=True, text=True, timeout=240,
        )
        (args.logs / f"{first}.stderr").write_text(run.stderr)
        (args.logs / f"{first}.status").write_text(f"{run.returncode}\n")
        actual = re.findall(r"^penv: ((?:register|memory) .*)$", run.stderr, re.M)
        passed = run.returncode == 0 and not run.stdout and actual == expected
        print(f"{'PASS' if passed else 'FAIL'} {case}", flush=True)
        if not passed:
            print(run.stderr, flush=True)
        failures += not passed
    raise SystemExit(int(failures != 0))


if __name__ == "__main__":
    main()
