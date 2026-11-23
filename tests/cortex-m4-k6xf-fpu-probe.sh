#!/usr/bin/env bash
# Locate the first boot blocker of the pinned Zephyr FPU-sharing ELF.
set -euo pipefail
cd "$(dirname "$0")/.."
work=${1:-target/shared-k6xf-fpu}
penv=${PENV_BIN:-target/debug/penv}
renode=$(realpath "${RENODE_ROOT:-../renode}")
test "$(git -C "$renode" rev-parse HEAD)" = c38ac3babcd38a7144b9498e3ca4c2226b76aa37
mkdir -p "$work"
sha=bef2795db6dd11f8106387ff6c6afd35686c48de
elf="$work/zephyr.elf"
if [[ ! -f "$elf" ]]; then
    curl --fail --location --max-time 60 --output "$elf.part" \
        "https://dl.antmicro.com/projects/renode/frdm_k64f--zephyr-fpu_sharing_generic.elf-s_861928-$sha"
    mv "$elf.part" "$elf"
fi
printf '%s  %s\n' "$sha" "$elf" | sha1sum --check -
original="$renode/platforms/cpus/nxp-k6xf.repl"
# Diagnostic only: preserve the original memory/NVIC layout and omit K6xF devices.
awk '/^[a-zA-Z_]+[0-9]*:/ { emit = /^sram_l:|^sram:|^flash:|^nvic:|^cpu:/ }
     emit { sub(/"cortex-m4f"/, "\"cortex-m4\""); print }' "$original" > "$work/core.repl"
probe() {
    local name=$1 fixture=$2 status=0
    "$penv" run --platform "$fixture" --platform-root "$renode" --max-steps 100000 --stats "$elf" \
        > "$work/$name.uart" 2> "$work/$name.log" || status=$?
    printf '%s\n' "$status" > "$work/$name.status"
    printf '%s\t%s\n' "$name" "$status"
}
probe original "$original"
probe diagnostic-core "$work/core.repl"
