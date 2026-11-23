#!/bin/sh
# nRF52840-only port of the Pigweed case's Observer-side UART assertion.
set -eu
if [ "$#" -ne 5 ]; then
    echo "Usage: $0 PENV RENODE_ROOT OBSERVER_ELF CONTROLLER_ELF ARTIFACT_DIR" >&2
    exit 1
fi
penv=$1
root=$2
observer=$3
controller=$4
out=$5
mkdir -p "$out"
printf '%s  %s\n' 3cdf09a1f95f708618a3600a360696b7d5c3a988 "$observer" \
    4bf9cccb70ca95fb12667896eec3f777f39f90ee "$controller" | sha1sum --check

cat > "$out/advertiser.ld" <<'LINKER'
ENTRY(reset)
SECTIONS {
    .vectors 0 : { KEEP(*(.vectors)) }
    .text : { *(.text*) *(.rodata*) }
    .ARM.exidx : { *(.ARM.exidx*) }
}
LINKER
clang --target=arm-none-eabi -mcpu=cortex-m4 -mthumb -Os -ffreestanding -nostdlib \
    -fuse-ld=lld -Wl,-T,"$out/advertiser.ld" "$(dirname "$0")/nrf52840-observer-advertiser.c" \
    -o "$out/advertiser.elf"
status=0
"$penv" run --platform "$root/platforms/boards/nrf52840dk_nrf52840.repl" \
    --platform-root "$root" --uart-steps --ble-peer "$out/advertiser.elf" \
    --ble-hci-controller "$controller" --max-steps 145000000 "$observer" \
    > "$out/observer.log" 2>&1 || status=$?
printf '%s\n' "$status" > "$out/observer.status"
cat "$out/observer.log"
[ "$status" -eq 2 ]
awk '
    /primary: Started scanning[.][.][.]/ { scanning = 1 }
    scanning && /primary: Device found: [0-9A-F:]+ \(random\) \(RSSI -10\), type 2, AD data len 28/ { found = 1 }
    END { exit !found }
' "$out/observer.log"
