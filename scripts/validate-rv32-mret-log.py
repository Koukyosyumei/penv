"""Run the fixed Renode RV32 MRET/TLB ELF with its original log conditions."""

import argparse
import hashlib
from pathlib import Path
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("penv", type=Path)
    parser.add_argument("elf", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    firmware = args.elf.read_bytes()
    if hashlib.sha1(firmware).hexdigest() != "257bec1c8918772e95eb9fdc1a432c852f052f99":
        parser.error("expected the pinned riscv-mret.robot ELF")
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    (output / "mret.elf").write_bytes(firmware)
    (output / "cpu.repl").write_text(
        'cpu: CPU.RiscV32 @ sysbus\n    cpuType: "rv32im_zicsr"\n'
        'ram: Memory.MappedMemory @ sysbus 0x10000\n    size: 0x8000\n'
        'forbidden: Memory.MappedMemory @ sysbus 0x2000\n    size: 0x2000\n'
    )
    script = output / "mret.resc"
    script.write_text(
        "mach create\nmachine LoadPlatformDescription @cpu.repl\n"
        "sysbus LoadELF @mret.elf\ncpu LogFunctionNames true\n"
    )

    def run(name, script, steps):
        result = subprocess.run(
            [args.penv.resolve(), "run", "--script", script, "--max-steps", str(steps)],
            capture_output=True, text=True, timeout=15,
        )
        (output / f"{name}.log").write_text(result.stdout + result.stderr)
        (output / f"{name}.status").write_text(f"{result.returncode}\n")
        return result

    original = run("execution", script, 44)
    # The Robot log tester rejects fail_loop before accepting unrecoverable.
    markers = ("Entering function unrecoverable ", "Entering function fail_loop ")
    first = next((line for line in original.stderr.splitlines() if line.startswith(markers)), "")
    negative = output / "negative.resc"
    negative.write_text(script.read_text() + "cpu PC 0x2018\n")
    control = run("negative", negative, 1)
    passed = (
        original.returncode == 2 and first.startswith(markers[0])
        and control.returncode == 2 and markers[1] in control.stderr
    )
    print(f"{'PASS' if passed else 'FAIL'} Should Invalidate TLB After Mode Change")
    return 0 if passed else 1


if __name__ == "__main__":
    raise SystemExit(main())
