#!/usr/bin/env bash
# Port the five cases in the fixed LSM6DSO_IMU.robot suite.
set -euo pipefail
cd "$(dirname "$0")/.."
renode=${RENODE_ROOT:-../renode}
assets=${PENV_ASSET_DIR:-$(mktemp -d)}
penv=${PENV_BIN:-target/release/penv}
logs=${1:?Pass the output directory}
robot="$renode/tests/peripherals/LSM6DSO_IMU.robot"
mkdir -p "$assets" "$logs"
"${PYTHON:-python3}" "$renode/tools/csv2resd/csv2resd.py" \
    --input "$renode/tests/peripherals/LSM6DSO-samples.csv" \
    --map angular_rate:gyro_x,gyro_y,gyro_z:x,y,z \
    --map acceleration:accel_x,accel_y,accel_z:x,y,z \
    --start-time 100000000 --frequency 12 "$logs/samples.resd"

wait_acc() { printf 'tester WaitUart sysbus.usart3 2 line "accel x:%s ms/2 y:%s ms/2 z:%s ms/2"\n' "$@"; }
wait_gyro() { printf 'tester WaitUart sysbus.usart3 2 line "gyro x:%s rad/s y:%s rad/s z:%s rad/s"\n' "$@"; }

failed=0
for scenario in poll-basic poll-stream irq-basic irq-stream errata; do
    case "$scenario" in
        poll-*) version=ZEPHYR420_BIN ;;
        irq-*) version=ZEPHYR440_BIN_INTERRUPT ;;
        errata) version=ZEPHYR440_BIN ;;
    esac
    url=$(awk -v key="\${$version}" '$1 == key { print $2 }' "$robot")
    name=${url##*/}
    [[ -n $name ]]
    if [[ ! -f "$assets/$name" ]]; then
        curl --fail --location "$url" --output "$assets/$name.part"
        mv "$assets/$name.part" "$assets/$name"
    fi
    printf '%s  %s\n' "${name##*-}" "$assets/$name" | sha1sum --check -
    work="$logs/$scenario"
    mkdir -p "$work"
    cat > "$work/board.repl" <<'REPL'
using "platforms/boards/nucleo_h753zi.repl"

lsm6dso: Sensors.LSM6DSO_IMU @ i2c1 0x6A
    Interrupt1 -> gpioPortC@13
REPL
    cat > "$work/case.resc" <<RESC
mach create "$scenario"
machine LoadPlatformDescription @board.repl
sysbus LoadELF @$url
cpu EnableZephyrMode
tester ObserveUart sysbus.usart3
tester WaitUart sysbus.usart3 8 line " Booting Zephyr OS build "
RESC
    {
        if [[ $scenario != errata ]]; then
            wait_acc 0.000000 0.000000 0.000000
            wait_gyro 0.000000 0.000000 0.000000
        fi
        case "$scenario" in
            *-stream)
                printf '%s\n' 'sysbus.i2c1.lsm6dso FeedAccelerationSamplesFromRESD @../samples.resd'
                printf '%s\n' 'sysbus.i2c1.lsm6dso FeedAngularRateSamplesFromRESD @../samples.resd'
                wait_acc 9.801001 19.601404 4.900500
                wait_gyro 5.000541 9.999861 5.000541
                wait_acc 14.701502 14.701502 4.900500
                wait_gyro 9.999861 5.000541 15.000403
                wait_acc 19.601404 9.801001 4.900500
                wait_gyro 1.500284 15.000403 9.999861
                wait_acc 0.000000 0.000000 0.000000
                wait_gyro 0.000000 0.000000 0.000000
                ;;
            *)
                printf 'sysbus.i2c1.lsm6dso DefaultAcceleration%s\n' 'X 1' 'Y -1' 'Z 0'
                wait_acc 9.801001 -9.801001 0.000000
                if [[ $scenario != errata ]]; then
                    printf 'sysbus.i2c1.lsm6dso DefaultAngularRate%s\n' 'X 100' 'Y 200' 'Z 300'
                    wait_gyro 1.745852 3.490483 5.236335
                fi
                ;;
        esac
    } >> "$work/case.resc"
    status=0
    "$penv" run --script "$work/case.resc" --platform-root "$renode" --asset-dir "$assets" \
        --max-steps 30000000 --uart-log-dir "$work" > "$work/monitor.log" 2> "$work/penv.log" || status=$?
    printf '%s\n' "$status" > "$work/exit"
    if (( status != 0 )); then
        cat "$work/penv.log" >&2
        failed=1
    fi
done
exit "$failed"
