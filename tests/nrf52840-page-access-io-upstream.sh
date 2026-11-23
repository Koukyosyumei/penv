#!/bin/sh
# CLI wiring smoke test for page-access-via-io.robot:76-125.
set -eu
[ "$#" -eq 2 ] || exit 1
penv=$1
out=$2
mkdir -p "$out"
cat >"$out/board.repl" <<'REPL'
cpu: CPU.CortexM @ sysbus
    cpuType: "cortex-m4"
    nvic: nvic
    vectorTableOffset: 0x800100
nvic: IRQControllers.NVIC @ sysbus 0xe000e000
    -> cpu@0
ram: Memory.MappedMemory @ sysbus 0x800000 { size: 0x40000 }
test_mem1: Memory.MappedMemory @ sysbus 0x31000000 { size: 0x1000 }
test_mem2: Memory.MappedMemory @ sysbus 0x32000000 { size: 0x1000 }
test_mem3: Memory.MappedMemory @ sysbus 0x33000000 { size: 0x1000 }
REPL
cat >"$out/pages.s" <<'ASM'
.syntax unified
.thumb
.global reset
read_page:
    ldr r0, [r1]
    bx lr
.thumb_func
reset:
    ldr r1, =0x31000000
    bl read_page
    ldr r1, =0x32000000
    bl read_page
    ldr r1, =0x33000000
    bl read_page
    bkpt 0
    .ltorg
.org 0x100
.word 0x840000, reset
ASM
clang --target=arm-none-eabi -mcpu=cortex-m4 -c "$out/pages.s" -o "$out/pages.o"
ld.lld -N -Ttext=0x800000 --entry=reset "$out/pages.o" -o "$out/pages.elf"
cat >"$out/pages.resc" <<'RESC'
mach create
machine LoadPlatformDescription @board.repl
sysbus LoadELF @pages.elf
cpu PC 0x800004
sysbus LogPeripheralAccess sysbus.test_mem1
sysbus LogPeripheralAccess sysbus.test_mem2
sysbus LogPeripheralAccess sysbus.test_mem3
sysbus SetPageAccessViaIo 0x32000000
sysbus SetPageAccessViaIo 0x31000000
sysbus ClearPageAccessViaIo 0x32000000
sysbus SetPageAccessViaIo 0x33000000
RESC
"$penv" run --script "$out/pages.resc" --max-steps 100 >"$out/pages.log" 2>"$out/pages.stderr"
cat >"$out/pages.expected" <<'LOG'
test_mem1: [cpu: 0x800000] ReadUInt32 from 0x0 (unknown), returned 0x0
test_mem3: [cpu: 0x800000] ReadUInt32 from 0x0 (unknown), returned 0x0
LOG
cmp "$out/pages.expected" "$out/pages.log"
grep -Fxq "cpu: Patching PC 0x800004 for Thumb mode." "$out/pages.stderr"
cat "$out/pages.log"
