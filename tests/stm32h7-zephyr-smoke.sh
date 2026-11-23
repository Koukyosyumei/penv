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
    frequency: 125000000
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
    $1 ~ /^(dwt|exti|nvicInput[0-9]+|flashController|flashBank[12]|dtcm|itcm|axiSram|system_flash_ro|sram[1-4]|backupSram|rcc|syscfg|spi4|dma[12]|dma2d|ltdc|dmamux1|bdma|adc3|adcM1S2|gpioPort[A-K]|i2c[1-4]|ethernet|pwr|fmc|rng|qspi|timer3|lptimer[1-5]|crc|fdcan[12]|canMessageRAM):$/ &&
    $2 ~ /[A-Z]/ {print}
' "$cpu" >> "$work/boot.repl"
# The reference REPL leaves HSEM IRQ unconnected. Route it for the notification check.
awk -v RS='' -v ORS='\n\n' '$1 == "hsem:" { print $0 "\n    IRQ -> nvic@125" }' "$cpu" >> "$work/boot.repl"
awk -v RS='' -v ORS='\n\n' '$1 == "watchdog:" {print}' "$cpu" >> "$work/boot.repl"
awk -v RS='' -v ORS='\n\n' '$1 ~ /^(usart2|watchdog|rcc):$/' "$board" >> "$work/boot.repl"
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

# Port the watchdog-reset check to the H7 reference ELF without reloading it.
events=()
for entry in '1000000:devmem 0x58004800 32 0x5555' '1150000:devmem 0x58004804 32 6'     '1300000:devmem 0x58004808 32 0x7f' '1450000:devmem 0x58004800 32 0xaaaa'     '1600000:devmem 0x58004800 32 0xcccc' '2200000:devmem 0x58004800 32 0xaaaa'     '2800000:kernel version' '4300000:kernel version'; do
    step=${entry%%:*}
    printf '%s\r' "${entry#*:}" > "$work/wdt-$step.txt"
    events+=(--uart-event "$step:$work/wdt-$step.txt")
done
status=0
target/release/penv run --script "$work/boot.resc" \
    --platform-root "$renode" --asset-dir "$assets" --max-steps 4900000 \
    "${events[@]}" > "$work/watchdog-uart.log" 2> "$work/watchdog-penv.log" || status=$?
if [[ "$status" != 2 ]] ||
    ! awk '
        /Booting STM32H7 Renode Platform Demo/ { boots++ }
        /Zephyr version 4\.4\.99/ { replies[boots]++ }
        END { exit !(boots == 2 && replies[1] == 1 && replies[2] == 1) }
    ' "$work/watchdog-uart.log"; then
    cat "$work/watchdog-penv.log" >&2
    cat "$work/watchdog-uart.log" >&2
    exit 1
fi

# Check acquisition, wrong-owner release, and IRQ 125 pending through the guest shell.
events=()
step=1000000
for command in '0x580244e0 32 0x02000000' '0x58026500 32 8'     '0x5802640c 32 0x8000032a' '0x5802640c 32 0x32b' '0x5802640c 32'     '0x58026508 32' '0x5802640c 32 0x32a' '0x5802650c 32'     '0xe000e20c 32' '0x58026504 32 8' '0xe000e28c 32 0x20000000'     '0x5802650c 32' '0xe000e20c 32' '0x58026490 32'; do
    printf 'devmem %s\r' "$command" > "$work/hsem-$step.txt"
    events+=(--uart-event "$step:$work/hsem-$step.txt")
    step=$((step + 150000))
done
status=0
target/release/penv run --script "$work/boot.resc" \
    --platform-root "$renode" --asset-dir "$assets" --max-steps "$step" \
    "${events[@]}" > "$work/hsem-uart.log" 2> "$work/hsem-penv.log" || status=$?
expected=$(printf '%s\n' 0x8000032a 0x0 0x8 0x20000000 0x0 0x0 0x80000300)
actual=$(awk '/^Read value / { sub(/\r$/, ""); print $3 }' "$work/hsem-uart.log")
if [[ "$status" != 2 || "$actual" != "$expected" ]]; then
    cat "$work/hsem-penv.log" >&2
    cat "$work/hsem-uart.log" >&2
    exit 1
fi
