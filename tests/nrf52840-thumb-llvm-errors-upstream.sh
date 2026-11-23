#!/bin/sh
# Shared disassembly error cases in llvm-asm-disasm.robot:501-515.
set -eu
[ "$#" -eq 2 ] || exit 1
penv=$1
out=$2
mkdir -p "$out"
cat >"$out/board.repl" <<'REPL'
cpu: CPU.CortexM @ sysbus
    cpuType: "cortex-m4"
    nvic: nvic
nvic: IRQControllers.NVIC @ sysbus 0xe000e000
    -> cpu@0
mem: Memory.MappedMemory @ sysbus 0
    size: 0x2000
REPL
cat >"$out/memory.s" <<'ASM'
.syntax unified
.thumb
.org 0x20
.hword 0xb700
.org 0x24
.word 0xffffffff
.org 0x1234
.global instruction
.thumb_func
instruction:
    movw r2, #0x1234
ASM
clang --target=arm-none-eabi -mcpu=cortex-m4 -c "$out/memory.s" -o "$out/memory.o"
ld.lld -N -Ttext=0 --entry=instruction "$out/memory.o" -o "$out/memory.elf"

decode() {
    "$penv" thumb-disassemble-memory "$out/board.repl" "$out/memory.elf" "$@"
}
expect_error() {
    label=$1
    address=$2
    width=$3
    diagnostic=$4
    status=0
    decode "$address" "$width" >"$out/$label.stdout" 2>"$out/$label.stderr" || status=$?
    [ "$status" -eq 1 ]
    [ ! -s "$out/$label.stdout" ]
    grep -Fq 'disassembly error detected:' "$out/$label.stderr"
    grep -Fq "$diagnostic" "$out/$label.stderr"
    cat "$out/$label.stderr"
}

decoded=$(decode 0x1234 4)
[ "$decoded" = '0x00001234: f2412234 movw r2, #4660' ]
printf '%s\n' "$decoded"
expect_error outside 0x02000000 4 'unmapped RAM access at 0x02000000'
expect_error invalid16 0x20 2 'invalid instruction encoding'
expect_error invalid32 0x24 4 'invalid instruction encoding'
