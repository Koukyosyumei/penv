#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
work=${1:-$(mktemp -d)}
mkdir -p "$work"
fixture=tests/fixtures/cortex-m4f-faults
cargo build --release
cat > "$work/base.repl" <<'REPL'
mem: Memory.MappedMemory @ sysbus 0x0
    size: 0x40000
nvic: IRQControllers.NVIC @ sysbus 0xe000e000
    -> cpu@0
REPL
cat > "$work/cpu.repl" <<'REPL'
using "base.repl"
cpu: CPU.CortexM @ sysbus
    cpuType: "cortex-m4"
    nvic: nvic
REPL
cat > "$work/busfault.repl" <<'REPL'
using "cpu.repl"
fault: Mocks.BusFaultingPeripheral @ sysbus 0x100000
uart: UART.LiteX_UART @ sysbus 0x300000
REPL
cat > "$work/fp-busfault.repl" <<'REPL'
using "cpu.repl"
fault: Mocks.BusFaultingPeripheral @ sysbus 0x100000
    FaultOffset: 0
uart: UART.LiteX_UART @ sysbus 0x300000
REPL
cat > "$work/nocp.repl" <<'REPL'
using "base.repl"
cpu: CPU.CortexM @ sysbus
    cpuType: "cortex-m4"
    nvic: nvic
    vectorTableOffset: 0x10000
uart: UART.LiteX_UART @ sysbus 0x100000
REPL
run_case() {
    local family=$1 case_id=$2 name=$3 platform=$4 expected=$5
    local layout=$family
    if [[ "$family" == fetch ]]; then layout=busfault; fi
    "${CLANG:-clang}" --target=arm-none-eabi -mcpu=cortex-m4 -mthumb -mfpu=fpv4-sp-d16 \
        -DCASE="$case_id" -nostdlib -fuse-ld=lld -Wl,-T,"$fixture/$layout.ld" \
        "$fixture/$family.S" -o "$work/$name.elf"
    target/release/penv run --platform "$work/$platform.repl" --max-steps 1000 "$work/$name.elf" > "$work/$name.log"
    diff -u <(printf '%s\n' "$expected") "$work/$name.log"
    printf '%s: %s\n' "$name" "$expected"
}
cases=(read write unmapped-read disabled-escalation nested-escalation continuous-read continuous-write vstr-s0 vstr-d0 vldr-s0 vldr-d0)
for i in "${!cases[@]}"; do
    platform=busfault
    if (( i >= 7 )); then platform=fp-busfault; fi
    run_case busfault "$i" "${cases[i]}" "$platform" 'BusFault OK'
done
run_case nocp 0 nocp-disabled nocp NOCP
run_case nocp 1 nocp-enabled nocp OK

for offset in 2 0x40; do
    cat > "$work/fetch-$offset.repl" <<REPL
using "cpu.repl"
fault: Mocks.BusFaultingPeripheral @ sysbus 0x100000
    FaultOffset: $offset
uart: UART.LiteX_UART @ sysbus 0x300000
REPL
done
run_case fetch 0 fetch-first-half busfault 'Fetch fault OK'
run_case fetch 1 fetch-second-half fetch-2 'Fetch fault OK'
run_case fetch 2 irq-vector-fetch fetch-0x40 'Fetch fault OK'
