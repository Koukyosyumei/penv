#!/usr/bin/env bash
# UART stimulus and observations from Should Read Correct Time From the PTP Clock.
set -euo pipefail
cd "$(dirname "$0")/.."
renode=${RENODE_ROOT:-../renode}
assets=${PENV_ASSET_DIR:-$(mktemp -d)}
penv=${PENV_BIN:-target/release/penv}
logs=${1:?Pass a directory for logs and exit status}
mkdir -p "$assets" "$logs"
name=$(awk '$1=="${PTP}" {sub(/^.*\//,"",$2); print $2; exit}' "$renode/tests/platforms/nucleo_h753zi.robot")
[[ -n $name ]]
url=https://dl.antmicro.com/projects/renode/$name
if [[ ! -f "$assets/$name" ]]; then
    curl --fail --location "$url" --output "$assets/$name.part"
    mv "$assets/$name.part" "$assets/$name"
fi
printf '%s  %s\n' "${name##*-}" "$assets/$name" | sha1sum --check -
cat >"$logs/clock.resc" <<RESC
mach create ptp
machine LoadPlatformDescription @platforms/boards/nucleo_h753zi.repl
sysbus LoadELF @$url
tester ObserveUart sysbus.usart3
tester WaitUart sysbus.usart3 8 partial "ptp_port: ptp_port_init: Port 1 initialized"
tester WaitUart sysbus.usart3 8 partial "uart:~\$"
tester WriteUartLine sysbus.usart3 "ptp_clock set PTP_CLOCK 25"
tester WriteUartLine sysbus.usart3 "ptp_clock get PTP_CLOCK"
tester WaitUart sysbus.usart3 8 line "25.00"
emulation RunFor 7.48
tester WriteUartLine sysbus.usart3 "ptp_clock get PTP_CLOCK"
tester WaitUart sysbus.usart3 8 line "32.48"
RESC
status=0
"$penv" run --script "$logs/clock.resc" --platform-root "$renode" --asset-dir "$assets" \
    --max-steps 40000000 --uart-log-dir "$logs" >"$logs/clock.log" 2>"$logs/clock.err" || status=$?
printf '%s\n' "$status" >"$logs/clock.exit"
if (( status != 0 )); then cat "$logs/clock.err" >&2; fi
exit "$status"
