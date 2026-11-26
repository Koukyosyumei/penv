#!/bin/sh
# arm-cortex-m-fpu.robot / Should Pass Context Stacking Preservation Test v7-M.
set -eu
if [ "$#" -ne 3 ]; then
    echo "Usage: $0 PENV RENODE_ROOT ARTIFACT_DIR" >&2
    exit 1
fi

penv=$1
out=$3
mkdir -p "$out"
elf=$out/stacking.elf
remote=cortex-v7m-extended-frame-test.elf-s_91928-d49ec01cc0c6270f7055330871953182a507efc6
if [ ! -f "$elf" ]; then
    curl -fsSL "https://dl.antmicro.com/projects/renode/$remote" -o "$elf.part"
    mv "$elf.part" "$elf"
fi
printf '%s  %s\n' "${remote##*-}" "$elf" | sha1sum --check

cat >"$out/stacking.repl" <<'REPL'
cpu: CPU.CortexM @ sysbus
    cpuType: "cortex-m4"
    nvic: nvic
    vectorTableOffset: 0
nvic: IRQControllers.NVIC @ sysbus 0xe000e000
    -> cpu@0
mem: Memory.MappedMemory @ sysbus 0
    size: 0x40000
uart: UART.NS16550 @ sysbus 0x80000
REPL
status=0
"$penv" run --platform "$out/stacking.repl" --max-steps 2500000 "$elf" >"$out/stacking.log" 2>&1 || status=$?
cat "$out/stacking.log"
if [ "$status" -ne 0 ] && [ "$status" -ne 2 ]; then
    exit "$status"
fi
grep -Fxq 'Test succeeded' "$out/stacking.log"

# arm-cortex-m-fpu.robot / Should Raise NOCP Exception.
# Read the same UFSR bits with a naturally aligned halfword access on Armv7-M.
python3 - "$2/tests/unit-tests/arm-cortex-m-fpu.robot" "$out/nocp.s" <<'PY'
from pathlib import Path
import sys

robot = Path(sys.argv[1]).read_text()
block = robot.split("${NOCP_ASSEMBLY}", 1)[1].split("${STACKING_PLATFORM_COMMON}", 1)[0]
lines = [
    line[3:].strip()
    for line in block.splitlines()
    if line.startswith("...") and '"""' not in line
]
assembly = "\n".join(lines)
assembly = assembly.replace("${NOCP_SCB_SHCSR}", "0xe000ed24")
assembly = assembly.replace("${NOCP_SCB_CSFR}", "0xe000ed2a")
assembly = assembly.replace("${NOCP_UART}", "0x100000")
assembly = assembly.replace("mov r2, Vector_Table", "mov r2, 0x10000")
handler, code = assembly.split("UsageFault_Handler:\n", 1)
assert "ldr r2, [r1]" in code
assembly = handler + "UsageFault_Handler:\n" + code.replace("ldr r2, [r1]", "ldrh r2, [r1]", 1)
Path(sys.argv[2]).write_text(".syntax unified\n.thumb\n.global Reset_Handler\n" + assembly + "\n")
PY

clang --target=arm-none-eabi -mcpu=cortex-m4 -mfpu=fpv4-sp-d16 -c "$out/nocp.s" -o "$out/nocp.o"
ld.lld -Ttext=0x10000 --entry=Reset_Handler "$out/nocp.o" -o "$out/nocp.elf"
cat >"$out/nocp.repl" <<'REPL'
cpu: CPU.CortexM @ sysbus
    cpuType: "cortex-m4"
    nvic: nvic
    vectorTableOffset: 0x10000
nvic: IRQControllers.NVIC @ sysbus 0xe000e000
    -> cpu@0
mem: Memory.MappedMemory @ sysbus 0
    size: 0x40000
uart: UART.TrivialUart @ sysbus 0x100000
REPL
status=0
"$penv" run --platform "$out/nocp.repl" --max-steps 1000 "$out/nocp.elf" >"$out/nocp.log" 2>&1 || status=$?
cat "$out/nocp.log"
if [ "$status" -ne 0 ] && [ "$status" -ne 2 ]; then
    exit "$status"
fi
grep -Fxq 'NOCP' "$out/nocp.log"
