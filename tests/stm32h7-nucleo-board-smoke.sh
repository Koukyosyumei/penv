#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."
renode=${RENODE_ROOT:-../renode}
assets=${PENV_ASSET_DIR:-$(mktemp -d)}
base=${1:?Pass the directory printed by tests/stm32h7-zephyr-smoke.sh}
mkdir -p "$assets"
work=$(mktemp -d)
printf 'STM32H753ZI board logs: %s\n' "$work"

board="$renode/platforms/boards/nucleo_h753zi.repl"
robot="$renode/tests/platforms/nucleo_h753zi.robot"
cp "$base/boot.repl" "$work/board.repl"
awk -v RS='' -v ORS='\n\n' '$1 ~ /^(GreenLED|YellowLED|RedLED|UserButton1|gpioPortB|gpioPortE|rcc):$/' \
    "$board" >> "$work/board.repl"
cat >> "$work/board.repl" <<'REPL'

usart3: UART.STM32F7_USART @ sysbus 0x40004800
    frequency: 125000000
    IRQ -> nvic@39

lpuart1: UART.STM32F7_USART @ sysbus 0x58000c00
    frequency: 125000000
    IRQ -> exti@64
REPL

for scenario in blinky button; do
    tag=${scenario^^}
    name=$(awk -v tag="\${$tag}" '$1 == tag {sub(/^.*\//,"",$2); print $2}' "$robot")
    [[ -n $name ]]
    url="https://dl.antmicro.com/projects/renode/$name"
    if [[ ! -f "$assets/$name" ]]; then
        curl --fail --location "$url" --output "$assets/$name.part"
        mv "$assets/$name.part" "$assets/$name"
    fi
    printf '%s  %s\n' "${name##*-}" "$assets/$name" | sha1sum --check -
    mkdir "$work/$scenario"
    script="$work/$scenario.resc"
    cat > "$script" <<RESC
mach create "$scenario"
machine LoadPlatformDescription @board.repl
sysbus LoadELF @$url
cpu VectorTableOffset \`sysbus GetSymbolAddress "_vector_table"\`
RESC
    if [[ $scenario == blinky ]]; then
        # GPIOB0 drives the upstream GreenLED. Allow startup time, then check eight one-second intervals.
        for ((sample = 0; sample <= 920; sample++)); do
            printf 'emulation RunFor "0.01"\nsysbus ReadDoubleWord 0x58020414\n'
        done >> "$script"
    else
        cat >> "$script" <<'RESC'
emulation RunFor "0.5"
sysbus ReadDoubleWord 0x58020414
sysbus.gpioPortC.UserButton1 Press
emulation RunFor "0.25"
sysbus ReadDoubleWord 0x58020414
sysbus.gpioPortC.UserButton1 Release
emulation RunFor "0.25"
sysbus ReadDoubleWord 0x58020414
RESC
    fi
    status=0
    target/release/penv run --script "$script" --platform-root "$renode" --asset-dir "$assets" \
        --max-steps 10000000 --uart-log-dir "$work/$scenario" \
        > "$work/$scenario/monitor.log" 2> "$work/$scenario/penv.log" || status=$?
    printf '%s\n' "$status" > "$work/$scenario/exit"
    if [[ $status != 0 ]]; then
        cat "$work/$scenario/penv.log" "$work/$scenario/0.uart.log" >&2
        exit 1
    fi
done

python3 - "$work" <<'PY'
import pathlib
import re
import sys

root = pathlib.Path(sys.argv[1])

def led_samples(case):
    lines = (root / case / 'monitor.log').read_text().splitlines()
    return [int(line.rsplit(' ', 1)[1], 16) & 1 for line in lines]

blink = (root / 'blinky/0.uart.log').read_text()
button = (root / 'button/0.uart.log').read_text()
assert '*** Booting Zephyr OS' in blink and re.search(r'LED state: (ON|OFF)', blink)
assert '*** Booting Zephyr OS' in button
assert button.index('Press the button') < button.index('Button pressed at')
assert led_samples('button') == [0, 1, 0]
levels = led_samples('blinky')
assert len(levels) == 921
edges = [i for i in range(1, len(levels)) if levels[i-1] != levels[i]]
assert len(edges) >= 9, edges
assert all(abs(b-a-100) <= 1 for a, b in zip(edges, edges[1:])), edges
print('PASS blinky UART and GPIOB0 edges (10 ms samples):', edges)
print('PASS button UART and GPIOB0 off/on/off')
PY
