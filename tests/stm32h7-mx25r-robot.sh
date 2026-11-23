#!/usr/bin/env bash
# H753ZI Macronix QuadSPI Robot cases.
set -euo pipefail
cd "$(dirname "$0")/.."
renode=${RENODE_ROOT:-../renode}
assets=${PENV_ASSET_DIR:-$(mktemp -d)}
penv=${PENV_BIN:-target/release/penv}
logs=${1:?Pass a directory for logs and exit status}
mkdir -p "$assets" "$logs"
failed=0
for sample in QSPI_RW QSPI_MemMapped QSPI_XIP; do
    case_dir=$logs/$sample
    mkdir -p "$case_dir"
    name=$(awk -v var='${'"$sample"'}' '$1==var {sub(/^.*\//,"",$2); print $2; exit}' "$renode/tests/platforms/nucleo_h753zi.robot")
    [[ -n $name ]]
    url=https://dl.antmicro.com/projects/renode/$name
    if [[ ! -f "$assets/$name" ]]; then
        curl --fail --location "$url" --output "$assets/$name.part"
        mv "$assets/$name.part" "$assets/$name"
    fi
    printf '%s  %s\n' "${name##*-}" "$assets/$name" | sha1sum --check -
    cat >"$case_dir/board.repl" <<'REPL'
using "platforms/boards/nucleo_h753zi.repl"
led1: Miscellaneous.LED @ gpioPortF 10 { invert: true; }
led3: Miscellaneous.LED @ gpioPortA 4 { invert: true; }
gpioPortF:
    10 -> led1@0
gpioPortA:
    4 -> led3@0
externalQspiFlash: SPI.Macronix_MX25R @ qspi { underlyingMemory: qspiMappedFlashMemory; }
qspiMappedFlashMemory: Memory.MappedMemory @ sysbus 0x90000000 { size: 0x10000000; }
REPL
    if [[ $sample == QSPI_RW ]]; then
        cat >>"$case_dir/board.repl" <<'REPL'
slave1: Mocks.DummyI2CSlave @ i2c1 0x42
slave2: Mocks.DummyI2CSlave @ i2c1 0x43
REPL
    fi
    led3_timeout=1
    if [[ $sample == QSPI_XIP ]]; then led3_timeout=20; fi
    cat >"$case_dir/case.resc" <<RESC
mach create qspi
machine LoadPlatformDescription @board.repl
sysbus LoadELF @$url
tester WaitLed led3 $led3_timeout false
tester WaitLed led1 1 false
tester WaitLed led1 10 true
RESC
    if [[ $sample == QSPI_XIP ]]; then
        cat >>"$case_dir/case.resc" <<'RESC'
tester WaitLed led3 1 true
emulation RunFor 0.01
RESC
    else
        cat >>"$case_dir/case.resc" <<'RESC'
tester WaitLed led3 1 false
tester WaitLed led1 1 false
tester WaitLed led3 1 false
tester WaitLed led1 1 true
tester WaitLed led3 1 false
RESC
    fi
    status=0
    "$penv" run --script "$case_dir/case.resc" --platform-root "$renode" --asset-dir "$assets" \
        --max-steps 40000000 >"$case_dir/monitor.log" 2>"$case_dir/penv.log" || status=$?
    printf '%s\n' "$status" >"$case_dir/exit"
    if (( status != 0 )); then
        printf '%s FAIL (%s)\n' "$sample" "$status"
        cat "$case_dir/penv.log" >&2
        failed=1
    elif [[ $sample == QSPI_XIP ]]; then
        printf '%s INCOMPLETE (Robot PC assertion 0x90000010 pending)\n' "$sample"
        failed=1
    else
        printf '%s PASS\n' "$sample"
    fi
done
exit "$failed"
