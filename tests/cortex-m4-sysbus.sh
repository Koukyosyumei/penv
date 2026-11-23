#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
work=${1:-$(mktemp -d)}
mkdir -p "$work"
cat >"$work/bus.repl" <<'REPL'
mem: Memory.MappedMemory @ sysbus 0 { size: 0x1000 }
nvic: IRQControllers.NVIC @ sysbus 0xe000e000 { -> cpu@0 }
cpu: CPU.CortexM @ sysbus { cpuType: "cortex-m4"; nvic: nvic }
uart: UART.LiteX_UART @ sysbus 0x40000000
sysbus:
    init:
        UnhandledAccessBehaviour Report
        Tag <0x1000, 0x10ff> "parent" 0 false false false false true
        Tag <0x1010, 0x101f> "child" 0x42
REPL
cat >"$work/bus.ld" <<'LD'
ENTRY(reset)
SECTIONS {
    .vectors 0 : { *(.vectors) }
    .text 0x100 : { *(.text) }
}
LD
cat >"$work/bus.S" <<'ASM'
.syntax unified
.thumb
.section .vectors,"a"
.word 0x800, reset, 0, hardfault
.text
.global reset
.thumb_func
reset:
    ldr r0, =0xabcd1234
    ldrb r1, [r0]
    cmp r1, #0
    bne failure
    movw r0, #0x1010
    ldrb r1, [r0]
    cmp r1, #0x42
    bne failure
    movw r0, #0x1000
    ldrb r1, [r0]
failure:
    bkpt #0

.thumb_func
hardfault:
    ldr r0, =0xe000ed28
    ldr r1, [r0]
    movw r2, #0x8200
    cmp r1, r2
    bne failure
    ldr r1, [r0, #0x10]
    movw r2, #0x1000
    cmp r1, r2
    bne failure
    ldr r0, =0x40000000
    movs r1, #'O'
    strb r1, [r0]
    movs r1, #'K'
    strb r1, [r0]
    movs r1, #10
    strb r1, [r0]
    bkpt #0
ASM
"${CLANG:-clang}" --target=arm-none-eabi -mcpu=cortex-m4 -mthumb -nostdlib -fuse-ld=lld \
    -Wl,-T,"$work/bus.ld" "$work/bus.S" -o "$work/bus.elf"
cargo build --release
target/release/penv run --platform "$work/bus.repl" --max-steps 128 "$work/bus.elf" >"$work/uart.log"
diff -u <(printf 'OK\n') "$work/uart.log"
cat "$work/uart.log"
