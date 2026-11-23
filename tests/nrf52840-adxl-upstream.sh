#!/bin/sh
# The continuous ADXL372 stimulus in NRF52840.robot (I2C and SPI).
set -eu
if [ "$#" -ne 3 ]; then
    echo "Usage: $0 PENV RENODE_ROOT ARTIFACT_DIR" >&2
    exit 1
fi

penv=$1
renode=$2
out=$3
mkdir -p "$out"

check() (
    bus=$1
    registration=$2
    remote=$3
    elf=$out/adxl-$bus.elf
    board=$out/adxl-$bus.repl
    log=$out/adxl-$bus.log
    if [ ! -f "$elf" ]; then
        curl -fsSL "https://dl.antmicro.com/projects/renode/$remote" -o "$elf.part"
        mv "$elf.part" "$elf"
    fi
    printf '%s  %s\n' "${remote##*-}" "$elf" | sha1sum --check
    printf 'using "platforms/cpus/nrf52840.repl"\nadxl372: Sensors.ADXL372 @ %s\n' "$registration" > "$board"
    if [ "$bus" = spi ]; then
        printf 'gpio0:\n    22 -> adxl372@0\n' >> "$board"
    fi
    status=0
    "$penv" run --platform "$board" --platform-root "$renode" --uart-steps \
        --adxl-event 150000000:adxl372:1:0:0 --adxl-event 350000000:adxl372:2:2:0 \
        --adxl-event 550000000:adxl372:3:3:3 --max-steps 730000000 "$elf" >"$log" 2>&1 || status=$?
    if [ "$status" -ne 0 ] && [ "$status" -ne 2 ]; then
        cat "$log" >&2
        exit "$status"
    fi
    awk '/Booting Zephyr/ { boot = 1 }
        / g:/ {
            ++sample
            step = $2 + 0
            if (!boot || sample > 4 || $3 != expected[sample] ||
                step < start[sample] || step >= finish[sample]) exit 1
        }
        BEGIN {
            split("0.00 1.00 2.83 5.20", expected)
            split("0 150000000 350000000 550000000", start)
            split("150000000 350000000 550000000 730000000", finish)
        }
        END { if (sample != 4) exit 1 }' "$log"
    printf 'PASS adxl-%s %s\n' "$bus" "$log"
)

check spi spi2 nrf52840--zephyr_adxl372_spi.elf-s_993780-1dedb945dae92c07f1b4d955719bfb1f1e604173 &
spi=$!
check i2c 'twi1 0x11' nrf52840--zephyr_adxl372_i2c.elf-s_944004-aacf7d772ebcc5a26c156f78ebdef2e03f803cc3 &
i2c=$!
failed=0
wait "$spi" || failed=1
wait "$i2c" || failed=1
test "$failed" -eq 0
