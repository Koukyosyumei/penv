#!/bin/sh
# arm-cortex-m-mve.robot:1433-1448, using the shared Thumb-2 encoding.
set -eu
[ "$#" -eq 2 ] || exit 1
penv=$1
out=$2
mkdir -p "$out"
cat >"$out/board.repl" <<'REPL'
cpu: CPU.CortexM @ sysbus
    cpuType: "cortex-m4"
    vectorTableOffset: 0
mem: Memory.MappedMemory @ sysbus 0
    size: 0x1000
REPL
cat >"$out/mov-sp.s" <<'ASM'
.syntax unified
.thumb
.global reset
.word 0xfeedc0d0, reset
.thumb_func
reset:
    ldr r1, =0xbadcaff0
    mov.w r0, sp
    mov.w sp, r1
    ldr r2, =0xfeedc0d0
    cmp r0, r2
    bne fail
    cmp sp, r1
    bne fail
    bkpt 0
fail:
    b .
ASM
clang --target=arm-none-eabi -mcpu=cortex-m4 -c "$out/mov-sp.s" -o "$out/mov-sp.o"
ld.lld -N -Ttext=0 --entry=reset "$out/mov-sp.o" -o "$out/mov-sp.elf"
"$penv" run --platform "$out/board.repl" --max-steps 50 "$out/mov-sp.elf" >"$out/run.log" 2>&1
printf '%s\n' 'MOV.W preserved both expected register values'
