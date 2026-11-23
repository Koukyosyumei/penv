#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."
penv=${PENV_BIN:-target/release/penv}
work=${1:-$(mktemp -d)}
fixture=tests/fixtures/stm32f407-freertos
platform_args=()
expected="$fixture/expected-uart.txt"
case "${2:-}" in
    "") "$fixture/build.sh" "$work" ;;
    --upstream|--fpu)
        "$fixture/build.sh" "$work" "$2"
        platform_args=(--platform-root "${RENODE_ROOT:-../renode}")
        expected="$work/expected-upstream-uart.txt"
        sed -e '/^RCC PLL/i Discovery bit-band GPIO and SRAM OK' \
            -e 's/^I2C echo 01 23 and NACK OK$/I2C unpopulated bus NACK OK/' \
            "$fixture/expected-uart.txt" > "$expected"
        if [[ "$2" == --fpu ]]; then
            sed -i -e '/^task A tick=05$/i FPU context OK: A' -e '/^task B tick=07$/i FPU context OK: B' "$expected"
        fi
        ;;
    *) echo "usage: $0 [OUTPUT_DIR] [--upstream|--fpu]" >&2; exit 1 ;;
esac

printf X > "$work/uart-x.txt"
printf YZ > "$work/uart-yz.txt"
printf '!' > "$work/uart-watchdog.txt"

if [[ -z ${PENV_BIN:-} ]]; then
    cargo build --locked --release
fi
printf 'STM32F407 ELF and logs: %s\n' "$work"

run() {
    local log=$1 errors=$2 limit=$3 status=0
    shift 3
    "$penv" run --script "$work/boot.resc" --max-steps "$limit" "${platform_args[@]}" \
        --uart-event "24000:$work/uart-x.txt" --uart-event "30000:$work/uart-yz.txt" \
        "$@" > "$work/$log.log" 2> "$work/$errors.log" || status=$?
    if [[ "$status" != 2 ]] || ! rg -q '^penv: instruction limit reached ' "$work/$errors.log"; then
        cat "$work/$errors.log" "$work/$log.log" >&2
        exit 1
    fi
}

run uart penv 36000
diff -u "$expected" "$work/uart.log"
cat "$work/uart.log"

run watchdog-uart watchdog-penv 90000 \
    --uart-event "38000:$work/uart-watchdog.txt" \
    --uart-event "75000:$work/uart-x.txt" --uart-event "81000:$work/uart-yz.txt"
{
    cat "$expected"
    printf '%s\n' 'IWDG armed' 'IWDG refresh held' 'IWDG reboot: SRAM retained'
    cat "$expected"
} | diff -u - "$work/watchdog-uart.log"
rg '^IWDG ' "$work/watchdog-uart.log"
