#!/bin/sh
# Guest-side checks for gdb.robot:478-498 and 645-694.
set -eu
if [ "$#" -ne 2 ]; then
    echo "Usage: $0 PENV ARTIFACT_DIR" >&2
    exit 1
fi
penv=$1
out=$2
mkdir -p "$out"
cat > "$out/board.repl" <<'REPL'
cpu: CPU.CortexM @ sysbus { cpuType: "cortex-m4"; nvic: nvic; vectorTableOffset: 0x200 }
nvic: IRQControllers.NVIC @ sysbus 0xe000e000 { -> cpu@0 }
mem: Memory.MappedMemory @ sysbus 0 { size: 0x1000 }
REPL
cat > "$out/watch.s" <<'ASM'
.syntax unified
.thumb
.global reset
.org 0x10
.thumb_func
reset:
    movw r0, #1
    movw r1, #0x100
    str.w r0, [r1]
    movw r0, #4
    str.w r0, [r1]
    movw r0, #1
    movw r1, #0x104
    str.w r0, [r1]
    wfi
.org 0x200
.word 0x1000, reset
ASM
clang --target=arm-none-eabi -mcpu=cortex-m4 -c "$out/watch.s" -o "$out/watch.o"
ld.lld -N -Ttext=0 --entry=reset "$out/watch.o" -o "$out/watch.elf"
for case in single multiple; do
    cat > "$out/$case.resc" <<'RESC'
mach create
machine LoadPlatformDescription @board.repl
sysbus LoadELF @watch.elf
cpu MaximumBlockSize 1
sysbus ReadDoubleWord 0x100
cpu Step 3
sysbus ReadDoubleWord 0x100
RESC
    case "$case" in
        single)
            printf '%s\n' 0x00000000 0x00000001 > "$out/$case.expected"
            pc=0x0000001c
            ;;
        multiple)
            cat >> "$out/$case.resc" <<'RESC'
cpu AddLogHook 0x1c "resume at 0x1c"
cpu AddLogHook 0x24 "resume at 0x24"
cpu Step 2
sysbus ReadDoubleWord 0x100
sysbus ReadDoubleWord 0x104
cpu Step 3
sysbus ReadDoubleWord 0x104
RESC
            printf '%s\n' 0x00000000 0x00000001 'resume at 0x1c' 0x00000004 0x00000000 'resume at 0x24' 0x00000001 > "$out/$case.expected"
            pc=0x00000030
            ;;
    esac
    status=0
    "$penv" run --script "$out/$case.resc" --max-steps 100         > "$out/$case.log" 2> "$out/$case.stderr" || status=$?
    printf '%s\n' "$status" > "$out/$case.status"
    [ "$status" -eq 0 ] || { cat "$out/$case.stderr" >&2; exit 1; }
    diff -u "$out/$case.expected" "$out/$case.log"
    grep -Fxq "penv: machine paused at pc=$pc" "$out/$case.stderr"
    printf '%s: PASS (guest writes)\n' "$case"
done
