#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."
renode=${RENODE_ROOT:-../renode}
assets=${PENV_ASSET_DIR:-$(mktemp -d)}
mkdir -p "$assets"
work=$(mktemp -d)
cpu="$renode/platforms/cpus/stm32h7.repl"
board="$renode/platforms/boards/stm32h7_renode_reference_board.repl"
upstream="$renode/scripts/single-node/stm32h7_renode_reference_board.resc"

url=$(sed -n 's/^\$bin?=@//p' "$upstream")
firmware="$assets/${url##*/}"
if [[ ! -f "$firmware" ]]; then
    curl --fail --location "$url" --output "$firmware.part"
    mv "$firmware.part" "$firmware"
fi
printf '%s  %s\n' c5f12b2aac7af72f7430894558749a984eb7d94e "$firmware" | sha1sum --check -

# Select the declarations needed by the firmware. Keep USART2 first for CLI UART input.
awk -v RS='' -v ORS='\n\n' '$1 == "cpu:" || $1 == "nvic:" {
    if ($1 == "nvic:") sub(/\n    ->/, "\n    systickFrequency: 480_000_000\n    ->");
    print
}' "$cpu" > "$work/boot.repl"
cat >> "$work/boot.repl" <<'REPL'
usart2: UART.STM32F7_USART @ sysbus 0x40004400
    frequency: 200000000
    IRQ -> exti@27

dbgmcu: Python.PythonPeripheral @ sysbus 0x5c001000
    size: 0x400
    initable: false
    script: '''
if request.Offset == 0:
    request.Value = 0x20030450
'''

canCc: Python.PythonPeripheral @ sysbus 0x4000a804
    size: 4
    initable: false
    script: '''
if request.Offset == 0:
    request.Value = 4
'''

REPL
awk -v RS='' -v ORS='\n\n' '
    $1 == "system_flash_ro:" {sub(/\n    init:.*/, ""); print}
    $1 ~ /^(dwt|exti|nvicInput[0-9]+|flashController|flashBank[12]|dtcm|itcm|axiSram|sram[1-4]|backupSram|rcc|syscfg|spi4|dma[12]|dma2d|ltdc|dmamux1|bdma|adc3|adcM1S2|gpioPort[A-K]|i2c[1-4]|ethernet|pwr|fmc|rng|qspi|timer3|crc|fdcan[12]|canMessageRAM):$/ &&
    $2 ~ /[A-Z]/ {print}
' "$cpu" >> "$work/boot.repl"
awk -v RS='' -v ORS='\n\n' '$1 ~ /^(qspiFlash|flashMem|phy):$/' "$board" >> "$work/boot.repl"
awk '$1 == "veml7700:" {print}' "$board" >> "$work/boot.repl"
printf '    Illuminance: 500\n' >> "$work/boot.repl"
sed 's|@platforms/boards/stm32h7_renode_reference_board.repl|@boot.repl|' \
    "$upstream" > "$work/boot.resc"
printf 'help\r' > "$work/help.txt"
printf 'kernel version\r' > "$work/kernel-version.txt"
printf 'sensor get veml7700@10 light\r' > "$work/sensor.txt"

cargo build --release
status=0
target/release/penv run --script "$work/boot.resc" \
    --platform-root "$renode" --asset-dir "$assets" --max-steps 2600000 \
    --uart-event "850000:$work/help.txt" \
    --uart-event "1300000:$work/kernel-version.txt" \
    --uart-event "1700000:$work/sensor.txt" \
    --stats > "$work/uart.log" 2> "$work/penv.log" || status=$?
printf 'STM32H7 UART and simulator logs: %s\n' "$work"
if [[ "$status" != 2 ]] ||
    ! rg -aqF 'Booting STM32H7 Renode Platform Demo' "$work/uart.log" ||
    ! rg -aqF 'demo:~$' "$work/uart.log" ||
    ! rg -aqF 'Available commands:' "$work/uart.log" ||
    ! rg -aqF 'Zephyr version 4.4.99' "$work/uart.log" ||
    ! rg -aq 'channel type=18\(light\).*\(499\.000000\)' "$work/uart.log"; then
    cat "$work/penv.log" >&2
    cat "$work/uart.log" >&2
    exit 1
fi
