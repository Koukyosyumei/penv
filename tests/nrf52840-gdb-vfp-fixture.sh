#!/bin/sh
# ArmVfpGDB.robot:58-71 Cortex-M4F fixture. Verifies guest state at the GDB breakpoint.
set -eu
if [ "$#" -ne 2 ]; then
    echo "Usage: $0 PENV ARTIFACT_DIR" >&2
    exit 1
fi
penv=$1
out=$2
mkdir -p "$out"
cat > "$out/vfp.repl" <<'REPL'
cpu: CPU.CortexM @ sysbus { cpuType: "cortex-m4f"; nvic: nvic }
nvic: IRQControllers.NVIC @ sysbus 0xe000e000 { -> cpu@0 }
mem: Memory.MappedMemory @ sysbus 0 { size: 0x4000 }
ram: Memory.MappedMemory @ sysbus 0x20000000 { size: 0x40000 }
uart: UART.TrivialUart @ sysbus 0x80000
REPL
cat > "$out/vfp.s" <<'ASM'
.syntax unified
.thumb
.fpu fpv4-sp-d16
.global reset
.word 0x20040000, reset
.org 0x100
.thumb_func
reset:
    ldr r0, =0xe000ed88
    ldr r1, [r0]
    orr r1, r1, #0xf00000
    str r1, [r0]
    isb
    ldr r0, =values
    vldmia r0, {s0-s31}
    b check_registers
.ltorg

.org 0x13a4
check_registers:
    ldr r0, =0x20000000
    vstmia r0, {s0-s31}
    ldr r1, =values
    movs r2, #32
compare:
    ldr r3, [r0], #4
    ldr r4, [r1], #4
    cmp r3, r4
    bne failed
    subs r2, #1
    bne compare
    ldr r0, =success
    b print
failed:
    ldr r0, =failure
print:
    ldr r1, =0x80000
next:
    ldrb r2, [r0], #1
    cbz r2, done
    strb r2, [r1]
    b next
done:
    bkpt 0
.ltorg
success: .asciz "VFP S0-S31 OK\n"
failure: .asciz "VFP S0-S31 FAIL\n"
.align 2
values:
ASM
value=100
while [ "$value" -lt 132 ]; do
    printf '    .float %s.0\n' "$value" >> "$out/vfp.s"
    value=$((value + 1))
done
clang --target=arm-none-eabi -mcpu=cortex-m4 -c "$out/vfp.s" -o "$out/vfp.o"
ld.lld -N -Ttext=0 --entry=reset "$out/vfp.o" -o "$out/vfp.elf"
cat > "$out/vfp.resc" <<'RESC'
mach create
machine LoadPlatformDescription @vfp.repl
sysbus LoadELF @vfp.elf
cpu AddLogHook 0x13a4 "VFP breakpoint address reached"
RESC
status=0
"$penv" run --script "$out/vfp.resc" --max-steps 1000 > "$out/vfp.log" 2> "$out/vfp.stderr" || status=$?
printf '%s\n' "$status" > "$out/vfp.status"
[ "$status" -eq 0 ] || { cat "$out/vfp.stderr" >&2; exit 1; }
printf '%s\n' 'VFP breakpoint address reached' 'VFP S0-S31 OK' > "$out/vfp.expected"
diff -u "$out/vfp.expected" "$out/vfp.log"
cat "$out/vfp.log"
