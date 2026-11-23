#!/bin/sh
# Guest-side byte comparison for gdb.robot:841-854.
set -eu
if [ "$#" -ne 3 ]; then
    echo "Usage: $0 PENV RENODE_ROOT ARTIFACT_DIR" >&2
    exit 1
fi
penv=$1
root=$2
out=$3
mkdir -p "$out"
python3 - "$out/memory_test_file.bin" <<'PY'
import sys
from pathlib import Path
Path(sys.argv[1]).write_bytes(bytes(index % 0x80 for index in range(0x1800)))
PY
cat > "$out/board.repl" <<'REPL'
using "platforms/cpus/nrf52840.repl"
mem: Memory.MappedMemory @ sysbus 0x80000000 { size: 0x3000 }
REPL
cat > "$out/dump.s" <<'ASM'
.syntax unified
.thumb
.global reset
.word 0x20040000, reset
.thumb_func
reset:
    ldr r0, =0x80000800
    ldr r1, =0x80002000
    ldr r2, =0x20000000
copy:
    ldrb r3, [r0], #1
    strb r3, [r2], #1
    cmp r0, r1
    bne copy
    ldr r0, =0x40002000
    movs r1, #8
    str r1, [r0, #0x500]
    ldr r1, =0x20000000
    str r1, [r0, #0x544]
    movw r1, #0x1800
    str r1, [r0, #0x548]
    movs r1, #1
    str r1, [r0, #8]
    bkpt 0
ASM
clang --target=arm-none-eabi -mcpu=cortex-m4 -c "$out/dump.s" -o "$out/dump.o"
ld.lld -N -Ttext=0 --entry=reset "$out/dump.o" -o "$out/dump.elf"
cat > "$out/dump.resc" <<'RESC'
mach create
machine LoadPlatformDescription @board.repl
sysbus LoadELF @dump.elf
sysbus LoadBinary @memory_test_file.bin 0x80000800
RESC
status=0
"$penv" run --script "$out/dump.resc" --platform-root "$root" --max-steps 30000 > "$out/memory_dump.bin" 2> "$out/dump.stderr" || status=$?
printf '%s\n' "$status" > "$out/dump.status"
[ "$status" -eq 0 ] || { cat "$out/dump.stderr" >&2; exit 1; }
cmp "$out/memory_test_file.bin" "$out/memory_dump.bin"
echo 'page-spanning guest read: 6144 bytes match'
