#!/bin/sh
# SPIMultiplexer.robot / Should Talk to Two SPI Devices.
set -eu
if [ "$#" -ne 3 ]; then
    echo "Usage: $0 PENV RENODE_ROOT ARTIFACT_DIR" >&2
    exit 1
fi
penv=$1
renode=$2
out=$3
mkdir -p "$out"
remote=nrf52840--zephyr-custom_spi_sensors_cs.elf-s_1125736-5cf7322357c0434481335500e9eb38ebe44948c3
elf=$out/mux.elf
if [ ! -f "$elf" ]; then
    curl -fsSL "https://dl.antmicro.com/projects/renode/$remote" -o "$elf.part"
    mv "$elf.part" "$elf"
fi
printf '%s  %s\n' "${remote##*-}" "$elf" | sha1sum --check
cat >"$out/mux.repl" <<'REPL'
using "platforms/cpus/nrf52840.repl"
spi_mux: SPI.SPIMultiplexer @ spi2
    init:
        SetActiveLow 0
        SetActiveLow 1
dummy0spi: Mocks.DummySPISlave @ spi_mux 0
    init:
        EnqueueValue 0x02
        EnqueueValue 0xB0
dummy1spi: Mocks.DummySPISlave @ spi_mux 1
    init:
        EnqueueValue 0xFF
        EnqueueValue 0xFF
        EnqueueValue 0xFF
        EnqueueValue 0xFF
        EnqueueValue 0xFF
        EnqueueValue 0x01
        EnqueueValue 0x00
        EnqueueValue 0x02
        EnqueueValue 0x00
        EnqueueValue 0x01
        EnqueueValue 0x00
gpio0:
    22 -> spi_mux@0
    23 -> spi_mux@1
REPL
status=0
"$penv" run --platform "$out/mux.repl" --platform-root "$renode" --max-steps 130000000 "$elf" >"$out/mux.log" 2>&1 || status=$?
cat "$out/mux.log"
if [ "$status" -ne 0 ] && [ "$status" -ne 2 ]; then
    exit "$status"
fi
awk '/Booting Zephyr/ && !boot { boot = NR }
    /Temperature: 21[.]50 C/ && !temp { temp = NR }
    /x 0[.]008750 , y 0[.]017500 , z 0[.]008750/ && !axes { axes = NR }
    END { exit !(boot && boot < temp && temp < axes) }' "$out/mux.log"
