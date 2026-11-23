#!/usr/bin/env bash
# Port Should Not Run Out Of Ring Buffer Space When Writing Long Line To Uart.
set -euo pipefail
cd "$(dirname "$0")/.."
renode=${RENODE_ROOT:-../renode}
assets=${PENV_ASSET_DIR:-$(mktemp -d)}
penv=${PENV_BIN:-target/release/penv}
logs=${1:?Pass a directory for logs and exit states}
robot="$renode/tests/platforms/nucleo_h753zi.robot"
mkdir -p "$assets" "$logs"
name=$(awk '$1=="${DHCP}" {sub(/^.*\//,"",$2); print $2}' "$robot")
line=$(awk -F '  +' '$2=="Write Line To Uart" && $3 ~ /^rem / { print $3 }' "$robot")
[[ -n $name && -n $line ]]
url="https://dl.antmicro.com/projects/renode/$name"
if [[ ! -f "$assets/$name" ]]; then
    curl --fail --location "$url" --output "$assets/$name.part"
    mv "$assets/$name.part" "$assets/$name"
fi
printf '%s  %s\n' "${name##*-}" "$assets/$name" | sha1sum --check -
cat > "$logs/case.resc" <<RESC
mach create "overflow"
machine LoadPlatformDescription @platforms/boards/nucleo_h753zi.repl
sysbus LoadELF @$url
tester ObserveUart sysbus.usart3
tester FailOnUart sysbus.usart3 "RX ring buffer full"
tester WaitUart sysbus.usart3 8 partial "uart:~\$"
tester WriteUartLine sysbus.usart3 "$line"
emulation RunFor "0.05"
tester WaitUart sysbus.usart3 8 partial "uart:~\$"
RESC
status=0
"$penv" run --script "$logs/case.resc" --platform-root "$renode" --asset-dir "$assets" \
    --max-steps 18000000 --uart-log-dir "$logs" \
    > "$logs/monitor.log" 2> "$logs/penv.log" || status=$?
printf '%s\n' "$status" > "$logs/exit"
if (( status != 0 )); then
    cat "$logs/penv.log" >&2
fi
exit "$status"
