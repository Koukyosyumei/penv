#!/bin/sh
# hooks.robot:53-81. Fixed Cortex-M0 and zero-filled instruction memory.
set -eu
[ "$#" -eq 2 ] || exit 1
penv=$1
out=$2
mkdir -p "$out"
cat >"$out/board.repl" <<'REPL'
cpu: CPU.CortexM @ sysbus { cpuType: "cortex-m0"; nvic: nvic; vectorTableOffset: 0x100 }
nvic: IRQControllers.NVIC @ sysbus 0xe000e000 { -> cpu@0 }
mem: Memory.MappedMemory @ sysbus 0 { size: 0x10000 }
REPL
cat >"$out/zero.s" <<'ASM'
.syntax unified
.thumb
.global reset
.thumb_func
reset:
.space 0x100
.word 0x10000, reset
ASM
clang --target=arm-none-eabi -mcpu=cortex-m0 -c "$out/zero.s" -o "$out/zero.o"
ld.lld -N -Ttext=0 --entry=reset "$out/zero.o" -o "$out/zero.elf"
cat >"$out/hooks.resc" <<'RESC'
mach create
machine LoadPlatformDescription @board.repl
sysbus LoadELF @zero.elf
cpu PC 0x0
cpu AddWrite32Hook 0x10 0xe000e010 0x1
cpu AddPauseHook 0x20
RESC
"$penv" run --script "$out/hooks.resc" --max-steps 64 >"$out/run.log" 2>"$out/run.stderr"
cat >"$out/expected.log" <<'LOG'
Systick_NS enabled but it won't be started as long as the reload value is zero
Machine paused
LOG
cmp "$out/expected.log" "$out/run.log"
grep -Fxq 'penv: machine paused at pc=0x00000020' "$out/run.stderr"
cat "$out/run.log"
