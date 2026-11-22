#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."
work=${1:-$(mktemp -d)}
fixture=tests/fixtures/stm32f407-freertos
"$fixture/build.sh" "$work"

printf X > "$work/uart-x.txt"
printf YZ > "$work/uart-yz.txt"

cargo build --release
status=0
target/release/penv run --script "$work/boot.resc" --max-steps 36000 \
    --uart-event "24000:$work/uart-x.txt" --uart-event "30000:$work/uart-yz.txt" \
    > "$work/uart.log" 2> "$work/penv.log" || status=$?

printf 'STM32F407 ELF and logs: %s\n' "$work"
if [[ "$status" != 2 ]] || ! rg -q '^penv: instruction limit reached ' "$work/penv.log"; then
    cat "$work/penv.log" "$work/uart.log" >&2
    exit 1
fi
diff -u "$fixture/expected-uart.txt" "$work/uart.log"
cat "$work/uart.log"
