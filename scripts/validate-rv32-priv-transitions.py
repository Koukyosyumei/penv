"""Port the fixed RV32 SFENCE.VMA and SRET privilege cases to real ELFs."""

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
    robot = (args.renode / "tests/unit-tests/riscv-priv.robot").read_text()
    cases = re.findall(
        r'(?ms)^(Should (?:Trap On SFENCE\.VMA|(?:Not )?Trap On Sret)[^\n]* RV32)\n'
        r"(.*?)(?=^\S|\Z)", robot,
    )
    if len(cases) != 3:
        parser.error(f"expected three RV32 transitions, found {len(cases)}")
    isa = "rv32" + re.search(r"rv\$\{bitness\}([a-z0-9_]+)", robot)[1]
    if "_v_" not in isa:
        parser.error("the fixed ISA no longer contains separated V")
    pc, mtvec, illegal = (
        int(re.search(rf"^\$\{{{name}\}}\s+(0x[0-9a-f]+)", robot, re.M)[1], 16)
        for name in ("starting_pc", "mtvec", "illegal_instruction")
    )
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    platforms = {}
    for mode, cpu_type in (("diagnostic", isa.replace("_v_", "_", 1)), ("fixed", isa)):
        platform = output / f"{mode}.repl"
        platform.write_text(
            f'cpu: CPU.RiscV32 @ sysbus {{ cpuType: "{cpu_type}"; pmpEntryCount: 0; timeProvider: clint }}\n'
            'clint: IRQControllers.CoreLevelInterruptor @ sysbus 0x44000000 { frequency: 66000000 }\n'
            'mem: Memory.MappedMemory @ sysbus 0x1000 { size: 0x40000 }\n'
        )
        platforms[mode] = platform
    results = []
    for index, (name, body) in enumerate(cases):
        mode = re.search(r"Change Privilege\s+\$\{(user|supervisor)_mode\}", body)[1]
        instruction = re.search(r'Run (?:Assembly|Instruction)\s+"([^"]+)"', body)[1]
        registers = dict(re.findall(r'Set Register\s+"(mstatus|sepc)"\s+(0x[0-9a-f]+)', body))
        trap = "Should Not Trap" not in body
        target = int(registers.get("sepc", "0xbad0"), 16)
        status = int(registers.get("mstatus", "0"), 16)
        mpp = 0 if mode == "user" else 1 << 11
        linker = output / f"{index}.ld"
        linker.write_text(
            "ENTRY(_start)\nPHDRS { code PT_LOAD FLAGS(5); }\n"
            f"SECTIONS {{ .trap {mtvec:#x} : {{ *(.trap) }} :code "
            f".test {pc:#x} : {{ *(.test) }} :code "
            f".text 0x3000 : {{ *(.text) }} :code .target {target:#x} : {{ *(.target) }} :code }}\n"
        )
        source = output / f"{index}.S"
        elf = source.with_suffix(".elf")
        source.write_text(
            f".option norvc\n.text\n.globl _start\n_start:\n"
            f"li t0, {mtvec}\ncsrw mtvec, t0\nli t0, {target}\ncsrw sepc, t0\n"
            f"li t0, {status | mpp}\ncsrw mstatus, t0\nli t0, {pc}\ncsrw mepc, t0\nmret\n"
            f"pass: li t1, 1\nj done\nfail: li t1, 3\ndone: li t2, 0x10000\nsw t1, 0(t2)\nj .\n"
            f'.section .trap, "ax"\ncsrr t0, mcause\nli t1, {illegal}\nbne t0, t1, trap_failed\n'
            f"csrr t0, mepc\nli t1, {pc}\nbne t0, t1, trap_failed\nj {'pass' if trap else 'fail'}\n"
            'trap_failed: j fail\n'
            f'.section .test, "ax"\n{instruction}\nj fail\n'
            f'.section .target, "ax"\nj {"fail" if trap else "pass"}\n'
        )
        subprocess.run(
            ["clang", "--target=riscv32-unknown-elf", "-march=rv32imac", "-mabi=ilp32",
             "-nostdlib", "-fuse-ld=lld", f"-Wl,-T,{linker}", source, "-o", elf],
            check=True, timeout=15,
        )
        for cpu_mode, platform in platforms.items():
            run = subprocess.run(
                [args.penv.resolve(), "run", "--platform", platform, "--tohost", "0x10000",
                 "--max-steps", "64", elf], capture_output=True, text=True, timeout=15,
            )
            log = output / f"{index}-{cpu_mode}.log"
            log.write_text(run.stdout + run.stderr)
            passed = run.returncode == 0 and "penv: tohost exit code 0" in run.stderr
            results.append({"case": name, "mode": cpu_mode, "passed": passed,
                            "exit_code": run.returncode, "log": log.name})
            print(f"{'PASS' if passed else 'FAIL'} {cpu_mode}: {name}")
    (output / "results.jsonl").write_text("".join(json.dumps(result) + "\n" for result in results))
    return 0 if all(result["passed"] for result in results if result["mode"] == "diagnostic") else 1


if __name__ == "__main__":
    raise SystemExit(main())
