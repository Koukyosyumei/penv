#!/bin/sh
# Fixed Cortex-M4 instructions and shared source-layout cases from llvm-asm-disasm.robot.
set -eu
[ "$#" -eq 1 ] || exit 1
penv=$1

round_trip() {
    address=$1
    memory_bytes=$2
    instruction_word=$3
    source=$4
    assembled=$("$penv" thumb-assemble "$source")
    [ "$assembled" = "$memory_bytes" ]
    decoded=$("$penv" thumb-disassemble "$address" "$memory_bytes")
    [ "$decoded" = "$address: $instruction_word $source" ]
    printf '%s -> %s\n' "$assembled" "$decoded"
}
round_trip 0x00000002 23f00304 f0230403 'bic r4, r3, #3'
round_trip 0x00000000 c858 58c8 'ldr r0, [r1, r3]'
round_trip 0x000009e8 4ff46d42 f44f426d 'mov.w r2, #60672'
round_trip 0x0000ad88 b610 10b6 'asrs r6, r6, #2'

# Thumb NOP is 16 bits. Check the two instructions of the multiline
# fixture and the six expansions of the assembler directive.
multiline=$("$penv" thumb-assemble "$(printf 'nop\nnop')")
[ "$multiline" = 00bf00bf ]
printf 'multiline: %s\n' "$multiline"
repeat=$("$penv" thumb-assemble '.rept 6; nop; .endr')
[ "$repeat" = 00bf00bf00bf00bf00bf00bf ]
printf 'directive: %s\n' "$repeat"
