"""Run the pinned Renode RV32 Zicond inputs as guest ELFs."""
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
    path = args.renode / "tests/unit-tests/riscv-zicond-extension.robot"
    robot = path.read_text()
    definitions, tests = robot.split("*** Test Cases ***", 1)
    cases = re.findall(r"^([^#\s][^\n]*)\n(.*?)(?=^[^#\s]|\Z)", tests, re.M | re.S)
    cases = [(name, body) for name, body in cases if "Create 32 Bit Machine" in body]
    if len(cases) != 7:
        parser.error(f"expected seven pinned RV32 cases, found {len(cases)}")

    def variable(name):
        return int(re.search(rf"(?m)^\$\{{{name}\}}\s+(0x[\da-f]+)", definitions)[1], 16)

    entry, mtvec, cause = map(variable, ("PROGRAM_COUNTER", "mtvec", "illegal_instruction"))
    args.output.mkdir(parents=True, exist_ok=False)
    linker = args.output / "link.ld"
    linker.write_text(f"ENTRY(_start)\nPHDRS {{ code PT_LOAD FLAGS(5); trap PT_LOAD FLAGS(5); }}\n"
                      f"SECTIONS {{ .trap {mtvec:#x} : {{ *(.trap) }} :trap "
                      f".text {entry:#x} : {{ *(.text) }} :code }}\n")
    results = []
    for index, (name, body) in enumerate(cases, 1):
        case = args.output / str(index)
        case.mkdir()
        disabled = "Without Zicond" in body
        cpu = "rv32gc" if disabled else "rv32gc_zicond"
        fixture = case / "cpu.repl"
        fixture.write_text(
            f'cpu: CPU.RiscV32 @ sysbus\n    cpuType: "{cpu}"\n'
            f'    timeProvider: empty\n    init:\n        MTVEC {mtvec:#x}\n'
            'dram: Memory.MappedMemory @ sysbus 0x80000000\n    size: 0x80000000\n'
            'trap: Memory.MappedMemory @ sysbus 0x1000\n    size: 0x40000\n'
        )
        operation = re.search(r"czero\.(eqz|nez)", name, re.I)[1].lower() if "czero." in name.lower() else "eqz"
        # Encode the pinned operands. The additional upstream CPU is only an assembler.
        funct3 = 5 if operation == "eqz" else 7

        def instruction(rd, rs1, rs2):
            return (7 << 25) | (rs2 << 20) | (rs1 << 15) | (funct3 << 12) | (rd << 7) | 0x33

        program = [".option norvc", ".text", ".global _start", "_start:"]
        if disabled:
            program.extend([f".word {instruction(10, 11, 12):#x}", "j failed", '.section .trap,"ax"',
                            "auipc t0, 0", f"li t1, {mtvec:#x}", "bne t0, t1, failed_trap",
                            "csrr t0, mcause", f"li t1, {cause}", "bne t0, t1, failed_trap",
                            "csrr t0, mepc", f"li t1, {entry:#x}", "bne t0, t1, failed_trap",
                            "li t2, 1", "j done_trap", "failed_trap: li t2, 3",
                            "done_trap: li t1, 0x2000", "sw t2, 0(t1)", "j .", ".text",
                            "failed: li t2, 3", "li t1, 0x2000", "sw t2, 0(t1)", "j ."])
        else:
            condition = re.search(r"Condition Is (0x[\da-f]+)", body, re.I)
            left = int(re.search(r'SetRegister "a0"\s+(0x[\da-f]+)', definitions, re.I)[1], 16)
            if condition:
                right = int(condition[1], 16)
                expected = 0 if "Should Zero When" in body else left
            else:
                left, right = (int(re.search(rf'SetRegister "{register}"\s+(0x[\da-f]+)', body, re.I)[1], 16)
                               for register in ("a0", "a1"))
                expected = int(re.search(r"Register Should Be Equal\s+a2\s+(0x[\da-f]+)", body, re.I)[1], 16)
            program.extend([f"li a0, {left:#x}", f"li a1, {right:#x}",
                            f".word {instruction(12, 10, 11):#x}",
                            f"li t0, {expected:#x}", "bne a2, t0, failed",
                            "li t2, 1", "j done", "failed: li t2, 3",
                            "done: li t1, 0x2000", "sw t2, 0(t1)", "j ."])
        source, elf = case / "test.S", case / "test.elf"
        source.write_text("\n".join(program) + "\n")
        subprocess.run(["clang", "--target=riscv32-unknown-elf", "-march=rv32imac", "-mabi=ilp32",
                        "-nostdlib", "-fuse-ld=lld", f"-Wl,-T,{linker}", source, "-o", elf], check=True)
        run = subprocess.run([args.penv.resolve(), "run", "--platform", fixture, "--tohost", "0x2000",
                              "--max-steps", "100", elf], text=True, capture_output=True, timeout=15)
        (case / "execution.log").write_text(run.stdout + run.stderr)
        passed = run.returncode == 0 and "penv: tohost exit code 0" in run.stderr
        results.append(dict(case=name, passed=passed, exit=run.returncode))
        print(f"{'PASS' if passed else 'FAIL'} {name}")
    (args.output / "results.json").write_text(json.dumps(results, indent=2) + "\n")
    return 0 if all(row["passed"] for row in results) else 1


if __name__ == "__main__":
    raise SystemExit(main())
