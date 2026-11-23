#!/bin/sh
set -eu

if [ "$#" -ne 2 ]; then
    echo "Usage: $0 PENV RENODE_ROOT" >&2
    exit 1
fi

penv=$1
repl=$2/platforms/cpus/nrf52840.repl
source=$(dirname "$0")/nrf52840-rng.c
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

awk '/^[a-z]/ { keep = ($1 ~ /^(nvic|cpu|uart0|rng|flash|ram):$/) } keep' \
    "$repl" > "$work/rng.repl"
cat > "$work/rng.ld" <<'LINKER'
ENTRY(reset)
SECTIONS {
    .vectors 0 : { KEEP(*(.vectors)) }
    .text : { *(.text*) *(.rodata*) }
    .ARM.exidx : { *(.ARM.exidx*) }
    .bss 0x20000100 (NOLOAD) : { *(.bss*) }
}
LINKER

clang --target=arm-none-eabi -mcpu=cortex-m0 -mthumb -Os -ffreestanding -nostdlib \
    -fuse-ld=lld -Wl,-T,"$work/rng.ld" "$source" -o "$work/rng.elf"
result=$("$penv" run --platform "$work/rng.repl" --max-steps 250000 "$work/rng.elf")
printf '%s\n' "$result"
test "$result" = "RNG OK"
