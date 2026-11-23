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

# Robot 1360-1377: scalar VSTR/VLDR on the first byte of the mock.
# The guest checks the same PC, registers, SP, stacked words and SCB state.
sed '/^faultingPeripheral:/a\    FaultOffset: 0' "$out/bus-fault.repl" >"$out/fp.repl"
cat >"$out/unmapped.resc" <<'RESC'
mach create
machine LoadPlatformDescription @bus-fault.repl
sysbus LoadELF @unmapped.elf
sysbus UnhandledAccessBehaviour ThrowException
RESC
for instruction in vstr-s0 vstr-d0 vldr-s0 vldr-d0 unmapped; do
    if [ "$instruction" = unmapped ]; then
        fault_address=0x200000
        fault_instruction='ldr r2, [r0]; movs r1, #0x22'
        expected_sp=0xfe0
        set -- --script "$out/unmapped.resc"
    else
        fault_address=0x100000
        fault_instruction="${instruction%-*} ${instruction#*-}, [r0]"
        expected_sp=0xf98
        set -- --platform "$out/fp.repl"
    fi
    cat >"$out/$instruction.s" <<ASM
.syntax unified
.thumb
.global reset
.word 0x1000, reset, 0, 0, 0, handler
.thumb_func
reset:
    ldr r3, =0xe000ed24
    ldr r4, =0x20000
    str r4, [r3]
    ldr r3, =0xe000ed88
    ldr r4, =0xf00000
    str r4, [r3]
    ldr r0, =$fault_address
    movs r1, #0x11
    ldr r2, =0xa5a5a5a5
    b fault
    .ltorg
    .org 0x200
fault:
    $fault_instruction
    b .
    .org 0x300
.thumb_func
handler:
    cmp r1, #0x11
    bne failed
    ldr r3, =0xa5a5a5a5
    cmp r2, r3
    bne failed
    mov r4, sp
    movw r5, #$expected_sp
    cmp r4, r5
    bne failed
    ldr r4, [sp, #4]
    cmp r4, r1
    bne failed
    ldr r4, [sp, #8]
    cmp r4, r2
    bne failed
    ldr r4, [sp, #24]
    cmp.w r4, #0x200
    bne failed
    ldr r3, =0xe000ed28
    ldr r4, [r3]
    movw r5, #0x8200
    cmp r4, r5
    bne failed
    ldr r4, [r3, #4]
    cmp r4, #0
    bne failed
    ldr r4, [r3, #16]
    cmp r4, r0
    bne failed
    bkpt 0
failed:
    b .
ASM
    clang --target=arm-none-eabi -mcpu=cortex-m4 -c "$out/$instruction.s" -o "$out/$instruction.o"
    ld.lld -N -Ttext=0 --entry=reset "$out/$instruction.o" -o "$out/$instruction.elf"
    "$penv" run "$@" --max-steps 100 "$out/$instruction.elf" \
        >"$out/$instruction.log" 2>&1
    printf '%s: all fault checks passed\n' "$instruction"
done

# The same ELF without the opt-in must fail at the unmapped instruction.
status=0
"$penv" run --platform "$out/bus-fault.repl" --max-steps 100 "$out/unmapped.elf" \
    >"$out/unmapped-default.log" 2>&1 || status=$?
[ "$status" -eq 1 ]
grep -Fq 'pc=0x00000200: unmapped RAM access at 0x00200000 (4 bytes)' "$out/unmapped-default.log"

# Robot 352-368 and 1212-1228. In both modes the first handler access
# repeats the original fault, with the original side-effect sentinel in R1.
for name in nested-handler ignored-in-hardfault; do
    case "$name" in
        nested-handler) register=0xe000ed24; value=0x20000 ;;
        ignored-in-hardfault) register=0xe000ed14; value=0x100 ;;
    esac
    cat >"$out/$name.s" <<ASM
.syntax unified
.thumb
.global reset
.word 0x1000, reset, 0, hardfault, 0, busfault
.thumb_func
reset:
    ldr r3, =$register
    ldr r4, =$value
    str r4, [r3]
    ldr r0, =0x100000
    movs r1, #0x11
    ldr r2, =0xa5a5a5a5
    b faulting
    .ltorg
    .org 0x200
faulting:
    ldr r2, [r0]
    movs r1, #0x22
    b .
    .org 0x300
.thumb_func
busfault:
    ldr r2, [r0]
    movs r1, #0x22
    b .
    .org 0x340
.thumb_func
hardfault:
ASM
    if [ "$name" = ignored-in-hardfault ]; then
        cat >>"$out/$name.s" <<'ASM'
    ldr r2, [r0]
    movs r1, #0x22
    cmp r1, #0x22
    bne failed
    ldr r4, [sp, #4]
    cmp r4, #0x11
    bne failed
ASM
    else
        cat >>"$out/$name.s" <<'ASM'
    cmp r1, #0x11
    bne failed
    ldr r3, =0xa5a5a5a5
    cmp r2, r3
    bne failed
    ldr r3, =0xfd8
    ldr r4, [r3]
    cmp.w r4, #0x300
    bne failed
ASM
    fi
    cat >>"$out/$name.s" <<'ASM'
    ldr r3, =0xe000ed28
    ldr r4, [r3]
    movw r5, #0x8200
    cmp r4, r5
    bne failed
    ldr r4, [r3, #4]
    cmp.w r4, #0x40000000
    bne failed
    ldr r4, [r3, #16]
    cmp r4, r0
    bne failed
    bkpt 0
failed:
    b .
ASM
    clang --target=arm-none-eabi -mcpu=cortex-m4 -c "$out/$name.s" -o "$out/$name.o"
    ld.lld -N -Ttext=0 --entry=reset "$out/$name.o" -o "$out/$name.elf"
    "$penv" run --platform "$out/bus-fault.repl" --max-steps 100 "$out/$name.elf" \
        >"$out/$name.log" 2>&1
    printf '%s: all fault checks passed\n' "$name"
done

# Robot 369-423. Monitor state and NMI injection are checked by cargo test.
cat >"$out/lockup.s" <<'ASM'
.syntax unified
.thumb
.global reset
.word 0x1000, reset, nmi, hardfault
.thumb_func
reset:
    ldr r0, =0x100000
    movs r1, #0x11
    ldr r2, =0xa5a5a5a5
    b fault
    .ltorg
    .org 0x200
fault:
    ldr r2, [r0]
    movs r1, #0x22
    b .
    .org 0x340
.thumb_func
hardfault:
    ldr r3, =0xe000ed2c
    ldr r4, =0x40000000
    str r4, [r3]
    cmp r5, #0
    itt eq
    ldreq r2, [r0]
    moveq r1, #0x22
    b .
    .ltorg
    .org 0x380
.thumb_func
nmi:
    b .
ASM
clang --target=arm-none-eabi -mcpu=cortex-m4 -c "$out/lockup.s" -o "$out/lockup.o"
ld.lld -N -Ttext=0 --entry=reset "$out/lockup.o" -o "$out/lockup.elf"
status=0
"$penv" run --platform "$out/bus-fault.repl" --max-steps 30 "$out/lockup.elf" \
    >"$out/lockup.log" 2>&1 || status=$?
[ "$status" -eq 2 ]
grep -Fxq 'penv: instruction limit reached at pc=0xeffffffe' "$out/lockup.log"
