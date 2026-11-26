#!/bin/sh
# bus-isolation.robot:172-192. Each load starts with an empty machine.
set -eu
[ "$#" -eq 2 ] || exit 1
penv=$1
out=$2
mkdir -p "$out"

reject_registration() {
    name=$1
    condition=$2
    expected=$3
    printf 'uart: UART.PL011 @ sysbus new Bus.BusPointRegistration { address: 0x0; condition: "%s" }\n' "$condition" >"$out/$name.repl"
    status=0
    "$penv" validate-platform "$out/$name.repl" \
        >"$out/$name.stdout" 2>"$out/$name.stderr" || status=$?
    [ "$status" -eq 1 ]
    [ ! -s "$out/$name.stdout" ]
    grep -Fq "$expected" "$out/$name.stderr"
    cat "$out/$name.stderr"
}

reject_registration condition-with-unregistered-initiator 'cpuSecure && initiator == reader' 'Invalid initiator: reader'
reject_registration unregistered-initiator 'initiator == reader' 'Invalid initiator: reader'
reject_registration no-initiators '!privileged' 'Conditions provided (!privileged) but there are no peripherals implementing IPeripheralWithTransactionState or they have no common state bits'
