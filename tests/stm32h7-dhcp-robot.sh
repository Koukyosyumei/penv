#!/usr/bin/env bash
# Port of Should Resolve DHCP and Be Pingable.
set -euo pipefail
cd "$(dirname "$0")/.."
renode=${RENODE_ROOT:-../renode}
assets=${PENV_ASSET_DIR:-$(mktemp -d)}
penv=${PENV_BIN:-target/release/penv}
logs=${1:?Pass a directory for logs and exit status}
mkdir -p "$assets" "$logs"
name=$(awk '$1=="${DHCP}" {sub(/^.*\//,"",$2); print $2; exit}' "$renode/tests/platforms/nucleo_h753zi.robot")
[[ -n $name ]]
url=https://dl.antmicro.com/projects/renode/$name
if [[ ! -f "$assets/$name" ]]; then
    curl --fail --location "$url" --output "$assets/$name.part"
    mv "$assets/$name.part" "$assets/$name"
fi
printf '%s  %s\n' "${name##*-}" "$assets/$name" | sha1sum --check -
{
    echo 'emulation CreateSwitch "switch"'
    for node in server client; do
        cat <<RESC
mach create "$node"
machine LoadPlatformDescription @platforms/boards/nucleo_h753zi.repl
sysbus LoadELF @$url
connector Connect sysbus.ethernet switch
tester ObserveUart sysbus.usart3
RESC
    done
    cat <<'RESC'
mach set "server"
tester WaitUart sysbus.usart3 8 partial "uart:~$"
tester WaitUart sysbus.usart3 8 partial "uart:~$"
tester WriteUartLine sysbus.usart3 "net dhcpv4 client stop 1"
tester WaitUart sysbus.usart3 8 partial "net dhcpv4 client stop 1"
tester WriteUartLine sysbus.usart3 "net ipv4 del 1 192.0.2.1"
tester WaitUart sysbus.usart3 8 partial "net ipv4 del 1 192.0.2.1"
tester WriteUartLine sysbus.usart3 "net ipv4 add 1 122.101.101.1 255.255.255.0"
tester WaitUart sysbus.usart3 8 partial "net ipv4 add 1 122.101.101.1 255.255.255.0"
tester WriteUartLine sysbus.usart3 "net dhcpv4 server start 1 122.101.101.2"
tester WaitUart sysbus.usart3 8 partial "net dhcpv4 server start 1 122.101.101.2"
tester WaitUart sysbus.usart3 8 line "DHCPv4 server started on interface 1"
mach set "client"
tester WaitUart sysbus.usart3 15 line "net_dhcpv4: Received: 122.101.101.2"
tester WaitUart sysbus.usart3 8 partial "uart:~$"
tester WriteUartLine sysbus.usart3 "net ping 122.101.101.1"
tester WaitUart sysbus.usart3 8 partial "net ping 122.101.101.1"
tester WaitUart sysbus.usart3 8 line "28 bytes from 122.101.101.1"
mach set "server"
tester WriteUartLine sysbus.usart3 "net ping 122.101.101.2"
tester WaitUart sysbus.usart3 8 partial "net ping 122.101.101.2"
tester WaitUart sysbus.usart3 8 line "28 bytes from 122.101.101.2"
RESC
} >"$logs/dhcp.resc"
status=0
"$penv" run --script "$logs/dhcp.resc" --platform-root "$renode" --asset-dir "$assets" \
    --max-steps 70000000 --uart-log-dir "$logs" >"$logs/dhcp.log" 2>"$logs/dhcp.err" || status=$?
printf '%s\n' "$status" >"$logs/dhcp.exit"
if (( status != 0 )); then cat "$logs/dhcp.err" >&2; fi
exit "$status"
