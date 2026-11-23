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
"$penv" run --platform "$renode/platforms/boards/nrf52840dk_nrf52840.repl" --platform-root "$renode" \
    --ble-peer "$out/hci-peripheral.elf" --ble-hci-controller "$out/hci-controller.elf" \
    --max-steps 340000000 "$out/hci-central.elf" >"$out/hci.log" 2>&1 || status=$?
cat "$out/hci.log"
if [ "$status" -ne 0 ] && [ "$status" -ne 2 ]; then
    exit "$status"
fi
awk '
    /^primary: .*Booting Zephyr OS build v3[.]6[.]0/ && !p { p = 1 }
    /^peer: .*Booting Zephyr OS build v3[.]6[.]0/ && !q { q = 1 }
    /^primary: Bluetooth initialized/ && p == 1 { p = 2 }
    /^peer: Bluetooth initialized/ && q == 1 { q = 2 }
    /^primary: Scanning successfully started/ && p == 2 { p = 3 }
    /^peer: Advertising successfully started/ && q == 2 { q = 3 }
    /^primary: Connected:/ && p == 3 { p = 4 }
    /^peer: Connected/ && q == 3 { q = 4 }
    /^peer: .*HRS notifications enabled/ && q == 4 { q = 5 }
    /^primary: \[SUBSCRIBED\]/ && p == 4 { p = 5 }
    /^primary: \[NOTIFICATION\]/ && p == 5 { p = 6 }
    END { exit !(p == 6 && q == 5) }
' "$out/hci.log"
