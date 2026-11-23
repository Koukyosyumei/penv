"""Port the fixed Renode riscv-unit-tests.robot RV32 ResetVector case."""

import argparse
from pathlib import Path
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("penv", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    (output / "cpu.repl").write_text(
        'clint: IRQControllers.CoreLevelInterruptor @ sysbus 0x44000000 { frequency: 66000000 }\n'
        'cpu: CPU.RiscV32 @ sysbus { timeProvider: clint; cpuType: "rv32gc_Zfh" }\n'
        'mem: Memory.MappedMemory @ sysbus 0x1000 { size: 0x40000 }\n'
    )
    script = output / "reset-vector.resc"
    script.write_text(
        "mach create\nmachine LoadPlatformDescription @cpu.repl\n"
        "cpu PC\n"
        "cpu ResetVector 0x1234\ncpu PC\n"
        "cpu ResetVector 0x1238\ncpu PC\n"
        "cpu PC 0x5678\ncpu PC\n"
        "cpu ResetVector 0x123C\ncpu PC\n"
        "cpu Reset\ncpu PC\n"
        "cpu ResetVector 0x1240\ncpu PC\n"
        "cpu ResetVector 0x2468\ncpu Reset\ncpu PC\n"
    )
    expected = ["0x1000", "0x1234", "0x1238", "0x5678", "0x5678", "0x123c", "0x1240", "0x2468"]
    run = subprocess.run(
        [args.penv.resolve(), "run", "--script", script],
        capture_output=True, text=True, timeout=15,
    )
    (output / "execution.log").write_text(run.stdout + run.stderr)
    (output / "execution.status").write_text(f"{run.returncode}\n")
    passed = run.returncode == 0 and not run.stdout and run.stderr.splitlines() == expected
    print(f"{'PASS' if passed else 'FAIL'} Should Properly Handle ResetVector After Creation And After Reset 32")
    return 0 if passed else 1


if __name__ == "__main__":
    raise SystemExit(main())
