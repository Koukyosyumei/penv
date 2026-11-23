#!/bin/sh
# arm-cortex-m-fpu.robot / Should Pass Context Stacking Preservation Test v7-M.
set -eu
if [ "$#" -ne 3 ]; then
    echo "Usage: $0 PENV RENODE_ROOT ARTIFACT_DIR" >&2
    exit 1
fi

penv=$1
out=$3
mkdir -p "$out"
elf=$out/stacking.elf
remote=cortex-v7m-extended-frame-test.elf-s_91928-d49ec01cc0c6270f7055330871953182a507efc6
if [ ! -f "$elf" ]; then
    curl -fsSL "https://dl.antmicro.com/projects/renode/$remote" -o "$elf.part"
    mv "$elf.part" "$elf"
fi
printf '%s  %s\n' "${remote##*-}" "$elf" | sha1sum --check

cat >"$out/stacking.repl" <<'REPL'
cpu: CPU.CortexM @ sysbus
    cpuType: "cortex-m4"
    nvic: nvic
    vectorTableOffset: 0
nvic: IRQControllers.NVIC @ sysbus 0xe000e000
    -> cpu@0
mem: Memory.MappedMemory @ sysbus 0
    size: 0x40000
uart: UART.NS16550 @ sysbus 0x80000
REPL
status=0
"$penv" run --platform "$out/stacking.repl" --max-steps 2500000 "$elf" >"$out/stacking.log" 2>&1 || status=$?
cat "$out/stacking.log"
if [ "$status" -ne 0 ] && [ "$status" -ne 2 ]; then
    exit "$status"
fi
grep -Fxq 'Test succeeded' "$out/stacking.log"
