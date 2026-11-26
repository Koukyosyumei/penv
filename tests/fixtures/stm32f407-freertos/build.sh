#!/usr/bin/env bash
set -euo pipefail

fixture=$(cd "$(dirname "$0")" && pwd)
work=${1:?usage: build.sh OUTPUT_DIR [--upstream|--fpu]}
boot=boot.resc
defines=()
arch=( -mfloat-abi=soft )
port_name=ARM_CM3
case "${2:-}" in
    "") ;;
    --upstream)
        boot=upstream.resc
        defines=(-DF407_UPSTREAM_BOARD)
        ;;
    --fpu)
        boot=upstream.resc
        defines=(-DF407_UPSTREAM_BOARD -DF407_FPU)
        arch=( -mfloat-abi=hard -mfpu=fpv4-sp-d16 )
        port_name=ARM_CM4F
        ;;
    *) echo "usage: build.sh OUTPUT_DIR [--upstream|--fpu]" >&2; exit 1 ;;
esac
mkdir -p "$work"
work=$(cd "$work" && pwd)
revision=dbf70559b27d39c1fdb68dfb9a32140b6a6777a0
kernel="$work/FreeRTOS-Kernel"
if [[ ! -d "$kernel/.git" ]]; then
    git -c advice.detachedHead=false clone --quiet --depth 1 --branch V11.1.0 \
        https://github.com/FreeRTOS/FreeRTOS-Kernel.git "$kernel"
fi
if [[ "$(git -C "$kernel" rev-parse HEAD)" != "$revision" ]] ||
    [[ -n "$(git -C "$kernel" status --porcelain)" ]]; then
    echo "Expected a clean FreeRTOS-Kernel checkout at $revision" >&2
    exit 1
fi

port="$kernel/portable/GCC/$port_name"
"${CLANG:-clang}" --target=arm-none-eabi -mcpu=cortex-m4 -mthumb "${arch[@]}" \
    "${defines[@]}" -std=c11 -Os -ffreestanding -ffunction-sections -fdata-sections \
    -I"$fixture" -I"$fixture/freestanding" -I"$kernel/include" -I"$port" \
    -nostdlib -fuse-ld=lld -Wl,--gc-sections -T "$fixture/memory.ld" \
    "$fixture/startup.S" "$fixture/firmware.c" "$fixture/can.c" "$fixture/spi.c" "$fixture/i2c.c" "$fixture/freestanding/string.c" \
    "$kernel/tasks.c" "$kernel/list.c" "$kernel/queue.c" "$port/port.c" -o "$work/freertos.elf"
cp "$fixture/board.repl" "$work/"
cp "$fixture/$boot" "$work/boot.resc"
