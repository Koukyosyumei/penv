#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."
renode=${RENODE_ROOT:-../renode}
assets=${PENV_ASSET_DIR:?Set PENV_ASSET_DIR to the reference ELF directory}
base=${1:?Pass the directory printed by tests/stm32h7-zephyr-smoke.sh}
work=$(mktemp -d)
printf 'STM32H7 TIM8 logs: %s\n' "$work"
cp "$base/boot.repl" "$work/boot.repl"
# Use both the registration and the GPIO output overlay from the fixed upstream REPL.
awk -v RS='' -v ORS='\n\n' '$1 == "timer8:" {print}' \
    "$renode/platforms/cpus/stm32h7.repl" >> "$work/boot.repl"
cp "$base/boot.resc" "$work/boot.resc"

cat >> "$work/boot.resc" <<'RESC'
emulation RunFor "1"
sysbus WriteDoubleWord 0xe000e284 0x7800
sysbus WriteDoubleWord 0x580244e8 2
sysbus ReadDoubleWord 0x4001042c
sysbus WriteDoubleWord 0x580244f0 2
sysbus ReadDoubleWord 0x4001042c
sysbus WriteDoubleWord 0x4001040c 0x40
sysbus WriteDoubleWord 0x40010414 0xfe
sysbus ReadDoubleWord 0x40010410
sysbus ReadDoubleWord 0xe000e204
sysbus WriteDoubleWord 0x4001040c 1
sysbus WriteDoubleWord 0x40010414 1
emulation RunFor "0.00001"
sysbus ReadDoubleWord 0x40010410
sysbus ReadDoubleWord 0xe000e204
sysbus WriteDoubleWord 0x40010410 0
sysbus WriteDoubleWord 0xe000e284 0x7800
sysbus WriteDoubleWord 0x40010418 1
sysbus WriteDoubleWord 0x40010408 0x54
sysbus WriteDoubleWord 0x40010424 0x42
sysbus WriteDoubleWord 0x4001040c 0x40
timer8 OnGPIO 0 true
emulation RunFor "0.00001"
sysbus ReadDoubleWord 0x40010424
sysbus ReadDoubleWord 0x40010410
sysbus ReadDoubleWord 0xe000e204
sysbus WriteDoubleWord 0x40010410 0
sysbus WriteDoubleWord 0xe000e284 0x7800
sysbus WriteDoubleWord 0x40010408 0
sysbus WriteDoubleWord 0x40010418 0x30
sysbus WriteDoubleWord 0x40010420 1
sysbus WriteDoubleWord 0x40010428 24999
sysbus WriteDoubleWord 0x40010434 2
sysbus WriteDoubleWord 0x40010414 1
sysbus WriteDoubleWord 0x4001040c 2
sysbus WriteDoubleWord 0x40010400 1
emulation RunFor "0.001"
sysbus ReadDoubleWord 0x40010410
sysbus ReadDoubleWord 0xe000e204
sysbus WriteDoubleWord 0x58024498 2
sysbus ReadDoubleWord 0x4001042c
sysbus WriteDoubleWord 0xe000e284 0x7800
sysbus WriteDoubleWord 0x58024498 0
sysbus ReadDoubleWord 0x4001042c
sysbus ReadDoubleWord 0x4001040c
emulation RunFor "0.00001"
sysbus ReadDoubleWord 0xe000e204
RESC

target/release/penv run --script "$work/boot.resc" --platform-root "$renode" \
    --asset-dir "$assets" --max-steps 1500000 --uart-log-dir "$work" \
    > "$work/monitor.log" 2> "$work/penv.log"
rg -aqF 'Booting STM32H7 Renode Platform Demo' "$work/0.uart.log"
printf '%s\n' 0x00000000 0x0000ffff 0x00000000 0x00000000 0x00000001 0x00001000 \
    0x00000000 0x00000040 0x00002000 0x00000002 0x00004000 0x00000000 \
    0x0000ffff 0x00000000 0x00000000 | diff -u - <(awk '/sysbus\[/ { print $NF }' "$work/monitor.log")
