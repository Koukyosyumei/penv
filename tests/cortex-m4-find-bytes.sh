#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
work=${1:-$(mktemp -d)}
mkdir -p "$work"
cat >"$work/find.repl" <<'REPL'
first: Memory.MappedMemory @ sysbus 0 { size: 0x100000 }
second: Memory.MappedMemory @ sysbus 0x100000 { size: 0x100000 }
nvic: IRQControllers.NVIC @ sysbus 0xe000e000 { -> cpu@0 }
cpu: CPU.CortexM @ sysbus { cpuType: "cortex-m4"; nvic: nvic }
sysbus:
    init:
        UnhandledAccessBehaviour DoNotReport
REPL
cat >"$work/find.resc" <<'RESC'
mach create
machine LoadPlatformDescription @find.repl
sysbus LoadBinary @pattern.bin 0
sysbus LoadBinary @pattern.bin 0xffffc
RESC
printf '\000\001\002\003\004\005\006\007' >"$work/pattern.bin"
cargo build --release
target/release/penv run --script "$work/find.resc" --find-bytes '00 01 02 03 04 05 06 07' \
    >"$work/addresses.log"
diff -u <(printf '%s\n' 0x00000000 0x000ffffc) "$work/addresses.log"
cat "$work/addresses.log"
