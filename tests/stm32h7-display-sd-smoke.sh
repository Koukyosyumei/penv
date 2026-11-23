#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."
renode=${RENODE_ROOT:-../renode}
assets=${PENV_ASSET_DIR:-$(mktemp -d)}
mkdir -p "$assets"
base=${1:?Pass the log directory printed by tests/stm32h7-zephyr-smoke.sh}
work=$(mktemp -d)
printf 'STM32H7 display and SD logs: %s\n' "$work"

fetch() {
    local url=$1 hash=$2 file="$assets/${1##*/}"
    if [[ ! -f "$file" ]]; then
        curl --fail --location "$url" --output "$file.part"
        mv "$file.part" "$file"
    fi
    printf '%s  %s\n' "$hash" "$file" | sha1sum --check - >&2
    printf '%s\n' "$url"
}

display_url=$(rg --only-matching 'https://\S+display\.elf-\S+' "$renode/tests/peripherals/STM32LTDC.robot")
sd_url=$(rg --only-matching 'https://\S+stm32h747i-fs_sample_ext\.elf-\S+' "$renode/tests/peripherals/STM32SDMMC.robot")
display_url=$(fetch "$display_url" 55334a5ed1b3104c7033b0f3f55e9eb451fafc71)
sd_url=$(fetch "$sd_url" 6a3b96a821387ade23d9633a12dcfe67dae02686)

cp "$base/boot.repl" "$work/h747.repl"
cat >> "$work/h747.repl" <<'REPL'

usart1: UART.STM32F7_USART @ sysbus 0x40011000
    frequency: 200000000
    IRQ -> nvic@37

extSdram: Memory.MappedMemory @ sysbus 0xd0000000
    size: 0x2000000

dsi: Video.STM32H7_DSI @ sysbus 0x50000000
REPL

run_sample() {
    local sample=$1 url=$2
    shift 2
    cat > "$work/$sample.resc" <<RESC
mach create
machine LoadPlatformDescription @h747.repl
showAnalyzer sysbus.usart1
sysbus LoadELF @$url
cpu EnableZephyrMode
cpu VectorTableOffset \`sysbus GetSymbolAddress "_vector_table"\`
RESC
    local status=0
    target/release/penv run --script "$work/$sample.resc" --platform-root "$renode" \
        --asset-dir "$assets" --max-steps 15000000 "$@" \
        > "$work/$sample-uart.log" 2> "$work/$sample-penv.log" || status=$?
    if [[ "$status" != 2 ]]; then
        cat "$work/$sample-penv.log" "$work/$sample-uart.log" >&2
        return 1
    fi
}

run_sample display "$display_url" --framebuffer-out "$work/display.ppm"
rg -aqF 'Display starts' "$work/display-uart.log"
rg -aqF 'Display updated' "$work/display-uart.log"
if rg -aqF 'DSI init failed' "$work/display-uart.log"; then
    cat "$work/display-uart.log" >&2
    exit 1
fi
python3 - "$work/display.ppm" <<'PY'
import pathlib
import sys

magic, size, maximum, rgb = pathlib.Path(sys.argv[1]).read_bytes().split(b'\n', 3)
assert (magic, size, maximum) == (b'P6', b'800 480', b'255')
assert len(rgb) == 800 * 480 * 3
assert len(set(zip(rgb[::3], rgb[1::3], rgb[2::3]))) > 1
PY

# With no SD card attached, the sample should reach its filesystem error path.
run_sample sd "$sd_url"
rg -aqF 'Booting Zephyr OS' "$work/sd-uart.log"
rg -aqF 'Error mounting disk.' "$work/sd-uart.log"
