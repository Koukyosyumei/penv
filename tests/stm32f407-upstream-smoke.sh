#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."
work=${1:-$(mktemp -d)}
renode=${RENODE_ROOT:-../renode}
assets=${PENV_ASSET_DIR:-"$work/assets"}
mkdir -p "$work" "$assets"

tests/stm32f407-freertos-smoke.sh "$work" --upstream

run_case() {
    local name=$1 asset=$2 sha=$3 platform=$4 status=0
    local firmware="$assets/$asset"
    if [[ ! -f "$firmware" ]]; then
        curl --fail --location "https://dl.antmicro.com/projects/renode/$asset" --output "$firmware.part"
        mv "$firmware.part" "$firmware"
    fi
    printf '%s  %s\n' "$sha" "$firmware" | sha1sum --check -
    cp "$firmware" "$work/$name.elf"
    cat > "$work/$name.resc" <<RESC
mach create "$name"
machine LoadPlatformDescription @$platform
sysbus LoadELF @$name.elf
showAnalyzer sysbus.usart2
RESC
    target/release/penv run --script "$work/$name.resc" --platform-root "$renode" \
        --max-steps 100000 > "$work/$name.log" 2> "$work/$name-penv.log" || status=$?
    if [[ "$status" != 2 ]] || ! rg -q '^penv: instruction limit reached ' "$work/$name-penv.log"; then
        cat "$work/$name-penv.log" "$work/$name.log" >&2
        exit 1
    fi
}

# Port the TIM2 upcount and downcount cases from STM32F4_Discovery.robot.
while read -r direction label suffix sha; do
    name="tim2-$direction"
    run_case "$name" "stm32f4disco-timer-${direction}count.elf-$suffix-$sha" "$sha" platforms/cpus/stm32f4.repl
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
    platforms/boards/stm32f4_discovery-kit.repl
if ! rg -aqF 'Booting Zephyr OS' "$work/zephyr.log" ||
    ! rg -aqF 'Hello World! stm32f4_disco' "$work/zephyr.log"; then
    cat "$work/zephyr.log" >&2
    exit 1
fi
printf 'Zephyr: Hello World! stm32f4_disco\n'
