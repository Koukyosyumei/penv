#!/usr/bin/env bash
# Port Should Read and Write IS25WP Flash in nucleo_h753zi.robot.
set -euo pipefail
cd "$(dirname "$0")/.."
renode=${RENODE_ROOT:-../renode}
assets=${PENV_ASSET_DIR:-$(mktemp -d)}
penv=${PENV_BIN:-target/release/penv}
logs=${1:?Pass a directory for logs and exit status}
mkdir -p "$assets" "$logs"
name=$(awk '$1=="${FLASH_IS25WP}" {sub(/^.*\//,"",$2); print $2; exit}' "$renode/tests/platforms/nucleo_h753zi.robot")
[[ -n $name ]]
url="https://dl.antmicro.com/projects/renode/$name"
if [[ ! -f "$assets/$name" ]]; then
    curl --fail --location "$url" --output "$assets/$name.part"
    mv "$assets/$name.part" "$assets/$name"
fi
printf '%s  %s\n' "${name##*-}" "$assets/$name" | sha1sum --check -
cat >"$logs/board.repl" <<'REPL'
using "platforms/boards/nucleo_h753zi.repl"
externalQspiFlash: SPI.ISSI_IS25WP @ qspi { underlyingMemory: qspiMappedFlashMemory; }
qspiMappedFlashMemory: Memory.MappedMemory @ sysbus 0x90000000 { size: 0x8000000; }
REPL
cat >"$logs/case.resc" <<RESC
mach create flash
machine LoadPlatformDescription @board.repl
sysbus LoadELF @$url
tester ObserveUart sysbus.usart3
tester WaitUart sysbus.usart3 8 line "qspi-nor-flash@0 SPI flash testing"
tester WaitUart sysbus.usart3 8 line "Perform test on single sector"
tester WaitUart sysbus.usart3 8 line "Flash erase succeeded!"
tester WaitUart sysbus.usart3 8 line "Data read matches data written. Good!!"
tester WaitUart sysbus.usart3 8 line "Perform test on multiple consecutive sectors"
tester WaitUart sysbus.usart3 8 line "Flash erase succeeded!"
tester WaitUart sysbus.usart3 8 line "Data read matches data written. Good!!"
RESC
status=0
"$penv" run --script "$logs/case.resc" --platform-root "$renode" --asset-dir "$assets" \
    --max-steps 16000000 --uart-log-dir "$logs" >"$logs/monitor.log" 2>"$logs/penv.log" || status=$?
printf '%s\n' "$status" >"$logs/exit"
if (( status != 0 )); then cat "$logs/penv.log" >&2; fi
exit "$status"
