#!/bin/sh
# bus-isolation.robot:204-206, without TrustZone.
set -eu
[ "$#" -eq 2 ] || exit 1
penv=$1
out=$2
mkdir -p "$out"
cat >"$out/conditional-vtor.repl" <<'REPL'
cpu: CPU.CortexM @ sysbus
    cpuType: "cortex-m4"
    enableTrustZone: false
    vectorTableOffset: 0x10000
    nvic: nvic
nvic: IRQControllers.NVIC @ sysbus 0xe000e000
    -> cpu@0
mem: Memory.ArrayMemory @ sysbus new Bus.BusPointRegistration { address: 0x10000; condition: "privileged && cpuSecure && attributionSecure" } { size: 0x1000 }
otherMem: Memory.MappedMemory @ sysbus 0x20000 { size: 0x1000 }
REPL
cat >"$out/conditional-vtor.s" <<'ASM'
.syntax unified
.thumb
.section .vectors, "a"
.word 0x20100, reset
.text
.global reset
.thumb_func
reset:
    b .
ASM
cat >"$out/conditional-vtor.ld" <<'LD'
PHDRS {
    vectors PT_LOAD FLAGS(4);
    program PT_LOAD FLAGS(5);
}
SECTIONS {
    .vectors 0x10000 : { *(.vectors) } :vectors
    .text 0x20200 : { *(.text) } :program
}
LD
clang --target=arm-none-eabi -mcpu=cortex-m4 -c "$out/conditional-vtor.s" -o "$out/conditional-vtor.o"
ld.lld -T "$out/conditional-vtor.ld" --entry=reset "$out/conditional-vtor.o" -o "$out/conditional-vtor.elf"
status=0
"$penv" run --platform "$out/conditional-vtor.repl" --max-steps 1 "$out/conditional-vtor.elf"     >"$out/run.log" 2>&1 || status=$?
[ "$status" -eq 2 ]
grep -Fxq 'penv: instruction limit reached at pc=0x00020200' "$out/run.log"
cat "$out/run.log"
