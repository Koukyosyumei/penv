#!/usr/bin/env bash
# Port Should Blink Led and Should See Button Press from nucleo_h753zi.robot.
set -euo pipefail

cd "$(dirname "$0")/.."
renode=${RENODE_ROOT:-../renode}
assets=${PENV_ASSET_DIR:-$(mktemp -d)}
penv=${PENV_BIN:-target/release/penv}
logs=${1:?Pass a directory for logs and exit states}
robot="$renode/tests/platforms/nucleo_h753zi.robot"
mkdir -p "$assets" "$logs"

failed=0
for scenario in blinky button; do
    mkdir -p "$logs/$scenario"
    case_dir="$logs/$scenario"
    tag=${scenario^^}
    name=$(awk -v tag="\${$tag}" '$1 == tag {sub(/^.*\//,"",$2); print $2}' "$robot")
    [[ -n $name ]]
    url="https://dl.antmicro.com/projects/renode/$name"
    if [[ ! -f "$assets/$name" ]]; then
        curl --fail --location "$url" --output "$assets/$name.part"
        mv "$assets/$name.part" "$assets/$name"
    fi
    printf '%s  %s\n' "${name##*-}" "$assets/$name" | sha1sum --check -
    cat > "$case_dir/case.resc" <<RESC
mach create "$scenario"
machine LoadPlatformDescription @platforms/boards/nucleo_h753zi.repl
sysbus LoadELF @$url
tester ObserveUart sysbus.usart3
tester WaitUart sysbus.usart3 8 partial "*** Booting Zephyr OS"
RESC
    if [[ $scenario == blinky ]]; then
        cat >> "$case_dir/case.resc" <<'RESC'
tester WaitUart sysbus.usart3 8 line "LED state: ON" "LED state: OFF"
tester BlinkLed GreenLED 8 1 1
RESC
    else
        cat >> "$case_dir/case.resc" <<'RESC'
tester WaitUart sysbus.usart3 8 line "Press the button"
tester WaitLed GreenLED 1 false
sysbus.gpioPortC.UserButton1 Press
tester WaitUart sysbus.usart3 8 partial "Button pressed at"
tester WaitLed GreenLED 1 true
sysbus.gpioPortC.UserButton1 Release
tester WaitLed GreenLED 1 false
RESC
    fi
    status=0
    "$penv" run --script "$case_dir/case.resc" --platform-root "$renode" --asset-dir "$assets" \
        --max-steps 30000000 --uart-log-dir "$case_dir" \
        > "$case_dir/monitor.log" 2> "$case_dir/penv.log" || status=$?
    printf '%s\n' "$status" > "$case_dir/exit"
    printf '%s %s\n' "$status" "$scenario"
    if (( status != 0 )); then
        cat "$case_dir/penv.log" >&2
        failed=1
    fi
done
exit "$failed"
