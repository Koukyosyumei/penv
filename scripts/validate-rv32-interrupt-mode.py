"""Port the fixed Renode guest and monitor trap-vector cases, including warning assertions."""
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
    cases = re.findall(r"(?m)^(Should Correct (MTVEC|STVEC) (Written By Software|From Monitor) in (\w+) Mode)\n"
                       r"(.*?)(?=^Should Correct |\Z)", robot, re.S | re.M)
    if len(cases) != 12:
        parser.error(f"expected twelve fixed cases, found {len(cases)}")
    linker = args.output / "link.ld"
    linker.write_text("ENTRY(_start)\nPHDRS { text PT_LOAD FLAGS(5); }\n"
                      "SECTIONS { . = 0; .text : { *(.text*) } :text }\n")
    results = []
    for name, register, origin, mode, body in cases:
        software = origin == "Written By Software"
        case = args.output / f"{register.lower()}-{mode.lower()}-{'software' if software else 'monitor'}"
        case.mkdir()
        fixture = case / "cpu.repl"
        fixture.write_text(f'cpu: CPU.RiscV32 @ sysbus\n    cpuType: "rv32imac"\n'
                           f'    timeProvider: empty\n    interruptMode: InterruptMode.{mode}\n'
                           '    privilegedArchitecture: PrivilegedArchitecture.Priv1_10\n'
                           'mem: Memory.MappedMemory @ sysbus 0x0\n    size: 0x10000000\n')
        dictionary = re.search(rf"^&\{{{register.lower()}_write_opcodes\}}\s+(.*)$", robot, re.M)
        opcodes = dict(item.split("=") for item in dictionary[1].split())
        expected = re.findall(rf"{register} Should Be Equal\s+(0x[\da-f]+)", body)
        write_command = f"Set {register} By Software" if software else rf"Execute Command\s+cpu {register}"
        warning_marker = f"{register} value written to CSR corrected" if software else f"modifying {register}"
        writes = re.findall(rf"{write_command}\s+(0x[\da-f]+)", body)
        warning_expected = re.findall(rf"(Should Not Be In Log|Wait For Log Entry)\s+{warning_marker}", body)
        if len(expected) != 6 or len(writes) != 5 or len(warning_expected) != 5:
            raise ValueError(f"unexpected pinned Robot expectations for {name}")
        program = [".section .text", ".option norvc", ".global _start", "_start:"]

        def check(value):
            program.extend([f"csrr t1, {register.lower()}", f"li t2, {value}", "bne t1, t2, failed"])

        if software:
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
        command = [args.penv.resolve(), "run", "--tohost", "0x100000", "--max-steps", "1000"]
        if software:
            command.extend(["--platform", fixture, elf])
        else:
            script = case / "test.resc"
            commands = ["mach create", 'machine LoadPlatformDescription @cpu.repl',
                        f"cpu {register}"]
            for value in writes:
                commands.extend([f"cpu {register} {value}", f"cpu {register}"])
            commands.append('sysbus LoadELF @test.elf')
            script.write_text("\n".join(commands) + "\n")
            command.extend(["--script", script])
        run = subprocess.run(command, text=True, capture_output=True, timeout=15)
        (case / "execution.log").write_text(run.stdout + run.stderr)
        actual_warnings = re.findall(rf"{warning_marker} from (0x[\da-f]+) to (0x[\da-f]+)", run.stderr)
        wanted_warnings = [
            (hex(int(write, 16)), hex(int(readback, 16)))
            for write, readback, warning in zip(writes, expected[1:], warning_expected)
            if warning == "Wait For Log Entry"
        ]
        passed = run.returncode == 0 and "penv: tohost exit code 0" in run.stderr and actual_warnings == wanted_warnings
        if not software:
            expected_events = []
            init_marker = f"Vectored interrupt mode set - updating {register}"
            if init_marker in body:
                expected_events.append("init")
            expected_events.append(hex(int(expected[0], 16)))
            for value, readback, warning in zip(writes, expected[1:], warning_expected):
                if warning == "Wait For Log Entry":
                    expected_events.append(f"{hex(int(value, 16))}->{hex(int(readback, 16))}")
                expected_events.append(hex(int(readback, 16)))
            actual_events = []
            for line in run.stderr.splitlines():
                if line.startswith(init_marker):
                    actual_events.append("init")
                elif line.startswith(f"{register} "):
                    actual_events.append(line.split()[1])
                elif line.startswith(warning_marker):
                    words = line.split()
                    actual_events.append(f"{words[-3]}->{words[-1]}")
            passed = passed and actual_events == expected_events
        results.append({"case": name, "mode": mode, "expected": expected, "warnings": actual_warnings,
                        "passed": passed, "exit": run.returncode})
        print(f"{'PASS' if passed else 'FAIL'} {name}", flush=True)
    (args.output / "results.json").write_text(json.dumps(results, indent=2) + "\n")
    return 0 if all(row["passed"] for row in results) else 1


if __name__ == "__main__":
    raise SystemExit(main())
