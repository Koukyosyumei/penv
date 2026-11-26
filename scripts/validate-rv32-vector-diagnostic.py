"""Check the fixed Renode RV32 disabled-vector program and its diagnostic."""
import argparse
from pathlib import Path
import re
import struct
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("penv", type=Path)
    parser.add_argument("renode", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    robot = (args.renode / "tests/unit-tests/riscv-unit-tests.robot").read_text()
    start = int(re.search(r"(?m)^\$\{starting_pc\}\s+(0x[0-9a-f]+)", robot)[1], 16)
    block = robot.split("Load Program With Invalid Instruction\n", 1)[1].split(
        "\nSet ResetVector And Verify PC\n", 1
    )[0]
    opcodes = {
        name: int(opcode, 16)
        for name, opcode in re.findall(r"\$\{(\w+_opcode)\}\s+Set Variable\s+(0x[0-9a-f]+)", block)
    }
    writes = re.findall(r"Write Opcode To\s+\$\{start_pc(?:\+(\d+))?\}\s+\$\{(\w+_opcode)\}", block)
    if [int(offset or 0) for offset, _ in writes] != list(range(0, 40, 4)):
        parser.error("the fixed program layout changed")
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    (output / "code.bin").write_bytes(b"".join(struct.pack("<I", opcodes[name]) for _, name in writes))
    (output / "cpu.repl").write_text(
        'clint: IRQControllers.CoreLevelInterruptor @ sysbus 0x44000000 { frequency: 66000000 }\n'
        'cpu: CPU.RiscV32 @ sysbus { timeProvider: clint; cpuType: "rv32gc_Zfh" }\n'
        'mem: Memory.MappedMemory @ sysbus 0x1000 { size: 0x40000 }\n'
    )
    script = output / "boot.resc"
    script.write_text(
        'mach create\nmachine LoadPlatformDescription @cpu.repl\n'
        f'sysbus LoadBinary @code.bin {start:#x}\ncpu PC {start:#x}\n'
    )
    run = subprocess.run(
        [args.penv.resolve(), "run", "--script", script, "--max-steps", "10"],
        capture_output=True, text=True, timeout=15,
    )
    (output / "execution.log").write_text(run.stdout + run.stderr)
    (output / "execution.status").write_text(f"{run.returncode}\n")
    diagnostics = re.findall(r"(?m)^PC: (0x[0-9a-f]+).* instruction set is not enabled for this CPU!", run.stderr)
    passed = run.returncode == 2 and diagnostics == [hex(start + 8)]
    print(f"{'PASS' if passed else 'FAIL'} Should Exit Translation Block After Invalid Instruction And Report Single Error")
    return 0 if passed else 1


if __name__ == "__main__":
    raise SystemExit(main())
