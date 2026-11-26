#!/usr/bin/env bash
# Port the three monitor-only TIM1 tests from STM32H7_RCC.robot.
set -euo pipefail
cd "$(dirname "$0")/.."
renode=${RENODE_ROOT:-../renode}
penv=${PENV_BIN:-target/release/penv}
logs=${1:?Pass the output directory}
mkdir -p "$logs"
failed=0
for case_name in reset frequency gate; do
    work="$logs/$case_name"
    mkdir -p "$work"
    cat > "$work/case.resc" <<'RESC'
mach create
machine LoadPlatformDescription @platforms/cpus/stm32h743.repl
RESC
    case "$case_name" in
        reset) cat >> "$work/case.resc" <<'RESC'
sysbus LogPeripheralAccess rcc
rcc WriteDoubleWord 0x98 0x1
RESC
            ;;
        frequency) cat >> "$work/case.resc" <<'RESC'
timer1 Frequency
rcc WriteDoubleWord 0x98 0x1
timer1 Frequency
RESC
            ;;
        gate) cat >> "$work/case.resc" <<'RESC'
sysbus IsPeripheralEnabled timer1
rcc WriteDoubleWord 0xF0 0xFFFFFFFE
sysbus IsPeripheralEnabled timer1
rcc WriteDoubleWord 0xF0 0x1
sysbus IsPeripheralEnabled timer1
RESC
            ;;
    esac
    status=0
    "$penv" run --script "$work/case.resc" --platform-root "$renode" \
        > "$work/monitor.log" 2> "$work/penv.log" || status=$?
    printf '%s\n' "$status" > "$work/exit"
    if (( status == 0 )); then
        case "$case_name" in
            reset) grep -Fxq 'Resetting TIM1' "$work/penv.log" || status=1 ;;
            frequency) printf '64000000\n64000000\n' | diff -u - "$work/monitor.log" || status=1 ;;
            gate) printf 'true\nfalse\ntrue\n' | diff -u - "$work/monitor.log" || status=1 ;;
        esac
    fi
    printf '%s\n' "$status" > "$work/assert.exit"
    if (( status != 0 )); then
        cat "$work/penv.log" >&2
        failed=1
    fi
done
exit "$failed"
