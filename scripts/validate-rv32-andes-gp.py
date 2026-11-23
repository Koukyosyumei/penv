"""Run fixed Renode RV32 Andes GP-relative operations as ELFs."""
import argparse
from pathlib import Path
import re
import subprocess

# opcode, funct3, access width, source/destination form
ENCODINGS = {
    "lbgp": (0x0b, 0, 1, False), "lbugp": (0x0b, 2, 1, False),
    "lhgp": (0x2b, 1, 2, False), "lhugp": (0x2b, 5, 2, False),
    "lwgp": (0x2b, 2, 4, False), "sbgp": (0x0b, 3, 1, True),
    "shgp": (0x2b, 0, 2, True), "swgp": (0x2b, 4, 4, True),
    "addigp": (0x0b, 1, 1, False),
}

CASES = (
    "Should Store Half Words With SHGP", "Should Execute LBGP", "Should Execute LBUGP",
    "Should Execute LHGP", "Should Execute LHUGP", "Should Execute LWGP",
    "Should Wrap ADDIGP At Thirty Two Bits", "Should Decode SBGP Displacements",
    "Should Decode SHGP Displacements", "Should Decode SWGP Displacements",
    "Should Decode Nineteen Bit LWGP Displacements",
)


def encode(op, displacement):
    opcode, funct3, width, store = ENCODINGS[op]
    bits = lambda start, mask: (displacement >> start) & mask
    encoded = opcode | funct3 << 12 | bits(12, 7) << 17 | bits(15, 3) << 15
    if width == 1:
        encoded |= bits(0, 1) << 14
    if store:
        encoded |= 10 << 20 | bits(5, 63) << 25 | bits(11, 1) << 7
        if width == 4:
            return encoded | bits(2, 7) << 9 | bits(17, 1) << 8 | bits(18, 1) << 31
        return encoded | bits(1, 15) << 8 | bits(17, 1) << 31
    encoded |= 12 << 7 | bits(11, 1) << 20
    if width == 4:
        return encoded | bits(2, 511) << 22 | bits(17, 1) << 21 | bits(18, 1) << 31
    return encoded | bits(1, 1023) << 21 | bits(17, 1) << 31


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
                    "SECTIONS { .text 0x2000 : { *(.text) } :code }\n")
    all_passed = True
    for number, name in enumerate(CASES):
        body = re.search(rf"(?ms)^{re.escape(name)}\n(.*?)(?=^\S|\Z)", robot)[1]
        inputs = re.findall(r"(?m)^ +Check (GP Load|GP Store|ADDIGP)\s+(.*)$", body)
        if len(inputs) != 3:
            parser.error(f"expected three pinned inputs for {name}")
        passed = True
        for index, (kind, operands) in enumerate(inputs):
            words = operands.split()
            setup = []
            check = ""
            gp = 0x80000
            stem = out / f"{number}-{index}"
            source, elf, script = (stem.with_suffix(f".{ext}") for ext in ("S", "elf", "resc"))
            if kind == "ADDIGP":
                gp, displacement, expected = (int(word, 0) for word in words)
                op = "addigp"
            else:
                mnemonic, _, first, second, value, expected = words
                op = mnemonic.removeprefix("nds.")
                _, _, width, store = ENCODINGS[op]
                displacement, address = (int(first, 0), int(second, 0)) if store else (
                    int(second, 0), int(first, 0)
                )
                data = stem.with_suffix(".bin")
                data.write_bytes((0 if store else int(value, 0)).to_bytes(width, "little"))
                setup.append(f"sysbus LoadBinary @{data.name} {address:#x}")
                if store:
                    setup.append(f"cpu SetRegister 10 {value}")
                    read = {1: "lbu", 2: "lhu", 4: "lw"}[width]
                    check = f"li t0, {address:#x}\n{read} a2, 0(t0)\n"
                expected = int(expected, 0)
            opcode = encode(op, displacement)
            source.write_text(
                f".option norvc\n.text\n.global _start\n_start:\n.word {opcode:#x}\n"
                + check + f"li t0, {expected:#x}\nbne a2, t0, fail\nli t0, 1\nj done\n"
                "fail: li t0, 3\ndone: li t1, 0x3000\nsw t0, 0(t1)\nj .\n"
            )
            subprocess.run(["clang", "--target=riscv32-unknown-elf", "-march=rv32imac",
                            "-mabi=ilp32", "-nostdlib", "-fuse-ld=lld", f"-Wl,-T,{link}",
                            source, "-o", elf], check=True, timeout=15)
            script.write_text("\n".join([
                "mach create", "machine LoadPlatformDescription @cpu.repl",
                f"sysbus LoadELF @{elf.name}", f"cpu SetRegister 3 {gp:#x}", *setup, "",
            ]))
            run = subprocess.run(
                [args.penv.resolve(), "run", "--script", script, "--tohost", "0x3000", "--max-steps", "40"],
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
