#!/usr/bin/env bash
# Exercise the initial sensor transaction in the pinned Zephyr ELF.
set -euo pipefail
cd "$(dirname "$0")/.."
work=${1:-target/stm32f407-hs3001}
renode=$(realpath "${RENODE_ROOT:-../renode}")
penv=${PENV_BIN:-target/release/penv}
test "$(git -C "$renode" rev-parse HEAD)" = c38ac3babcd38a7144b9498e3ca4c2226b76aa37
timing=(--f4-repl-timing --virtual-mips 100)
output=initial
case ${2:-upstream} in
    upstream) ;;
    --default-timing) timing=(); output=diagnostic ;;
    *) echo "usage: $0 [OUTPUT_DIR] [--default-timing]" >&2; exit 1 ;;
esac
mkdir -p "$work"
sha=b107cceed1ebc23c894d983a6e519a6e494aee88
elf="$work/zephyr.elf"
if [[ ! -f "$elf" ]]; then
    curl --fail --location --output "$elf.part" \
        "https://dl.antmicro.com/projects/renode/nucleo_f401re--zephyr-dht_polling.elf-s_651648-$sha"
    mv "$elf.part" "$elf"
fi
printf '%s  %s\n' "$sha" "$elf" | sha1sum --check -
cat > "$work/sensor.repl" <<'REPL'
using "platforms/cpus/stm32f4.repl"
hs3001: Sensors.HS3001 @ i2c1 0x44
REPL
cat > "$work/sensor.resc" <<'RESC'
mach create
machine LoadPlatformDescription @sensor.repl
sysbus LoadELF @zephyr.elf
showAnalyzer sysbus.usart2
RESC
printf 'wait\t8\ttemp is 0.00 °C humidity is 0.00 %%RH\n' > "$work/initial.tsv"
"$penv" platform check --file "$work/sensor.repl" --platform-root "$renode" \
    > "$work/platform.log" 2>&1
"$penv" run --script "$work/sensor.resc" --platform-root "$renode" \
    "${timing[@]}" --uart-check "$work/initial.tsv" \
    > "$work/$output.uart" 2> "$work/$output.log"
