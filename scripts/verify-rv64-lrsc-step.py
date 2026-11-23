#!/usr/bin/env python3
"""Replay five fixed PolarFire LR/SC single-step cases."""
import argparse
from pathlib import Path
import re
import subprocess

BASE = 0x80000000
FIRST, SECOND = "u54_1", "u54_2"
A, B = 0x81000000, 0x81000100


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--renode", type=Path, default=Path("../renode"))
    parser.add_argument("--penv", type=Path, default=Path("target/release/penv"))
    parser.add_argument("--cc", required=True)
    parser.add_argument("--logs", type=Path, required=True)
    parser.add_argument("--representative", action="store_true")
    args = parser.parse_args()
    args.logs.mkdir(parents=True, exist_ok=True)
    failed = 0

    def verify(name, configure):
        nonlocal failed
        commands = []
        expected = []
        file_name = name.replace("/", "-")

        def code(cpu, assembly):
            stem = args.logs / f"{file_name}-{cpu}"
            pc = BASE if cpu == FIRST else BASE + 0x100
            stem.with_suffix(".S").write_text(".text\n.globl _start\n_start:\n" + assembly + "\n")
            subprocess.run([args.cc, "-march=rv64gc", "-mabi=lp64d", "-nostdlib",
                            f"-Wl,-Ttext={pc:#x},--no-relax", "-o", stem.with_suffix(".elf"), stem.with_suffix(".S")], check=True)
            subprocess.run([args.cc.removesuffix("gcc") + "objcopy", "-j", ".text", "-O", "binary",
                            stem.with_suffix(".elf"), stem.with_suffix(".bin")], check=True)
            commands.extend(("--load-binary", f"{pc:#x}:{stem.with_suffix('.bin')}",
                             "--set-pc", f"{cpu}:{pc:#x}"))

        def step(cpu, count=1):
            commands.extend(("--step-cpu", cpu) * count)

        def reg(cpu, register, value):
            commands.extend(("--read-register-now", f"{cpu}@{register}"))
            expected.append(f"register {cpu}@{register} = {value:#018x}")

        def write(address, width, value):
            commands.extend(("--write-memory", f"{address:#x}:{width}:{value:#x}"))

        def read(address, value):
            commands.extend(("--read-memory-now", f"{address:#x}:4"))
            expected.append(f"memory sysbus@{address:#018x}:4 = {value:#018x}")

        configure(commands, code, step, reg, write, read)
        run = subprocess.run([args.penv, "run", "--platform-root", args.renode,
                              "--platform", args.renode / "platforms/cpus/polarfire-soc.repl",
                              "--max-steps", "0", *commands], capture_output=True, text=True, timeout=30)
        actual = re.findall(r"^penv: ((?:register|memory) .*)$", run.stderr, re.M)
        passed = run.returncode == 0 and not run.stdout and actual == expected
        (args.logs / f"{file_name}.stderr").write_text(run.stderr)
        (args.logs / f"{file_name}.status").write_text(f"{run.returncode}\n")
        print(f"{'PASS' if passed else 'FAIL'} {name}")
        if not passed:
            print(run.stderr)
            for i, value in enumerate(expected):
                if i >= len(actual) or actual[i] != value:
                    print(f"first mismatch at {i}: {value}")
                    break
        failed += not passed

    def paired(mode):
        def case(commands, code, step, reg, write, read):
            foreign = mode.startswith("foreign")
            code(FIRST, f"li a0, {A:#x}; lr.d a1, (a0); li a2, 6; "
                 + (f"li a0, {B:#x}; " if foreign else "")
                 + "sc.d a3, a2, (a0); sc.d a3, zero, (a0)")
            middle = {"foreign": ".2byte 0", "foreign-supplemental": "nop"}.get(mode, "li a2, 7")
            code(SECOND, f"li a0, {B:#x}; lr.d a1, (a0); {middle}; sc.d a3, a2, (a0)")
            if mode == "foreign":
                commands.extend(("--set-mtvec", f"{SECOND}:0x1010"))
            for address in (A, B):
                write(address, 4, 5)
            for cpu, count, address in ((FIRST, 2, A), (SECOND, 3, B)):
                step(cpu, count)
                reg(cpu, "x10", address)
            for cpu in (FIRST, SECOND):
                step(cpu)
                reg(cpu, "x11", 5)
            step(FIRST)
            reg(FIRST, "x12", 6)
            if foreign:
                step(FIRST, 3)
                reg(FIRST, "x10", B)
                step(FIRST)
                reg(FIRST, "x13", 1)
            if mode == "foreign":
                step(SECOND)
                reg(SECOND, "pc", 0x1010)
                reg(SECOND, "mcause", 2)
                reg(SECOND, "x13", 0)
            elif mode != "repeated":
                step(SECOND)
                reg(SECOND, "x12", 0 if foreign else 7)
                step(SECOND)
                reg(SECOND, "x13", 0)
            if not foreign:
                step(FIRST)
                reg(FIRST, "x13", 0)
            if mode == "repeated":
                read(A, 6)
                step(FIRST)
                reg(FIRST, "x13", 1)
                read(A, 6)
        return case

    def loop(commands, code, step, reg, write, read):
        code(FIRST, "repeat: lr.w a4, (a0); addi a4, a4, 1; sc.w a4, a4, (a0); "
             "bnez a4, repeat; addi a1, a1, -1; bnez a1, repeat; j .")
        write(A, 4, 0)
        commands.extend(("--set-gpr", f"{FIRST}@x10:{A:#x}", "--set-gpr", f"{FIRST}@x11:100"))
        for i in range(100):
            step(FIRST)
            reg(FIRST, "x14", i)
            step(FIRST)
            reg(FIRST, "x14", i + 1)
            step(FIRST)
            reg(FIRST, "x14", 0)
            read(A, i + 1)
            step(FIRST, 2)
            reg(FIRST, "x11", 99 - i)
            step(FIRST)
        reg(FIRST, "x11", 0)
        read(A, 100)

    def unaligned(commands, code, step, reg, write, read):
        code(FIRST, "li t0, 0x1001; li t1, 1; sc.w t0, t1, (t0)")
        commands.extend(("--set-mtvec", f"{FIRST}:0x80001010"))
        step(FIRST, 4)
        fault_pc = BASE + 6
        reg(FIRST, "pc", 0x80001010)
        reg(FIRST, "mcause", 6)
        reg(FIRST, "mepc", fault_pc)
        for word, cause in ((0x1862b2af, 6), (0x1002a32f, 4), (0x1002b32f, 4)):
            write(fault_pc, 4, word)
            commands.extend(("--set-pc", f"{FIRST}:{fault_pc:#x}"))
            step(FIRST)
            reg(FIRST, "pc", 0x80001010)
            reg(FIRST, "mcause", cause)

    verify("Should Handle Multiple Writes To Same Reservation", paired("repeated"))
    if not args.representative:
        verify("Should Work On Multiple Cores", paired("independent"))
        verify("Should Drop Other Core Reservation On SC", paired("foreign"))
        verify("Should Drop Other Core Reservation On SC (supplemental)", paired("foreign-supplemental"))
        verify("Should Handle Looping LR/SC Pairs To Same Reservation", loop)
        verify("Should Trap On Unaligned LR/SC", unaligned)
    raise SystemExit(int(failed != 0))


if __name__ == "__main__":
    main()
