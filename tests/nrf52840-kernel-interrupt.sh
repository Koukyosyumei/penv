#!/bin/sh
# Port of "Should Run Zephyr's kernel.interrupt Test" in NRF52840.robot.
set -eu

if [ "$#" -ne 3 ]; then
    echo "Usage: $0 PENV RENODE_ROOT KERNEL_INTERRUPT_ELF" >&2
    exit 1
fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
status=0
"$1" run --platform "$2/platforms/cpus/nrf52840.repl" --platform-root "$2" \
    --max-steps 100000000 "$3" > "$work/uart" 2> "$work/stderr" || status=$?
cat "$work/uart" "$work/stderr"
if [ "$status" -ne 0 ] && [ "$status" -ne 2 ]; then
    exit "$status"
fi
grep -Fq 'PROJECT EXECUTION SUCCESSFUL' "$work/uart"
