#!/usr/bin/env bash
set -euo pipefail

fixture=$(cd "$(dirname "$0")" && pwd)
work=${1:?usage: build.sh OUTPUT_DIR}
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

port="$kernel/portable/GCC/ARM_CM3"
"${CLANG:-clang}" --target=arm-none-eabi -mcpu=cortex-m4 -mthumb -mfloat-abi=soft \
    -std=c11 -Os -ffreestanding -ffunction-sections -fdata-sections \
    -I"$fixture" -I"$fixture/freestanding" -I"$kernel/include" -I"$port" \
    -nostdlib -fuse-ld=lld -Wl,--gc-sections -T "$fixture/memory.ld" \
    "$fixture/startup.S" "$fixture/firmware.c" "$fixture/can.c" "$fixture/spi.c" "$fixture/i2c.c" "$fixture/freestanding/string.c" \
    "$kernel/tasks.c" "$kernel/list.c" "$kernel/queue.c" "$port/port.c" -o "$work/freertos.elf"
cp "$fixture/board.repl" "$fixture/boot.resc" "$work/"
