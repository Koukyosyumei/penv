#!/usr/bin/env bash
# Port INA228.robot with the pinned firmware, CSV conversion, and tolerances.
set -euo pipefail
cd "$(dirname "$0")/.."
renode=${RENODE_ROOT:-../renode}
assets=${PENV_ASSET_DIR:-$(mktemp -d)}
penv=${PENV_BIN:-target/release/penv}
logs=${1:?Pass the output directory}
robot="$renode/tests/peripherals/INA228.robot"
mkdir -p "$assets" "$logs"
url=$(awk '$1=="${BIN}" {print $2}' "$robot")
name=${url##*/}
[[ -n $name ]]
if [[ ! -f "$assets/$name" ]]; then
    curl --fail --location "$url" --output "$assets/$name.part"
    mv "$assets/$name.part" "$assets/$name"
fi
printf '%s  %s\n' "${name##*-}" "$assets/$name" | sha1sum --check -

failed=0
for scenario in frequency timestamp; do
    work="$logs/$scenario"
    mkdir -p "$work"
    timing=(--frequency 0.5)
    if [[ $scenario == timestamp ]]; then timing=(--timestamp timestamp); fi
    "${PYTHON:-python3}" "$renode/tools/csv2resd/csv2resd.py" \
        --input "$renode/tests/peripherals/INA228-samples.csv" \
        --map voltage:shuntvoltage::0 --map voltage:busvoltage::1 --map temperature:temperature::0 \
        --start-time 1000000000 "${timing[@]}" "$work/samples.resd"
    cat > "$work/board.repl" <<'REPL'
using "platforms/boards/nucleo_h753zi.repl"

ina228: Sensors.INA228 @ i2c1 0x40
REPL
    cat > "$work/case.resc" <<RESC
mach create "$scenario"
machine LoadPlatformDescription @board.repl
sysbus LoadELF @$url
tester ObserveUart sysbus.usart3
tester WaitUart sysbus.usart3 8 line " Booting Zephyr OS build "
tester WaitUart sysbus.usart3 3 line "Shunt:"
sysbus.i2c1.ina228 FeedShuntVoltageSamplesFromRESD @samples.resd 0
sysbus.i2c1.ina228 FeedBusVoltageSamplesFromRESD @samples.resd 1
sysbus.i2c1.ina228 FeedTemperatureSamplesFromRESD @samples.resd
tester WaitUart sysbus.usart3 3 line "Shunt:"
tester WaitUart sysbus.usart3 3 line "Shunt:"
tester WaitUart sysbus.usart3 3 line "Shunt:"
RESC
    status=0
    "$penv" run --script "$work/case.resc" --platform-root "$renode" --asset-dir "$assets" \
        --max-steps 21000000 --uart-log-dir "$work" > "$work/monitor.log" 2> "$work/penv.log" || status=$?
    printf '%s\n' "$status" > "$work/exit"
    if (( status != 0 )); then
        cat "$work/penv.log" >&2
        failed=1
        continue
    fi
    status=0
    python3 - "$work/0.uart.log" > "$work/compare.log" 2>&1 <<'PY' || status=$?
import re
import sys
from pathlib import Path

number = r'(-?\d+\.\d+)'
fields = [('Shunt', 'V'), ('Bus', 'V'), ('Current', 'A'), ('Power', 'W'),
          ('Temp', 'C'), ('Energy', 'J'), ('Charge', 'C')]
pattern = ' -- '.join(fr'{name}: {number} \[{unit}\]' for name, unit in fields)
expected = [(0, 0, 0, 0, 0, 0, 0), (.097, 48, 6, 288.17, 25, 288.29, 6.01),
            (.048, 52, 3, 156.09, -10, 444.27, 9), (.016, 44, 1, 44.29, 85, 200, 4)]
tolerance = [.01, .1, .1, 1, .1, 10, .1]
rows = re.findall(pattern, Path(sys.argv[1]).read_text())
if len(rows) != len(expected):
    raise SystemExit(f'Expected {len(expected)} complete rows, observed {len(rows)}')
failures = []
for index, (row, reference) in enumerate(zip(rows, expected)):
    print(f'Row {index}: {row}')
    for (name, _), actual, wanted, limit in zip(fields, map(float, row), reference, tolerance):
        if abs(actual - wanted) > limit:
            failures.append(f'Row {index} {name}: {actual} differs from {wanted} by more than {limit}')
if failures:
    raise SystemExit('\n'.join(failures))
PY
    printf '%s\n' "$status" > "$work/compare.exit"
    if (( status != 0 )); then
        cat "$work/compare.log" >&2
        failed=1
    fi
done
exit "$failed"
