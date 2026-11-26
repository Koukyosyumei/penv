"""Port the fixed RV32 Andes register-instruction cases."""
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
    operations = {"lea.h": 5, "lea.w": 6, "lea.d": 7, "ffb": 16,
                  "ffzmism": 17, "ffmism": 18, "flmism": 19}
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=False)
    (out / "cpu.repl").write_text(
        'cpu: CPU.RiscV32 @ sysbus { cpuType: "rv32imac_xandesperf"; timeProvider: empty }\n'
        'mem: Memory.MappedMemory @ sysbus 0x0 { size: 0x100000 }\n'
    )
    link = out / "link.ld"
    link.write_text("ENTRY(_start)\nPHDRS { code PT_LOAD FLAGS(5); }\n"
                    "SECTIONS { .text 0x2000 : { *(.text) } :code }\n")
    all_passed = True
    for op, funct7 in operations.items():
        name = f"Should Execute {op.upper().replace('.', ' ')}"
        body = re.search(rf"(?ms)^{re.escape(name)}\n(.*?)(?=^\S|\Z)", robot)[1]
        inputs = re.findall(
            rf"(?m)^ +Check Register Instruction\s+nds\.{re.escape(op)} +"
            r"(0x\w+) +(0x\w+) +(0x\w+)(?: +(a0))? *$", body,
        )
        if not inputs:
            parser.error(f"missing {name} parameters")
        passed = True
        for index, (a, b, expected, destination) in enumerate(inputs):
            stem = out / f"{op}-{index}"
            source, elf, script = (Path(f"{stem}.{ext}") for ext in ("S", "elf", "resc"))
            rd = 10 if destination == "a0" else 12
            opcode = funct7 << 25 | 11 << 20 | 10 << 15 | rd << 7 | 0x5b
            source.write_text(
                f".option norvc\n.text\n.global _start\n_start:\n.word {opcode:#x}\n"
                f"li t0, {expected}\nbne x{rd}, t0, fail\nli t0, 1\nj done\n"
                "fail: li t0, 3\ndone: li t1, 0x3000\nsw t0, 0(t1)\nj .\n"
            )
            subprocess.run(["clang", "--target=riscv32-unknown-elf", "-march=rv32imac",
                            "-mabi=ilp32", "-nostdlib", "-fuse-ld=lld", f"-Wl,-T,{link}",
                            source, "-o", elf], check=True, timeout=15)
            script.write_text(
                "mach create\nmachine LoadPlatformDescription @cpu.repl\n"
                f"sysbus LoadELF @{elf.name}\ncpu SetRegister 10 {a}\ncpu SetRegister 11 {b}\n"
            )
            run = subprocess.run(
                [args.penv.resolve(), "run", "--script", script, "--tohost", "0x3000", "--max-steps", "40"],
                capture_output=True, text=True, timeout=15,
            )
            Path(f"{stem}.log").write_text(run.stdout + run.stderr)
            Path(f"{stem}.status").write_text(f"{run.returncode}\n")
            passed &= run.returncode == 0 and "penv: tohost exit code 0" in run.stderr
        print(f"{'PASS' if passed else 'FAIL'} {name} ({len(inputs)} inputs)")
        all_passed &= passed
    return 0 if all_passed else 1


if __name__ == "__main__":
    raise SystemExit(main())
