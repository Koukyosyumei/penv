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
#ifdef COMMAND_ERROR
    ldr r1, [r0]
#else
    ldrb r1, [r0]
#endif
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

sed '/"parent"/d' "$work/bus.repl" >"$work/command-error.repl"
printf '%s\n' 'fault: Mocks.CommandErrorPeripheral @ sysbus <0x1000, +0x100>' >>"$work/command-error.repl"
"${CLANG:-clang}" --target=arm-none-eabi -mcpu=cortex-m4 -mthumb -nostdlib -fuse-ld=lld \
    -DCOMMAND_ERROR -Wl,-T,"$work/bus.ld" "$work/bus.S" -o "$work/command-error.elf"
target/release/penv run --platform "$work/command-error.repl" --max-steps 128 \
    "$work/command-error.elf" >"$work/command-error.uart"
diff -u <(printf 'OK\n') "$work/command-error.uart"
printf 'CommandError: '
cat "$work/command-error.uart"

cat >"$work/registration.S" <<'ASM'
.syntax unified
.thumb
.section .vectors,"a"
.word 0x380, reset
.text
.global reset
.thumb_func
reset:
    movw r0, #1000
    ldr r1, [r0]
    movw r2, #0x666
    cmp r1, r2
    bne done
#ifdef MULTIPLE
    movw r0, #2000
    ldr r1, [r0]
    adds r2, #1
    cmp r1, r2
    bne done
    movw r0, #3000
    ldr r1, [r0]
    adds r2, #1
    cmp r1, r2
    bne done
#endif
    ldr r0, =0x40000000
    movs r1, #'K'
    strb r1, [r0]
done:
    bkpt #0
ASM
for fixture in single multiple; do
    head -n 4 "$work/bus.repl" | sed 's/size: 0x1000/size: 0x380/' >"$work/$fixture.repl"
    flags=()
    if [[ $fixture == single ]]; then
        echo 'peri1: Mocks.DoubleWordPeripheral @ sysbus <1000, +1000> { value: 0x666 }' >>"$work/$fixture.repl"
    else
        cat >>"$work/$fixture.repl" <<'REPL'
peri1: Mocks.DoubleWordPeripheral @ sysbus <1000, +100> { value: 0x666 }
peri2: Mocks.DoubleWordPeripheral @ sysbus <2000, +100> { value: 0x667 }
peri3: Mocks.DoubleWordPeripheral @ sysbus <3000, +100> { value: 0x668 }
REPL
        flags+=(-DMULTIPLE)
    fi
    "${CLANG:-clang}" --target=arm-none-eabi -mcpu=cortex-m4 -mthumb -nostdlib -fuse-ld=lld \
        "${flags[@]}" -Wl,-T,"$work/bus.ld" "$work/registration.S" -o "$work/$fixture.elf"
    target/release/penv run --platform "$work/$fixture.repl" --max-steps 128 \
        "$work/$fixture.elf" >"$work/$fixture.uart"
    diff -u <(printf K) "$work/$fixture.uart"
    printf 'Doubleword registration (%s): OK\n' "$fixture"
done
