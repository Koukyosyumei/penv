#!/bin/sh
# Run the nRF52840 acceptance matrix from one release build.
set -eu

if [ "$#" -ne 3 ]; then
    echo "Usage: $0 PENV RENODE_ROOT ARTIFACT_DIR" >&2
    exit 1
fi

penv=$1
renode=$2
artifacts=$3
test_dir=$(dirname "$0")
mkdir -p "$artifacts"

asset() {
    path="$artifacts/$1"
    remote=$2
    if [ ! -f "$path" ]; then
        curl -fsSL "https://dl.antmicro.com/projects/renode/$remote" -o "$path.part"
        mv "$path.part" "$path"
    fi
    printf '%s  %s\n' "${remote##*-}" "$path" | sha1sum --check
}

# The peripheral name in NRF52840.robot uses an unavailable underscore URL.
asset radio-central.elf nrf52840--zephyr-bluetooth_central_hr.elf-s_3380332-316e27f81dcda3c2b0e7f2c3516001e7b27ad051
asset radio-peripheral.elf nrf52840--zephyr-bluetooth_peripheral_hr.elf-s_3217940-7b59adc9629f8be90067b131e663a13d2d4bb711

run_case() {
    name=$1
    set -- "$penv" "$renode"
    case $name in
        audio-upstream) set -- "$@" "$artifacts/audio" ;;
        ble-central) set -- "$@" "$artifacts/radio-central.elf" ;;
        ble-pair) set -- "$@" "$artifacts/radio-central.elf" "$artifacts/radio-peripheral.elf" ;;
    esac
    status=0
    "$test_dir/nrf52840-$name.sh" "$@" >"$artifacts/$name.log" 2>&1 || status=$?
    printf '%s\n' "$status" >"$artifacts/$name.exit"
    if [ "$status" -ne 0 ]; then
        printf 'FAIL %s (%s) %s\n' "$name" "$status" "$artifacts/$name.log" >&2
        return "$status"
    fi
    printf 'PASS %s %s\n' "$name" "$artifacts/$name.log"
}

pids=
for name in audio-upstream ble-central ble-pair ecb egu i2s pdm radio rng spi timer twi; do
    run_case "$name" &
    pids="$pids $!"
done
failed=0
for pid in $pids; do
    wait "$pid" || failed=1
done
test "$failed" -eq 0
printf 'PASS all nRF52840 firmware groups\n'
