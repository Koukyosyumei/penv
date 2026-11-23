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


REPL
awk -v RS='' -v ORS='\n\n' '
    $1 ~ /^(dwt|exti|nvicInput[0-9]+|flashController|flashBank[12]|dtcm|itcm|axiSram|system_flash_ro|sram[1-4]|backupSram|rcc|syscfg|spi4|dma[12]|dma2d|ltdc|dmamux1|bdma|adc3|adcM1S2|gpioPort[A-K]|i2c[1-4]|ethernet|pwr|fmc|rng|qspi|timer3|sdmmc|usb[12]|lptimer[1-5]|crc|fdcan[12]|canMessageRAM):$/ &&
    $2 ~ /[A-Z]/ {print}
' "$cpu" >> "$work/boot.repl"
awk -v RS='' -v ORS='\n\n' '$1 == "sysbus:" {print}' "$cpu" >> "$work/boot.repl"
awk -v RS='' -v ORS='\n\n' '$1 == "crypto:" {print}' \
    "$renode/platforms/cpus/stm32h753.repl" >> "$work/boot.repl"
# The reference REPL leaves HSEM IRQ unconnected. Route it for the notification check.
awk -v RS='' -v ORS='\n\n' '$1 == "hsem:" { print $0 "\n    IRQ -> nvic@125" }' "$cpu" >> "$work/boot.repl"
awk -v RS='' -v ORS='\n\n' '$1 == "watchdog:" {print}' "$cpu" >> "$work/boot.repl"
awk -v RS='' -v ORS='\n\n' '$1 ~ /^(usart2|watchdog|rcc):$/' "$board" >> "$work/boot.repl"
awk -v RS='' -v ORS='\n\n' '$1 ~ /^(qspiFlash|flashMem|phy):$/' "$board" >> "$work/boot.repl"
awk -v RS='' -v ORS='\n\n' '$1 ~ /^(redRgbLed|greenRgbLed|blueRgbLed|gpioPortD):$/' \
    "$board" >> "$work/boot.repl"
awk -v RS='' -v ORS='\n\n' '$1 ~ /^(pwmled0|pwmled3|gpioPortB):$/' "$board" >> "$work/boot.repl"
awk -v RS='' -v ORS='\n\n' '$1 == "timer3:" && NF > 1 && $2 == "0" {print}' "$cpu" >> "$work/boot.repl"
awk '$1 == "iis2mdc:" {print $0 "\n"}' "$board" >> "$work/boot.repl"
awk '$1 == "tmp108:" {print $0 "\n    Temperature: 23.5\n"}' "$board" >> "$work/boot.repl"
awk '$1 == "veml7700:" {print}' "$board" >> "$work/boot.repl"
printf '    Illuminance: 500\n' >> "$work/boot.repl"
sed 's|@platforms/boards/stm32h7_renode_reference_board.repl|@boot.repl|' \
    "$upstream" > "$work/boot.resc"
printf 'help\r' > "$work/help.txt"
printf 'kernel version\r' > "$work/kernel-version.txt"
printf 'sensor get veml7700@10 light\r' > "$work/sensor.txt"

printf 'sensor get iis2mdc@1e magn_xyz\r' > "$work/magnetometer.txt"

printf 'sensor get tmp108@48 ambient_temp\r' > "$work/temperature.txt"

cargo build --release
status=0
target/release/penv run --script "$work/boot.resc" \
    --platform-root "$renode" --asset-dir "$assets" --max-steps 2600000 \
    --uart-event "850000:$work/help.txt" \
    --uart-event "1300000:$work/kernel-version.txt" \
    --uart-event "1700000:$work/sensor.txt" \
    --uart-event "2100000:$work/magnetometer.txt" \
    --uart-event "2350000:$work/temperature.txt" \
    --stats > "$work/uart.log" 2> "$work/penv.log" || status=$?
printf 'STM32H7 UART and simulator logs: %s\n' "$work"
if [[ "$status" != 2 ]] ||
    ! rg -aqF 'Booting STM32H7 Renode Platform Demo' "$work/uart.log" ||
    ! rg -aqF 'demo:~$' "$work/uart.log" ||
    ! rg -aqF 'Available commands:' "$work/uart.log" ||
    ! rg -aqF 'Zephyr version 4.4.99' "$work/uart.log" ||
    ! rg -aq 'channel type=18\(light\).*\(499\.000000\)' "$work/uart.log" ||
    ! rg -aq 'channel type=11\(magn_xyz\).*\(0\.000000, 0\.000000, 0\.000000\)' "$work/uart.log" ||
    ! rg -aq 'channel type=13\(ambient_temp\).*\(23\.500000\)' "$work/uart.log"; then
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

# Check no-card command timeout, masking, and write-one-to-clear through the guest.
events=()
step=1000000
for command in '0x52007000 32 3' '0x5200703c 32 4' '0x5200700c 32 0x1108' \
    '0x52007034 32' '0xe000e204 32' '0x52007038 32 4' '0xe000e284 32 0x20000' \
    '0x52007034 32' '0xe000e204 32' '0x4000a804 32' '0x40040040 32'; do
    printf 'devmem %s\r' "$command" > "$work/sdmmc-$step.txt"
    events+=(--uart-event "$step:$work/sdmmc-$step.txt")
    step=$((step + 150000))
done
status=0
target/release/penv run --script "$work/boot.resc" \
    --platform-root "$renode" --asset-dir "$assets" --max-steps "$step" \
    "${events[@]}" > "$work/sdmmc-uart.log" 2> "$work/sdmmc-penv.log" || status=$?
expected=$(printf '%s\n' 0x80004 0x20000 0x80000 0x0 0x4 0x4f54310a)
actual=$(awk '/^Read value / { sub(/\r$/, ""); print $3 }' "$work/sdmmc-uart.log")
if [[ "$status" != 2 || "$actual" != "$expected" ]]; then
    cat "$work/sdmmc-penv.log" >&2
    cat "$work/sdmmc-uart.log" >&2
    exit 1
fi

# Port the shadowing and zero-line underrun checks from STM32LTDC.robot.
# DMA2D writes the source pixel and the final restore preserves the PPM check.
cp "$work/boot.repl" "$work/fill.repl"
printf '\ndma2dScratch: Memory.MappedMemory @ sysbus 0x60000000\n    size: 0x1000\n' >> "$work/fill.repl"
sed 's/@boot.repl/@fill.repl/' "$work/boot.resc" > "$work/fill.resc"
events=()
step=1000000
for command in '0x580244d4 32 0x10' '0x52001038 32 0x1234abcd' \
    '0x5200103c 32 0x60000000' '0x52001044 32 0x00010001' \
    '0x52001000 32 0x00030201' '0x52001004 32' '0x60000000 32' '0xe000e208 32' \
    '0x580244e4 32 0x8' '0x500010ac 32 0x60000000' '0x500010b0 32 0x0004000b' \
    '0x500010b4 32 1' '0x50001084 32 1' '0x50001024 32 1' '0x50001018 32 1' \
    '0x5000103c 32 0xf' '0x50001034 32 2' \
    '0x500010b4 32 0' '0x500010b4 32' '0x50001038 32' \
    '0x50001024 32 1' '0x50001038 32' '0xe000e208 32' \
    '0x500010b4 32 1' '0x50001024 32 1' '0x5000103c 32 0xf' '0x50001038 32'; do
    printf 'devmem %s\r' "$command" > "$work/ltdc-$step.txt"
    events+=(--uart-event "$step:$work/ltdc-$step.txt")
    step=$((step + 200000))
done
status=0
target/release/penv run --script "$work/fill.resc" \
    --platform-root "$renode" --asset-dir "$assets" --max-steps "$step" \
    "${events[@]}" --framebuffer-out "$work/frame.ppm" \
    > "$work/ltdc-uart.log" 2> "$work/ltdc-penv.log" || status=$?
expected=$(printf '%s\n' 0x2 0x1234abcd 0x4000000 0x0 0x0 0xa 0x5000000 0x0)
actual=$(awk '/^Read value / { sub(/\r$/, ""); print $3 }' "$work/ltdc-uart.log")
if [[ "$status" != 2 || "$actual" != "$expected" ]] ||
    ! printf 'P6\n1 1\n255\n\x34\xab\xcd' | cmp - "$work/frame.ppm"; then
    cat "$work/ltdc-penv.log" >&2
    cat "$work/ltdc-uart.log" >&2
    exit 1
fi

# Run the AES-128 ECB known-answer vector against the guest-visible CRYP FIFO.
events=()
step=1000000
for command in '0x48021030 32 0x00010203' '0x48021034 32 0x04050607' \
    '0x48021038 32 0x08090a0b' '0x4802103c 32 0x0c0d0e0f' \
    '0x48021014 32 2' '0x48021000 32 0x8020' '0x48021008 32 0x00112233' \
    '0x48021008 32 0x44556677' '0x48021008 32 0x8899aabb' '0x48021008 32 0xccddeeff' \
    '0x4802101c 32' '0x4802100c 32' '0x4802100c 32' '0x4802100c 32' '0x4802100c 32' \
    '0x4802101c 32' '0x580244e0 32 0x8' '0x58020c00 32 0x04000000' \
    '0x58020c18 32 0x2000' '0x580244e0 32 0xa' '0x58020400 32 0x200' \
    '0x58020420 32 0x20000' '0x580244e8 32 2' '0x4000042c 32 0x63' \
    '0x40000434 32 0x19' '0x40000418 32 0x60' '0x40000420 32 1' '0x40000400 32 1'; do
    printf 'devmem %s\r' "$command" > "$work/cryp-$step.txt"
    events+=(--uart-event "$step:$work/cryp-$step.txt")
    step=$((step + 150000))
done
status=0
target/release/penv run --script "$work/boot.resc" \
    --platform-root "$renode" --asset-dir "$assets" --max-steps "$step" \
    "${events[@]}" --stats > "$work/cryp-uart.log" 2> "$work/cryp-penv.log" || status=$?
expected=$(printf '%s\n' 0x2 0x69c4e0d8 0x6a7b0430 0xd8cdb780 0x70b4c55a 0x0)
actual=$(awk '/^Read value / { sub(/\r$/, ""); print $3 }' "$work/cryp-uart.log")
if [[ "$status" != 2 || "$actual" != "$expected" ]] ||
    ! rg -qxF 'redRgbLed=on' "$work/cryp-penv.log" ||
    ! rg -qxF 'greenRgbLed=off' "$work/cryp-penv.log" ||
    ! rg -qxF 'pwmled0=25.00%' "$work/cryp-penv.log"; then
    cat "$work/cryp-penv.log" >&2
    cat "$work/cryp-uart.log" >&2
    exit 1
fi
