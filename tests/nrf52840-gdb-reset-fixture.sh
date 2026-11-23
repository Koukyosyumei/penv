#!/bin/sh
# gdb.robot:990-1030. Thumb fixture and monitor-side reset checks.
set -eu
if [ "$#" -ne 2 ]; then
    echo "Usage: $0 PENV ARTIFACT_DIR" >&2
    exit 1
fi
penv=$1
out=$2
mkdir -p "$out"
cat > "$out/board.repl" <<'REPL'
cpu: CPU.CortexM @ sysbus { cpuType: "cortex-m4"; nvic: nvic }
nvic: IRQControllers.NVIC @ sysbus 0xe000e000 { -> cpu@0 }
mem: Memory.MappedMemory @ sysbus 0 { size: 0x1000 }
REPL
cat > "$out/guest.s" <<'ASM'
.syntax unified
.thumb
.global reset, main, factorial
.word 0x1000, reset
.org 0x80
.thumb_func
reset:
    b main
.org 0x100
.thumb_func
main:
    bl factorial
    b .
.org 0x120
.thumb_func
factorial:
    bx lr
ASM
clang --target=arm-none-eabi -mcpu=cortex-m4 -c "$out/guest.s" -o "$out/guest.o"
ld.lld -N -Ttext=0 --entry=reset "$out/guest.o" -o "$out/guest.elf"
cat > "$out/base.resc" <<'RESC'
mach create
machine LoadPlatformDescription @board.repl
sysbus LoadELF @guest.elf
RESC
cp "$out/base.resc" "$out/debug.resc"
echo 'machine StartGdbServer 3333' >> "$out/debug.resc"
cat > "$out/remember.gdb" <<'GDB'
target remote :3333
break main
monitor machine Reset
continue
p $pc
GDB
cat > "$out/replace.gdb" <<'GDB'
target remote :3333
break factorial
continue
p $pc
delete 1
break main
monitor machine Reset
continue
p $pc
GDB
for case in remember replace; do
    cp "$out/base.resc" "$out/$case.resc"
    case "$case" in
        remember)
            cat >> "$out/$case.resc" <<'RESC'
cpu AddLogHook 0x100 "main"
cpu AddPauseHook 0x100
machine Reset
cpu Step 20
RESC
            printf '%s\n' main > "$out/$case.expected"
            ;;
        replace)
            # Retain the factorial hook but distinguish the post-reset path
            # by stopping one instruction after the reset vector.
            cat >> "$out/$case.resc" <<'RESC'
cpu AddLogHook 0x120 "factorial"
cpu AddPauseHook 0x120
cpu Step 20
machine Reset
cpu Step 1
RESC
            printf '%s\n' factorial > "$out/$case.expected"
            ;;
    esac
    status=0
    "$penv" run --script "$out/$case.resc" --max-steps 100         > "$out/$case.raw" 2> "$out/$case.stderr" || status=$?
    printf '%s\n' "$status" > "$out/$case.status"
    [ "$status" -eq 0 ] || { cat "$out/$case.stderr" >&2; exit 1; }
    sed '/^Machine paused$/d' "$out/$case.raw" > "$out/$case.log"
    diff -u "$out/$case.expected" "$out/$case.log"
    grep -Fxq 'penv: machine paused at pc=0x00000100' "$out/$case.stderr"
    printf '%s: PASS (monitor reset fixture)\n' "$case"
done
