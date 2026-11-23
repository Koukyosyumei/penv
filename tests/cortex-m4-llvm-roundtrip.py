#!/usr/bin/env python3
"""Check the pinned Cortex-M Robot encodings with standalone LLVM tools."""
import re
import subprocess
import sys
from pathlib import Path

root = Path(__file__).resolve().parents[1]
renode = root.parent / "renode"
work = Path(sys.argv[1])
work.mkdir(parents=True, exist_ok=True)
revision = subprocess.check_output(["git", "-C", renode, "rev-parse", "HEAD"], text=True).strip()
assert revision == "c38ac3babcd38a7144b9498e3ca4c2226b76aa37", revision
suite = (renode / "tests/unit-tests/llvm-asm-disasm.robot").read_text()
section = suite.split("\nShould Assemble And Disassemble ARM Cortex-M\n", 1)[1].split("\nShould ", 1)[0]
cases = []
for line in section.splitlines():
    if not line.strip().startswith("RoundTrip Thumb"):
        continue
    _, encoding, mnemonic, operands, *options = re.split(r" {2,}", line.strip())
    size = int(options.pop(0)) if options and options[0].isdigit() else 4
    address = int(options[0].removeprefix("hex_addr="), 16) if options else 0
    cases.append((encoding, mnemonic + " " + operands.replace(r"\#", "#"), size, address))

assert len(cases) == 4
assembly = [".syntax unified", ".thumb", ".text"]
for _, instruction, _, address in sorted(cases, key=lambda case: case[3]):
    assembly.extend((f".org {address:#x}", instruction))
source, obj = work / "roundtrip.s", work / "roundtrip.o"
source.write_text("\n".join(assembly) + "\n")
subprocess.run(["llvm-mc", "-triple=thumbv7em-none-eabi", "-mcpu=cortex-m4",
                "-filetype=obj", "-o", obj, source], check=True)
dump = subprocess.check_output(["llvm-objdump", "-d", "--mcpu=cortex-m4", obj], text=True)
(work / "disassembly.log").write_text(dump)
failed = False
for encoding, instruction, size, address in cases:
    match = re.search(rf"^\s*{address:x}:\s+((?:[0-9a-f]{{2,4}}\s+)+)([^\n]+)", dump, re.M)
    verdict = bool(match and "".join(match[1].split()) == encoding
                   and len(encoding) // 2 == size
                   and match[2].split() == instruction.split())
    failed |= not verdict
    print(f"{address:#06x}\t{encoding}\t{instruction}\t{'PASS' if verdict else 'FAIL'}")
sys.exit(int(failed))
