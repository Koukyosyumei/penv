#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."
work=${1:-$(mktemp -d)}
renode=${RENODE_ROOT:-../renode}
assets=${PENV_ASSET_DIR:-"$work/assets"}
mkdir -p "$work" "$assets"

tests/stm32f407-freertos-smoke.sh "$work" --upstream
tests/stm32f407-freertos-smoke.sh "$work/cm4f" --fpu
tests/stm32f407-freertos-smoke.sh "$work/loopback"

# STM32F4_Discovery.robot: Peripheral Export File Created.
map="$work/peripherals.json"
target/release/penv peripherals export "$map" \
    --platform "$renode/platforms/cpus/stm32f4.repl" --platform-root "$renode"
test -f "$map" && test -s "$map"
for marker in 'Peripheral Map 1.0' SynopsysEthernetMAC STM32_GPIOPort STM32F4_RCC; do
    rg -qF "$marker" "$map"
done
printf 'Peripheral Export File Created: %s\n' "$map"

fetch_asset() {
    local asset=$1 sha=$2
    local firmware="$assets/$asset"
    if [[ ! -f "$firmware" ]]; then
        curl --fail --location "https://dl.antmicro.com/projects/renode/$asset" --output "$firmware.part"
        mv "$firmware.part" "$firmware"
    fi
    printf '%s  %s\n' "$sha" "$firmware" | sha1sum --check -
}

sha=827a0dedd3790f4559d7518320006613768b5e72
fetch_asset "stm32f4discovery.elf-s_445441-$sha" "$sha"
status=0
target/release/penv run --script "$renode/scripts/single-node/stm32f4_discovery.resc" \
    --platform-root "$renode" --asset-dir "$assets" --max-steps 16000000 \
    > "$work/contiki.log" 2> "$work/contiki-penv.log" || status=$?
if [[ "$status" != 2 ]] ||
    ! rg -q '^penv: instruction limit reached ' "$work/contiki-penv.log" ||
    ! rg -aqF 'Contiki 3.x started.' "$work/contiki.log" ||
    ! rg -aqF 'ETH_BSP_Config done!' "$work/contiki.log" ||
    ! rg -aqF 'UDP server started' "$work/contiki.log"; then
    cat "$work/contiki-penv.log" "$work/contiki.log" >&2
    exit 1
fi
rg -a 'Contiki 3.x started|ETH_BSP_Config done|UDP server started' "$work/contiki.log"

run_case() {
    local name=$1 asset=$2 sha=$3 platform=$4 limit=$5
    shift 5
    fetch_asset "$asset" "$sha"
    cp "$assets/$asset" "$work/$name.elf"
    cat > "$work/$name.resc" <<RESC
mach create "$name"
machine LoadPlatformDescription @$platform
sysbus LoadELF @$name.elf
showAnalyzer sysbus.usart2
RESC
    run_to_limit "$name" --script "$work/$name.resc" --max-steps "$limit" "$@"
}

run_to_limit() {
    local name=$1 status=0
    shift
    target/release/penv run --platform-root "$renode" "$@" \
        > "$work/$name.log" 2> "$work/$name-penv.log" || status=$?
    if [[ "$status" != 2 ]] || ! rg -q '^penv: instruction limit reached ' "$work/$name-penv.log"; then
        cat "$work/$name-penv.log" "$work/$name.log" >&2
        exit 1
    fi
}

# arm-cortex-m-fpu.robot: Should Pass Context Stacking Preservation Test v7-M.
cat > "$work/fpu-v7m.repl" <<'REPL'
mem: Memory.MappedMemory @ sysbus 0x0
    size: 0x40000
uart: UART.NS16550 @ sysbus 0x80000
cpu: CPU.CortexM @ sysbus
    cpuType: "cortex-m4"
    nvic: nvic
nvic: IRQControllers.NVIC @ sysbus 0xe000e000
    -> cpu@0
REPL
sha=d49ec01cc0c6270f7055330871953182a507efc6
asset="cortex-v7m-extended-frame-test.elf-s_91928-$sha"
fetch_asset "$asset" "$sha"
cp "$assets/$asset" "$work/fpu-v7m.elf"
cat > "$work/fpu-v7m.resc" <<'RESC'
mach create
machine LoadPlatformDescription @fpu-v7m.repl
sysbus LoadELF @fpu-v7m.elf
cpu VectorTableOffset `sysbus GetSymbolAddress "vector_table"`
RESC
run_to_limit fpu-v7m --script "$work/fpu-v7m.resc" --max-steps 100000
if ! rg -aqx 'Test succeeded\r?' "$work/fpu-v7m.log"; then
    cat "$work/fpu-v7m.log" >&2
    exit 1
fi
printf 'Cortex-M4F context stacking: Test succeeded\n'

# Port the TIM2 upcount and downcount cases from STM32F4_Discovery.robot.
while read -r direction label suffix sha; do
    name="tim2-$direction"
    run_case "$name" "stm32f4disco-timer-${direction}count.elf-$suffix-$sha" "$sha" platforms/cpus/stm32f4.repl 100000
    if ! awk -v banner="*** Timer2 $label Overflow Example ***" '
            { sub(/^\000/, ""); sub(/\r$/, "") }
            $0 == banner && state == 0 { state = 1 }
            $0 == "Tim2 IRQ enabled" && state == 1 { state = 2 }
            $0 == "Tim2 started" && state == 2 { state = 3 }
            $0 == "period elapsed callback" && state == 3 { callbacks++ }
            END { exit callbacks < 3 }' "$work/$name.log"; then
        cat "$work/$name.log" >&2
        exit 1
    fi
    printf 'TIM2 %s: at least three update callbacks after start\n' "$direction"
done <<'ASSETS'
up Upcount g2d98d1b-s_1021132 961284be838516abea9db8302c9af2dcb67b482a
down Downcount g2d98d1b-s_1021136 4995992fa219c49c38d7163da1381104c26c823a
ASSETS

sha=080256edf201b1e2f7c67bf15000ba1ffa031990
faultmask="stm32f4disco-faultmask.elf-s_434744-$sha"
run_case faultmask "$faultmask" "$sha" platforms/cpus/stm32f4.repl 2600000 \
    --f4-repl-timing --uart-transcript "$work/faultmask.tsv"
if ! awk -F '\t' '
    $3 == "Setting FAULTMASK to 1" && state == 0 { state = 1; next }
    $3 == "Timer IRQ enabled" && state == 1 { state = 2; next }
    $3 == "Sleeping" && state == 2 { state = 3; sleep_end = $2; next }
    state == 3 {
        if ($3 != "Setting FAULTMASK to 0" || $1 - sleep_end < 1000000) invalid = 1
        state = 4
        printf "FAULTMASK: UART idle for %.6f s after Sleeping\n", ($1 - sleep_end) / 1000000
        next
    }
    $3 == "Timer interrupt" { if (state != 4) invalid = 1; ++interrupts }
    END { exit invalid || interrupts < 3 }' "$work/faultmask.tsv"; then
    cat "$work/faultmask.tsv" >&2
    exit 1
fi
sha=802f85357028150b4586bb3a54a5e44e7a3c2ec5
run_case faultmask-noclearing "stm32f4disco-faultmask-noclearing.elf-s_433676-$sha" "$sha" platforms/cpus/stm32f4.repl 500000 --f4-repl-timing
if ! awk '{ sub(/\r$/, "") } $0 == "Timer interrupt" { ++interrupts }
    END { exit interrupts < 4 }' "$work/faultmask-noclearing.log"; then
    cat "$work/faultmask-noclearing.log" >&2
    exit 1
fi
printf 'FAULTMASK: repeated IRQ after exception return\n'

# Use the original RESC with the Zephyr ELF override from the Robot case.
sha=2180a4018e82fcbc8821ef4330c9b5f3caf2dcdb
zephyr="stm32f4_discovery--zephyr-hello_world.elf-s_515008-$sha"
fetch_asset "$zephyr" "$sha"
run_to_limit zephyr "$assets/$zephyr" --script "$renode/scripts/single-node/stm32f4_discovery.resc" \
    --max-steps 100000
if ! rg -aqF 'Booting Zephyr OS' "$work/zephyr.log" ||
    ! rg -aqF 'Hello World! stm32f4_disco' "$work/zephyr.log"; then
    cat "$work/zephyr.log" >&2
    exit 1
fi
printf 'Zephyr: Hello World! stm32f4_disco\n'

sha=32b7284e794fd3ff5398ae120d77fabe7b0f832f
run_case rtc-writes "stm32f4disco-rtc-modified-test.elf-s_537004-$sha" "$sha" platforms/cpus/stm32f4.repl 100000
if ! awk '
    /^RTC date and time: 2006-01-09 00:00:[0-9][0-9]/ && state == 0 { state = 1 }
    /^RTC date and time: 2006-10-09 20:00:[0-9][0-9]/ && state == 1 { state = 2 }
    /^RTC date and time: 2006-10-09 18:00:[0-9][0-9]/ && state == 2 { state = 3 }
    END { exit state != 3 }' "$work/rtc-writes.log"; then
    cat "$work/rtc-writes.log" >&2
    exit 1
fi
printf 'RTC: month and hour writes preserved the expected calendar values\n'

cat > "$work/rtc-32khz.repl" <<'REPL'
using "platforms/cpus/stm32f4.repl"

rtc:
    wakeupTimerFrequency: 32000
REPL
printf s > "$work/rtc-start.txt"
sha=ca2effb6a0a8bcde39496b99bcb8b160a4ed292e
rtc_limit=11100000
run_case rtc-alarm "stm32f4_discovery--riot-tests_periph_rtc.elf-s_1249644-$sha" "$sha" \
    rtc-32khz.repl "$rtc_limit" --f4-repl-timing --uart-transcript "$work/rtc-alarm.tsv" \
    --uart-reply 'Help: Press s to start test, r to print it is ready' "$work/rtc-start.txt"
if ! awk -F '\t' -v limit="$rtc_limit" '
    $3 == "Help: Press s to start test, r to print it is ready" && state == 0 { state = 1; next }
    $3 ~ /This is RIOT!/ && state == 1 { state = 2; next }
    $3 == "RIOT RTC low-level driver test" && state == 2 { state = 3; next }
    $3 ~ /^This test will display .Alarm!. every 2 seconds for 4 times$/ && state == 3 { state = 4; next }
    $3 == "Alarm!" {
        if (state != 4 || int($2 / 1000000) != 2 * ++alarms) invalid = 1
        printf "RTC: Alarm! at %.6f s\n", $2 / 1000000
    }
    END { exit invalid || alarms != 4 || $3 != "Alarm!" || limit - $2 < 3000000 }' "$work/rtc-alarm.tsv"; then
    cat "$work/rtc-alarm.tsv" >&2
    exit 1
fi
printf 'RTC: four RIOT alarms followed by three seconds of UART silence\n'

while read -r name size sha; do
    run_case "$name" "stm32f4--cube_mx-hello_world.elf-s_$size-$sha" "$sha" platforms/cpus/stm32f4.repl 3500000
    if ! awk '/^Hello World!\r?$/ { ++lines } END { exit lines < 3 }' "$work/$name.log"; then
        cat "$work/$name.log" >&2
        exit 1
    fi
    printf '%s: at least three Hello World! lines\n' "$name"
done <<'ASSETS'
cubemx 625976 606092c29de896f3bd83a4e981f2c7f3a6ed3142
cubemx-flash-latency 625992 119d2b1d81ef6bb85498d1024c61736bb53cee4c
ASSETS

# The overlays reproduce the Robot test's patch cables without changing the CPU REPL.
cat > "$work/pwm-pins.repl" <<'REPL'
using "platforms/cpus/stm32f4.repl"

gpioPortA: { 6 -> gpioPortD@12 }
REPL
cat > "$work/qdec-pins.repl" <<'REPL'
using "platforms/cpus/stm32f4.repl"

gpioPortD:
    12 -> gpioPortA@6
    13 -> gpioPortA@7
REPL

# Renode BaseCPU defaults to 100 MIPS; both peripherals keep the REPL clocks.
while read -r name sha; do
    run_case "$name" "stm32f4disco-$name.elf-s_575008-$sha" "$sha" pwm-pins.repl 250000000 \
        --virtual-mips 100 --f4-repl-timing
    # Robot skips the first matching frequency and captures only the integer part of duty.
    if ! awk -v name="$name" '
        !first && /freq=[[:alnum:]_]+ / { first = 1; next }
        first && !checked && match($0, /freq=[0-9]+ .*duty=[0-9]+/) {
            checked = 1
            capture = substr($0, RSTART, RLENGTH)
            freq = substr(capture, 6) + 0
            sub(/^.*duty=/, "", capture)
            duty = capture + 0
            valid = freq >= 975 && freq <= 1025 && duty >= 32 && duty <= 34
            printf "%s: %d Hz, %d%% duty (Robot capture)\n", name, freq, duty
        }
        END { exit !valid }' "$work/$name.log"; then
        cat "$work/$name.log" >&2
        exit 1
    fi
done <<'ASSETS'
pwm-capture 5743073970bd27bb63e78a9a3990ed0b9e8db373
pwm-capture-four-channel-capture-support af21ada0861c0be7406f52dbe4280d413b96fbd4
ASSETS

sha=c521785d34d4e4afd660947f14dd6123972a7ff8
run_case qdec "stm32f4disco-qdec.elf-s_728468-$sha" "$sha" qdec-pins.repl 2100000
if ! awk '
    /^Position = [0-9]+ degrees/ {
        ++samples
        if (samples > 2) next
        expected = 10 * samples
        if ($3 < expected - 1 || $3 > expected + 1) invalid = 1
        printf "Quadrature: %d degrees\n", $3
    }
    END { exit samples < 2 || invalid }' "$work/qdec.log"; then
    cat "$work/qdec.log" >&2
    exit 1
fi
