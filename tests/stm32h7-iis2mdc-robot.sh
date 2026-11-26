#!/usr/bin/env bash
# Port the polling and trigger scenarios in IIS2MDC.robot.
set -euo pipefail
cd "$(dirname "$0")/.."
renode=${RENODE_ROOT:-../renode}
assets=${PENV_ASSET_DIR:-$(mktemp -d)}
penv=${PENV_BIN:-target/release/penv}
logs=${1:?Pass the output directory}
robot="$renode/tests/peripherals/IIS2MDC.robot"
mkdir -p "$assets" "$logs"
"${PYTHON:-python3}" "$renode/tools/csv2resd/csv2resd.py" \
    --input "$renode/tests/peripherals/iis2mdc-samples.csv" \
    --map magnetic_flux_density:magnetic_flux_density_x,magnetic_flux_density_y,magnetic_flux_density_z:x,y,z \
    --start-time 3000000000 --frequency 2 --timestamp timestamp "$logs/samples.resd"

failed=0
for mode in POLLING TRIGGER; do
    work="$logs/$mode"
    mkdir -p "$work"
    url=$(awk -v tag="\${${mode}_BIN}" '$1 == tag {print $2}' "$robot")
    name=${url##*/}
    [[ -n $name ]]
    if [[ ! -f "$assets/$name" ]]; then
        curl --fail --location "$url" --output "$assets/$name.part"
        mv "$assets/$name.part" "$assets/$name"
    fi
    printf '%s  %s\n' "${name##*-}" "$assets/$name" | sha1sum --check -
    cat > "$work/board.repl" <<'REPL'
using "platforms/cpus/stm32h753.repl"

iis2mdc: Sensors.IIS2MDC @ i2c1 0x1e
    -> gpioPortG@11
REPL
    cat > "$work/case.resc" <<RESC
mach create "$mode"
machine LoadPlatformDescription @board.repl
sysbus LoadELF @$url
tester ObserveUart sysbus.usart2
sysbus.i2c1.iis2mdc FeedMagneticSamplesFromRESD @../samples.resd
RESC
    while read -r x y z; do
        if [[ $mode == POLLING ]]; then
            printf 'tester WaitUart sysbus.usart2 8 line "( x y z ) = ( %s  %s  %s )"\n' "$x" "$y" "$z"
        else
            printf 'tester WaitUart sysbus.usart2 8 line "iis2mdc@1e (x, y, z):    (%12s, %12s, %12s)"\n' "$x" "$y" "$z"
        fi
    done >> "$work/case.resc" <<'VALUES'
0.150000 0.300000 0.001500
0.150000 0.000000 0.000000
0.000000 0.150000 0.000000
0.000000 0.000000 0.150000
-0.150000 -0.300000 -0.001500
-0.150000 0.000000 0.000000
0.000000 -0.150000 0.000000
0.000000 0.000000 -0.150000
-1.234500 0.678000 0.000000
VALUES
    status=0
    "$penv" run --script "$work/case.resc" --platform-root "$renode" --asset-dir "$assets" \
        --max-steps 74000000 --uart-log-dir "$work" > "$work/monitor.log" 2> "$work/penv.log" || status=$?
    printf '%s\n' "$status" > "$work/exit"
    if (( status != 0 )); then
        cat "$work/penv.log" >&2
        failed=1
    fi
done
exit "$failed"
