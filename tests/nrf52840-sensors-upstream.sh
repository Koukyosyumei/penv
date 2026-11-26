#!/bin/sh
# BMP180.robot, LSM330.robot and LSM303.robot on nRF52840.
set -eu
if [ "$#" -ne 3 ]; then
    echo "Usage: $0 PENV RENODE_ROOT ARTIFACT_DIR" >&2
    exit 1
fi
penv=$1
renode=$2
out=$3
mkdir -p "$out"

asset() {
    path=$out/$1.elf
    remote=$2
    if [ ! -f "$path" ]; then
        curl -fsSL "https://dl.antmicro.com/projects/renode/$remote" -o "$path.part"
        mv "$path.part" "$path"
    fi
    printf '%s  %s\n' "${remote##*-}" "$path" | sha1sum --check
}

asset bmp180 BMP180_I2C.ino.arduino.mbed.nano33ble.elf-s_3127076-ba5f49cd34cd9549c2aa44f83af8e2011ecd1c22
asset lsm330 nano33ble--LSM330.arduino.mbed.elf-s_3002380-fb992eb29148d2cf83ff43b1255024364f1e3d79
asset lsm303 nrf52840--zephyr_lsm303dlhc.elf-s_740272-51b2a14ca50e54790a80d65ed347f04d7d36c373

run() {
    label=$1
    asset=$2
    step_limit=$3
    shift 3
    board=$out/$label.repl
    log=$out/$label.log
    printf 'using "platforms/cpus/nrf52840.repl"\n' >"$board"
    cat >>"$board"
    status=0
    "$penv" run --platform "$board" --platform-root "$renode" --max-steps "$step_limit" "$out/$asset.elf" >"$log" 2>&1 || status=$?
    if [ "$status" -ne 0 ] && [ "$status" -ne 2 ]; then
        cat "$log" >&2
        return "$status"
    fi
    tr -d '\000\r' <"$log" >"$out/$label.txt"
    expected=$(printf '%s\n' "$@")
    awk -v expected="$expected" '
        BEGIN { count = split(expected, lines, "\n"); row = 1 }
        row <= count && index($0, lines[row]) { row++ }
        END { exit !(row > count) }
    ' "$out/$label.txt"
    printf 'PASS %s %s\n' "$label" "$log"
}

check_temp() {
    run bmp180-temp bmp180 160000000 'Temperature: 24.00 degC' <<'REPL'
bmp180: Sensors.BMP180 @ twi0 0x77
    temperature: 24
REPL
}

check_pressure() {
    run bmp180-pressure bmp180 160000000 'Pressure: 231048.00 Pa' <<'REPL'
bmp180: Sensors.BMP180 @ twi0 0x77
    uncompensatedPressure: 1100
REPL
}

check_lsm330_accel() {
    run lsm330-accel lsm330 160000000 'Acceleration in X-Axis : 16383' 'Acceleration in Y-Axis : 49153' 'Acceleration in Z-Axis : 32766' <<'REPL'
lsm330_a: Sensors.LSM330_Accelerometer @ twi0 0x1d
    accelerationX: 1
    accelerationY: -1
    accelerationZ: 2
lsm330_g: Sensors.LSM330_Gyroscope @ twi0 0x6a
REPL
}

check_lsm330_gyro() {
    run lsm330-gyro lsm330 160000000 'X-Axis of Rotation :1300' 'Y-Axis of Rotation :52536' 'Z-Axis of Rotation :32500' <<'REPL'
lsm330_a: Sensors.LSM330_Accelerometer @ twi0 0x1d
lsm330_g: Sensors.LSM330_Gyroscope @ twi0 0x6a
    angularRateX: 100
    angularRateY: -100
    angularRateZ: 250
REPL
}

check_lsm303_accel() {
    run lsm303-accel lsm303 170000000 'Accelerometer data:' '( x y z ) = ( 11.998728  3.993192  -2.001384 )' <<'REPL'
lsm303dlhc_a: Sensors.LSM303DLHC_Accelerometer @ twi1 0x19
    [IRQ0,IRQ1] -> gpio0@[26,27]
    accelerationX: 12
    accelerationY: 4
    accelerationZ: -2
lsm303dlhc_g: Sensors.LSM303DLHC_Gyroscope @ twi1 0x1e
REPL
}

check_lsm303_mag() {
    run lsm303-mag lsm303 170000000 'Magnetometer data:' '( x y z ) = ( 20.000000  5.000000  -5.000000 )' <<'REPL'
lsm303dlhc_a: Sensors.LSM303DLHC_Accelerometer @ twi1 0x19
    [IRQ0,IRQ1] -> gpio0@[26,27]
lsm303dlhc_g: Sensors.LSM303DLHC_Gyroscope @ twi1 0x1e
    magneticFieldX: 20
    magneticFieldY: 5
    magneticFieldZ: -5
REPL
}

pids=
for check in check_temp check_pressure check_lsm330_accel check_lsm330_gyro check_lsm303_accel check_lsm303_mag; do
    "$check" &
    pids="$pids $!"
done
failed=0
for pid in $pids; do
    wait "$pid" || failed=1
done
test "$failed" -eq 0
