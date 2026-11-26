#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
work=${1:-$(mktemp -d)}
mkdir -p "$work"
cargo build --release
cat > "$work/board.repl" <<'REPL'
mem: Memory.MappedMemory @ sysbus 0x0
    size: 0x8000000
nvic: IRQControllers.NVIC @ sysbus 0xe000e000
    -> cpu@0
cpu: CPU.CortexM @ sysbus
    cpuType: "cortex-m4"
    nvic: nvic
    vectorTableOffset: 0x10000
uart: UART.LiteX_UART @ sysbus 0x40000000
REPL
cat > "$work/exclusive.ld" <<'LD'
ENTRY(reset)
SECTIONS {
    .text 0 : { *(.text) }
    .vectors 0x10000 : { *(.vectors) }
}
LD
run() {
    local case_name=$1 status=$2 value=$3
    {
        cat <<'ASM'
.syntax unified
.thumb
.text
.global reset
.thumb_func
reset:
    movw r0, #0x1000
    movw r2, #0x100
ASM
        cat
        cat <<'ASM'
    ldr r3, [r0]
    ldr r4, =0x40000000
    .irp reg,r2,r3
        strb \reg, [r4]
        lsrs \reg, \reg, #8
        strb \reg, [r4]
        lsrs \reg, \reg, #8
        strb \reg, [r4]
        lsrs \reg, \reg, #8
        strb \reg, [r4]
    .endr
    bkpt #0
.section .vectors,"a"
.word 0x8000000, reset
ASM
    } > "$work/$case_name.S"
    "${CLANG:-clang}" --target=arm-none-eabi -mcpu=cortex-m4 -mthumb \
        -nostdlib -fuse-ld=lld -Wl,-T,"$work/exclusive.ld" "$work/$case_name.S" -o "$work/$case_name.elf"
    target/release/penv run --platform "$work/board.repl" --max-steps 64 "$work/$case_name.elf" > "$work/$case_name.uart"
    python3 - "$work/$case_name.uart" "$status" "$value" <<'PY'
import struct
import sys
from pathlib import Path

path = Path(sys.argv[1])
expected = tuple(int(value, 0) for value in sys.argv[2:])
actual = struct.unpack('<II', path.read_bytes())
assert actual == expected, (path, actual, expected)
print(f'{path.stem}: status={actual[0]} memory=0x{actual[1]:08x}')
PY
}

# Single-CPU cases in Renode arm-atomic.robot, translated from A32 to Thumb-2.
run load-store 0 0xa <<'ASM'
    ldrex r1, [r0]
    mov.w r1, #0xa
    strex r2, r1, [r0]
ASM

run store-without-load 1 0 <<'ASM'
    mov.w r1, #0xa
    strex r2, r1, [r0]
ASM

run store-after-write 1 0xa <<'ASM'
    ldrex r1, [r0]
    mov.w r1, #0xa
    str r1, [r0]
    mov.w r1, #0xb
    strex r2, r1, [r0]
ASM

# Additional Cortex-M4 regression for the valid LR encoding in both opcodes.
run lr-operands 0 0xa <<'ASM'
    ldrex lr, [r0]
    mov.w lr, #0xa
    strex r2, lr, [r0]
    ldrex lr, [r0]
    strex lr, r1, [r0]
    orrs r2, lr
    ldrex lr, [r0]
    mov.w r1, #0xa
    strex lr, r1, [r0]
    orrs r2, lr
ASM
