#!/bin/sh
# it-status.robot:277-320, following its single-step block boundaries.
set -eu
[ "$#" -eq 3 ] || exit 1
penv=$1
robot=$2/tests/unit-tests/it-status.robot
out=$3
mkdir -p "$out"
cat >"$out/board.repl" <<'REPL'
cpu: CPU.CortexM @ sysbus { cpuType: "cortex-m4"; nvic: nvic; vectorTableOffset: 0 }
nvic: IRQControllers.NVIC @ sysbus 0xe000e000 { -> cpu@0 }
mem: Memory.MappedMemory @ sysbus 0 { size: 0x1000 }
REPL
{
    printf '.syntax unified\n.thumb\n.global reset\n.word 0x800, reset\n.org 0x100\n.thumb_func\nreset:\n'
    awk '/^\$\{ITSTATE_ASM\}/ { inside=1; next } inside && /^\.\.\./ { sub(/^\.\.\.[[:space:]]*/, ""); if ($0 != "\"\"\"") print; next } inside { exit }' "$robot"
} >"$out/itstate.s"
clang --target=arm-none-eabi -mcpu=cortex-m4 -c "$out/itstate.s" -o "$out/itstate.o"
ld.lld -N -Ttext=0 --entry=reset "$out/itstate.o" -o "$out/itstate.elf"
cat >"$out/end.expected" <<'LOG'
PC 0x114;IT_state 0x5
PC 0x116;IT_state 0xa
PC 0x118;IT_state 0x14
PC 0x11a;IT_state 0x8
Checking IT_STATE, while not in IT block
PC 0x11c;IT_state 0x0
LOG
{
    printf '%s\n' 'Checking IT_STATE, while not in IT block' 'PC 0x112;IT_state 0x0'
    cat "$out/end.expected"
} >"$out/begin.expected"
for phase in Begin End; do
    case "$phase" in
        Begin) label=begin; steps=9; lines=8 ;;
        End) label=end; steps=8; lines=6 ;;
    esac
    cat >"$out/$label.resc" <<RESC
mach create
machine LoadPlatformDescription @board.repl
sysbus LoadELF @itstate.elf
cpu SetSingleStepItStateLog $phase
RESC
    status=0
    "$penv" run --script "$out/$label.resc" --max-steps "$steps" >"$out/$label.log" 2>"$out/$label.stderr" || status=$?
    [ "$status" -eq 2 ]
    tail -n "$lines" "$out/$label.log" >"$out/$label.observed"
    cmp "$out/$label.expected" "$out/$label.observed"
    cat "$out/$label.observed"
done
