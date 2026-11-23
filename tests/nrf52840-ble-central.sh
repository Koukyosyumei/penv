#!/bin/sh
# Single-node initialization and scan-start portion of Renode's Bluetooth sample.
set -eu

if [ "$#" -ne 3 ]; then
    echo "Usage: $0 PENV RENODE_ROOT CENTRAL_ELF" >&2
    exit 1
fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
status=0
"$1" run --platform "$2/platforms/cpus/nrf52840.repl" --platform-root "$2" \
    --max-steps 10000000 "$3" > "$work/uart" 2> "$work/stderr" || status=$?
cat "$work/uart" "$work/stderr"
grep -Fq 'Bluetooth initialized' "$work/uart"
grep -Fq 'Scanning successfully started' "$work/uart"
case "$status" in
    0|2) ;;
    1) grep -Fq 'unmapped RAM access at 0x4000f500' "$work/stderr" ;;
    *) exit "$status" ;;
esac
