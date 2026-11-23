#!/bin/sh
# arm-cortex-m Thumb cases in opcodes-counting.robot:179-213.
set -eu
[ "$#" -eq 2 ] || exit 1
penv=$1
out=$2
mkdir -p "$out"
cat >"$out/board.repl" <<'REPL'
cpu: CPU.CortexM @ sysbus
    cpuType: "cortex-m4"
    nvic: nvic
    vectorTableOffset: 0
nvic: IRQControllers.NVIC @ sysbus 0xe000e000
    -> cpu@0
mem: Memory.MappedMemory @ sysbus 0
    size: 0x1000
REPL

for name in movw nop; do
    case "$name" in
        movw) instruction='movw r0, #0x1234'; pattern=11110010010000010010000000110100; pc=0x00000104 ;;
        nop) instruction='nop; nop'; pattern=00000000000000001011111100000000; pc=0x00000102 ;;
    esac
    cat >"$out/$name.s" <<ASM
.syntax unified
.thumb
.global reset
.word 0x1000, reset
.org 0x100
.thumb_func
reset:
    $instruction
ASM
    cat >"$out/$name.resc" <<RESC
mach create
machine LoadPlatformDescription @board.repl
sysbus LoadELF @$name.elf
cpu InstallOpcodeCounterPattern "$name" "$pattern"
cpu InstallOpcodeCounterPattern "unexpected" "11110010010000010010000000110101"
cpu EnableOpcodesCounting true
cpu GetOpcodeCounter "$name"
cpu GetOpcodeCounter "unexpected"
RESC
    clang --target=arm-none-eabi -mcpu=cortex-m4 -c "$out/$name.s" -o "$out/$name.o"
    ld.lld -N -Ttext=0 --entry=reset "$out/$name.o" -o "$out/$name.elf"
    status=0
    "$penv" run --script "$out/$name.resc" --max-steps 1 >"$out/$name.log" 2>&1 || status=$?
    [ "$status" -eq 2 ]
    grep -Fxq "opcode $name: 1" "$out/$name.log"
    grep -Fxq 'opcode unexpected: 0' "$out/$name.log"
    grep -Fxq "penv: instruction limit reached at pc=$pc" "$out/$name.log"
done
