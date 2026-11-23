#!/bin/sh
# The Renode Bluetooth sample's two-node milestones, with independent UART labels.
set -eu

if [ "$#" -ne 4 ]; then
    echo "Usage: $0 PENV RENODE_ROOT CENTRAL_ELF PERIPHERAL_ELF" >&2
    exit 1
fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
status=0
"$1" run --platform "$2/platforms/cpus/nrf52840.repl" --platform-root "$2" \
    --ble-peer "$4" --max-steps 180000000 "$3" > "$work/uart" 2> "$work/stderr" || status=$?
cat "$work/uart" "$work/stderr"
if [ "$status" -ne 0 ] && [ "$status" -ne 2 ]; then
    exit "$status"
fi
for message in 'primary: Bluetooth initialized' 'peer: Bluetooth initialized' \
    'primary: Scanning successfully started' 'peer: Advertising successfully started' \
    'primary: Connected:' 'peer: Connected' \
    'primary: [SUBSCRIBED]'; do
    if ! grep -Fq "$message" "$work/uart"; then
        echo "Missing Bluetooth milestone: $message" >&2
        exit 1
    fi
done
if ! grep -Eq '^peer: .*HRS notifications enabled' "$work/uart"; then
    echo "Missing peripheral subscription confirmation" >&2
    exit 1
fi
if ! grep -Eq '^primary: \[NOTIFICATION\].*length 2' "$work/uart"; then
    echo "Missing two-byte Heart Rate Measurement" >&2
    exit 1
fi
