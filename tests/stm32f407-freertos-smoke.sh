#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."
work=${1:-$(mktemp -d)}
fixture=tests/fixtures/stm32f407-freertos
"$fixture/build.sh" "$work"

printf X > "$work/uart-x.txt"
printf YZ > "$work/uart-yz.txt"
printf '!' > "$work/uart-watchdog.txt"

cargo build --release
printf 'STM32F407 ELF and logs: %s\n' "$work"

run() {
    local log=$1 errors=$2 limit=$3 status=0
    shift 3
    target/release/penv run --script "$work/boot.resc" --max-steps "$limit" \
        --uart-event "24000:$work/uart-x.txt" --uart-event "30000:$work/uart-yz.txt" \
        "$@" > "$work/$log.log" 2> "$work/$errors.log" || status=$?
    if [[ "$status" != 2 ]] || ! rg -q '^penv: instruction limit reached ' "$work/$errors.log"; then
        cat "$work/$errors.log" "$work/$log.log" >&2
        exit 1
    fi
}

run uart penv 36000
diff -u "$fixture/expected-uart.txt" "$work/uart.log"
cat "$work/uart.log"

run watchdog-uart watchdog-penv 90000 \
    --uart-event "38000:$work/uart-watchdog.txt" \
    --uart-event "75000:$work/uart-x.txt" --uart-event "81000:$work/uart-yz.txt"
{
    cat "$fixture/expected-uart.txt"
    printf '%s\n' 'IWDG armed' 'IWDG refresh held' 'IWDG reboot: SRAM retained'
    cat "$fixture/expected-uart.txt"
} | diff -u - "$work/watchdog-uart.log"
rg '^IWDG ' "$work/watchdog-uart.log"
