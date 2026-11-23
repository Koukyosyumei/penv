#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."
penv=${PENV_BIN:-target/release/penv}
renode=${RENODE_ROOT:-../renode}
assets=${PENV_ASSET_DIR:-$(mktemp -d)}
base=${1:?Pass the directory printed by tests/stm32h7-zephyr-smoke.sh}
mkdir -p "$assets"
work=$(mktemp -d)
printf 'STM32H7 LSM6DSO logs: %s\n' "$work"
robot="$renode/tests/peripherals/LSM6DSO_IMU.robot"

for version in ZEPHYR420_BIN ZEPHYR440_BIN ZEPHYR440_BIN_INTERRUPT; do
    url=$(awk -v key="\${$version}" '$1 == key { print $2 }' "$robot")
    [[ -n $url ]]
    file="$assets/${url##*/}"
    if [[ ! -f $file ]]; then
        curl --fail --location "$url" --output "$file.part"
        mv "$file.part" "$file"
    fi
    printf '%s  %s\n' "${file##*-}" "$file" | sha1sum --check -
done

cp "$base/boot.repl" "$work/imu.repl"
cat >> "$work/imu.repl" <<'REPL'

usart3: UART.STM32F7_USART @ sysbus 0x40004800
    frequency: 125000000
    IRQ -> nvic@39

lpuart1: UART.STM32F7_USART @ sysbus 0x58000c00
    frequency: 125000000
    IRQ -> exti@64

lsm6dso: Sensors.LSM6DSO_IMU @ i2c1 0x6a
    Interrupt1 -> gpioPortC@13
REPL
cp "$renode/tests/peripherals/LSM6DSO-samples.csv" "$work/samples.csv"

for scenario in poll-basic poll-stream irq-basic irq-stream errata; do
    case "$scenario" in
        poll-*) version=ZEPHYR420_BIN ;;
        irq-*) version=ZEPHYR440_BIN_INTERRUPT ;;
        errata) version=ZEPHYR440_BIN ;;
    esac
    url=$(awk -v key="\${$version}" '$1 == key { print $2 }' "$robot")
    mkdir "$work/$scenario"
    script="$work/$scenario.resc"
    cat > "$script" <<RESC
mach create "$scenario"
machine LoadPlatformDescription @imu.repl
sysbus LoadELF @$url
cpu EnableZephyrMode
cpu VectorTableOffset \`sysbus GetSymbolAddress "_vector_table"\`
emulation RunFor "0.6"
RESC
    case "$scenario" in
        *-basic|errata)
            cat >> "$script" <<'RESC'
sysbus.i2c1.lsm6dso DefaultAccelerationX 1
sysbus.i2c1.lsm6dso DefaultAccelerationY -1
sysbus.i2c1.lsm6dso DefaultAccelerationZ 0
emulation RunFor "1.1"
RESC
            if [[ $scenario != errata ]]; then
                cat >> "$script" <<'RESC'
sysbus.i2c1.lsm6dso DefaultAngularRateX 100
sysbus.i2c1.lsm6dso DefaultAngularRateY 200
sysbus.i2c1.lsm6dso DefaultAngularRateZ 300
emulation RunFor "1.1"
RESC
            fi ;;
        *-stream)
            cat >> "$script" <<'RESC'
sysbus.i2c1.lsm6dso FeedSamplesFromCsv @samples.csv
emulation RunFor "4.1"
RESC
            ;;
    esac
    "$penv" run --script "$script" --platform-root "$renode" --asset-dir "$assets" \
        --max-steps 5000000 --uart-log-dir "$work/$scenario" \
        > "$work/$scenario/monitor.log" 2> "$work/$scenario/penv.log"
done

python3 - "$work" <<'PY'
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
zero_acc = 'accel x:0.000000 ms/2 y:0.000000 ms/2 z:0.000000 ms/2'
zero_gyro = 'gyro x:0.000000 rad/s y:0.000000 rad/s z:0.000000 rad/s'
static_acc = 'accel x:9.801001 ms/2 y:-9.801001 ms/2 z:0.000000 ms/2'
static_gyro = 'gyro x:1.745852 rad/s y:3.490483 rad/s z:5.236335 rad/s'
stream = [
    'accel x:9.801001 ms/2 y:19.601404 ms/2 z:4.900500 ms/2',
    'gyro x:5.000541 rad/s y:9.999861 rad/s z:5.000541 rad/s',
    'accel x:14.701502 ms/2 y:14.701502 ms/2 z:4.900500 ms/2',
    'gyro x:9.999861 rad/s y:5.000541 rad/s z:15.000403 rad/s',
    'accel x:19.601404 ms/2 y:9.801001 ms/2 z:4.900500 ms/2',
    'gyro x:1.500284 rad/s y:15.000403 rad/s z:9.999861 rad/s',
    zero_acc, zero_gyro,
]
for scenario in ['poll-basic', 'poll-stream', 'irq-basic', 'irq-stream', 'errata']:
    path = root / scenario / '0.uart.log'
    uart = iter(path.read_text().splitlines())
    expected = ['*** Booting Zephyr OS build']
    if scenario == 'errata':
        expected += [static_acc]
    else:
        if scenario.startswith('irq'):
            expected += ['Testing LSM6DSO sensor in trigger mode.']
        expected += [zero_acc, zero_gyro]
        expected += stream if scenario.endswith('stream') else [static_acc, static_gyro]
    for pattern in expected:
        if not any(pattern in line for line in uart):
            raise SystemExit(f'{path}: missing ordered UART output {pattern!r}')
    print(f'PASS {scenario}: {path}')
PY
