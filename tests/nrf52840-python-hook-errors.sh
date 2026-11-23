#!/bin/sh
set -eu
[ "$#" -eq 2 ] || exit 1
penv=$1
out=$2
mkdir -p "$out"
cat > "$out/fixture.repl" <<'REPL'
cpu: CPU.RiscV32 @ sysbus { cpuType: "rv32gc"; timeProvider: empty }
mem: Memory.MappedMemory @ sysbus 0 { size: 0x1000 }
uart: UART.LiteX_UART @ sysbus 0x2000
radio: Wireless.NRF52840_Radio @ sysbus 0x3000
REPL
failed=0
while IFS='|' read -r name command; do
    cat > "$out/$name.resc" <<RESC
mach create
using sysbus
machine LoadPlatformDescription @fixture.repl
$command
RESC
    status=0
    "$penv" run --script "$out/$name.resc" > "$out/$name.log" 2>&1 || status=$?
    printf '%s\n' "$status" > "$out/$name.status"
    if [ "$status" -eq 1 ] &&
        grep -Fq '[FatalError] unexpected EOF while parsing (Line 1, Column 9)' "$out/$name.log"; then
        echo "$name PASS"
    else
        echo "$name FAIL"
        failed=1
    fi
done <<'CASES'
block|cpu AddHook 0xC0FFEE "if error"
interrupt|cpu AddHookAtInterruptBegin "if error"
peripheral|sysbus SetHookAfterPeripheralRead uart "if error"
watchpoint|sysbus AddWatchpointHook 0xC0FFEE Byte Read "if error"
uart|uart AddLineHook "foobar" "if error"
user-state|machine AddUserStateHook "foobar" "if error"
CASES
exit "$failed"
