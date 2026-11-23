#!/bin/sh
set -eu
[ "$#" -eq 2 ] || { echo "Usage: $0 PENV ARTIFACT_DIR" >&2; exit 1; }
penv=$1
out=$2
mkdir -p "$out"
cat > "$out/board.repl" <<'REPL'
cpu: CPU.CortexM @ sysbus { cpuType: "cortex-m4"; nvic: nvic }
nvic: IRQControllers.NVIC @ sysbus 0xe000e000 { -> cpu@0 }
mem: Memory.MappedMemory @ sysbus 0 { size: 0x1000 }
dwt: Miscellaneous.DWT @ sysbus 0xe0001000 { frequency: 72000000 }
REPL
cat > "$out/clock.s" <<'ASM'
.syntax unified
.thumb
.word 0x1000, reset
.org 0x100
.global reset
.thumb_func
reset:
    ldr r0, =0xe0001000
    movs r1, #1
    str r1, [r0]
    ldr r2, [r0, #4]
    nop
    nop
    ldr r3, [r0, #4]
    cmp r2, r3
    beq frozen
    b running
.ltorg
.org 0x140
frozen: b frozen
.org 0x150
running: b running
ASM
clang --target=arm-none-eabi -mcpu=cortex-m4 -c "$out/clock.s" -o "$out/clock.o"
ld.lld -N -Ttext=0 --entry=reset "$out/clock.o" -o "$out/clock.elf"
for clocked in True False; do
    cat > "$out/clock-$clocked.resc" <<RESC
mach create
machine LoadPlatformDescription @board.repl
sysbus LoadELF @clock.elf
dwt Clocked $clocked
cpu AddLogHook 0x140 "frozen"
cpu AddLogHook 0x150 "running"
cpu AddPauseHook 0x140
cpu AddPauseHook 0x150
RESC
    status=0
    "$penv" run --script "$out/clock-$clocked.resc" --max-steps 40 > "$out/clock-$clocked.log" 2>&1 || status=$?
    printf '%s\n' "$status" > "$out/clock-$clocked.status"
    [ "$status" -eq 0 ]
    case "$clocked" in
        True) expected=running ;;
        False) expected=frozen ;;
    esac
    [ "$(sed -n "1p" "$out/clock-$clocked.log")" = "$expected" ]
    printf '%s: %s\n' "$clocked" "$expected"
done
