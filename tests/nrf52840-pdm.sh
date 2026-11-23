#!/bin/sh
set -eu

if [ "$#" -ne 2 ]; then
    echo "Usage: $0 PENV RENODE_ROOT" >&2
    exit 1
fi

penv=$1
repl=$2/platforms/cpus/nrf52840.repl
source=$(dirname "$0")/nrf52840-pdm.c
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

awk '/^[a-z]/ { keep = ($1 ~ /^(nvic|cpu|uart0|pdm|flash|ram):$/) } keep' \
    "$repl" > "$work/pdm.repl"
cat > "$work/pdm.ld" <<'LINKER'
ENTRY(reset)
SECTIONS {
    .vectors 0 : { KEEP(*(.vectors)) }
    .text : { *(.text*) *(.rodata*) }
    .ARM.exidx : { *(.ARM.exidx*) }
    .bss 0x20000100 (NOLOAD) : { *(.bss*) }
}
LINKER

clang --target=arm-none-eabi -mcpu=cortex-m0 -mthumb -Os -ffreestanding -nostdlib \
    -fuse-ld=lld -Wl,-T,"$work/pdm.ld" "$source" -o "$work/pdm.elf"
printf '\064\222\377\177\000\200\315\253' > "$work/input.pcm"
result=$("$penv" run --pdm-input "$work/input.pcm" --platform "$work/pdm.repl" --max-steps 100000 "$work/pdm.elf")
printf '%s\n' "$result"
test "$result" = "PDM OK"
