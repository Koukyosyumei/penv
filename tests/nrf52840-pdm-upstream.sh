#!/bin/sh
# Port of "Should Detect Yes Pattern" in Renode tests/platforms/NRF52840.robot.
set -eu

if [ "$#" -ne 4 ]; then
    echo "Usage: $0 PENV RENODE_ROOT MICRO_SPEECH_ELF YES_PCM" >&2
    exit 1
fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
status=0
"$1" run --platform "$2/platforms/cpus/nrf52840.repl" --platform-root "$2" --pdm-input "$4" --max-steps 320000000 "$3" \
    > "$work/uart" || status=$?
cat "$work/uart"
if [ "$status" -ne 0 ] && [ "$status" -ne 2 ]; then
    exit "$status"
fi
grep -Fq 'Heard yes' "$work/uart"
