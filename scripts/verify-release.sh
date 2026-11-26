#!/usr/bin/env bash
# Run from the committed integration checkout. Arguments are local asset directories
# prepared using the pinned firmware instructions in README.md.
set -euo pipefail
export LC_ALL=C

if [[ $# != 8 ]]; then
    echo "Usage: $0 RENODE_ROOT NEW_LOG_DIR F072_ASSETS RV32_ARCHIVE STARLIGHT_ASSETS REBOOT_ASSETS VIRT_ASSETS FIRE_ASSETS" >&2
    exit 1
fi
renode=$(realpath "$1")
out=$(realpath -m "$2")
arm=$(realpath "$3")
rv32=$(realpath "$4")
starlight=$(realpath "$5")
reboot=$(realpath "$6")
virt=$(realpath "$7")
fire=$(realpath "$8")
cd "$(dirname "$0")/.."
commit=$(git rev-parse HEAD)
renode_pin=c38ac3babcd38a7144b9498e3ca4c2226b76aa37
test "$(git -C "$renode" rev-parse HEAD)" = "$renode_pin"
test -z "$(git status --porcelain)"
git -C "$renode" diff --quiet HEAD --
mkdir -- "$out"
exec > >(tee "$out/verification.log") 2>&1
printf 'penv %s\nRenode %s\n' "$commit" "$renode_pin"
for sha in 45e874f8fcdc11135788f9aa677c3732b735ecfa \
    22777ec2d3e2352b5c3a436c9cf5bee9e8cc8f1a 0edb80bd79fced68f8eab7b40b949ba6ab2dc4ed \
    21624b76fabbfee9151003a849b967d65356b27c 049e0ecf5d5c647a3bdcdc4303d4342e63686047 \
    c4960230193b12a5dc36fd31c52d6ad346fdb52a; do
    git merge-base --is-ancestor "$sha" "$commit"
done
rustc --version
cargo --version
cargo test --locked --color never 2>&1 | tee "$out/cargo-test.log"

# Validate the checked-in STM32F072 script assets before Cargo runs the CLI tests.
sed -En 's/^sysbus Load(ELF|HEX) @//p' tests/fixtures/stm32f072-*.resc | sort -u > "$out/f072-assets.txt"
test "$(wc -l < "$out/f072-assets.txt")" -eq 7
while IFS= read -r url; do
    name=${url##*/}
    [[ "$name" =~ -s_([0-9]+)-([0-9a-f]{40})$ ]]
    test "$(wc -c < "$arm/$name")" -eq "${BASH_REMATCH[1]}"
    printf '%s  %s\n' "${BASH_REMATCH[2]}" "$arm/$name" | sha1sum -c -
done < "$out/f072-assets.txt"

# Cargo builds the release CLI required by these tests. Every subsequent
# firmware invocation uses the same executable and verifies its digest.
PENV_RENODE_ROOT="$renode" PENV_RENODE_ASSETS="$arm" \
    cargo test --locked --release --test cli --color never upstream_stm32f072 -- --ignored \
    2>&1 | tee "$out/stm32f072.log"
test "$(rg -c '^test upstream_stm32f072_.* \.\.\. ok$' "$out/stm32f072.log")" -eq 8
release_bin=$(realpath "${CARGO_TARGET_DIR:-target}/release/penv")
test -x "$release_bin"
export PENV_BIN="$out/penv"
install -m 0755 "$release_bin" "$PENV_BIN"
cmp "$release_bin" "$PENV_BIN"
export RENODE_ROOT="$renode"
sha256sum "$PENV_BIN" | tee "$out/penv.sha256"

run_case() {
    local name=$1
    shift
    if "$@" > "$out/$name.log" 2>&1; then
        printf 'PASS %s\n' "$name"
    else
        printf 'FAIL %s (%s)\n' "$name" "$out/$name.log"
        return 1
    fi
}
pids=()
queue() {
    run_case "$@" &
    pids+=("$!")
}
port_base=${PENV_RELEASE_PORT_BASE:-30800}
[[ "$port_base" =~ ^[1-9][0-9]{3,4}$ ]] && (( port_base + 8 <= 65535 )) || {
    echo "PENV_RELEASE_PORT_BASE must reserve nine UDP ports in 1000–65535" >&2
    exit 1
}
queue rv32 bash scripts/validate-riscv-acceptance.sh "$renode" "$out/rv32" "$rv32"
queue nrf52 bash tests/nrf52840-acceptance.sh "$PENV_BIN" "$renode" "$out/nrf52"
queue stm32f407 bash tests/stm32f407-upstream-smoke.sh "$out/stm32f407"
queue starlight python3 scripts/verify-starlight.py "$starlight" --penv "$PENV_BIN" --renode "$renode" \
    --ports "$((port_base + 1))" "$((port_base + 2))" --switch-port "$port_base" --upstream-cases --logs "$out/starlight"
queue starlight-pmp python3 scripts/verify-starlight.py "$starlight" --penv "$PENV_BIN" --renode "$renode" \
    --ports "$((port_base + 4))" "$((port_base + 5))" --switch-port "$((port_base + 3))" --upstream-cases --external-pmp --logs "$out/starlight-pmp"
queue starlight-reboot python3 scripts/verify-starlight.py "$reboot" --penv "$PENV_BIN" --renode "$renode" \
    --ports "$((port_base + 7))" "$((port_base + 8))" --switch-port "$((port_base + 6))" --recovery --linux-reboot --logs "$out/starlight-reboot"
queue rv64-virt python3 scripts/verify-rv64-virt.py "$virt" --penv "$PENV_BIN" --logs "$out/virt"
queue fire python3 scripts/verify-fire.py "$fire" --penv "$PENV_BIN" --renode "$renode" \
    --max-steps 9000000000 --timeout 1800 --logs "$out/fire"

run_h7() {
    export PENV_ASSET_DIR="$out/stm32h7-assets"
    export TMPDIR="$out/stm32h7"
    mkdir -- "$TMPDIR" || return
    bash tests/stm32h7-zephyr-smoke.sh | tee "$TMPDIR/base.log" || return
    local base
    base=$(sed -n 's/^STM32H7 UART and simulator logs: //p' "$TMPDIR/base.log")
    test -d "$base" || return
    bash tests/stm32h7-display-sd-smoke.sh "$base" || return
    bash tests/stm32h7-io-smoke.sh "$base"
}
queue stm32h7 run_h7

failed=0
for pid in "${pids[@]}"; do
    wait "$pid" || failed=1
done
sha256sum -c "$out/penv.sha256"
test "$(git rev-parse HEAD)" = "$commit"
test -z "$(git status --porcelain)"
git -C "$renode" diff --quiet HEAD --
if [[ "$failed" != 0 ]]; then
    echo "FAIL release verification $commit"
    exit 1
fi
printf 'PASS release verification %s (six teams)\n' "$commit"
