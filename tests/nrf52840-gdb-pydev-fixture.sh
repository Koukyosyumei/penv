#!/bin/sh
# Guest-side preconditions of gdb.robot:587-644,1210-1224.
set -eu
if [ "$#" -ne 3 ]; then
    echo "Usage: $0 PENV RENODE_ROOT ARTIFACT_DIR" >&2
    exit 1
fi
penv=$1
root=$2
out=$3
mkdir -p "$out"
cat > "$out/board.repl" <<'REPL'
cpu: CPU.CortexM @ sysbus { cpuType: "cortex-m4"; nvic: nvic; vectorTableOffset: 0x08001000 }
nvic: IRQControllers.NVIC @ sysbus 0xe000e000 { -> cpu@0 }
flash: Memory.MappedMemory @ sysbus 0x08000000 { size: 0x2000 }
ram: Memory.MappedMemory @ sysbus 0x20000000 { size: 0x1000 }
rolling: Python.PythonPeripheral @ sysbus 0x40000000
    size: 0x400
    initable: true
    filename: "scripts/pydev/rolling-bit.py"
repeater: Python.PythonPeripheral @ sysbus 0xf0000000
    size: 4
    initable: true
    filename: "scripts/pydev/repeater.py"
REPL
cat > "$out/guest.s" <<'ASM'
.syntax unified
.thumb
.global reset
.thumb_func
reset:
    .hword 0xf04f, 0x4380  // mov.w r3, #0x40000000
    .hword 0xf640, 0x32ad  // movw r2, #0xbad
    .hword 0x601a, 0xe7fd  // str r2, [r3]; b.n
.org 0x20
prepare:
    ldr r1, =0xf0000000
    movw r0, #0x147
    str r0, [r1]
    b read_access
.ltorg
.org 0x30
prepare_write:
    ldr r1, =0xf0000000
    movw r0, #0x147
    str r0, [r1]
    b write_access
.ltorg
.org 0x40
read_access:
    ldr r0, [r1]
    nop
    nop
    b .
.org 0x50
write_access:
    movs r0, #1
    str r0, [r1]
    b .
.org 0x1000
.word 0x20001000, reset
ASM
clang --target=arm-none-eabi -mcpu=cortex-m4 -c "$out/guest.s" -o "$out/guest.o"
ld.lld -N -Ttext=0x08000000 --entry=reset "$out/guest.o" -o "$out/guest.elf"
for case in read write no-pause; do
    file="$out/$case.resc"
    cat > "$file" <<'RESC'
mach create
machine LoadPlatformDescription @board.repl
sysbus LoadELF @guest.elf
RESC
    case "$case" in
        read|write)
            entry=0x08000021
            if [ "$case" = write ]; then entry=0x08000031; fi
            cat >> "$file" <<RESC
cpu PC $entry
sysbus ReadDoubleWord 0xf0000000
cpu Step 4
sysbus ReadDoubleWord 0xf0000000
RESC
            ;;
    esac
    case "$case" in
        read)
            cat >> "$file" <<'RESC'
cpu AddLogHook 0x08000042 "first nop"
cpu AddLogHook 0x08000044 "second nop"
sysbus AddWatchpointHook 0xf0000000 4 Read ""
cpu Step 3
sysbus ReadDoubleWord 0xf0000000
cpu Step 1
sysbus ReadDoubleWord 0xf0000000
cpu Step 1
sysbus ReadDoubleWord 0xf0000000
RESC
            printf '%s\n' 0x00000000 0x00000147 0x00000147 'first nop' 0x00000147 'second nop' 0x00000147 > "$out/$case.expected"
            pc=0x08000046
            ;;
        write)
            # Write fixture follows the same setup, then executes the store.
            cat >> "$file" <<'RESC'
cpu Step 2
sysbus ReadDoubleWord 0xf0000000
RESC
            printf '%s\n' 0x00000000 0x00000147 0x00000001 > "$out/$case.expected"
            pc=0x08000054
            ;;
        no-pause)
            cat >> "$file" <<'RESC'
cpu AddLogHook 0x0800000a "reached address after Python MMIO"
cpu AddPauseHook 0x0800000a
RESC
            printf '%s\n' 'reached address after Python MMIO' > "$out/$case.expected"
            pc=0x0800000a
            ;;
    esac
    status=0
    "$penv" run --script "$file" --platform-root "$root" --max-steps 20 > "$out/$case.raw" 2> "$out/$case.stderr" || status=$?
    printf '%s\n' "$status" > "$out/$case.status"
    [ "$status" -eq 0 ] || { cat "$out/$case.stderr" >&2; exit 1; }
    sed '/^Machine paused$/d' "$out/$case.raw" > "$out/$case.log"
    diff -u "$out/$case.expected" "$out/$case.log"
    grep -Fxq "penv: machine paused at pc=$pc" "$out/$case.stderr"
    printf '%s: PASS (guest fixture)\n' "$case"
done
