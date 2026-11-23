#!/bin/sh
# cpu-pause-halt.robot:127-220, Cortex-M4 port of the fixed Thumb loop.
set -eu
[ "$#" -eq 2 ] || exit 1
penv=$1
out=$2
mkdir -p "$out"
cat >"$out/board.repl" <<'REPL'
cpu: CPU.CortexM @ sysbus { cpuType: "cortex-m4"; nvic: nvic; vectorTableOffset: 0 }
nvic: IRQControllers.NVIC @ sysbus 0xe000e000 { -> cpu@0 }
mem: Memory.MappedMemory @ sysbus 0 { size: 0x1000 }
REPL
cat >"$out/control.s" <<'ASM'
.syntax unified
.thumb
.global reset
.word 0x800, reset
.org 0x100
.thumb_func
reset:
    nop
    nop
    nop
jump:
    nop
    b jump
ASM
clang --target=arm-none-eabi -mcpu=cortex-m4 -c "$out/control.s" -o "$out/control.o"
ld.lld -N -Ttext=0 --entry=reset "$out/control.o" -o "$out/control.elf"
for scenario in start paused halted; do
    cat >"$out/$scenario.resc" <<'RESC'
mach create
machine LoadPlatformDescription @board.repl
sysbus LoadELF @control.elf
cpu AddLogHook 0x104 "PC moved"
RESC
    case "$scenario" in
        paused) echo 'cpu Pause' >>"$out/$scenario.resc"; expected=0 ;;
        halted) echo 'cpu IsHalted true' >>"$out/$scenario.resc"; expected=2 ;;
        *) expected=2 ;;
    esac
    status=0
    "$penv" run --script "$out/$scenario.resc" --max-steps 8 >"$out/$scenario.log" 2>"$out/$scenario.stderr" || status=$?
    [ "$status" -eq "$expected" ]
    case "$scenario" in
        start) grep -Fxq 'PC moved' "$out/$scenario.log" ;;
        paused) [ ! -s "$out/$scenario.log" ]; grep -Fxq 'penv: machine paused at pc=0x00000100' "$out/$scenario.stderr" ;;
        halted) [ ! -s "$out/$scenario.log" ]; grep -Fxq 'penv: instruction limit reached at pc=0x00000100' "$out/$scenario.stderr" ;;
    esac
    printf '%s: expected CPU state observed\n' "$scenario"
done

# Robot 221-267 and 291-337. Keep CPU-control command order in the RESC.
for state in running halted paused halted-paused; do
    for kind in cpu machine; do
        name=$state-reset-$kind
        cat >"$out/$name.resc" <<'RESC'
mach create
machine LoadPlatformDescription @board.repl
sysbus LoadELF @control.elf
cpu AddLogHook 0x104 "PC moved"
machine SetResetLog "Machine resetting"
RESC
        case "$state" in
            paused|halted-paused) echo 'cpu Pause' >>"$out/$name.resc" ;;
        esac
        case "$state" in
            halted|halted-paused) echo 'cpu IsHalted true' >>"$out/$name.resc" ;;
        esac
        echo "$kind Reset" >>"$out/$name.resc"
        status=0
        "$penv" run --script "$out/$name.resc" --max-steps 8 >"$out/$name.log" 2>"$out/$name.stderr" || status=$?
        if [ "$kind" = machine ]; then
            [ "$status" -eq 2 ]
            printf '%s\n' 'Machine resetting' 'PC moved' >"$out/reset.expected"
            cmp "$out/reset.expected" "$out/$name.log"
        else
            [ "$status" -eq 0 ]
            [ ! -s "$out/$name.log" ]
            grep -Fxq 'penv: machine paused at pc=0x00000100' "$out/$name.stderr"
        fi
        printf '%s: expected reset state observed\n' "$name"
    done
done
