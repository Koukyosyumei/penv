#!/bin/sh
# Preboot snapshot verification for the shared ARM candidate.
set -eu
if [ "$#" -ne 4 ]; then
    echo "Usage: $0 PENV RENODE_ROOT SHELL_ELF ARTIFACT_DIR" >&2
    exit 1
fi
penv=$1
root=$2
elf=$3
out=$4
mkdir -p "$out"
cp "$elf" "$out/shell.elf"
printf 'demo ping\r' > "$out/ping.txt"
cat > "$out/boot.resc" <<'RESC'
mach create
machine LoadPlatformDescription @platforms/cpus/nrf52840.repl
sysbus LoadELF @shell.elf
Save @nrf.save
Clear
Load @nrf.save
mach set 0
waitForUartPrompt "uart:~$"
waitForUart "pong"
RESC
status=0
"$penv" run --script "$out/boot.resc" --platform-root "$root" \
    --uart-event "100000:$out/ping.txt" --max-steps 1000000 > "$out/boot.log" 2>&1 || status=$?
printf '%s\n' "$status" > "$out/boot.status"
[ "$status" -eq 0 ] || { cat "$out/boot.log" >&2; exit 1; }
test -s "$out/nrf.save"
grep -Fxq 'pong' "$out/boot.log" || tr -d '\r' < "$out/boot.log" | grep -Fxq 'pong'
echo 'boot snapshot: PASS'

sed '/^Save/,$d' "$out/boot.resc" > "$out/postrun.resc"
printf '%s\n' 'emulation RunFor "0.00001"' 'Save @postrun.save' >> "$out/postrun.resc"
status=0
"$penv" run --script "$out/postrun.resc" --platform-root "$root" \
    --max-steps 1000000 > "$out/postrun.log" 2>&1 || status=$?
printf '%s\n' "$status" > "$out/postrun.status"
[ "$status" -eq 1 ]
grep -Fq 'memory snapshots require a single-core Cortex-M machine before execution' "$out/postrun.log"
echo 'post-execution snapshot: BLOCKED'
