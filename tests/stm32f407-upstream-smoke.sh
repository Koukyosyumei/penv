#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."
work=${1:-$(mktemp -d)}
renode=${RENODE_ROOT:-../renode}
assets=${PENV_ASSET_DIR:-"$work/assets"}
mkdir -p "$work" "$assets"

tests/stm32f407-freertos-smoke.sh "$work" --upstream
tests/stm32f407-freertos-smoke.sh "$work/cm4f" --fpu

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
    local name=$1 asset=$2 sha=$3 platform=$4 limit=$5 status=0
    shift 5
    fetch_asset "$asset" "$sha"
    cp "$assets/$asset" "$work/$name.elf"
    cat > "$work/$name.resc" <<RESC
mach create "$name"
machine LoadPlatformDescription @$platform
sysbus LoadELF @$name.elf
showAnalyzer sysbus.usart2
RESC
    target/release/penv run --script "$work/$name.resc" --platform-root "$renode" \
        --max-steps "$limit" "$@" > "$work/$name.log" 2> "$work/$name-penv.log" || status=$?
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

# Port the Zephyr hello-world case using the unchanged kit REPL.
sha=2180a4018e82fcbc8821ef4330c9b5f3caf2dcdb
run_case zephyr "stm32f4_discovery--zephyr-hello_world.elf-s_515008-$sha" "$sha" \
    platforms/boards/stm32f4_discovery-kit.repl 100000
if ! rg -aqF 'Booting Zephyr OS' "$work/zephyr.log" ||
    ! rg -aqF 'Hello World! stm32f4_disco' "$work/zephyr.log"; then
    cat "$work/zephyr.log" >&2
    exit 1
fi
printf 'Zephyr: Hello World! stm32f4_disco\n'

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
