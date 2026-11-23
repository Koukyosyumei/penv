#!/bin/sh
# Validate the production LED connection path with the fixed Zephyr Button ELF.
set -eu
[ "$#" -eq 4 ] || exit 1
penv=$1
renode=$2
elf=$3
out=$4
mkdir -p "$out"
printf '%s  %s\n' 50c3b674193c8105624dae389420904e2036f9c0 "$elf" | sha1sum --check
cat >"$out/button.repl" <<'REPL'
using "platforms/cpus/nrf52840.repl"
gpio0:
    13 -> led@0
button: Miscellaneous.Button @ gpio0 11
    invert: true
    -> gpio0@11
led: Miscellaneous.LED @ gpio0 13
REPL
sed 's/led@0/led@1/' "$out/button.repl" >"$out/invalid.repl"
status=0
"$penv" run --platform-root "$renode" --platform "$out/invalid.repl" "$elf" \
    >"$out/invalid.log" 2>&1 || status=$?
[ "$status" -eq 1 ]
grep -Fq 'Cannot connect 1th input of led; it has only 1 GPIO inputs.' "$out/invalid.log"
status=0
"$penv" run --platform-root "$renode" --platform "$out/button.repl" --led-events \
    --button-event 500000:button:press --button-event 1500000:button:release \
    --max-steps 3000001 "$elf" >"$out/button.log" 2>&1 || status=$?
[ "$status" -eq 2 ]
awk '/Booting Zephyr OS/ { boot = NR }
    /Press the button/ { prompt = NR }
    /^LED led / { events = events " " $3 }
    END { exit !(boot && boot < prompt && events == " on off on") }' "$out/button.log"
cat "$out/button.log"

# Exercise bus unregistration on the same board and firmware.
cat >"$out/detached.resc" <<'RESC'
mach create
machine LoadPlatformDescription @button.repl
sysbus LoadELF @button.elf
sysbus Unregister led
RESC
status=0
"$penv" run --platform-root "$renode" --script "$out/detached.resc" --led-events \
    --button-event 500000:button:press --button-event 1500000:button:release \
    --max-steps 3000001 "$elf" >"$out/detached.log" 2>&1 || status=$?
[ "$status" -eq 2 ]
grep -Fq 'Button pressed' "$out/detached.log"
if grep -q '^LED led ' "$out/detached.log"; then
    exit 1
fi
cat "$out/detached.log"
