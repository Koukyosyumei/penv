#!/bin/sh
# bus-performance.robot:43-47. Run its full functional input without timing it.
set -eu
[ "$#" -eq 3 ] || exit 1
penv=$1
robot=$2/tests/unit-tests/bus-performance.robot
out=$3
mkdir -p "$out"
cat >"$out/board.repl" <<'REPL'
cpu: CPU.CortexM @ sysbus
    cpuType: "cortex-m4"
    nvic: nvic
    vectorTableOffset: 0x100
nvic: IRQControllers.NVIC @ sysbus 0xe000e000
    -> cpu@0
rom: Memory.MappedMemory @ sysbus 0
    size: 0x40000
trivial: Mocks.TrivialPeripheral @ sysbus 0x10000000
REPL
cat >"$out/trivial.resc" <<'RESC'
mach create
machine LoadPlatformDescription @board.repl
sysbus LoadELF @trivial.elf
sysbus UnhandledAccessBehaviour Report
RESC
{
    printf '.syntax unified\n.thumb\n.global reset\n.thumb_func\nreset:\n'
    awk '/^\$\{program\}/ { inside=1; next } inside && /^\.\.\./ { sub(/^\.\.\.[[:space:]]*/,  ""); print; next } inside { exit }' "$robot"
    printf '\n.ltorg\n.org 0x100\n.word 0x40000, reset\n'
} >"$out/trivial.s"
grep -Fq 'ldr r1, =10000000' "$out/trivial.s"
clang --target=arm-none-eabi -mcpu=cortex-m4 -c "$out/trivial.s" -o "$out/trivial.o"
ld.lld -N -Ttext=0 --entry=reset "$out/trivial.o" -o "$out/trivial.elf"

status=0
"$penv" run --script "$out/trivial.resc" --max-steps 30000005 \
    >"$out/run.log" 2>&1 || status=$?
[ "$status" -eq 2 ]
[ "$(grep -Fc 'non existing peripheral' "$out/run.log")" -eq 1 ]
grep -Fxq 'sysbus: ReadDoubleWord from non existing peripheral at 0xFFFFFFC' "$out/run.log"
cat "$out/run.log"
