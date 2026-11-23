"""Port the fixed five RV32 cases in Renode riscv-external-pmp.robot."""
import argparse
from pathlib import Path
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("penv", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=False)
    (out / "cpu.repl").write_text(
        'dram: Memory.MappedMemory @ sysbus 0x80000000 { size: 0x80000000 }\n'
        'mtvec: Memory.MappedMemory @ sysbus 0x1000 { size: 0x40000 }\n'
        'cpu: CPU.RiscV32 @ sysbus { cpuType: "rv32gc_zicsr"; timeProvider: empty }\n'
        'pmp: Miscellaneous.RiscVExternalPMP @ cpu { numberOfPMPEntries: 16 }\n'
    )
    link = out / "link.ld"
    link.write_text("ENTRY(_start)\nPHDRS { code PT_LOAD FLAGS(5); }\n"
                    "SECTIONS { .text 0x1000 : { *(.text) } :code }\n")

    def run(name, commands, expected, program=None):
        script = out / f"{name}.resc"
        boot = ["mach create", "machine LoadPlatformDescription @cpu.repl"]
        options = []
        if program is not None:
            source = out / f"{name}.S"
            elf = source.with_suffix(".elf")
            source.write_text(
                ".option norvc\n.text\n.global _start\n_start:\n"
                + "\n".join(program)
                + "\nli t2, 1\nj done\nfail: li t2, 3\n"
                  "done: li t1, 0x3000\nsw t2, 0(t1)\nj .\n"
            )
            subprocess.run(["clang", "--target=riscv32-unknown-elf", "-march=rv32imac",
                            "-mabi=ilp32", "-nostdlib", "-fuse-ld=lld", f"-Wl,-T,{link}",
                            source, "-o", elf], check=True, timeout=15)
            boot.append(f"sysbus LoadELF @{elf.name}")
            options = ["--tohost", "0x3000", "--max-steps", "512"]
            expected = [*expected, "penv: tohost exit code 0"]
        script.write_text("\n".join(boot + commands) + "\n")
        result = subprocess.run([args.penv.resolve(), "run", "--script", script, *options],
                                capture_output=True, text=True, timeout=15)
        (out / f"{name}.log").write_text(result.stdout + result.stderr)
        (out / f"{name}.status").write_text(f"{result.returncode}\n")
        passed = (result.returncode == 0 and not result.stdout
                  and result.stderr.splitlines() == ["cpu: Enabling External PMP", *expected])
        print(f"{'PASS' if passed else 'FAIL'} {name}")
        return passed

    results = []
    for kind, count in [("Config", 4), ("Address", 16)]:
        values = [0x03030303] * count if kind == "Config" else list(range(count))
        commands = [
            command
            for index, value in enumerate(values)
            for command in (f"cpu.pmp {kind}CSRWrite {index} {value}",
                            f"cpu.pmp {kind}CSRRead {index}")
        ]
        results.append(run(f"monitor-{kind}", commands, [hex(value) for value in values]))

        prefix = "pmpcfg" if kind == "Config" else "pmpaddr"
        program = [f"li a0, {values[0]}"]
        for index, value in enumerate(values):
            csr = f"{prefix}{index}"
            program.extend([f"csrw {csr}, a0", f"csrr t0, {csr}", f"li t2, {value}",
                            "bne a0, t2, fail", "bne t0, t2, fail"])
            if kind == "Address":
                program.append("addi a0, a0, 1")
        if kind == "Address":
            program.extend(["li t2, 16", "bne a0, t2, fail"])
        results.append(run(f"cpu-{kind}", [], [], program))

    commands = ["cpu.pmp ConfigCSRWrite 17 0x3", "cpu.pmp ConfigCSRRead 17",
                "cpu.pmp AddressCSRWrite 17 0xbadcafe", "cpu.pmp AddressCSRRead 17"]
    program = ["csrw pmpaddr17, a0", "csrr t0, pmpaddr17",
               "csrw pmpcfg5, a0", "csrr t0, pmpcfg5"]
    expected = []
    for kind, index, monitor in [("config", 17, True), ("address", 17, True),
                                 ("address", 17, False), ("config", 5, False)]:
        expected.extend([f"Attempted to write to invalid PMP {kind} register {index}",
                         f"Attempted to read from invalid PMP {kind} register {index}"])
        if monitor:
            expected.append("0x0")
    results.append(run("invalid", commands, expected, program))
    return 0 if all(results) else 1


if __name__ == "__main__":
    raise SystemExit(main())
