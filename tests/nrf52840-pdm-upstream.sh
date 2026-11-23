#!/bin/sh
# Port of "Should Detect Yes Pattern" in Renode tests/platforms/NRF52840.robot.
set -eu

if [ "$#" -ne 4 ]; then
    echo "Usage: $0 PENV RENODE_ROOT MICRO_SPEECH_ELF YES_PCM" >&2
    exit 1
fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
awk '/^[a-z]/ { keep = ($1 ~ /^(nvic|cpu|uart0|uart1|rtc[012]|wdt|ppi|twi[01]|spi2|timer[0-4]|i2s|pdm|gpio[01]|gpiote|flash|ram|clock|sysbus):$/) } keep' \
    "$2/platforms/cpus/nrf52840.repl" > "$work/pdm.repl"

status=0
"$1" run --platform "$work/pdm.repl" --pdm-input "$4" --max-steps 500000000 "$3" \
    > "$work/uart" || status=$?
cat "$work/uart"
if [ "$status" -ne 0 ] && [ "$status" -ne 2 ]; then
    exit "$status"
fi
grep -Fq 'Heard yes' "$work/uart"
