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

# Robot 1042-1073. Start with VTOR pointing at the faulting peripheral.
for offset in 4 0; do
    sed -e 's/^    vectorTableOffset: 0$/    vectorTableOffset: 0x100000/' \
        -e "/^faultingPeripheral:/a\    FaultOffset: $offset" \
        "$out/bus-fault.repl" >"$out/reset-fault-$offset.repl"
    status=0
    "$penv" run --platform "$out/reset-fault-$offset.repl" --max-steps 10 "$out/lockup.elf" \
        >"$out/reset-fault-$offset.log" 2>&1 || status=$?
    [ "$status" -eq 2 ]
    grep -Fxq 'penv: instruction limit reached at pc=0xeffffffe' "$out/reset-fault-$offset.log"
    printf 'reset vector fault offset %s: lockup reached\n' "$offset"
done

# Exercise the monitor command with a readable HardFault vector. A startup
# pend enters the handler before the fixture's reset code sets up a fault.
cat >"$out/pending-hardfault.resc" <<'RESC'
mach create
machine LoadPlatformDescription @bus-fault.repl
sysbus LoadELF @lockup.elf
nvic SetPendingIRQ 3
RESC
status=0
"$penv" run --script "$out/pending-hardfault.resc" --max-steps 15 \
    >"$out/pending-hardfault.log" 2>&1 || status=$?
[ "$status" -eq 2 ]
grep -Fxq 'penv: instruction limit reached at pc=0x00000350' "$out/pending-hardfault.log"

# Robot 444-491: IRQ16 stays pending when stacking selects a derived fault.
for selection in hardfault busfault; do
    case "$selection" in
        hardfault) hard=check; bus=failed; enables=0; forced=0x40000000 ;;
        busfault) hard=failed; bus=check; enables=0x20000; forced=0 ;;
    esac
    cat >"$out/stacking-$selection.s" <<ASM
.syntax unified
.thumb
.global reset
.word 0x100020, reset, 0, $hard, 0, $bus
.org 0x40
.word failed
.thumb_func
reset:
    ldr r0, =0xe000ed24
    ldr r1, =$enables
    str r1, [r0]
    ldr r0, =0xe000e100
    movs r1, #1
    str r1, [r0]
    str r1, [r0, #0x100]
    b .
.thumb_func
check:
    mov r0, sp
    cmp.w r0, #0x100000
    bne failed
    ldr r0, =0xe000ed28
    ldr r1, [r0]
    cmp.w r1, #0x1000
    bne failed
    ldr r1, [r0, #4]
    cmp.w r1, #$forced
    bne failed
    ldr r0, =0xe000e200
    ldr r1, [r0]
    tst r1, #1
    beq failed
    ldr r1, [r0, #0x100]
    tst r1, #1
    bne failed
    bkpt 0
.thumb_func
failed:
    b .
ASM
    clang --target=arm-none-eabi -mcpu=cortex-m4 -c "$out/stacking-$selection.s" -o "$out/stacking-$selection.o"
    ld.lld -N -Ttext=0 --entry=reset "$out/stacking-$selection.o" -o "$out/stacking-$selection.elf"
    "$penv" run --platform "$out/bus-fault.repl" --max-steps 100 "$out/stacking-$selection.elf" \
        >"$out/stacking-$selection.log" 2>&1
    printf 'stacking fault: %s selected\n' "$selection"
done

# Robot 544-575: a real ELF drives the stacking fault by pending NMI
# after the HardFault handler redirects MSP into the faulting peripheral.
cat >"$out/nmi-stacking-lockup.s" <<'ASM'
.syntax unified
.thumb
.global reset
.word 0x1000, reset, nmi, hardfault
.thumb_func
reset:
    ldr r0, =0x100000
    ldr r2, [r0]
    b .
.thumb_func
hardfault:
    ldr r0, =0xe000ed28
    mov.w r1, #-1
    str r1, [r0]
    mov.w r1, #0x40000000
    str r1, [r0, #4]
    ldr r0, =0x100020
    msr msp, r0
    ldr r0, =0xe000ed04
    mov.w r1, #0x80000000
    str r1, [r0]
    b .
.thumb_func
nmi:
    b .
ASM
clang --target=arm-none-eabi -mcpu=cortex-m4 -c "$out/nmi-stacking-lockup.s" -o "$out/nmi-stacking-lockup.o"
ld.lld -N -Ttext=0 --entry=reset "$out/nmi-stacking-lockup.o" -o "$out/nmi-stacking-lockup.elf"
status=0
"$penv" run --platform "$out/bus-fault.repl" --max-steps 100 "$out/nmi-stacking-lockup.elf" \
    >"$out/nmi-stacking-lockup.log" 2>&1 || status=$?
[ "$status" -eq 2 ]
grep -Fxq 'penv: instruction limit reached at pc=0xeffffffe' "$out/nmi-stacking-lockup.log"
