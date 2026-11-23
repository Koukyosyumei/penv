#!/usr/bin/env bash
set -euo pipefail
export LC_ALL=C

repo=$(cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo"
renode_root=$(realpath "${1:-../renode}")
evidence=$(realpath -m "${2:?usage: verify-stm32f072.sh RENODE_ROOT NEW_EVIDENCE_DIRECTORY}")
renode_pin=c38ac3babcd38a7144b9498e3ca4c2226b76aa37
commit=$(git rev-parse HEAD)
test "$(git -C "$renode_root" rev-parse HEAD)" = "$renode_pin"
git diff --quiet HEAD --
git -C "$renode_root" diff --quiet HEAD --
mkdir -p -- "$(dirname -- "$evidence")"
mkdir -- "$evidence"
mkdir -- "$evidence/assets"
exec > >(tee "$evidence/verification.log") 2>&1

printf 'penv_commit=%s\nrenode_commit=%s\n' "$commit" "$renode_pin"
rustc --version
cargo --version

prefix=https://dl.antmicro.com/projects/renode
{
    awk -v prefix="$prefix" '
        /^[[:space:]]+Create Machine +stm32.*\.elf/ { print prefix "/" $NF }
        /sysbus LoadELF @https:/ { sub(/^@/, "", $NF); print $NF }
    ' "$renode_root/tests/platforms/STM32F072b.robot"
    awk '/^[[:space:]]+Create Machine +https:/ { print $NF }' \
        "$renode_root/tests/unit-tests/hex.robot"
} | sort -u >"$evidence/upstream-assets.txt"
sed -En 's/^sysbus Load(ELF|HEX) @//p' tests/fixtures/stm32f072-*.resc |
    sort -u >"$evidence/fixture-assets.txt"
diff -u "$evidence/upstream-assets.txt" "$evidence/fixture-assets.txt"
test "$(wc -l <"$evidence/fixture-assets.txt")" -eq 7
while IFS= read -r url; do
    name=${url##*/}
    if [[ ! "$name" =~ -s_([0-9]+)-([0-9a-f]{40})$ ]]; then
        printf 'Unrecognized upstream asset name: %s\n' "$name" >&2
        exit 1
    fi
    expected_size=${BASH_REMATCH[1]}
    expected_sha1=${BASH_REMATCH[2]}
    printf 'Fetching %s\n' "$url"
    curl -fLsS --retry 2 --max-time 60 -o "$evidence/assets/$name" -- "$url"
    test "$(wc -c <"$evidence/assets/$name")" -eq "$expected_size"
    printf '%s  %s\n' "$expected_sha1" "$evidence/assets/$name" | sha1sum --check
done <"$evidence/fixture-assets.txt"
(
    cd "$evidence"
    sha256sum assets/* >firmware.sha256
)

cargo test --locked --color never 2>&1 | tee "$evidence/cargo-test.log"
PENV_RENODE_ROOT="$renode_root" PENV_RENODE_ASSETS="$evidence/assets" \
    cargo test --locked --release --test cli --color never upstream_stm32f072 -- --ignored \
    2>&1 | tee "$evidence/stm32f072.log"

cat >"$evidence/expected-tests.txt" <<'CASES'
upstream_stm32f072_adc_waits_for_uart_between_voltage_changes
upstream_stm32f072_blinky_alternates_led_states
upstream_stm32f072_blinky_pauses_while_systick_is_not_clocked
upstream_stm32f072_button_updates_led_and_reports_press
upstream_stm32f072_console_echo_accepts_resc_uart_input
upstream_stm32f072_console_echo_replies_after_uart_prompt
upstream_stm32f072_crc_firmware_passes_840_cases
upstream_stm32f072_hello_world_boots_elf_and_hex
CASES
sed -En 's/^test (upstream_stm32f072_[^ ]+) \.\.\. ok$/\1/p' "$evidence/stm32f072.log" |
    sort >"$evidence/passed-tests.txt"
diff -u "$evidence/expected-tests.txt" "$evidence/passed-tests.txt"
test "$(git rev-parse HEAD)" = "$commit"
test "$(git -C "$renode_root" rev-parse HEAD)" = "$renode_pin"
git diff --quiet HEAD --
git -C "$renode_root" diff --quiet HEAD --
printf 'PASS %s: cargo test and 8 STM32F072 firmware tests (9 scenarios)\n' "$commit"
