#!/bin/sh
# monitor-tests.robot / Should Call GPIO Set.
set -eu
if [ "$#" -ne 3 ]; then
    echo "Usage: $0 PENV RENODE_ROOT ARTIFACT_DIR" >&2
    exit 1
fi
penv=$1
root=$2
out=$3
mkdir -p "$out"
name=renode-nrf52840-zephyr_shell_module.elf-gf8d05cf-s_1310072-c00fbffd6b65c6238877c4fe52e8228c2a38bf1f
elf=$out/shell.elf
if [ ! -f "$elf" ]; then
    curl -fsSL "https://dl.antmicro.com/projects/renode/$name" -o "$elf.part"
    mv "$elf.part" "$elf"
fi
printf '%s  %s\n' "${name##*-}" "$elf" | sha1sum --check
{
    cat "$root/scripts/single-node/nrf52840.resc"
    printf '\nuart0 IRQ\nuart0 IRQ Set true\nuart0 IRQ\n'
} >"$out/irq.resc"

status=0
"$penv" run --script "$out/irq.resc" --platform-root "$root" --max-steps 1 "$elf" >"$out/irq.log" 2>&1 || status=$?
cat "$out/irq.log"
[ "$status" -eq 2 ]
expected=$(printf 'GPIO: unset\nGPIO: set')
actual=$(grep '^GPIO:' "$out/irq.log")
[ "$actual" = "$expected" ]
