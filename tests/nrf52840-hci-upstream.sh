#!/bin/sh
# NRF52840.robot / Should Run Bluetooth Hci Uart Sample.
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
    name=$1
    remote=$2
    if [ ! -f "$out/$name" ]; then
        curl -fsSL "https://dl.antmicro.com/projects/renode/$remote" -o "$out/$name.part"
        mv "$out/$name.part" "$out/$name"
    fi
    printf '%s  %s\n' "${remote##*-}" "$out/$name" | sha1sum --check
}

asset hci-central.elf nrf52840dk--zephyr-ble_central_hr_hci_uart.elf-s_3302344-e0d303c14ad52fda2b16157c09ce62f1d065bc31
asset hci-peripheral.elf nrf52840dk--zephyr-ble_peripheral_hr_hci_uart.elf-s_3094464-d15ff2ff7cf4ba4105b1896b445b5e4baeb7a487
asset hci-controller.elf nrf52840dk--zephyr-ble_hci_uart.elf-s_2437336-4bf9cccb70ca95fb12667896eec3f777f39f90ee

status=0
python3 "$(dirname "$0")/nrf52840-ble-uart.py" hci "$out/hci.log" -- \
    "$penv" run --platform "$renode/platforms/boards/nrf52840dk_nrf52840.repl" --platform-root "$renode" \
    --uart-steps --ble-peer "$out/hci-peripheral.elf" --ble-hci-controller "$out/hci-controller.elf" \
    --max-steps 8900000000 "$out/hci-central.elf" || status=$?
cat "$out/hci.log"
exit "$status"
