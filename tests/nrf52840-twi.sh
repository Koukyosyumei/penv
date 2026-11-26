#!/bin/sh
set -eu

if [ "$#" -ne 2 ]; then
    echo "Usage: $0 PENV RENODE_ROOT" >&2
    exit 1
fi

penv=$1
repl=$2/platforms/cpus/nrf52840.repl
source=$(dirname "$0")/nrf52840-twi.c
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

awk '/^[a-z]/ { keep = ($1 ~ /^(nvic|cpu|uart0|twi[01]|flash|ram):$/) } keep' \
    "$repl" > "$work/twi.repl"
cat >> "$work/twi.repl" <<'REPL'
eeprom: Memory.GenericI2cEeprom @ twi1 0x50
    size: 0x100
REPL
cat > "$work/twi.ld" <<'LINKER'
ENTRY(reset)
SECTIONS {
    .vectors 0 : { KEEP(*(.vectors)) }
    .text : { *(.text*) *(.rodata*) }
    .ARM.exidx : { *(.ARM.exidx*) }
    .bss 0x20000100 (NOLOAD) : { *(.bss*) }
}
LINKER

# Build a freestanding Thumb ELF using instructions common to Cortex-M0 and M4.
clang --target=arm-none-eabi -mcpu=cortex-m0 -mthumb -Os -ffreestanding -nostdlib \
    -fuse-ld=lld -Wl,-T,"$work/twi.ld" "$source" -o "$work/twi.elf"
result=$("$penv" run --platform "$work/twi.repl" --max-steps 100000 "$work/twi.elf")
printf '%s\n' "$result"
test "$result" = "TWI OK"
