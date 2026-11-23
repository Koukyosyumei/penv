"""Run fixed Renode RV32 Andes branch and bit-field inputs as ELFs."""
import argparse
from pathlib import Path
import re
import subprocess


def encode(op, first, second):
    base = 10 << 15 | 0x5b
    if op in ("bfoz", "bfos"):
        return base | (2 + (op == "bfos")) << 12 | 12 << 7 | first << 26 | second << 20
    funct3 = {"bbc": 7, "bbs": 7, "beqc": 5, "bnec": 6}[op]
    bit30 = op == "bbs" if funct3 == 7 else bool(first & 64)
    return (base | funct3 << 12 | (first & 31) << 20 | (first & 32) << 2
            | bit30 << 30 | (second & 0x400) << 21
            | (second & 0x3e0) << 20 | (second & 0x1e) << 7)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("penv", type=Path)
    parser.add_argument("renode", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    robot = (args.renode / "tests/unit-tests/riscv-andestar-v5.robot").read_text()
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=False)
    (out / "cpu.repl").write_text(
        'cpu: CPU.RiscV32 @ sysbus { cpuType: "rv32imac_xandesperf"; timeProvider: empty }\n'
        'mem: Memory.MappedMemory @ sysbus 0x0 { size: 0x100000 }\n'
    )
    link = out / "link.ld"
    link.write_text("ENTRY(_start)\nPHDRS { code PT_LOAD FLAGS(5); }\n"
                    "SECTIONS { .text 0x1c00 : { *(.text) } :code }\n")
    all_passed = True
    for op in ("bbc", "bbs", "beqc", "bnec", "bfoz", "bfos"):
        name = f"Should Execute {op.upper()}"
        body = re.search(rf"(?ms)^{name}\n(.*?)(?=^\S|\Z)", robot)[1]
        inputs = re.findall(
            rf"(?m)^ +Check (?:Immediate Branch|Bit Field)\s+nds\.{op} +"
            r"(0x[\da-f]+) +(\d+) +(-?\d+) +(0x[\da-f]+) *$", body,
        )
        if not inputs:
            parser.error(f"missing {name} parameters")
        passed = True
        for index, (original, first, second, expected) in enumerate(inputs):
            stem = out / f"{op}-{index}"
            source, elf, script = (stem.with_suffix(f".{ext}") for ext in ("S", "elf", "resc"))
            opcode = encode(op, int(first), int(second))
            field = op.startswith("bf")
            target = 0x2004 if field else int(expected, 0)
            assembly = [
                (0x2000, f"_start:\n.word {opcode:#x}"),
            ]
            # A self-branch is detected at the one-step boundary instead of adding code
            # at its destination, which would overwrite the upstream instruction.
            if target != 0x2000:
                assembly.append((
                    target,
                    f"li t0, {expected if field else original}\n"
                    f"bne {'a2' if field else 'a0'}, t0, fail\n"
                    "li t0, 1\nj done\nfail: li t0, 3\n"
                    "done: li t1, 0x3000\nsw t0, 0(t1)\nj .",
                ))
            source.write_text(".option norvc\n.text\n.global _start\n" + "".join(
                f".org {address - 0x1c00:#x}\n{code}\n" for address, code in sorted(assembly)
            ))
            subprocess.run(["clang", "--target=riscv32-unknown-elf", "-march=rv32imac",
                            "-mabi=ilp32", "-nostdlib", "-fuse-ld=lld", f"-Wl,-T,{link}",
                            source, "-o", elf], check=True, timeout=15)
            script.write_text(
                "mach create\nmachine LoadPlatformDescription @cpu.repl\n"
                f"sysbus LoadELF @{elf.name}\ncpu SetRegister 10 {original}\n"
            )
            command = [args.penv.resolve(), "run", "--script", script]
            if not field:
                step = subprocess.run(command + ["--stats", "--max-steps", "1"],
                                      capture_output=True, text=True, timeout=15)
                stem.with_suffix(".step.log").write_text(step.stdout + step.stderr)
                stem.with_suffix(".step.status").write_text(f"{step.returncode}\n")
                passed &= step.returncode == 2 and bool(re.search(
                    rf"(?m)^Limit, 1 instructions, .*pc=0x{target:08x}$", step.stderr
                ))
            if target != 0x2000:
                run = subprocess.run(
                    command + ["--tohost", "0x3000", "--max-steps", "40"],
                    capture_output=True, text=True, timeout=15,
                )
                stem.with_suffix(".log").write_text(run.stdout + run.stderr)
                stem.with_suffix(".status").write_text(f"{run.returncode}\n")
                passed &= run.returncode == 0 and "penv: tohost exit code 0" in run.stderr
        print(f"{'PASS' if passed else 'FAIL'} {name} ({len(inputs)} inputs)")
        all_passed &= passed
    return 0 if all_passed else 1


if __name__ == "__main__":
    raise SystemExit(main())
