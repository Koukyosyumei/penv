#!/bin/sh
# LIS2DS12.robot / Should Read Acceleration.
set -eu
if [ "$#" -ne 3 ]; then
    echo "Usage: $0 PENV RENODE_ROOT ARTIFACT_DIR" >&2
    exit 1
fi
penv=$1
renode=$2
out=$3
mkdir -p "$out"
remote=nrf52840--zephyr_lis2dh.elf-s_747800-163b7e7cc986d4b1115f06b5f3df44ed0defc1fa
elf=$out/lis.elf
if [ ! -f "$elf" ]; then
    curl -fsSL "https://dl.antmicro.com/projects/renode/$remote" -o "$elf.part"
    mv "$elf.part" "$elf"
fi
printf '%s  %s\n' "${remote##*-}" "$elf" | sha1sum --check
cat >"$out/lis.repl" <<'REPL'
using "platforms/cpus/nrf52840.repl"
lis2ds12: Sensors.LIS2DS12 @ twi1 0x1c
    IRQ -> gpio0@28
    accelerationX: 10
    accelerationY: 5
    accelerationZ: -5
REPL
status=0
"$penv" run --platform "$out/lis.repl" --platform-root "$renode" --max-steps 130000000 "$elf" >"$out/lis.log" 2>&1 || status=$?
cat "$out/lis.log"
if [ "$status" -ne 0 ] && [ "$status" -ne 2 ]; then
    exit "$status"
fi
grep -Fq 'x 9.997213 , y 4.997410 , z -4.999803' "$out/lis.log"
