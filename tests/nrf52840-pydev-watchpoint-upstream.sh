#!/bin/sh
set -eu
[ "$#" -eq 3 ] || exit 1
penv=$1
root=$2
out=$3
mkdir -p "$out"
cat > "$out/board.repl" <<'REPL'
cpu: CPU.CortexM @ sysbus { cpuType: "cortex-m4"; nvic: nvic }
nvic: IRQControllers.NVIC @ sysbus 0xe000e000 { -> cpu@0 }
mem: Memory.MappedMemory @ sysbus 0 { size: 0x1000 }
ctr: Python.PythonPeripheral @ sysbus 0xf0000004
    size: 4
    initable: true
    filename: "scripts/pydev/counter.py"
REPL
cat > "$out/watch.s" <<'ASM'
.syntax unified
.thumb
.word 0x1000, reset
.org 0x10
loop:
    ldrb r0, [r1]
    b loop
.org 0x20
.global reset
.thumb_func
reset:
    ldr r1, =0xf0000004
    b loop
ASM
clang --target=arm-none-eabi -mcpu=cortex-m4 -c "$out/watch.s" -o "$out/watch.o"
ld.lld -N -Ttext=0 --entry=reset "$out/watch.o" -o "$out/watch.elf"
cat > "$out/watch.resc" <<'RESC'
mach create
machine LoadPlatformDescription @board.repl
sysbus LoadELF @watch.elf
sysbus ReadDoubleWord 0xf0000004
sysbus ReadDoubleWord 0xf0000004
cpu Step 2
sysbus AddWatchpointHook 0xf0000004 1 Read ""
sysbus ReadDoubleWord 0xf0000004
cpu Step 2
sysbus ReadDoubleWord 0xf0000004
cpu Step 2
sysbus ReadDoubleWord 0xf0000004
cpu Step 2
sysbus ReadDoubleWord 0xf0000004
RESC
status=0
"$penv" run --script "$out/watch.resc" --platform-root "$root" > "$out/watch.log" 2> "$out/watch.stderr" || status=$?
printf '%s\n' "$status" > "$out/watch.status"
[ "$status" -eq 0 ] || { cat "$out/watch.stderr" >&2; exit 1; }
printf '%s\n' 0x00000000 0x00000001 0x00000002 0x00000004 0x00000006 0x00000008 > "$out/watch.expected"
diff -u "$out/watch.expected" "$out/watch.log"
grep -Fxq 'penv: machine paused at pc=0x00000012' "$out/watch.stderr"
echo 'PASS hooks.robot:23-52 Should Stop On Peripheral Read Watchpoint'
