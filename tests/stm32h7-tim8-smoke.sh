#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."
renode=${RENODE_ROOT:-../renode}
assets=${PENV_ASSET_DIR:?Set PENV_ASSET_DIR to the reference ELF directory}
base=${1:?Pass the directory printed by tests/stm32h7-zephyr-smoke.sh}
case ${2:-timer8} in
    timer1)
        timer=timer1 timer_base=0x400100 rcc_bit=1
        pending=0xe000e200 clear=0xe000e280 irq_mask=0x0f000000
        update=0x02000000 trigger=0x04000000 compare=0x08000000
        ;;
    timer8)
        timer=timer8 timer_base=0x400104 rcc_bit=2
        pending=0xe000e204 clear=0xe000e284 irq_mask=0x7800
        update=0x00001000 trigger=0x00002000 compare=0x00004000
        ;;
    *) printf 'Expected timer1 or timer8\n' >&2; exit 1 ;;
esac
work=$(mktemp -d)
printf 'STM32H7 %s logs: %s\n' "TIM${timer#timer}" "$work"
cp "$base/boot.repl" "$work/boot.repl"
# Use both the registration and the GPIO output overlay from the fixed upstream REPL.
awk -v RS='' -v ORS='\n\n' -v timer="$timer:" '$1 == timer {print}' \
    "$renode/platforms/cpus/stm32h7.repl" >> "$work/boot.repl"
cp "$base/boot.resc" "$work/boot.resc"

cat >> "$work/boot.resc" <<RESC
emulation RunFor "1"
sysbus WriteDoubleWord $clear $irq_mask
sysbus WriteDoubleWord 0x580244f0 0
sysbus WriteDoubleWord 0x580244e8 $rcc_bit
sysbus ReadDoubleWord ${timer_base}2c
sysbus WriteDoubleWord 0x580244f0 $rcc_bit
sysbus ReadDoubleWord ${timer_base}2c
sysbus WriteDoubleWord ${timer_base}0c 0x40
sysbus WriteDoubleWord ${timer_base}14 0xfe
sysbus ReadDoubleWord ${timer_base}10
sysbus ReadDoubleWord $pending
sysbus WriteDoubleWord ${timer_base}0c 1
sysbus WriteDoubleWord ${timer_base}14 1
emulation RunFor "0.00001"
sysbus ReadDoubleWord ${timer_base}10
sysbus ReadDoubleWord $pending
sysbus WriteDoubleWord ${timer_base}10 0
sysbus WriteDoubleWord $clear $irq_mask
sysbus WriteDoubleWord ${timer_base}18 1
sysbus WriteDoubleWord ${timer_base}08 0x54
sysbus WriteDoubleWord ${timer_base}24 0x42
sysbus WriteDoubleWord ${timer_base}0c 0x40
$timer OnGPIO 0 true
emulation RunFor "0.00001"
sysbus ReadDoubleWord ${timer_base}24
sysbus ReadDoubleWord ${timer_base}10
sysbus ReadDoubleWord $pending
sysbus WriteDoubleWord ${timer_base}10 0
sysbus WriteDoubleWord $clear $irq_mask
sysbus WriteDoubleWord ${timer_base}08 0
sysbus WriteDoubleWord ${timer_base}18 0x30
sysbus WriteDoubleWord ${timer_base}20 1
sysbus WriteDoubleWord ${timer_base}28 24999
sysbus WriteDoubleWord ${timer_base}34 2
sysbus WriteDoubleWord ${timer_base}14 1
sysbus WriteDoubleWord ${timer_base}0c 2
sysbus WriteDoubleWord ${timer_base}00 1
emulation RunFor "0.001"
sysbus ReadDoubleWord ${timer_base}10
sysbus ReadDoubleWord $pending
sysbus WriteDoubleWord 0x58024498 $rcc_bit
sysbus ReadDoubleWord ${timer_base}2c
sysbus WriteDoubleWord $clear $irq_mask
sysbus WriteDoubleWord 0x58024498 0
sysbus ReadDoubleWord ${timer_base}2c
sysbus ReadDoubleWord ${timer_base}0c
emulation RunFor "0.00001"
sysbus ReadDoubleWord $pending
RESC

target/release/penv run --script "$work/boot.resc" --platform-root "$renode" \
    --asset-dir "$assets" --max-steps 1500000 --uart-log-dir "$work" \
    > "$work/monitor.log" 2> "$work/penv.log"
rg -aqF 'Booting STM32H7 Renode Platform Demo' "$work/0.uart.log"
printf '%s\n' 0x00000000 0x0000ffff 0x00000000 0x00000000 0x00000001 "$update" \
    0x00000000 0x00000040 "$trigger" 0x00000002 "$compare" 0x00000000 \
    0x0000ffff 0x00000000 0x00000000 | diff -u - <(awk '/sysbus\[/ { print $NF }' "$work/monitor.log")
