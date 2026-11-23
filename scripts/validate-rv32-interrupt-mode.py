"""Port the fixed Renode guest CSR write cases, including warning assertions."""
import argparse
import json
from pathlib import Path
import re
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("penv", type=Path)
    parser.add_argument("renode", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    robot = (args.renode / "tests/unit-tests/riscv-interrupt-mode.robot").read_text()
    cases = re.findall(r"(?m)^(Should Correct (MTVEC|STVEC) Written By Software in (\w+) Mode)\n"
                       r"(.*?)(?=^Should Correct |\Z)", robot, re.S | re.M)
    if len(cases) != 6:
        parser.error(f"expected six fixed software cases, found {len(cases)}")
    linker = args.output / "link.ld"
    linker.write_text("ENTRY(_start)\nPHDRS { text PT_LOAD FLAGS(5); }\n"
                      "SECTIONS { . = 0; .text : { *(.text*) } :text }\n")
    results = []
    for name, register, mode, body in cases:
        case = args.output / f"{register.lower()}-{mode.lower()}"
        case.mkdir()
        fixture = case / "cpu.repl"
        fixture.write_text(f'cpu: CPU.RiscV32 @ sysbus\n    cpuType: "rv32imac"\n'
                           f'    timeProvider: empty\n    interruptMode: InterruptMode.{mode}\n'
                           '    privilegedArchitecture: PrivilegedArchitecture.Priv1_10\n'
                           'mem: Memory.MappedMemory @ sysbus 0x0\n    size: 0x10000000\n')
        dictionary = re.search(rf"^&\{{{register.lower()}_write_opcodes\}}\s+(.*)$", robot, re.M)
        opcodes = dict(item.split("=") for item in dictionary[1].split())
        expected = re.findall(rf"{register} Should Be Equal\s+(0x[\da-f]+)", body)
        writes = re.findall(rf"Set {register} By Software\s+(0x[\da-f]+)", body)
        warning_expected = re.findall(rf"(Should Not Be In Log|Wait For Log Entry)\s+{register} value written to CSR corrected", body)
        if len(expected) != 6 or len(writes) != 5 or len(warning_expected) != 5:
            raise ValueError(f"unexpected pinned Robot expectations for {name}")
        program = [".section .text", ".option norvc", ".global _start", "_start:"]

        def check(value):
            program.extend([f"csrr t1, {register.lower()}", f"li t2, {value}", "bne t1, t2, failed"])

        check(expected[0])
        for value, readback in zip(writes, expected[1:]):
            program.append(f".word {opcodes[value]}")
            check(readback)
        program.extend(["li t2, 1", "j done", "failed: li t2, 3", "done: li t1, 0x100000", "sw t2, 0(t1)", "j ."])
        source = case / "test.S"
        source.write_text("\n".join(program) + "\n")
        elf = case / "test.elf"
        subprocess.run(["clang", "--target=riscv32-unknown-elf", "-march=rv32imac", "-mabi=ilp32",
                        "-nostdlib", "-fuse-ld=lld", f"-Wl,-T,{linker}", source, "-o", elf], check=True)
        run = subprocess.run([args.penv.resolve(), "run", "--platform", fixture, "--tohost", "0x100000",
                              "--max-steps", "1000", elf], text=True, capture_output=True, timeout=15)
        (case / "execution.log").write_text(run.stdout + run.stderr)
        actual_warnings = re.findall(rf"{register} value written to CSR corrected from (0x[\da-f]+) to (0x[\da-f]+)", run.stderr)
        wanted_warnings = [
            (hex(int(write, 16)), hex(int(readback, 16)))
            for write, readback, warning in zip(writes, expected[1:], warning_expected)
            if warning == "Wait For Log Entry"
        ]
        passed = run.returncode == 0 and "penv: tohost exit code 0" in run.stderr and actual_warnings == wanted_warnings
        results.append({"case": name, "mode": mode, "expected": expected, "warnings": actual_warnings,
                        "passed": passed, "exit": run.returncode})
        print(f"{'PASS' if passed else 'FAIL'} {name}", flush=True)
    (args.output / "results.json").write_text(json.dumps(results, indent=2) + "\n")
    return 0 if all(row["passed"] for row in results) else 1


if __name__ == "__main__":
    raise SystemExit(main())
