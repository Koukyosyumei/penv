#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
work=${1:-$(mktemp -d)}
renode=${RENODE_ROOT:-../renode}
test "$(git -C "$renode" rev-parse HEAD)" = c38ac3babcd38a7144b9498e3ca4c2226b76aa37
mkdir -p "$work"
cat > "$work/crc.ld" <<'LD'
ENTRY(reset)
SECTIONS {
    .vectors 0x08000000 : { *(.vectors) }
    .text : { *(.text) }
}
LD
cat > "$work/crc.S" <<'ASM'
.syntax unified
.thumb
.section .vectors,"a"
.word 0x20040000, reset
.text
.global reset
.thumb_func
reset:
    ldr r0, =0x40023800       /* RCC */
    ldr r1, [r0, #0x30]
    orr r1, r1, #0x1000       /* CRCEN */
    str r1, [r0, #0x30]
    ldr r1, [r0, #0x40]
    orr r1, r1, #0x20000      /* USART2EN */
    str r1, [r0, #0x40]
    ldr r4, =0x40004400       /* USART2 */
    movw r1, #0x2008
    str r1, [r4, #12]        /* UE | TE */

    ldr r0, =0x40023000       /* CRC */
    ldr r1, [r0]
    ldr r2, =0xffffffff
    cmp r1, r2
    bne fail
    ldr r1, =0x31323334
    str r1, [r0]
    ldr r1, =0x35363738
    str r1, [r0]
    ldr r1, [r0]
    ldr r2, =0x49e3c2fb
    cmp r1, r2
    bne fail

    movs r1, #0x5a
    str r1, [r0, #4]
    movs r1, #1
    str r1, [r0, #8]         /* reset CRC accumulator */
    ldr r1, [r0]
    ldr r2, =0xffffffff
    cmp r1, r2
    bne fail
    ldr r1, [r0, #4]
    cmp r1, #0x5a
    bne fail

    adr r0, passed_message
    b print
fail:
    adr r0, failed_message
print:
    ldrb r1, [r0]
    cmp r1, #0
    beq end
    str r1, [r4, #4]
    adds r0, #1
    b print
end:
    bkpt #0
    b end
.align 2
passed_message: .asciz "STM32F407 CRC DR=49E3C2FB RESET=FFFFFFFF IDR=5A\n"
.align 2
failed_message: .asciz "STM32F407 CRC FAIL\n"
ASM
"${CLANG:-clang}" --target=arm-none-eabi -mcpu=cortex-m4 -mthumb -nostdlib -fuse-ld=lld \
    -Wl,-T,"$work/crc.ld" "$work/crc.S" -o "$work/crc.elf"
cargo build --release
target/release/penv run --platform "$renode/platforms/boards/stm32f4_discovery-kit.repl" \
    --platform-root "$renode" --max-steps 1000 "$work/crc.elf" > "$work/uart.log"
diff -u <(printf '%s\n' 'STM32F407 CRC DR=49E3C2FB RESET=FFFFFFFF IDR=5A') "$work/uart.log"
cat "$work/uart.log"
