#!/usr/bin/env bash
# Port the LPS25HB Robot cases with runtime temperature and pressure changes.
set -euo pipefail
cd "$(dirname "$0")/.."
renode=${RENODE_ROOT:-../renode}
assets=${PENV_ASSET_DIR:-$(mktemp -d)}
penv=${PENV_BIN:-target/release/penv}
logs=${1:?Pass the output directory}
robot="$renode/tests/peripherals/LPS25HB.robot"
mkdir -p "$assets" "$logs"

failed=0
for scenario in ZEPHYR420_BIN ZEPHYR440_BIN resd; do
    firmware=$scenario
    if [[ $scenario == resd ]]; then
        firmware=ZEPHYR420_BIN
    fi
    work="$logs/$scenario"
    mkdir -p "$work"
    url=$(awk -v tag="\${$firmware}" '$1 == tag {sub(/^@/, "", $2); print $2}' "$robot")
    name=${url##*/}
    [[ -n $name ]]
    if [[ ! -f "$assets/$name" ]]; then
        curl --fail --location "$url" --output "$assets/$name.part"
        mv "$assets/$name.part" "$assets/$name"
    fi
    printf '%s  %s\n' "${name##*-}" "$assets/$name" | sha1sum --check -
    cat > "$work/board.repl" <<'REPL'
using "platforms/boards/nucleo_h753zi.repl"

lps25hb: Sensors.LPS25HB @ i2c1 0x5D
REPL
    cat > "$work/case.resc" <<RESC
mach create "$scenario"
machine LoadPlatformDescription @board.repl
sysbus LoadELF @$url
cpu EnableZephyrMode
tester ObserveUart sysbus.usart3
RESC
    set_environment() {
        printf 'sysbus.i2c1.lps25hb Temperature %s\nsysbus.i2c1.lps25hb Pressure %s\n' "$1" "$2"
    }
    check_environment() {
        printf 'tester WaitUart sysbus.usart3 1 line "temp %s Cel, pressure %s kPa"\n' "$1" "$2"
    }
    {
        if [[ $scenario == ZEPHYR420_BIN ]]; then
            printf 'tester WaitUart sysbus.usart3 8 line " Booting Zephyr OS build "\n'
            check_environment 42.50 0.000000
            set_environment 100 1013.25
            check_environment 100.31 101.325000
            set_environment 0 1013.25
            check_environment 0.74 101.325000
            set_environment -10 1013.25
            check_environment -9.31 101.325000
            set_environment 0 1000
            set_environment 0 200
            check_environment 0.74 100.000000
            set_environment 0 1300
            check_environment 0.74 100.000000
            set_environment 100 1013.25
            set_environment -200 1013.25
            check_environment 100.31 101.325000
            set_environment 200 1013.25
            check_environment 100.31 101.325000
        elif [[ $scenario == resd ]]; then
            "${PYTHON:-python3}" "$renode/tools/csv2resd/csv2resd.py" \
                --input "$renode/tests/peripherals/LPS25HB-samples.csv" \
                --map temperature:temp::0 --map pressure:pressure::0 \
                --start-time 70000000 --frequency 25 "$work/samples.resd" > "$work/resd.log" 2>&1
            set_environment 42.50 1013.25
            printf 'tester WaitUart sysbus.usart3 8 line " Booting Zephyr OS build "
'
            printf 'sysbus.i2c1.lps25hb FeedTemperatureSamplesFromRESD @samples.resd
'
            printf 'sysbus.i2c1.lps25hb FeedPressureSamplesFromRESD @samples.resd
'
            check_environment 25.90 101.325000
            check_environment 100.31 30.000000
            check_environment -9.31 40.000000
            check_environment 42.50 101.325000
            check_environment 42.50 101.325000
        else
            set_environment 100 1013.25
            check_environment 100.31 101.325000
        fi
    } >> "$work/case.resc"
    status=0
    "$penv" run --script "$work/case.resc" --platform-root "$renode" --asset-dir "$assets" \
        --max-steps 18000000 --uart-log-dir "$work" \
        > "$work/monitor.log" 2> "$work/penv.log" || status=$?
    printf '%s\n' "$status" > "$work/exit"
    if (( status != 0 )); then
        cat "$work/penv.log" >&2
        failed=1
    fi
done
exit "$failed"
