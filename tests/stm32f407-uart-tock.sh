#!/usr/bin/env bash
# Reproduce the Tock boot prerequisite of the upstream UART tester cases.
set -euo pipefail
cd "$(dirname "$0")/.."
work=${1:-target/stm32f407-uart-tock}
renode=$(realpath "${RENODE_ROOT:-../renode}")
penv=${PENV_BIN:-target/release/penv}
test "$(git -C "$renode" rev-parse HEAD)" = c38ac3babcd38a7144b9498e3ca4c2226b76aa37
mkdir -p "$work"
script="$renode/scripts/single-node/stm32f4_tock.resc"
for asset in \
    stm32f412gdiscovery--tock_kernel.elf-s_3392340-6da12cfcd5c4180b60ce7bf2ad32f019c9e8216e \
    cortex_m4--tock-cxx_hello.tbf-s_16384-2bf6dfc3ffd2894bea56341901beec1e903c3135
do
    if [[ ! -f "$work/$asset" ]]; then
        curl --fail --location --max-time 60 --output "$work/$asset.part" \
            "https://dl.antmicro.com/projects/renode/$asset"
        mv "$work/$asset.part" "$work/$asset"
    fi
    printf '%s  %s\n' "${asset##*-}" "$work/$asset" | sha1sum --check -
done
status=0
"$penv" run --script "$script" --platform-root "$renode" --asset-dir "$work" \
    --max-steps "${PENV_TOCK_STEPS:-1000000}" --stats > "$work/boot.uart" 2> "$work/boot.log" ||
    status=$?
echo "$status" > "$work/boot.status"
verdict=PARTIAL
for marker in 'Initialization complete. Entering main loop' 'D2 says hello' 'D1 says hello'; do
    if ! grep -aqF "$marker" "$work/boot.uart"; then
        printf 'BLOCKED: %s absent from USART output\n' "$marker" >&2
        cat "$work/boot.log" >&2
        verdict=BLOCKED
        break
    fi
done
printf 'source_line\tcase\tboot_prerequisite\n' > "$work/cases.tsv"
awk -v verdict="$verdict" 'NR == 191 || NR == 201 || NR == 212 {
    printf "%s\t%s\t%s\n", NR, $0, verdict
}' "$renode/tests/unit-tests/uart-tester.robot" >> "$work/cases.tsv"
[[ "$verdict" != BLOCKED ]]
printf '%s\n' 'PARTIAL: boot markers observed; UART failing-string semantics unverified'
