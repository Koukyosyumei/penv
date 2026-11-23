"""Check the fixed Renode RV32 scalar Andes BF16 cases as real ELFs."""
import argparse
from pathlib import Path
import re
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("penv", type=Path)
    parser.add_argument("renode", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    robot = (args.renode / "tests/unit-tests/riscv-andestar-v5.robot").read_text()
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=False)
    link = out / "link.ld"
    link.write_text(
        "ENTRY(_start)\nPHDRS { code PT_LOAD FLAGS(5); }\n"
        "SECTIONS { .text 0x1000 : { *(.text) } :code }\n"
    )
    cases = {}
    for name in ("Should Box Scalar BF16 With FLEN Thirty Two",
                 "Should Widen Scalar BF16", "Should Narrow Scalar BF16"):
        body = re.search(rf"(?ms)^{name}\n(.*?)(?=^\S|\Z)", robot)[1]
        cases[name] = [
            (op.endswith("bf16.s"), int(source, 0), int(expected, 0),
             int(flags), int(mode or "0"), "FLEN" not in name, False, False)
            for op, source, expected, flags, mode in re.findall(
                r"(?m)^ +Check Scalar BF16 +(\S+) +(0x\w+) +(0x\w+) +(\d+)(?: +(\d+))?$", body
            )
        ]
        if len(cases[name]) != 3:
            parser.error(f"expected three inputs for {name}")
    name = "Should Round Scalar BF16 In Every Rounding Mode"
    body = re.search(rf"(?ms)^{name}\n(.*?)(?=^\S|\Z)", robot)[1]
    rows = re.findall(r"(?m)^ +\.\.\. +(\d+) +(0x\w+) +(0x\w+)$", body)
    if len(rows) != 5:
        parser.error("expected all five rounding modes")
    cases[name] = [
        (True, source, int(expected, 0), 1, int(mode), True, False, False)
        for mode, pos, neg in rows
        for source, expected in [(0xffffffff3f808000, pos), (0xffffffffbf808000, neg)]
    ]
    cases["Should Reject Scalar BF16 With Floating Point Disabled"] = [
        (False, 0, 0, 0, 0, True, True, True),
    ]
    cases["Should Reject Reserved BF16 Rounding Modes"] = [
        (True, 0xffffffff3f800000, 0, 0, mode, True, True, False) for mode in (5, 6, 7)
    ]

    all_passed = True
    for case, (name, inputs) in enumerate(cases.items()):
        passed = True
        for index, (narrow, value, expected, flags, mode, double, trap, fs_off) in enumerate(inputs):
            stem = out / f"{case}-{index}"
            source, elf, repl = (stem.with_suffix(f".{ext}") for ext in ("S", "elf", "repl"))
            isa = "gc" if double else "imaf"
            repl.write_text(
                f'cpu: CPU.RiscV32 @ sysbus {{ cpuType: "rv32{isa}_xandesbfhcvt"; timeProvider: empty }}\n'
                'mem: Memory.MappedMemory @ sysbus 0x0 { size: 0x100000 }\n'
            )
            opcode = 0x0011c2db if narrow else 0x001142db
            setup = []
            if not fs_off:
                setup = ["li t0, 0x2000", "csrw mstatus, t0", f"csrwi frm, {mode}", "csrwi fflags, 0"]
                if double:
                    setup += ["la t0, input", "fld f1, 0(t0)"]
                else:
                    setup += [f"li t0, {value & 0xffffffff:#x}", "fmv.w.x f1, t0"]
            verify = []
            if double:
                verify += ["li t0, 0x4000", "fsd f5, 0(t0)", "lw t1, 4(t0)",
                           f"li t2, {expected >> 32:#x}", "bne t1, t2, fail", "lw t1, 0(t0)"]
            else:
                verify += ["fmv.x.w t1, f5"]
            verify += [f"li t2, {expected & 0xffffffff:#x}", "bne t1, t2, fail",
                       "csrr t1, fflags", f"li t2, {flags}", "bne t1, t2, fail",
                       "li t0, 1", "j done"]
            program = [
                ".option norvc", ".text", ".global _start",
                "handler:", "csrr t1, mcause", "li t2, 2", "bne t1, t2, handler_fail",
                "csrr t1, mepc", "li t2, 0x2000", "bne t1, t2, handler_fail",
                f"j {'trap_pass' if trap else 'fail'}", "handler_fail: j fail",
                ".org 0x800", "_start:", "li t0, 0x1000", "csrw mtvec, t0",
                *setup, "j operation", ".org 0x1000", "operation:", f".word {opcode:#x}",
                *(["j fail"] if trap else verify),
                "trap_pass: li t0, 1", "j done", "fail: li t0, 3",
                "done: li t1, 0x3000", "sw t0, 0(t1)", "j .",
                ".balign 8", f"input: .quad {value:#x}",
            ]
            source.write_text("\n".join(program) + "\n")
            subprocess.run(
                ["clang", "--target=riscv32-unknown-elf", f"-march=rv32{isa}", "-mabi=ilp32",
                 "-nostdlib", "-fuse-ld=lld", f"-Wl,-T,{link}", source, "-o", elf],
                check=True, timeout=15,
            )
            result = subprocess.run(
                [args.penv.resolve(), "run", "--platform", repl, "--tohost", "0x3000",
                 "--max-steps", "100", elf],
                capture_output=True, text=True, timeout=15,
            )
            stem.with_suffix(".log").write_text(result.stdout + result.stderr)
            stem.with_suffix(".status").write_text(f"{result.returncode}\n")
            passed &= result.returncode == 0 and "penv: tohost exit code 0" in result.stderr
        print(f"{'PASS' if passed else 'FAIL'} {name} ({len(inputs)} inputs)")
        all_passed &= passed
    return 0 if all_passed else 1


if __name__ == "__main__":
    raise SystemExit(main())
