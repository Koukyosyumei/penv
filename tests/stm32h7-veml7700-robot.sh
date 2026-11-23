#!/usr/bin/env bash
# Port the VEML7700 Robot polling cases.
set -euo pipefail
cd "$(dirname "$0")/.."
renode=${RENODE_ROOT:-../renode}
assets=${PENV_ASSET_DIR:-$(mktemp -d)}
penv=${PENV_BIN:-target/release/penv}
logs=${1:?Pass the output directory}
robot="$renode/tests/peripherals/VEML7700.robot"
mkdir -p "$assets" "$logs"

failed=0
for scenario in ZEPHYR420_BIN ZEPHYR440_BIN frequency timestamp; do
    firmware=ZEPHYR420_BIN
    if [[ $scenario == ZEPHYR440_BIN ]]; then
        firmware=ZEPHYR440_BIN
    fi
    work="$logs/$scenario"
    mkdir -p "$work"
    url=$(awk -v firmware="\${$firmware}" '$1 == firmware {print $2}' "$robot")
    name=${url##*/}
    [[ -n $name ]]
    if [[ ! -f "$assets/$name" ]]; then
        curl --fail --location "$url" --output "$assets/$name.part"
        mv "$assets/$name.part" "$assets/$name"
    fi
    printf '%s  %s\n' "${name##*-}" "$assets/$name" | sha1sum --check -
    cat > "$work/board.repl" <<'REPL'
using "platforms/boards/nucleo_h753zi.repl"

als: Sensors.VEML7700 @ i2c1 0x10
REPL
    cat > "$work/case.resc" <<RESC
mach create "$scenario"
machine LoadPlatformDescription @board.repl
sysbus LoadELF @$url
tester ObserveUart sysbus.usart3
RESC
    if [[ $scenario == frequency || $scenario == timestamp ]]; then
        timing=(--frequency 1)
        if [[ $scenario == timestamp ]]; then
            timing=(--timestamp timestamp)
        fi
        "${PYTHON:-python3}" "$renode/tools/csv2resd/csv2resd.py" \
            --input "$renode/tests/peripherals/veml7700-samples.csv" \
            --map illuminance:illuminance::0 --start-time 0 "${timing[@]}" "$work/samples.resd"
        cat >> "$work/case.resc" <<'RESC'
sysbus.i2c1.als FeedIlluminanceSamplesFromRESD @samples.resd
tester WaitUart sysbus.usart3 8 line "lux: 0"
tester WaitUart sysbus.usart3 8 line "lux: 499"
tester WaitUart sysbus.usart3 8 line "lux: 12000"
tester WaitUart sysbus.usart3 8 line "lux: 12000"
RESC
    else
        cat >> "$work/case.resc" <<'RESC'
tester WaitUart sysbus.usart3 8 line "lux: 0"
sysbus.i2c1.als Illuminance 500
tester WaitUart sysbus.usart3 8 line "lux: 499"
RESC
    fi
    if [[ $scenario == ZEPHYR420_BIN ]]; then
        cat >> "$work/case.resc" <<'RESC'
sysbus.i2c1.als Illuminance 30
tester WaitUart sysbus.usart3 8 line "lux: 29"
sysbus.i2c1.als Illuminance 10000.5
tester WaitUart sysbus.usart3 8 line "lux: 10000"
RESC
    fi
    status=0
    "$penv" run --script "$work/case.resc" --platform-root "$renode" --asset-dir "$assets" \
        --max-steps 33000000 --uart-log-dir "$work" \
        > "$work/monitor.log" 2> "$work/penv.log" || status=$?
    printf '%s\n' "$status" > "$work/exit"
    if (( status != 0 )); then
        cat "$work/penv.log" >&2
        failed=1
    fi
done
exit "$failed"
