#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
work=${1:-$(mktemp -d)}
mkdir -p "$work"
cat >"$work/bus.repl" <<'REPL'
memory: Memory.MappedMemory @ { sysbus 0x0; sysbus 0xc0000000 }
    size: 16
code: Memory.MappedMemory @ sysbus 0x10000
    size: 0x1000
nvic: IRQControllers.NVIC @ sysbus 0xe000e000 { -> cpu@0 }
cpu: CPU.CortexM @ sysbus { cpuType: "cortex-m4"; nvic: nvic; vectorTableOffset: 0x10000 }
uart: UART.LiteX_UART @ sysbus 0x40000000
sysbus:
    init:
        UnhandledAccessBehaviour DoNotReport
REPL
cat >"$work/boot.resc" <<'RESC'
mach create
machine LoadPlatformDescription @bus.repl
sysbus LoadELF @probe.elf
sysbus LoadBinary @payload.bin 0xbffffffc
RESC
cat >"$work/probe.ld" <<'LD'
ENTRY(reset)
SECTIONS {
    .vectors 0x10000 : { *(.vectors) }
    .text : { *(.text) }
}
LD
cat >"$work/probe.S" <<'ASM'
.syntax unified
.thumb
.section .vectors,"a"
.word 0x11000, reset
.text
.global reset
.thumb_func
reset:
    ldr r0, =0xbffffffc
    ldr r1, [r0]
    cmp r1, #0
    bne stop
    ldr r1, [r0, #4]
    ldr r2, =0x07060504
    cmp r1, r2
    bne stop
    movs r0, #0
    ldr r1, [r0]
    cmp r1, r2
    bne stop
    ldr r0, =0x40000000
    movs r1, #'O'
    strb r1, [r0]
    movs r1, #'K'
    strb r1, [r0]
    movs r1, #10
    strb r1, [r0]
stop:
    bkpt #0
ASM
printf '\000\001\002\003\004\005\006\007' >"$work/payload.bin"
"${CLANG:-clang}" --target=arm-none-eabi -mcpu=cortex-m4 -mthumb -nostdlib -fuse-ld=lld \
    -Wl,-T,"$work/probe.ld" "$work/probe.S" -o "$work/probe.elf"
cargo build --release
target/release/penv run --script "$work/boot.resc" --max-steps 128 >"$work/uart.log"
diff -u <(printf 'OK\n') "$work/uart.log"
cat "$work/uart.log"

cat >"$work/mixed.repl" <<'REPL'
first: Memory.MappedMemory @ sysbus 50
    size: 100
second: Mocks.BytePeripheral @ sysbus <200, +100>
code: Memory.MappedMemory @ sysbus 0x10000
    size: 0x1000
nvic: IRQControllers.NVIC @ sysbus 0xe000e000 { -> cpu@0 }
cpu: CPU.CortexM @ sysbus { cpuType: "cortex-m4"; nvic: nvic; vectorTableOffset: 0x10000 }
uart: UART.LiteX_UART @ sysbus 0x40000000
sysbus:
    init:
        UnhandledAccessBehaviour DoNotReport
REPL
cat >"$work/mixed.resc" <<'RESC'
mach create
machine LoadPlatformDescription @mixed.repl
sysbus LoadELF @mixed.elf
sysbus LoadBinary @mixed.bin 0
RESC
cat >"$work/mixed.S" <<'ASM'
.syntax unified
.thumb
.section .vectors,"a"
.word 0x11000, reset
.text
.global reset
.thumb_func
reset:
    movs r0, #0
    movw r4, #350
check:
    cmp r0, #50
    blo hole
    cmp r0, #150
    blo mapped
    cmp r0, #200
    blo hole
    cmp.w r0, #300
    bhs hole
mapped:
    mov r2, r0
    cmp r2, #255
    blo compare
    subs r2, #255
    b compare
hole:
    movs r2, #0
compare:
    ldrb r1, [r0]
    cmp r1, r2
    bne stop
    adds r0, #1
    cmp r0, r4
    blo check
    ldr r0, =0x40000000
    movs r1, #'O'
    strb r1, [r0]
    movs r1, #'K'
    strb r1, [r0]
    movs r1, #10
    strb r1, [r0]
stop:
    bkpt #0
ASM
python3 - "$work/mixed.bin" <<'PY'
import sys
from pathlib import Path

Path(sys.argv[1]).write_bytes(bytes(i % 255 for i in range(350)))
PY
"${CLANG:-clang}" --target=arm-none-eabi -mcpu=cortex-m4 -mthumb -nostdlib -fuse-ld=lld \
    -Wl,-T,"$work/probe.ld" "$work/mixed.S" -o "$work/mixed.elf"
target/release/penv run --script "$work/mixed.resc" --max-steps 10000 >"$work/mixed-uart.log"
diff -u <(printf 'OK\n') "$work/mixed-uart.log"
cat "$work/mixed-uart.log"
