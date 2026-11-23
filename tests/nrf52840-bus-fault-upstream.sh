#!/bin/sh
# Fixed arm-cortex-m-bus-fault.robot E2E_READ_ASSEMBLY / E2E_WRITE_ASSEMBLY.
set -eu
if [ "$#" -ne 3 ]; then
    echo "Usage: $0 PENV RENODE_ROOT ARTIFACT_DIR" >&2
    exit 1
fi
penv=$1
robot=$2/tests/unit-tests/arm-cortex-m-bus-fault.robot
out=$3
mkdir -p "$out"

cat >"$out/bus-fault.repl" <<'REPL'
cpu: CPU.CortexM @ sysbus
    cpuType: "cortex-m4"
    nvic: nvic
    vectorTableOffset: 0
nvic: IRQControllers.NVIC @ sysbus 0xe000e000
    -> cpu@0
mem: Memory.MappedMemory @ sysbus 0
    size: 0x1000
faultingPeripheral: Mocks.BusFaultingPeripheral @ sysbus 0x100000
uart: UART.TrivialUart @ sysbus 0x80000
REPL

python3 - "$robot" "$out" <<'PY'
import re
import sys
from pathlib import Path

robot = Path(sys.argv[1]).read_text()
for operation in ("READ", "WRITE"):
    name = f"E2E_{operation}_ASSEMBLY"
    block = re.search(rf"^\$\{{{name}\}}.*\n((?:\.\.\..*\n)+)", robot, re.M)[1]
    assembly = "\n".join(line[3:].strip() for line in block.splitlines()
                         if '"""' not in line)
    for variable in ("STACK_TOP", "FAULTING_PERIPHERAL_ADDRESS"):
        literal = re.search(rf"^\$\{{{variable}\}}[ \t]+(0x[0-9a-fA-F]+)$", robot, re.M)[1]
        assembly = assembly.replace("${" + variable + "}", literal)
    assembly = assembly.replace("${SCB_SHCSR}", "0xe000ed24")
    # Observe the upstream post-step checks just after R11 is captured,
    # before the original WFI. Wrong handler or missing fault produces no PASS.
    capture = "ldr r11, [sp, #24] /* stacked PC */"
    assert assembly.count(capture) == 1
    assembly = assembly.replace(capture, capture + """
    cmp r10, #0
    beq failed
    cmp r10, r11
    bne failed
    ldr r0, =0xe000ed28
    ldr r1, [r0]
    movw r2, #0x8200
    cmp r1, r2
    bne failed
    ldr r0, =0xe000ed38
    ldr r1, [r0]
    ldr r2, =0x100000
    cmp r1, r2
    bne failed
    ldr r0, =0x80000
    movs r1, #'P'
    strb r1, [r0]
    movs r1, #'A'
    strb r1, [r0]
    movs r1, #'S'
    strb r1, [r0]
    strb r1, [r0]
    movs r1, #10
    strb r1, [r0]
failed:
""")
    Path(sys.argv[2], operation.lower() + ".s").write_text(
        ".syntax unified\n.thumb\n.global Reset_Handler\n" + assembly + "\n"
    )
PY

for operation in read write; do
    clang --target=arm-none-eabi -mcpu=cortex-m4 -c "$out/$operation.s" -o "$out/$operation.o"
    ld.lld -N -Ttext=0 --entry=Reset_Handler "$out/$operation.o" -o "$out/$operation.elf"
    status=0
    "$penv" run --platform "$out/bus-fault.repl" --max-steps 100 "$out/$operation.elf" \
        >"$out/$operation.log" 2>&1 || status=$?
    cat "$out/$operation.log"
    [ "$status" -eq 2 ]
    grep -Fxq PASS "$out/$operation.log"
done
