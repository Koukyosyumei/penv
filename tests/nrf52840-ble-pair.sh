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
python3 "$(dirname "$0")/nrf52840-ble-uart.py" heart-rate "$work/uart" -- \
    "$1" run --platform "$2/platforms/cpus/nrf52840.repl" --platform-root "$2" \
    --uart-steps --ble-peer "$4" --max-steps 8800000000 "$3" || status=$?
cat "$work/uart"
exit "$status"
