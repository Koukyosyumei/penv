#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
work=${1:-target/stm32f407-mpu-zephyr}
renode=$(realpath "${RENODE_ROOT:-../renode}")
penv=${PENV_BIN:-target/release/penv}
test "$(git -C "$renode" rev-parse HEAD)" = c38ac3babcd38a7144b9498e3ca4c2226b76aa37
mkdir -p "$work"
printf 'wait\t8\tPROJECT EXECUTION SUCCESSFUL\n' > "$work/uart.tsv"
failed=0
selected=0
mode=(--uart-check "$work/uart.tsv")
if [[ -n ${PENV_MPU_DIAGNOSTIC_STEPS:-} ]]; then
    mode=(--max-steps "$PENV_MPU_DIAGNOSTIC_STEPS" --stats)
fi
while read -r name url; do
    [[ -z ${2:-} || $2 == "$name" ]] || continue
    selected=$((selected + 1))
    asset=${url##*/}
    elf="$work/$asset"
    if [[ ! -f "$elf" ]]; then
        curl --fail --location --output "$elf.part" "$url"
        mv "$elf.part" "$elf"
    fi
    printf '%s  %s\n' "${asset##*-}" "$elf" | sha1sum --check --status -
    cat > "$work/$name.resc" <<RESC
mach create
machine LoadPlatformDescription @platforms/boards/stm32f4_discovery-kit.repl
sysbus LoadELF @$asset
showAnalyzer sysbus.usart2
RESC
    status=0
    "$penv" run --script "$work/$name.resc" --platform-root "$renode" \
        --f4-repl-timing --virtual-mips 100 "${mode[@]}" \
        > "$work/$name.uart" 2> "$work/$name.log" || status=$?
    printf '%s\n' "$status" > "$work/$name.status"
    printf '%s\t%s\n' "$name" "$status"
    if [[ "$status" != 0 ]]; then failed=1; fi
done < <(awk '/Run Test +https:.*--zephyr-(mem_protect|protection|stackprot|userspace)\.elf/ {
    name=$NF; sub(/^.*--zephyr-/, "", name); sub(/\.elf.*/, "", name); print name, $NF
}' "$renode/tests/unit-tests/mpu.robot")
((selected > 0)) || { echo "no MPU firmware cases selected" >&2; exit 1; }
exit "$failed"
