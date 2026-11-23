"""Run the fixed Renode RV32 counter cases with only V removed from the ISA."""
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
    robot = (args.renode / "tests/unit-tests/riscv-priv.robot").read_text()
    isa = re.search(r"rv\$\{bitness\}([a-z0-9_]+)", robot)[1]
    if "_v_" not in isa:
        parser.error("the fixed ISA no longer contains separated V")
    isa = "rv32" + isa.replace("_v_", "_", 1)
    cases = re.findall(
        r"(?ms)^(Should (?:Increment|Inhibit) M(?:CYCLE|INSTRET)[^\n]* RV32)\n"
        r"(.*?)(?=^\S|\Z)",
        robot,
    )
    if len(cases) != 6:
        parser.error("expected six fixed RV32 machine counter cases")
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    platform = output / "cpu.repl"
    platform.write_text(
        f'cpu: CPU.RiscV32 @ sysbus {{ cpuType: "{isa}"; pmpEntryCount: 0; timeProvider: clint }}\n'
        'clint: IRQControllers.CoreLevelInterruptor @ sysbus 0x44000000 { frequency: 66000000 }\n'
        'mem: Memory.MappedMemory @ sysbus 0x1000 { size: 0x40000 }\n'
    )
    linker = output / "link.ld"
    linker.write_text(
        "ENTRY(_start)\nPHDRS { code PT_LOAD FLAGS(5); }\n"
        "SECTIONS { .text 0x2000 : { *(.text) } :code }\n"
    )
    passed = True
    for index, (name, body) in enumerate(cases):
        instructions = re.findall(r"(?m)^    \.\.\.\s+(\S.*)", body)
        expected = int(re.search(r'Register Should Be Equal\s+"a3"\s+(\w+)', body)[1], 0)
        source = output / f"{index}.S"
        elf = source.with_suffix(".elf")
        source.write_text(
            ".option norvc\n.text\n.global _start\n_start:\n"
            + "\n".join(instructions)
            + f"\nli t1, {expected}\nbne a3, t1, fail\nli t1, 1\nj done\n"
            "fail: li t1, 3\ndone: li t2, 0x3000\nsw t1, 0(t2)\nj .\n"
        )
        subprocess.run(
            ["clang", "--target=riscv32-unknown-elf", "-march=rv32imac", "-mabi=ilp32",
             "-nostdlib", "-fuse-ld=lld", f"-Wl,-T,{linker}", source, "-o", elf],
            check=True, capture_output=True, text=True, timeout=15,
        )
        run = subprocess.run(
            [args.penv.resolve(), "run", "--platform", platform, "--tohost", "0x3000",
             "--max-steps", "64", elf],
            capture_output=True, text=True, timeout=15,
        )
        (output / f"{index}.log").write_text(run.stdout + run.stderr)
        (output / f"{index}.status").write_text(f"{run.returncode}\n")
        ok = run.returncode == 0 and "penv: tohost exit code 0" in run.stderr
        print(f"{'PASS' if ok else 'FAIL'} {name}")
        passed &= ok
    return 0 if passed else 1


if __name__ == "__main__":
    raise SystemExit(main())
