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
    local provision=
    if [[ "${1:-}" == --tim3-clock ]]; then
        # The two upstream FAULTMASK ELFs assume TIM3 is already clocked.
        provision='sysbus WriteDoubleWord 0x40023840 0x00000002'
        shift
    fi
    fetch_asset "$asset" "$sha"
    cp "$assets/$asset" "$work/$name.elf"
    cat > "$work/$name.resc" <<RESC
mach create "$name"
machine LoadPlatformDescription @$platform
sysbus LoadELF @$name.elf
showAnalyzer sysbus.usart2
$provision
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

# Port the TIM2 upcount and downcount cases from STM32F4_Discovery.robot.
while read -r direction label suffix sha; do
    name="tim2-$direction"
    run_case "$name" "stm32f4disco-timer-${direction}count.elf-$suffix-$sha" "$sha" platforms/cpus/stm32f4.repl 100000
    if ! rg -aqF "Timer2 $label Overflow Example" "$work/$name.log" ||
        ! rg -aqF 'Tim2 IRQ enabled' "$work/$name.log" ||
        ! awk '/Tim2 started/ { started = 1 }
            started && /period elapsed callback/ { callbacks++ }
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
run_case faultmask-masked "$faultmask" "$sha" platforms/cpus/stm32f4.repl 1000000 --tim3-clock
if ! rg -aqx 'Sleeping\r?' "$work/faultmask-masked.log" ||
    rg -aq 'Setting FAULTMASK to 0|Timer interrupt' "$work/faultmask-masked.log"; then
    cat "$work/faultmask-masked.log" >&2
    exit 1
fi
run_case faultmask "$faultmask" "$sha" platforms/cpus/stm32f4.repl 1800000 --tim3-clock
if ! awk '
    { sub(/\r$/, "") }
    $0 == "Setting FAULTMASK to 1" && state == 0 { state = 1 }
    $0 == "Timer IRQ enabled" && state == 1 { state = 2 }
    $0 == "Sleeping" && state == 2 { state = 3 }
    $0 == "Setting FAULTMASK to 0" && state == 3 { state = 4 }
    $0 == "Timer interrupt" { if (state != 4) exit 1; ++interrupts }
    END { if (interrupts < 3) exit 1 }' "$work/faultmask.log"; then
    cat "$work/faultmask.log" >&2
    exit 1
fi
sha=802f85357028150b4586bb3a54a5e44e7a3c2ec5
run_case faultmask-noclearing "stm32f4disco-faultmask-noclearing.elf-s_433676-$sha" "$sha" platforms/cpus/stm32f4.repl 500000 --tim3-clock
if ! awk '{ sub(/\r$/, "") } $0 == "Timer interrupt" { ++interrupts }
    END { exit interrupts < 4 }' "$work/faultmask-noclearing.log"; then
    cat "$work/faultmask-noclearing.log" >&2
    exit 1
fi
printf 'FAULTMASK: IRQ deferred until unmask, repeated IRQ after exception return\n'

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

while read -r name sha mips limit; do
    run_case "$name" "stm32f4disco-$name.elf-s_575008-$sha" "$sha" pwm-pins.repl "$limit" --virtual-mips "$mips"
    # Like the upstream case, disregard the initial calibration sample.
    if ! awk -v name="$name" '
        /^freq=[0-9]+ Hz duty=[0-9.]+ %/ {
            if (++samples != 2) next
            freq = substr($1, 6) + 0
            duty = substr($3, 6) + 0
            valid = freq >= 975 && freq <= 1025 && duty >= 32 && duty <= 34
            printf "%s: %d Hz, %.1f%% duty\n", name, freq, duty
        }
        END { exit !valid }' "$work/$name.log"; then
        cat "$work/$name.log" >&2
        exit 1
    fi
done <<'ASSETS'
pwm-capture 5743073970bd27bb63e78a9a3990ed0b9e8db373 1 1200000
pwm-capture-four-channel-capture-support af21ada0861c0be7406f52dbe4280d413b96fbd4 24 26500000
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
